use super::*;
use std::ffi::OsStr;
use store::{dirs, write};

fn leaf(p: &Path) -> R<&str> {
    id(p.file_name()
        .and_then(OsStr::to_str)
        .ok_or("invalid: filename")?)
}
fn component(s: &str) -> String {
    s.replace('%', "%25").replace('.', "%2E")
}
pub(super) fn scoped(inst: &str, local: &str) -> String {
    format!("{}.{}", component(inst), component(local))
}
pub(super) fn normalize_tool_webui(tool: &mut Value) -> R<()> {
    let spec = tool
        .as_object_mut()
        .ok_or("invalid: expected tool definition")?;
    let Some(config) = spec.get_mut("webui") else {
        return Ok(());
    };
    if config.is_null() || config.as_object().is_some_and(|obj| obj.is_empty()) {
        spec.remove("webui");
        return Ok(());
    }
    let config = config
        .as_object_mut()
        .ok_or("invalid: expected webui object")?;
    if let Some(suggestion) = config.get_mut("suggestion") {
        for (field, limit) in [("label", 64), ("prompt", 1200)] {
            let text = suggestion[field].as_str().unwrap_or("").trim();
            if text.is_empty() || text.encode_utf16().count() > limit {
                return Err(format!(
                    "invalid: webui.suggestion.{field} must be 1 to {limit} characters"
                ));
            }
            suggestion[field] = json!(text);
        }
    }
    Ok(())
}
pub(super) fn descriptor(mut t: Value, source: &str, name: &str) -> R<Value> {
    if !t.is_object() {
        return Err("invalid: invalid tool declaration".into());
    }
    t["id"] = json!(name);
    t["source"] = json!(source);
    t["direct"] = json!(t["exposure"] == "direct");
    t["requires_approval"] = json!(t["requires_approval"] == true || t["approval"] == "required");
    if t.get("input_schema").is_none() {
        t["input_schema"] = json!({"type":"object"})
    }
    if t.get("description").is_none() {
        t["description"] = json!(name)
    }
    for k in ["input_schema", "output_schema"] {
        if let Some(s) = t.get(k) {
            jsonschema::validator_for(s).map_err(|e| format!("invalid {k}: {e}"))?;
        }
    }
    Ok(t)
}
impl Catalog {
    pub fn plugin_routes(&self, owner: &str, admin: bool) -> R<Value> {
        let mut paths = json!({});
        for i in self.all("plugin")? {
            if i["enabled"] != true || (!admin && !includes(&i["allowed_owners"], owner)) {
                continue;
            }
            let instance = string(&i, "id")?;
            let package = self.package_for(&i)?;
            for r in array(&package["routes"]) {
                let path = format!("/api/extensions/{instance}{}", string(r, "path")?);
                let method = string(r, "method")?.to_ascii_lowercase();
                paths[path][method] = json!({"x-loom-instance":instance,"x-loom-version":i["version"],"responses":{"200":{"description":"Plugin response"}}});
            }
        }
        Ok(paths)
    }
    fn guide(reference: &str) -> R<Value> {
        let (name, text) = match reference {
            "agent-authoring" | "agent-authoring@1" => (
                "agent-authoring",
                include_str!("../../docs/spec/reference/home.md"),
            ),
            "tool-sources" | "tool-sources@1" => (
                "tool-sources",
                include_str!("../../docs/spec/reference/tool-sources.md"),
            ),
            "plugin-contracts" | "plugin-contracts@1" => (
                "plugin-contracts",
                include_str!("../../docs/spec/reference/contracts.md"),
            ),
            _ => return Err("not found: guide version".into()),
        };
        Ok(
            json!({"id":name,"version":"1","doc_ref":format!("{name}@1"),"content":text,"revision":sha(&json!(text))}),
        )
    }
    pub fn apply_hook(
        &self,
        owner: &str,
        agent_id: &str,
        kind: &str,
        input: &Value,
    ) -> R<Option<Value>> {
        let slot = match kind {
            "trigger" => "trigger_plugin",
            "context" => "context_plugin",
            _ => return Err("invalid: hook kind".into()),
        };
        let agent = self.get_agent(owner, false, agent_id)?;
        let Some(instance) = agent[slot].as_str() else {
            return Ok(None);
        };
        let plugin = self.get("plugin", id(instance)?)?;
        if plugin["enabled"] != true
            || !(includes(&agent["grants"]["plugins"], instance)
                || includes(&plugin["allowed_agents"], agent_id))
        {
            return Err("forbidden: hook binding".into());
        }
        let package = self.package_for(&plugin)?;
        let ctx = json!({"agent_id":agent_id,"agent_config":agent["extensions"][instance],"settings":self.get("settings",instance).unwrap_or(json!({}))});
        let value = lua::call_hook(string(&package, "code")?, kind, &ctx, input)?;
        if (kind == "trigger" && !value.is_boolean()) || (kind == "context" && !value.is_array()) {
            return Err("invalid: hook result".into());
        }
        Ok(Some(value))
    }
    pub fn sections(&self, owner: &str, admin: bool, agent: &str) -> R<Vec<Value>> {
        let a = self.get_agent(owner, admin, agent)?;
        let mut ss = vec![];
        for i in self.all("plugin")? {
            if i["enabled"] == true
                && (includes(&a["grants"]["plugins"], string(&i, "id")?)
                    || includes(&i["allowed_agents"], agent))
            {
                for s in array(&self.package_for(&i)?["sections"]) {
                    let mut s = s.clone();
                    s["instance"] = i["id"].clone();
                    s["revision"] = i["revision"].clone();
                    ss.push(s);
                }
            }
        }
        Ok(ss)
    }
    pub fn subscription_plugins(&self) -> R<Vec<Value>> {
        let mut out = vec![];
        for i in self.all("plugin")? {
            if i["enabled"] == true {
                let p = self.package_for(&i)?;
                let k = string(&i, "id")?;
                out.push(json!({"id":k,"package":i["package"],"version":i["version"],"revision":i["revision"],"owner":i["owner"].as_str().unwrap_or("admin"),"code":p["code"],"settings":self.get("settings",k).unwrap_or(json!({}))}));
            }
        }
        Ok(out)
    }
    fn resource(&self, path: impl AsRef<Path>) -> R<PathBuf> {
        safe(&self.home, path.as_ref())
    }
    pub(super) fn file(&self, path: &Path) -> R<String> {
        let p = path.strip_prefix(self.home.as_path()).map_err(err)?;
        fs::read_to_string(self.resource(p)?).map_err(err)
    }
    fn load_package(&self, key: &str, ver: &str, dir: &Path) -> R<Value> {
        let manifest: Value =
            serde_yaml::from_str(&self.file(&dir.join("plugin.yaml"))?).map_err(err)?;
        if manifest["id"] != key || manifest["version"].as_str() != Some(ver) {
            return Err("invalid: package identity mismatch".into());
        }
        let code = self.file(&dir.join(manifest["entrypoint"].as_str().unwrap_or("plugin.lua")))?;
        let mut templates = vec![];
        for f in array(&manifest["agent_templates"]) {
            let f = f.as_str().ok_or("invalid: template path")?;
            templates.push(serde_yaml::from_str::<Value>(&self.file(&dir.join(f))?).map_err(err)?);
        }
        self.record_package(key, ver, manifest, code, templates)
    }
    fn record_package(
        &self,
        key: &str,
        ver: &str,
        manifest: Value,
        code: String,
        templates: Vec<Value>,
    ) -> R<Value> {
        let (tools, sets) = lua::declarations(&code)?;
        let (routes, sections) = lua::extensions(&code)?;
        for t in &tools {
            descriptor(t.clone(), "plugin", id(string(t, "id")?)?)?;
        }
        for t in &sets {
            id(string(t, "id")?)?;
        }
        let mut seen = BTreeSet::new();
        for t in &templates {
            if !seen.insert(id(string(t, "id")?)?) || !t["spec"].is_object() {
                return Err("invalid: duplicate or malformed Agent template".into());
            }
        }
        let mut resources = BTreeSet::new();
        for r in &routes {
            let path = string(r, "path")?;
            let method = string(r, "method")?;
            if !path.starts_with('/')
                || path.contains(['?', '#'])
                || path.split('/').any(|x| x == ".." || x == ".")
                || !matches!(method, "GET" | "POST" | "PATCH" | "DELETE" | "PUT")
                || !resources.insert((method, path))
            {
                return Err("invalid: duplicate or malformed plugin route".into());
            }
        }
        for key in ["settings_schema", "agent_config_schema"] {
            if let Some(s) = manifest.get(key) {
                jsonschema::validator_for(s).map_err(|e| format!("invalid {key}: {e}"))?;
            }
        }
        let k = format!("{key}/{ver}");
        let d = json!({"id":key,"version":ver,"manifest":manifest,"code":code,"templates":templates,"tools":tools,"toolsets":sets,"routes":routes,"sections":sections});
        if let Ok(old) = self.get("package", &k) {
            let revision = sha(&d);
            if old["revision"] == revision {
                return Ok(old);
            }
            // Older hosts stored the same package source without its derived WebUI metadata.
            // Compare the entire previous representation before updating its catalog revision.
            let mut previous = d.clone();
            for tool in previous["tools"]
                .as_array_mut()
                .expect("tool declarations array")
            {
                tool.as_object_mut()
                    .expect("tool declaration")
                    .remove("webui");
            }
            if old["revision"] != sha(&previous) {
                return Err("conflict: package version is immutable".into());
            }
            return self
                .save("package", &k, d, old["revision"].as_str(), None)
                .or_else(|e| {
                    // Another worker may finish the same upgrade first.
                    self.get("package", &k)
                        .ok()
                        .filter(|current| current["revision"] == revision)
                        .ok_or(e)
                });
        }
        self.save("package", &k, d, Some(""), None)
    }
    fn load_home_tool(&self, dir: &Path) -> R<()> {
        let key = leaf(dir)?;
        let name = scoped("home", key);
        self.save_with("tool", &name, None, None, || {
            let mut src =
                serde_yaml::from_str::<Value>(&self.file(&dir.join("tool.yaml"))?).map_err(err)?;
            if src["id"] != key {
                return Err("invalid: tool ID does not match directory".into());
            }
            let code = self.file(&dir.join(string(&src, "handler")?))?;
            lua::validate(&code)?;
            normalize_tool_webui(&mut src)?;
            let mut t = descriptor(src, "home", &name)?;
            t["code"] = json!(code);
            Ok(t)
        })?;
        Ok(())
    }
    fn instance_tools(&self, inst: &Value) -> R<()> {
        if inst["enabled"] != true {
            return Ok(());
        }
        let package = self.package_for(inst)?;
        let key = string(inst, "id")?;
        let settings = self.get("settings", key).unwrap_or_else(|_| json!({}));
        for local in array(&package["tools"]) {
            let name = string(local, "id")?;
            id(name)?;
            let k = scoped(key, name);
            let mut t = descriptor(local.clone(), "plugin", &k)?;
            t["local_id"] = json!(name);
            t["instance"] = json!(key);
            t["instance_revision"] = inst["revision"].clone();
            t["version"] = package["version"].clone();
            t["code"] = package["code"].clone();
            t["allowed_agents"] = inst["allowed_agents"].clone();
            t["settings"] = settings.clone();
            t["settings_revision"] = settings["revision"].clone();
            self.save("tool", &k, t, None, None)?;
        }
        for local in array(&package["toolsets"]) {
            let k = scoped(key, string(local, "id")?);
            let members: Vec<_> = array(&local["members"])
                .filter_map(Value::as_str)
                .map(|n| {
                    if n.contains('.') {
                        n.into()
                    } else {
                        scoped(key, n)
                    }
                })
                .collect();
            self.save("toolset",&k,json!({"id":k,"members":members,"instance":key,"instance_revision":inst["revision"]}),None,None)?;
        }
        Ok(())
    }
    pub(super) fn source_enabled(&self, item: &Value) -> R<bool> {
        if let Some(i) = item["instance"].as_str() {
            let inst = self.get("plugin", i)?;
            if inst["enabled"] != true {
                return Ok(false);
            }
            if let Some(v) = item.get("instance_revision")
                && inst["revision"] != *v
            {
                return Ok(false);
            }
        }
        if item["source"] == "oi" {
            if self.get("settings", string(item, "instance")?)?["revision"]
                != item["settings_revision"]
            {
                return Ok(false);
            }
            if item["toolset"].is_string() {
                let set = self.get("toolset", string(item, "toolset")?);
                return Ok(set.is_ok_and(|s| {
                    s["ready"] == true && includes(&s["members"], item["id"].as_str().unwrap_or(""))
                }));
            }
            return Ok(item["ready"] == true);
        }
        if item["source"] == "mcp" {
            let source = self.get("connection", string(item, "connection_key")?)?;
            let cache = self.get("discovery", string(item, "connection_key")?);
            return Ok(source["enabled"] != false
                && source["revision"] == item["connection_revision"]
                && cache.is_ok_and(|c| {
                    c["healthy"] == true
                        && c["connection_revision"] == source["revision"]
                        && includes(&c["members"], item["id"].as_str().unwrap_or(""))
                }));
        }
        Ok(true)
    }
    pub(super) fn reload(&self) -> R<()> {
        self.reload_home_sets()?;
        for pkg in dirs(&self.resource("packages")?)? {
            if !pkg.is_dir() {
                continue;
            }
            let Some(key) = pkg.file_name().and_then(OsStr::to_str) else {
                continue;
            };
            id(key)?;
            for dir in dirs(&pkg)? {
                if !dir.is_dir() {
                    continue;
                }
                let Some(ver) = dir.file_name().and_then(OsStr::to_str) else {
                    continue;
                };
                id(ver)?;
                if let Err(e) = self.load_package(key, ver, &dir) {
                    self.diagnose("package", &format!("{key}/{ver}"), &e, &json!({"path":dir}))?;
                }
            }
        }
        for dir in dirs(&self.resource("plugins")?)? {
            if !dir.is_dir() {
                continue;
            }
            let key = leaf(&dir)?;
            let f = dir.join("instance.yaml");
            if !f.exists() {
                continue;
            }
            let load = (|| -> R<()> {
                let val: Value = serde_yaml::from_str(&self.file(&f)?).map_err(err)?;
                if val["id"] != key {
                    return Err("invalid: plugin ID".into());
                }
                let package = self.package_for(&val)?;
                let settings = dir.join("settings.yaml");
                if settings.exists() {
                    let mut v: Value = serde_yaml::from_str(&self.file(&settings)?).map_err(err)?;
                    if let Some(o) = v.as_object_mut() {
                        o.remove("revision");
                    }
                    if let Some(s) = package["manifest"].get("settings_schema") {
                        schema(s, &v)?
                    }
                    self.save("settings", key, v, None, None)?;
                }
                let inst = self.save("plugin", key, val, None, None)?;
                self.instance_tools(&inst)?;
                for path in dirs(&dir.join("toolsets"))? {
                    if path.extension() != Some(OsStr::new("yaml")) {
                        continue;
                    }
                    let mut s: Value = serde_yaml::from_str(&self.file(&path)?).map_err(err)?;
                    let k = scoped(key, string(&s, "id")?);
                    s["id"] = json!(k);
                    s["instance"] = json!(key);
                    self.save("toolset", &k, s, None, None)?;
                }
                for path in dirs(&dir.join("connections"))? {
                    if path.extension() != Some(OsStr::new("yaml")) {
                        continue;
                    }
                    let mut c: Value = serde_yaml::from_str(&self.file(&path)?).map_err(err)?;
                    let name = id(string(&c, "id")?)?.to_owned();
                    c["instance"] = json!(key);
                    self.save("connection", &scoped(key, &name), c, None, None)?;
                }
                Ok(())
            })();
            if let Err(e) = load {
                self.diagnose("plugin", key, &e, &json!({"path":f}))?
            }
        }
        for dir in dirs(&self.resource("tools")?)? {
            if !dir.is_dir() {
                continue;
            }
            if let Err(e) = self.load_home_tool(&dir) {
                self.diagnose("tool",&scoped("home",leaf(&dir)?),&e,&json!({"path":dir,"definition":self.file(&dir.join("tool.yaml")).ok(),"code":self.file(&dir.join("handler.lua")).ok()}))?;
            }
        }
        for dir in dirs(&self.resource("agents")?)? {
            if !dir.is_dir() {
                continue;
            }
            let key = leaf(&dir)?;
            let f = dir.join("agent.yaml");
            if let Err(e) = self.reload_agent(key, &f) {
                self.diagnose("agent", key, &e, &json!({"path":f}))?
            }
        }
        Ok(())
    }
    fn install(&self, key: &str, ver: &str, body: &Value) -> R<Value> {
        id(key)?;
        id(ver)?;
        let mut m = body.get("manifest").cloned().unwrap_or(json!({}));
        m["id"] = json!(key);
        m["version"] = json!(ver);
        m["entrypoint"] = json!("plugin.lua");
        let code = string(body, "code")?.to_owned();
        let templates: Vec<_> = array(&body["templates"]).cloned().collect();
        let mut paths = vec![];
        for t in &templates {
            paths.push(format!("templates/agents/{}.yaml", id(string(t, "id")?)?))
        }
        if !paths.is_empty() {
            m["agent_templates"] = json!(paths)
        }
        if array(&m["agent_templates"]).count() != templates.len() {
            return Err("invalid: template files missing".into());
        }
        let package = self.record_package(key, ver, m.clone(), code.clone(), templates.clone())?;
        let dir = self.resource(PathBuf::from("packages").join(key).join(ver))?;
        fs::create_dir_all(&dir).map_err(err)?;
        for (p, t) in paths.iter().zip(templates) {
            let f = safe(&dir, Path::new(p))?;
            write(&f, serde_yaml::to_string(&t).map_err(err)?.as_bytes())?
        }
        write(&safe(&dir, Path::new("plugin.lua"))?, code.as_bytes())?;
        write(
            &safe(&dir, Path::new("plugin.yaml"))?,
            serde_yaml::to_string(&m).map_err(err)?.as_bytes(),
        )?;
        Ok(package)
    }
    fn plugin_write(&self, key: &str, patch: &Value, create: bool) -> R<Value> {
        id(key)?;
        let mut val = if create {
            json!({"enabled":false})
        } else {
            self.get("plugin", key)?
        };
        let old = val["revision"].as_str().unwrap_or("").to_owned();
        merge(&mut val, patch);
        val["id"] = json!(key);
        self.package_for(&val)?;
        let p = self.resource(PathBuf::from("plugins").join(key).join("instance.yaml"))?;
        let v = self.save(
            "plugin",
            key,
            val,
            Some(patch["expected_revision"].as_str().unwrap_or(&old)),
            Some(&p),
        )?;
        self.instance_tools(&v)?;
        Ok(v)
    }
    fn settings_write(&self, key: &str, body: &Value) -> R<Value> {
        let inst = self.get("plugin", id(key)?)?;
        let pkg = self.package_for(&inst)?;
        let mut s = self.get("settings", key).unwrap_or(json!({}));
        let rev = s["revision"].as_str().unwrap_or("").to_owned();
        merge(&mut s, body);
        if let Some(o) = s.as_object_mut() {
            o.remove("expected_revision");
            o.remove("revision");
        }
        if let Some(def) = pkg["manifest"].get("settings_schema") {
            schema(def, &s)?;
        }
        let p = self.resource(PathBuf::from("plugins").join(key).join("settings.yaml"))?;
        let v = self.save(
            "settings",
            key,
            s,
            Some(body["expected_revision"].as_str().unwrap_or(&rev)),
            Some(&p),
        )?;
        self.instance_tools(&inst)?;
        Ok(v)
    }
    pub(super) async fn builtin_call(
        &self,
        owner: &str,
        a: &Value,
        name: &str,
        args: Value,
    ) -> R<Value> {
        let key = args["id"].as_str().unwrap_or("");
        let agent = string(a, "id")?;
        let plugin_ok = includes(&a["grants"]["plugins"], key);
        match name {
            "agents.get" => Ok(a.clone()),
            "harness.inspect" => Ok(
                json!({"templates":self.templates_for(owner,false)?,"version":env!("CARGO_PKG_VERSION")}),
            ),
            "harness.docs" => {
                let document = args["id"].as_str().unwrap_or("agent-authoring");
                let reference = args["version"]
                    .as_str()
                    .map(|v| format!("{document}@{v}"))
                    .unwrap_or_else(|| document.to_string());
                let mut guide = Self::guide(&reference)?;
                if guide["id"] == "agent-authoring" {
                    guide["authoring"] = json!(
                        "Return a Lua function taking loom. Call loom.register_tool({id,description,input_schema,handler=function(ctx,args) ... end}). A handler sees only its instance settings and agent_config. If granted, ctx.read_note(args.note_ref) reads that Note. Install with id, version, manifest, code; enable an authorized instance and bind its qualified tool ID."
                    );
                }
                Ok(guide)
            }
            "plugins.get" if plugin_ok => {
                if let Some(ver) = args["version"].as_str() {
                    self.get("package", &format!("{}/{}", id(key)?, id(ver)?))
                } else {
                    self.get("plugin", key)
                }
            }
            "plugins.install" if plugin_ok => self.install(key, string(&args, "version")?, &args),
            "plugins.set_enabled" if plugin_ok => {
                if args["package"] == "internal-tools"
                    || key == "internal-tools"
                    || self
                        .get("plugin", key)
                        .is_ok_and(|i| i["package"] == "internal-tools")
                {
                    return Err(
                        "forbidden: OIS connection management requires an administrator".into(),
                    );
                }
                let mut p = json!({"enabled":args["enabled"].as_bool().ok_or("invalid: enabled")?});
                let create = self.get("plugin", key).is_err();
                if create {
                    p["owner"] = json!(owner);
                    p["allowed_owners"] = json!([owner]);
                }
                let package = args["package"].as_str().unwrap_or(key);
                if package != key && !includes(&a["grants"]["plugins"], package) {
                    return Err("forbidden: package".into());
                }
                if create || args.get("package").is_some() {
                    p["package"] = json!(package)
                }
                if let Some(v) = args.get("version") {
                    p["version"] = v.clone()
                }
                self.plugin_write(key, &p, create)
            }
            "agents.update" if key == agent || key.is_empty() => {
                self.patch_agent(owner, false, agent, args["patch"].clone())
            }
            "settings.get" if plugin_ok => self.get("settings", key),
            "settings.update"
                if plugin_ok && self.get("plugin", key)?["package"] != "internal-tools" =>
            {
                self.settings_write(key, &args["settings"])
            }
            "sessions.history" | "events.search" | "write_note" | "search_notes" | "read_note"
            | "context.preview" => Err("runtime: session context required".into()),
            _ => Err("forbidden: target outside delegated scope".into()),
        }
    }
    pub async fn manage(
        &self,
        owner: &str,
        admin: bool,
        method: &str,
        path: &str,
        body: Value,
    ) -> R<Value> {
        let p: Vec<_> = path.trim_matches('/').split('/').collect();
        let p = p.strip_prefix(&["api"]).unwrap_or(&p);
        if method == "GET" {
            match p {
                ["docs"] => {
                    return Ok(json!([
                        "agent-authoring@1",
                        "tool-sources@1",
                        "plugin-contracts@1"
                    ]));
                }
                ["docs", reference] => return Self::guide(reference),
                ["openapi.json"] => {
                    return Ok(
                        json!({"openapi":"3.1.0","info":{"title":"Loom installed plugin routes","version":env!("CARGO_PKG_VERSION")},"paths":self.plugin_routes(owner,admin)?}),
                    );
                }
                _ => {}
            }
        }
        if method == "GET" && p == ["agent-templates"] {
            return self.templates_for(owner, admin);
        }
        if let ["extensions", instance, tail @ ..] = p {
            let inst = self.get("plugin", id(instance)?)?;
            if inst["enabled"] != true || (!admin && !includes(&inst["allowed_owners"], owner)) {
                return Err("forbidden: plugin service".into());
            }
            let route = format!("/{}", tail.join("/"));
            let pkg = self.package_for(&inst)?;
            let ctx = json!({"owner":owner,"instance":instance,"settings":self.get("settings",instance).unwrap_or(json!({}))});
            return lua::run_route(string(&pkg, "code")?, method, &route, &ctx, &body);
        }
        if !admin {
            return Err("forbidden: catalog administration".into());
        }
        match (method, p) {
            ("POST", ["plugins", instance, "refresh"]) => self.refresh_oi(instance).await,
            ("GET", ["plugins", instance, "discovery"]) => self.oi_status(instance),
            ("GET", ["plugins"]) => Ok(json!(self.all("plugin")?)),
            ("POST", ["plugins"]) => self.plugin_write(string(&body, "id")?, &body, true),
            ("GET", ["plugins", p]) => self.get("plugin", id(p)?),
            ("PATCH", ["plugins", p]) => self.plugin_write(p, &body, false),
            ("POST", ["plugins", p, "versions"]) => {
                self.install(p, string(&body, "version")?, &body)
            }
            ("GET", ["plugins", p, "versions"]) => Ok(json!(
                self.all("package")?
                    .into_iter()
                    .filter(|v| v["id"] == *p)
                    .collect::<Vec<_>>()
            )),
            ("GET", ["plugins", p, "versions", v]) => {
                self.get("package", &format!("{}/{}", id(p)?, id(v)?))
            }
            ("GET", ["settings", p]) => {
                self.get("plugin", id(p)?)?;
                Ok(self.get("settings", p).unwrap_or(json!({})))
            }
            ("GET", ["settings", p, "schema"]) => Ok(self
                .package_for(&self.get("plugin", id(p)?)?)?["manifest"]["settings_schema"]
                .clone()),
            ("GET", ["settings", p, "versions", r]) => self.version("settings", id(p)?, r),
            ("PATCH", ["settings", p]) => self.settings_write(p, &body),
            ("POST", ["catalog", "apply"]) => {
                self.reload()?;
                self.diagnostics()
            }
            ("GET", ["catalog", "diagnostics"]) => self.diagnostics(),
            ("GET", ["catalog", "tools"]) => Ok(json!(self.all("tool")?)),
            ("GET", ["catalog", "tools", tool, "versions", revision]) => {
                self.version("tool", tool, revision)
            }
            ("GET", ["mcp", "connections"]) => Ok(json!(self.all("connection")?)),
            ("POST", ["mcp", "connections"]) => self.connection_write(
                string(&body, "instance")?,
                string(&body, "id")?,
                &body,
                true,
            ),
            ("GET", ["mcp", "connections", i, c]) => {
                self.get("connection", &scoped(id(i)?, id(c)?))
            }
            ("PATCH", ["mcp", "connections", i, c]) => self.connection_write(i, c, &body, false),
            ("POST", ["mcp", "connections", i, c, "refresh"]) => self.discover(i, c).await,
            ("GET", ["mcp", "connections", i, c, "discovery"]) => {
                self.get("discovery", &scoped(id(i)?, id(c)?))
            }
            _ => Err("not found: catalog route".into()),
        }
    }
    fn connection_write(&self, i: &str, c: &str, body: &Value, create: bool) -> R<Value> {
        self.get("plugin", id(i)?)?;
        id(c)?;
        let key = scoped(i, c);
        let mut v = if create {
            json!({})
        } else {
            self.get("connection", &key)?
        };
        let rev = v["revision"].as_str().unwrap_or("").to_owned();
        merge(&mut v, body);
        v["instance"] = json!(i);
        v["id"] = json!(c);
        let url = reqwest::Url::parse(string(&v, "url")?).map_err(err)?;
        if !["http", "https"].contains(&url.scheme())
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err("invalid: MCP URL".into());
        }
        let p = self.resource(
            PathBuf::from("plugins")
                .join(i)
                .join("connections")
                .join(format!("{c}.yaml")),
        )?;
        self.save(
            "connection",
            &key,
            v,
            Some(body["expected_revision"].as_str().unwrap_or(&rev)),
            Some(&p),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_package_can_recover_its_declared_suggestion_without_changing_source() {
        let home = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(home.path()).unwrap();
        let code = "register_tool({id='read',webui={suggestion={label=' Read ',prompt=' Read it '}},handler=function() return 1 end})";
        let old = json!({
            "id":"hints", "version":"1", "manifest":{"id":"hints","version":"1","entrypoint":"plugin.lua"},
            "code":code, "templates":[], "tools":[{"id":"read"}],
            "toolsets":[], "routes":[], "sections":[]
        });
        catalog
            .save("package", "hints/1", old, Some(""), None)
            .unwrap();
        let request = json!({"code":code});
        let upgraded = catalog.install("hints", "1", &request).unwrap();
        assert_eq!(
            upgraded["tools"][0]["webui"]["suggestion"],
            json!({"label":"Read","prompt":"Read it"})
        );
        assert_eq!(catalog.install("hints", "1", &request).unwrap(), upgraded);
        let changed = json!({"code":code.replace("Read it","Run it")});
        assert_eq!(
            catalog.install("hints", "1", &changed).unwrap_err(),
            "conflict: package version is immutable"
        );
    }
}
