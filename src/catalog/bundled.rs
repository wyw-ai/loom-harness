use super::*;
use rusqlite::TransactionBehavior;

const OFFICIAL: &[(&str, &str, &str)] = &[
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
];

// Publish complete files once. Restarts preserve the selected version and enabled state.
fn seed(path: &Path, bytes: &[u8]) -> R<()> {
    if path.exists() {
        return Ok(());
    }
    store::write(path, bytes)
}
impl Catalog {
    pub(super) fn seed_official(&self) -> R<()> {
        let mut db = self.db()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(err)?;
        for &(id, manifest, code) in OFFICIAL {
            let pkg = PathBuf::from("packages").join(id).join("1.0.0");
            seed(&safe(&self.home, &pkg.join("plugin.lua"))?, code.as_bytes())?;
            seed(
                &safe(&self.home, &pkg.join("plugin.yaml"))?,
                manifest.as_bytes(),
            )?;
            let instance = json!({"apiVersion":"loom/v1alpha1","kind":"PluginInstance","id":id,"package":id,"version":"1.0.0","enabled":true});
            let p = safe(
                &self.home,
                &PathBuf::from("plugins").join(id).join("instance.yaml"),
            )?;
            seed(
                &p,
                serde_yaml::to_string(&instance).map_err(err)?.as_bytes(),
            )?;
        }
        tx.commit().map_err(err)
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
        let Some((mut meta, result)) =
            self.strategy("composer-suggestions", "suggestion_prompt", &json!({}))?
        else {
            return Ok(None);
        };
        meta["prompt"] = json!(
            result
                .as_str()
                .ok_or("invalid: expected suggestion prompt")?
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
