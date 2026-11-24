//! Versioned HOME resources, agent templates and dispatch-time tool authorization.
pub(crate) mod lua;
mod mcp;
mod resources;
mod store;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use store::{err, id, safe, sha};
pub type R<T> = Result<T, String>;
pub use lua::evaluate_dispatch_rule;
#[derive(Clone)]
pub struct Catalog {
    home: Arc<PathBuf>,
    http: reqwest::Client,
}

fn string<'a>(v: &'a Value, k: &str) -> R<&'a str> {
    v[k].as_str().ok_or_else(|| format!("invalid: missing {k}"))
}
fn array(v: &Value) -> impl Iterator<Item = &Value> {
    v.as_array().into_iter().flatten()
}
fn merge(v: &mut Value, patch: &Value) {
    if let (Some(dst), Some(src)) = (v.as_object_mut(), patch.as_object()) {
        for (k, b) in src {
            dst.insert(k.clone(), b.clone());
        }
    }
}
fn includes(v: &Value, t: &str) -> bool {
    array(v).any(|x| x == t)
}
fn schema(s: &Value, v: &Value) -> R<()> {
    let validator = jsonschema::validator_for(s).map_err(|e| format!("invalid: schema: {e}"))?;
    validator.validate(v).map_err(|e| format!("invalid: {e}"))
}
const DAILY: &[&str] = &[
    "agents.get",
    "sessions.history",
    "events.search",
    "write_note",
    "search_notes",
    "read_note",
];
const AUTHOR: &[&str] = &[
    "harness.inspect",
    "harness.docs",
    "plugins.get",
    "plugins.install",
    "plugins.set_enabled",
];
const SELF: &[&str] = &[
    "agents.update",
    "settings.get",
    "settings.update",
    "context.preview",
];
fn builtin(name: &str) -> Value {
    let keys: &[(&str, &str)] = match name {
        "write_note" => &[("description", "string"), ("body", "string")],
        "read_note" => &[
            ("note_ref", "string"),
            ("start_line", "integer"),
            ("end_line", "integer"),
        ],
        "search_notes" => &[
            ("keyword", "string"),
            ("cursor", "string"),
            ("limit", "integer"),
        ],
        "events.search" => &[("scope", "string")],
        "plugins.get" | "harness.docs" => &[("id", "string"), ("version", "string")],
        "plugins.install" => &[
            ("id", "string"),
            ("version", "string"),
            ("manifest", "object"),
            ("code", "string"),
        ],
        "plugins.set_enabled" => &[
            ("id", "string"),
            ("package", "string"),
            ("version", "string"),
            ("enabled", "boolean"),
        ],
        "settings.get" => &[("id", "string")],
        "settings.update" => &[("id", "string"), ("settings", "object")],
        "agents.update" => &[("id", "string"), ("patch", "object")],
        _ => &[],
    };
    let props: serde_json::Map<_, _> = keys
        .iter()
        .map(|(k, t)| (k.to_string(), json!({"type":t})))
        .collect();
    json!({"id":name,"description":name,"input_schema":{"type":"object","properties":props},"direct":false,"source":"builtin","revision":"1"})
}
impl Catalog {
    pub fn open(home: impl AsRef<Path>) -> R<Self> {
        fs::create_dir_all(home.as_ref()).map_err(err)?;
        let home = home.as_ref().canonicalize().map_err(err)?;
        for dir in [
            "state",
            "agents",
            "tools",
            "packages",
            "plugins",
            "dispatches",
            "artifacts",
        ] {
            fs::create_dir_all(safe(&home, Path::new(dir))?).map_err(err)?;
        }
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(err)?;
        let catalog = Self {
            home: Arc::new(home),
            http,
        };
        catalog.initialize()?;
        catalog.reload()?;
        Ok(catalog)
    }
    pub fn templates(&self) -> R<Value> {
        self.templates_for("", false)
    }
    pub fn templates_for(&self, owner: &str, admin: bool) -> R<Value> {
        let mut result = vec![];
        for (name, bindings) in [
            ("standard", vec!["loom.daily"]),
            (
                "creator",
                vec!["loom.daily", "plugins.authoring", "loom.self_management"],
            ),
        ] {
            let tools: Vec<_> = bindings.iter().map(|t| json!({"toolset":t})).collect();
            result.push(
                json!({"id":name,"source":"loom","version":"1","spec":{"name":name,"tools":tools}}),
            );
        }
        for inst in self.all("plugin")? {
            let visible = admin || !inst["allowed_owners"].is_array() || includes(&inst["allowed_owners"], owner) || inst["owner"] == owner;
            if inst["enabled"] == true && visible {
                let package = self.package_for(&inst)?;
                for t in array(&package["templates"]) {
                    let mut t = t.clone();
                    t["source"] = inst["id"].clone();
                    t["version"] = inst["version"].clone();
                    result.push(t)
                }
            }
        }
        Ok(json!(result))
    }
    pub fn list_agents(&self, owner: &str, admin: bool) -> R<Value> {
        Ok(json!(
            self.all("agent")?
                .into_iter()
                .filter(|x| admin || x["owner"] == owner)
                .collect::<Vec<_>>()
        ))
    }
    pub fn get_agent(&self, owner: &str, admin: bool, key: &str) -> R<Value> {
        let a = self.get("agent", id(key)?)?;
        if !admin && a["owner"] != owner {
            return Err("not found: agent".into());
        }
        Ok(a)
    }
    pub fn create_agent(&self, owner: &str, admin: bool, request: Value) -> R<Value> {
        let key = id(string(&request, "id")?)?;
        let t = request
            .get("template")
            .or(request.get("template_id"))
            .cloned()
            .unwrap_or(json!("standard"));
        let name = t.as_str().or(t["id"].as_str()).ok_or("invalid: template")?;
        let options = self.templates_for(owner,admin)?;
        let mut matching = array(&options).filter(|x| {
            x["id"] == name
                && t.get("source").is_none_or(|s| x["source"] == *s)
                && t.get("version").is_none_or(|v| x["version"] == *v)
        });
        let template = matching.next().ok_or("not found: template")?;
        if matching.next().is_some() {
            return Err("invalid: ambiguous template source".into());
        }
        let mut agent = template["spec"].clone();
        let mut supplied = request.clone();
        if let Some(m) = supplied.as_object_mut() {
            for k in [
                "id",
                "template",
                "template_id",
                "spec",
                "owner",
                "grants",
                "revision",
                "expected_revision",
            ] {
                m.remove(k);
            }
        }
        merge(&mut agent, &supplied);
        if let Some(v) = request.get("spec") {
            merge(&mut agent, v)
        }
        agent["id"] = json!(key);
        agent["kind"] = json!("Agent");
        agent["owner"] = json!(if admin {
            request["owner"].as_str().unwrap_or(owner)
        } else {
            owner
        });
        agent["template"] =
            json!({"id":name,"source":template["source"],"version":template["version"]});
        agent["grants"] = if admin {
            request.get("grants").cloned().unwrap_or(json!({}))
        } else {
            json!({})
        };
        self.validate_agent(&agent)?;
        self.save_agent(key, agent, Some(""))
    }
    pub fn patch_agent(&self, owner: &str, admin: bool, key: &str, patch: Value) -> R<Value> {
        let mut a = self.get_agent(owner, admin, key)?;
        let mut p = patch.clone();
        let fields = p.as_object_mut().ok_or("invalid: expected patch")?;
        for k in ["id", "owner", "template", "revision", "expected_revision"] {
            fields.remove(k);
        }
        if !admin && fields.contains_key("grants") {
            return Err("forbidden: agent grants".into());
        }
        let rev = patch["expected_revision"]
            .as_str()
            .unwrap_or(string(&a, "revision")?)
            .to_string();
        merge(&mut a, &p);
        self.validate_agent(&a)?;
        self.save_agent(key, a, Some(&rev))
    }
    fn save_agent(&self, key: &str, a: Value, expect: Option<&str>) -> R<Value> {
        let p = safe(
            &self.home,
            &PathBuf::from("agents").join(id(key)?).join("agent.yaml"),
        )?;
        self.save("agent", key, a, expect, Some(&p))
    }
    fn validate_agent(&self, a: &Value) -> R<()> {
        if let Some(e) = a.get("extensions") {
            for (inst, config) in e.as_object().ok_or("invalid: extensions object")? {
                let src = self.package_for(&self.get("plugin", id(inst)?)?)?;
                let s = src["manifest"]
                    .get("agent_config_schema")
                    .ok_or("invalid: target has no Agent configuration schema")?;
                schema(s, config)?;
            }
        }
        Ok(())
    }
    fn package_for(&self, i: &Value) -> R<Value> {
        self.get(
            "package",
            &format!("{}/{}", string(i, "package")?, string(i, "version")?),
        )
    }
    fn available(&self, a: &Value) -> R<(BTreeMap<String, Value>, BTreeMap<String, Vec<String>>)> {
        let mut tools = BTreeMap::new();
        let mut sets = BTreeMap::new();
        let grants = &a["grants"];
        for (set, group) in [
            ("loom.daily", DAILY),
            ("plugins.authoring", AUTHOR),
            ("loom.self_management", SELF),
        ] {
            let mut members = vec![];
            for &name in group {
                let readable = DAILY.contains(&name)
                    || matches!(name, "harness.docs" | "harness.inspect" | "context.preview");
                if readable || includes(&grants["operations"], name) {
                    tools.insert(name.to_owned(), builtin(name));
                    members.push(name.to_owned());
                }
            }
            sets.insert(set.to_owned(), members);
        }
        for src in self.all("tool")? {
            let name = string(&src, "id")?;
            if !self.source_enabled(&src)? {
                continue;
            }
            let own_plugin = src["source"] == "plugin"
                && src["instance"]
                    .as_str()
                    .is_some_and(|i| includes(&grants["plugins"], i))
                && includes(&grants["operations"], "plugins.set_enabled");
            let permitted = own_plugin
                || includes(&grants["tools"], name)
                || includes(&src["allowed_agents"], a["id"].as_str().unwrap_or(""));
            if !permitted {
                continue;
            }
            let mut d = src.clone();
            if let Some(m) = d.as_object_mut() {
                for k in [
                    "code",
                    "url",
                    "credential_ref",
                    "allowed_agents",
                    "settings",
                ] {
                    m.remove(k);
                }
            }
            tools.insert(name.into(), d);
        }
        for set in self.all("toolset")? {
            if self.source_enabled(&set)? {
                sets.insert(
                    string(&set, "id")?.into(),
                    array(&set["members"])
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .collect(),
                );
            }
        }
        Ok((tools, sets))
    }
    pub fn tools(&self, owner: &str, admin: bool, agent_id: &str) -> R<Value> {
        let a = self.get_agent(owner, admin, agent_id)?;
        let (mut tools, sets) = self.available(&a)?;
        let mut chosen = BTreeSet::new();
        for b in array(&a["tools"]) {
            if let Some(t) = b["tool"].as_str().or(b.as_str()) {
                chosen.insert(t.to_string());
            }
            if let Some(s) = b["toolset"].as_str() {
                if let Some(m) = sets.get(s) {
                    chosen.extend(m.iter().cloned());
                }
            }
        }
        Ok(json!(
            chosen
                .into_iter()
                .filter_map(|n| tools.remove(&n))
                .collect::<Vec<_>>()
        ))
    }
    pub fn authorize(
        &self,
        owner: &str,
        admin: bool,
        agent_id: &str,
        name: &str,
        snapshot: &Value,
    ) -> R<Value> {
        let old = if snapshot.is_array() {
            array(snapshot).find(|t| t["id"] == name)
        } else if snapshot["id"] == name {
            Some(snapshot)
        } else {
            None
        }
        .ok_or("forbidden: tool absent from model snapshot")?;
        let now = self.tools(owner, admin, agent_id)?;
        let curr = array(&now)
            .find(|t| t["id"] == name)
            .ok_or("forbidden: tool binding revoked")?;
        if curr["revision"] != old["revision"] {
            return Err("conflict: tool binding changed".into());
        }
        Ok(curr.clone())
    }
    pub async fn execute(
        &self,
        owner: &str,
        admin: bool,
        agent_id: &str,
        name: &str,
        args: Value,
        snapshot: &Value,
    ) -> R<Value> {
        self.execute_with_context(owner, admin, agent_id, name, args, snapshot, &Value::Null)
            .await
    }
    pub async fn execute_with_context(
        &self,
        owner: &str,
        admin: bool,
        agent_id: &str,
        name: &str,
        args: Value,
        snapshot: &Value,
        context: &Value,
    ) -> R<Value> {
        self.execute_tracked(
            owner,
            admin,
            agent_id,
            name,
            args,
            snapshot,
            context,
            &Value::Null,
        )
        .await
    }
    pub async fn execute_tracked(
        &self,
        owner: &str,
        admin: bool,
        agent_id: &str,
        name: &str,
        args: Value,
        snapshot: &Value,
        context: &Value,
        tracking: &Value,
    ) -> R<Value> {
        let tool = self.authorize(owner, admin, agent_id, name, snapshot)?;
        schema(&tool["input_schema"], &args)?;
        let a = self.get_agent(owner, admin, agent_id)?;
        if tool["source"] == "builtin" {
            return self.builtin_call(owner, &a, name, args).await;
        }
        let raw = self.get("tool", name)?;
        if raw["revision"] != tool["revision"] {
            return Err("conflict: tool binding changed".into());
        }
        let inst = raw["instance"].as_str().unwrap_or("");
        let mut ctx = json!({"agent_id":agent_id,"agent_config":a["extensions"][inst],"settings":raw["settings"]});
        if let Some(note) = context.get("note") {
            if note["note_ref"].is_string() && note["note_ref"] == args["note_ref"] {
                ctx["note"] = note.clone();
            }
        }
        let result = match raw["source"].as_str() {
            Some("home") => lua::run(string(&raw, "code")?, &ctx, &args)?,
            Some("plugin") => lua::run_package(
                string(&raw, "code")?,
                string(&raw, "local_id")?,
                &ctx,
                &args,
            )?,
            Some("mcp") => self.remote_call(&raw, &args, tracking).await?,
            _ => return Err("invalid: tool source".into()),
        };
        if let Some(s) = raw.get("output_schema") {
            let data = if raw["source"] == "mcp" {
                result
                    .get("structuredContent")
                    .ok_or("invalid: missing structured result")?
            } else {
                &result
            };
            schema(s, data)?
        }
        Ok(result)
    }
}
