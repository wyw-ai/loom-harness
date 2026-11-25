use super::*;
use rusqlite::TransactionBehavior;

const WEBUI_MANIFEST: &str = include_str!("bundled/webui/plugin.yaml");
const WEBUI_ENTRYPOINT: &str = include_str!("bundled/webui/plugin.lua");
const WEBUI_VERSION: &str = "1.3.0";
const WEBUI_1_2_ASSETS: &[&str] = &[
    "index.html",
    "app.css",
    "app.js",
    "desk.js",
    "shell.js",
    "richtext.js",
    "chat-flow.css",
    "desk-workspace.css",
];
const WEBUI_ASSETS: &[(&str, &str, &str)] = &[
    (
        "index.html",
        "text/html; charset=utf-8",
        include_str!("bundled/webui/index.html"),
    ),
    (
        "app.css",
        "text/css; charset=utf-8",
        include_str!("bundled/webui/app.css"),
    ),
    (
        "app.js",
        "text/javascript; charset=utf-8",
        include_str!("bundled/webui/app.js"),
    ),
    (
        "desk.js",
        "text/javascript; charset=utf-8",
        include_str!("bundled/webui/desk.js"),
    ),
    (
        "shell.js",
        "text/javascript; charset=utf-8",
        include_str!("bundled/webui/shell.js"),
    ),
    (
        "richtext.js",
        "text/javascript; charset=utf-8",
        include_str!("bundled/webui/richtext.js"),
    ),
    (
        "usage.js",
        "text/javascript; charset=utf-8",
        include_str!("bundled/webui/usage.js"),
    ),
    (
        "chat-flow.css",
        "text/css; charset=utf-8",
        include_str!("bundled/webui/chat-flow.css"),
    ),
    (
        "desk-workspace.css",
        "text/css; charset=utf-8",
        include_str!("bundled/webui/desk-workspace.css"),
    ),
];

const OFFICIAL: &[(&str, &str, &str)] = &[
    (
        "internal-tools",
        include_str!("bundled/internal-tools/plugin.yaml"),
        include_str!("bundled/internal-tools/plugin.lua"),
    ),
    (
        "compaction",
        include_str!("bundled/compaction/plugin.yaml"),
        include_str!("bundled/compaction/plugin.lua"),
    ),
    (
        "goal",
        include_str!("bundled/goal/plugin.yaml"),
        include_str!("bundled/goal/plugin.lua"),
    ),
    (
        "composer-suggestions",
        include_str!("bundled/composer-suggestions/plugin.yaml"),
        include_str!("bundled/composer-suggestions/plugin.lua"),
    ),
    ("webui", WEBUI_MANIFEST, WEBUI_ENTRYPOINT),
];

pub(super) fn is_official(package: &Value) -> bool {
    OFFICIAL.iter().any(|(id, manifest, code)| {
        package["id"] == *id
            && package["code"]
                .as_str()
                .is_some_and(|s| s.lines().eq(code.lines()))
            && serde_yaml::from_str::<Value>(manifest).is_ok_and(|v| v == package["manifest"])
    })
}

// Publish complete files once. Restarts preserve the selected version and enabled state.
fn seed(path: &Path, bytes: &[u8]) -> R<()> {
    if path.exists() {
        return Ok(());
    }
    store::write(path, bytes)
}
impl Catalog {
    pub fn package_index(&self) -> R<Value> {
        let mut entries = Vec::new();
        for package in self.all("package")? {
            let official = is_official(&package);
            entries.push(json!({"id":package["id"],"version":package["version"],
                "description":package["manifest"]["description"],"official":official}));
        }
        Ok(json!(entries))
    }
    pub(super) fn seed_official(&self) -> R<()> {
        let mut db = self.db()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(err)?;
        for &(id, manifest, code) in OFFICIAL {
            let version = if id == "webui" {
                WEBUI_VERSION
            } else {
                "1.0.0"
            };
            let pkg = PathBuf::from("packages").join(id).join(version);
            seed(&safe(&self.home, &pkg.join("plugin.lua"))?, code.as_bytes())?;
            seed(
                &safe(&self.home, &pkg.join("plugin.yaml"))?,
                manifest.as_bytes(),
            )?;
            if id == "internal-tools" {
                // Each configured OIS has its own explicitly enabled instance.
                continue;
            }
            if id == "webui" {
                for &(name, _, body) in WEBUI_ASSETS {
                    seed(&safe(&self.home, &pkg.join(name))?, body.as_bytes())?;
                }
            }
            let instance = json!({"apiVersion":"loom/v1alpha1","kind":"PluginInstance","id":id,"package":id,"version":version,"enabled":true});
            let p = safe(
                &self.home,
                &PathBuf::from("plugins").join(id).join("instance.yaml"),
            )?;
            if id == "webui" && p.is_file() {
                self.upgrade_webui_instance(&p)?;
            }
            seed(
                &p,
                serde_yaml::to_string(&instance).map_err(err)?.as_bytes(),
            )?;
        }
        tx.commit().map_err(err)
    }
    fn upgrade_webui_instance(&self, path: &Path) -> R<()> {
        // Existing installations retain membership, ownership and enabled state.
        let Ok(mut instance) = serde_yaml::from_str::<Value>(&self.file(path)?) else {
            return Ok(());
        };
        if instance["id"] != "webui" || instance["package"] != "webui" {
            return Ok(());
        }
        let (version, assets) = match instance["version"].as_str() {
            Some("1.0.0") => ("1.0.0", json!(["index.html", "app.css", "app.js"])),
            Some("1.1.0") => (
                "1.1.0",
                json!(["index.html", "app.css", "app.js", "desk.js"]),
            ),
            Some("1.2.0") => ("1.2.0", json!(WEBUI_1_2_ASSETS)),
            _ => return Ok(()),
        };
        let legacy = self.home.join("packages/webui").join(version);
        let mut expected = serde_yaml::from_str::<Value>(WEBUI_MANIFEST).map_err(err)?;
        expected["version"] = json!(version);
        expected["webui"]["assets"] = assets;
        if self
            .file(&legacy.join("plugin.yaml"))
            .ok()
            .and_then(|s| serde_yaml::from_str::<Value>(&s).ok())
            == Some(expected)
            && self
                .file(&legacy.join("plugin.lua"))
                .is_ok_and(|s| s.lines().eq(WEBUI_ENTRYPOINT.lines()))
        {
            instance["version"] = json!(WEBUI_VERSION);
            instance.as_object_mut().unwrap().remove("revision");
            store::write(
                path,
                serde_yaml::to_string(&instance).map_err(err)?.as_bytes(),
            )?;
        }
        Ok(())
    }
    pub fn webui_asset(&self, name: &str) -> R<Option<(&'static str, &'static str)>> {
        let Some(&(_, content_type, body)) = WEBUI_ASSETS.iter().find(|a| a.0 == name) else {
            return Ok(None);
        };
        let instance = self.get("plugin", "webui")?;
        if instance["enabled"] != true
            || instance["package"] != "webui"
            || instance["version"] != WEBUI_VERSION
        {
            return Ok(None);
        }
        // An existing HOME may already have a package with this name and version.
        // Only the official entrypoint and manifest can activate the compiled assets.
        let package = self.package_for(&instance)?;
        if !string(&package, "code")?
            .lines()
            .eq(WEBUI_ENTRYPOINT.lines())
            || package["manifest"] != serde_yaml::from_str::<Value>(WEBUI_MANIFEST).map_err(err)?
        {
            return Ok(None);
        }
        Ok(Some((content_type, body)))
    }
    fn strategy(
        &self,
        instance: &str,
        registration: &str,
        input: &Value,
    ) -> R<Option<(Value, Value)>> {
        let i = self.get("plugin", instance)?;
        if i["enabled"] != true {
            return Ok(None);
        }
        let package = self.package_for(&i)?;
        let settings = self.get("settings", instance).unwrap_or(json!({}));
        let revision = sha(&json!([
            i["revision"],
            package["revision"],
            settings.get("revision")
        ]));
        let result = lua::call_hook(
            string(&package, "code")?,
            registration,
            &json!({"instance":instance,"settings":settings}),
            input,
        )?;
        Ok(Some((
            json!({"instance":instance,"version":i["version"],"revision":revision}),
            result,
        )))
    }
    pub fn goal_strategy(&self, facts: &Value) -> R<Option<Value>> {
        let Some((mut meta, result)) = self.strategy("goal", "goal_strategy", facts)? else {
            return Ok(None);
        };
        let decision = string(&result, "decision")?;
        if !matches!(decision, "continue" | "wait" | "stop") {
            return Err("invalid: goal decision".into());
        }
        meta["decision"] = json!(decision);
        if decision == "continue" {
            meta["instruction"] = json!(string(&result, "instruction")?)
        }
        Ok(Some(meta))
    }
    pub fn suggestion_prompt(&self) -> R<Option<Value>> {
        self.text_strategy("composer-suggestions", "suggestion_prompt")
    }
    pub fn compaction_prompt(&self) -> R<Option<Value>> {
        self.text_strategy("compaction", "compaction_prompt")
    }
    fn text_strategy(&self, instance: &str, registration: &str) -> R<Option<Value>> {
        let Some((mut meta, result)) = self.strategy(instance, registration, &json!({}))? else {
            return Ok(None);
        };
        meta["prompt"] = json!(
            result
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| format!("invalid: {registration} must return a nonempty string"))?
        );
        Ok(Some(meta))
    }
    pub fn suggestion_access(&self, owner: &str, subject: &str) -> R<Value> {
        let settings = match self.get("settings", "composer-suggestions") {
            Ok(v) => v,
            Err(e) if e.starts_with("not found:") => json!({}),
            Err(e) => return Err(e),
        };
        let rule = &settings["principals"][owner][subject];
        Ok(json!({"enabled":rule["enabled"].as_bool().unwrap_or(true),
            "read_private":rule["read_private"].as_bool().unwrap_or(false),
            "timeout_ms":rule["timeout_ms"].as_u64().unwrap_or(15000),
            "max_samples_per_minute":rule["max_samples_per_minute"].as_u64().unwrap_or(30)}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn webui_serves_only_bundled_files_from_the_selected_official_package() {
        let home = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(home.path()).unwrap();
        let package_dir = home.path().join("packages/webui").join(WEBUI_VERSION);
        for &(name, content_type, body) in WEBUI_ASSETS {
            assert_eq!(fs::read_to_string(package_dir.join(name)).unwrap(), body);
            assert_eq!(
                catalog.webui_asset(name).unwrap(),
                Some((content_type, body))
            );
        }
        fs::write(package_dir.join("app.js"), "untrusted").unwrap();
        assert_ne!(
            catalog.webui_asset("app.js").unwrap().unwrap().1,
            "untrusted"
        );
        for path in ["plugin.lua", "../index.html", "foo/app.js", "%2e%2e/app.js"] {
            assert!(catalog.webui_asset(path).unwrap().is_none());
        }
        catalog
            .manage(
                "operator",
                true,
                "POST",
                "/api/plugins/webui/versions",
                json!({"version":"2.0.0","code":WEBUI_ENTRYPOINT}),
            )
            .await
            .unwrap();
        for selection in [
            json!({"version":"2.0.0"}),
            json!({"package":"goal","version":"1.0.0"}),
        ] {
            catalog
                .manage("operator", true, "PATCH", "/api/plugins/webui", selection)
                .await
                .unwrap();
            assert!(catalog.webui_asset("index.html").unwrap().is_none());
        }
    }

    #[test]
    fn official_webui_upgrade_preserves_disabled_instances_and_owner_scope() {
        for (version, assets) in [
            ("1.0.0", json!(["index.html", "app.css", "app.js"])),
            ("1.2.0", json!(WEBUI_1_2_ASSETS)),
            (
                "1.1.0",
                json!(["index.html", "app.css", "app.js", "desk.js"]),
            ),
        ] {
            let home = tempfile::tempdir().unwrap();
            let legacy = home.path().join("packages/webui").join(version);
            let mut manifest: Value = serde_yaml::from_str(WEBUI_MANIFEST).unwrap();
            manifest["version"] = json!(version);
            manifest["webui"]["assets"] = assets;
            store::write(
                &legacy.join("plugin.yaml"),
                serde_yaml::to_string(&manifest).unwrap().as_bytes(),
            )
            .unwrap();
            store::write(&legacy.join("plugin.lua"), WEBUI_ENTRYPOINT.as_bytes()).unwrap();
            let instance = json!({"id":"webui","package":"webui","version":version,"enabled":false,"allowed_owners":["qa"]});
            store::write(
                &home.path().join("plugins/webui/instance.yaml"),
                instance.to_string().as_bytes(),
            )
            .unwrap();
            let catalog = Catalog::open(home.path()).unwrap();
            let selected = catalog.get("plugin", "webui").unwrap();
            assert_eq!(selected["version"], WEBUI_VERSION);
            assert_eq!(selected["enabled"], false);
            assert_eq!(selected["allowed_owners"], instance["allowed_owners"]);
            assert!(catalog.webui_asset("desk.js").unwrap().is_none());
        }
    }

    #[test]
    fn preexisting_package_named_webui_cannot_claim_the_browser_route() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("packages/webui").join(WEBUI_VERSION);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("plugin.yaml"), WEBUI_MANIFEST).unwrap();
        fs::write(dir.join("plugin.lua"), "return function(loom) end").unwrap();
        let catalog = Catalog::open(home.path()).unwrap();
        assert!(catalog.webui_asset("index.html").unwrap().is_none());
    }

    #[tokio::test]
    async fn compaction_uses_the_enabled_lua_package_revision() {
        let home = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(home.path()).unwrap();
        let original = catalog.compaction_prompt().unwrap().unwrap();
        assert_eq!(original["instance"], "compaction");
        assert!(
            original["prompt"]
                .as_str()
                .unwrap()
                .starts_with("Summarize")
        );
        let route = "/api/plugins/compaction";
        catalog
            .manage("operator", true, "PATCH", route, json!({"enabled":false}))
            .await
            .unwrap();
        drop(catalog);
        let catalog = Catalog::open(home.path()).unwrap();
        assert!(catalog.compaction_prompt().unwrap().is_none());
        let source = "return function(loom) loom.register_compaction_prompt(function() return 'Summarize failure evidence.' end) end";
        catalog
            .manage(
                "operator",
                true,
                "POST",
                "/api/plugins/compaction/versions",
                json!({"version":"2.0.0","code":source}),
            )
            .await
            .unwrap();
        catalog
            .manage(
                "operator",
                true,
                "PATCH",
                route,
                json!({"enabled":true,"version":"2.0.0"}),
            )
            .await
            .unwrap();
        let current = catalog.compaction_prompt().unwrap().unwrap();
        assert_eq!(current["version"], "2.0.0");
        assert_ne!(current["revision"], original["revision"]);
        assert_eq!(current["prompt"], "Summarize failure evidence.");
    }
}
