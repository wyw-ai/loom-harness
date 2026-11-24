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
fn descriptor(mut t: Value, source: &str, name: &str) -> R<Value> {
    if !t.is_object() {
        return Err("invalid: invalid tool declaration".into());
    }
    t["id"] = json!(name);
    t["source"] = json!(source);
    t["direct"] = json!(t["exposure"] == "direct");
    if t.get("input_schema").is_none() {
        t["input_schema"] = json!({"type":"object"})
    }
    if t.get("description").is_none() {
        t["description"] = json!(name)
    }
    Ok(t)
}
impl Catalog {
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
    fn file(&self, path: &Path) -> R<String> {
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
        let k = format!("{key}/{ver}");
        let d = json!({"id":key,"version":ver,"manifest":manifest,"code":code,"templates":templates,"tools":tools,"toolsets":sets,"routes":routes,"sections":sections});
        if let Ok(old) = self.get("package", &k) {
            if old["revision"] != sha(&d) {
                return Err("conflict: package version is immutable".into());
            }
            return Ok(old);
        }
        self.save("package", &k, d, Some(""), None)
    }
    fn load_home_tool(&self, dir: &Path) -> R<()> {
        let key = leaf(dir)?;
        let src =
            serde_yaml::from_str::<Value>(&self.file(&dir.join("tool.yaml"))?).map_err(err)?;
        if src["id"] != key {
            return Err("invalid: tool ID does not match directory".into());
        }
        let code = self.file(&dir.join(string(&src, "handler")?))?;
        lua::validate(&code)?;
        let name = scoped("home", key);
        let mut t = descriptor(src, "home", &name)?;
        t["code"] = json!(code);
        self.save("tool", &name, t, None, None)?;
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
            if let Some(v) = item.get("instance_revision") {
                if inst["revision"] != *v {
                    return Ok(false);
                }
            }
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
                self.package_for(&val)?;
                let settings = dir.join("settings.yaml");
                if settings.exists() {
                    let v = serde_yaml::from_str(&self.file(&settings)?).map_err(err)?;
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
            let load = (|| -> R<()> {
                let mut a: Value = serde_yaml::from_str(&self.file(&f)?).map_err(err)?;
                if a["id"] != key {
                    return Err("invalid: agent ID".into());
                }
                if a.get("owner").is_none() {
                    a["owner"] = json!("admin")
                }
                self.validate_agent(&a)?;
                self.save("agent", key, a, None, None)?;
                Ok(())
            })();
            if let Err(e) = load {
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
            "harness.inspect" => {
                Ok(json!({"templates":self.templates_for(owner,false)?,"version":env!("CARGO_PKG_VERSION")}))
            }
            "harness.docs" => Ok(
                json!({"version":env!("CARGO_PKG_VERSION"),"guide":include_str!("../../docs/spec/reference/home.md"),"authoring":"Return a Lua function taking loom, then call loom.register_tool({id,description,input_schema,handler=function(ctx,args) ... end}). A handler receives only its instance settings and agent_config. Install with id, version, manifest, code; enable an authorized instance and bind its qualified tool ID."}),
            ),
            "plugins.get" if plugin_ok => {
                if let Some(ver) = args["version"].as_str() {
                    self.get("package", &format!("{}/{}", id(key)?, id(ver)?))
                } else {
                    self.get("plugin", key)
                }
            }
            "plugins.install" if plugin_ok => self.install(key, string(&args, "version")?, &args),
            "plugins.set_enabled" if plugin_ok => {
                let mut p = json!({"enabled":args["enabled"].as_bool().ok_or("invalid: enabled")?});
                let create = self.get("plugin", key).is_err();
                if create { p["owner"]=json!(owner);p["allowed_owners"]=json!([owner]); }
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
            "settings.update" if plugin_ok => self.settings_write(key, &args["settings"]),
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
        if method == "GET" && p == ["agent-templates"] {
            return self.templates_for(owner,admin);
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
            ("PATCH", ["settings", p]) => self.settings_write(p, &body),
            ("POST", ["catalog", "apply"]) => {
                self.reload()?;
                self.diagnostics()
            }
            ("GET", ["catalog", "diagnostics"]) => self.diagnostics(),
            ("GET", ["catalog", "tools"]) => Ok(json!(self.all("tool")?)),
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
