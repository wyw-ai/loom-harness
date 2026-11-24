use super::resources::{descriptor, normalize_tool_webui, scoped};
use super::*;

fn catalog_key(local: &str) -> R<String> {
    Ok(scoped("home", id(local)?))
}

fn set_record(local: &str, members: &Value) -> R<Value> {
    let rows = members
        .as_array()
        .ok_or("invalid: expected members array")?;
    let mut seen = BTreeSet::new();
    for member in rows {
        let key = member
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .ok_or("invalid: tool ID")?;
        if !seen.insert(key) {
            return Err("invalid: duplicate tool ID".into());
        }
    }
    Ok(json!({"id":catalog_key(local)?,"source":"home","members":rows}))
}

impl Catalog {
    pub(super) fn reload_home_sets(&self) -> R<()> {
        for path in store::dirs(&safe(&self.home, Path::new("toolsets"))?)? {
            if path.extension().and_then(|s| s.to_str()) != Some("yaml") {
                continue;
            }
            let local = path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or("invalid: toolset filename")?;
            let key = catalog_key(local)?;
            let result = self.save_with("toolset", &key, None, None, || {
                let yaml: Value = serde_yaml::from_str(&self.file(&path)?).map_err(err)?;
                if yaml["id"] != local {
                    return Err("invalid: toolset ID does not match filename".into());
                }
                set_record(local, &yaml["members"])
            });
            if let Err(e) = result {
                self.diagnose("toolset", &key, &e, &json!({"path":path}))?;
            }
        }
        Ok(())
    }

    pub fn catalog_entries(&self, kind: &str) -> R<Value> {
        let mut values = self.all(kind)?;
        for v in &mut values {
            v["editable"] = json!(v["source"] == "home" && v["instance"].is_null());
        }
        for (name, group) in [
            ("loom.daily", DAILY),
            ("plugins.authoring", AUTHOR),
            ("loom.self_management", SELF),
        ] {
            if kind == "toolset" {
                values.push(json!({"id":name,"members":group,"source":"builtin","editable":false}));
            } else {
                values.extend(group.iter().map(|name| builtin(name)));
            }
        }
        Ok(json!(values))
    }

    pub fn write_home_entry(&self, kind: &str, target: Option<&str>, change: &Value) -> R<Value> {
        let (local, old) = if let Some(key) = target {
            let old = self.get(kind, key)?;
            if old["source"] != "home" || !old["instance"].is_null() {
                return Err("forbidden: edit this resource at its source".into());
            }
            let local = key
                .strip_prefix("home.")
                .ok_or("invalid: expected HOME id")?
                .replace("%2E", ".");
            if catalog_key(&local)? != key {
                return Err("invalid: non-canonical resource ID".into());
            }
            (local, old)
        } else {
            (id(string(change, "id")?)?.to_owned(), json!({}))
        };
        let key = catalog_key(&local)?;
        let old_revision = old["revision"].as_str().unwrap_or("");
        let expect = if target.is_some() {
            change["expected_revision"].as_str().unwrap_or(old_revision)
        } else {
            ""
        };
        if kind == "toolset" {
            let value = set_record(&local, &change["members"])?;
            let yaml = json!({"kind":"Toolset","id":local,"members":value["members"]});
            let path = safe(
                &self.home,
                &PathBuf::from("toolsets").join(format!("{local}.yaml")),
            )?;
            return self.save_with(kind, &key, Some(expect), Some((&path, Some(&yaml))), || {
                Ok(value)
            });
        }
        let dir = PathBuf::from("tools").join(&local);
        let path = safe(&self.home, &dir.join("tool.yaml"))?;
        let mut yaml = if target.is_some() {
            serde_yaml::from_str::<Value>(&self.file(&path)?).map_err(err)?
        } else {
            json!({"kind":"Tool","id":local})
        };
        for field in [
            "description",
            "input_schema",
            "output_schema",
            "exposure",
            "requires_approval",
            "approval",
            "allowed_agents",
            "webui",
        ] {
            if let Some(v) = change.get(field) {
                yaml[field] = v.clone();
            }
        }
        normalize_tool_webui(&mut yaml)?;
        let code = change
            .get("code")
            .or_else(|| old.get("code"))
            .and_then(Value::as_str)
            .ok_or("invalid: missing tool code")?;
        lua::validate(code)?;
        // Publish complete source first. Replacing the declaration atomically then
        // selects that exact source, even across crashes and competing workers.
        let handler = format!("handler-{}.lua", sha(&json!(code)));
        yaml["handler"] = json!(handler);
        let code_path = safe(&self.home, &dir.join(handler))?;
        let mut value = descriptor(yaml.clone(), "home", &key)?;
        value["code"] = json!(code);
        self.save_with(kind, &key, Some(expect), Some((&path, Some(&yaml))), || {
            if !code_path.is_file() {
                store::write(&code_path, code.as_bytes())?;
            }
            Ok(value)
        })
    }
}
