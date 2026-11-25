use super::{resources::scoped, *};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::time::{Duration, Instant};

const PACKAGE: &str = "internal-tools";

pub(super) fn check_names(tools: &[Value]) -> R<()> {
    if !tools.iter().any(|tool| tool["source"] == "oi") {
        return Ok(());
    }
    let mut names = BTreeMap::from([
        ("code_exec".to_string(), "harness"),
        ("goal_complete".to_string(), "harness"),
    ]);
    for tool in tools.iter().filter(|t| t["direct"] == true) {
        let id = string(tool, "id")?;
        let name = crate::engine::model_name(tool, id);
        if let Some(previous) = names.insert(name.clone(), id) {
            return Err(format!(
                "conflict: model tool {name} is published by {previous} and {id}"
            ));
        }
    }
    Ok(())
}

fn connection(settings: &Value, service: &str, reference: &str) -> R<Value> {
    let mut url = reqwest::Url::parse(string(settings, "base_url")?).map_err(err)?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err("invalid: base_url must be the OIS HTTP origin".into());
    }
    url.set_path(&format!("/mcp/{}", id(service)?));
    Ok(json!({"url":url.as_str(),"credential_ref":id(reference)?}))
}

fn consumer(settings: &Value, service: &str) -> R<Value> {
    let reference = settings["service_credentials"]
        .get(service)
        .unwrap_or(&settings["consumer_credential_ref"]);
    connection(
        settings,
        service,
        reference
            .as_str()
            .ok_or("missing Service consumption credential")?,
    )
}

struct ServiceSnapshot {
    id: String,
    definitions: R<Vec<Value>>,
}

impl Catalog {
    fn oi_instance(&self, key: &str) -> R<Value> {
        let inst = self.get("plugin", id(key)?)?;
        if inst["package"] != PACKAGE || inst["version"] != "1.0.0" {
            return Err("not found: internal-tools instance".into());
        }
        if !bundled::is_official(&self.package_for(&inst)?) {
            return Err("forbidden: official internal-tools package required".into());
        }
        Ok(inst)
    }

    pub(super) fn oi_connection(&self, tool: &Value) -> R<Value> {
        let settings = self.get("settings", string(tool, "instance")?)?;
        if settings["revision"] != tool["settings_revision"] {
            return Err("conflict: OIS connection settings changed".into());
        }
        consumer(&settings, string(&tool["binding"], "service")?)
    }

    async fn oi_definitions(
        &self,
        inst: &Value,
        settings: &Value,
        service: &Value,
    ) -> R<Vec<Value>> {
        if service.pointer("/publish/mcp") != Some(&Value::Bool(true)) {
            return Err("Service does not publish MCP".into());
        }
        let instance = string(inst, "id")?;
        let sid = string(service, "id")?;
        let set = scoped(instance, sid);
        let conn = consumer(settings, sid)?;
        let mut actions = BTreeMap::new();
        for action in service["actions"]
            .as_array()
            .ok_or("invalid: missing Service actions")?
        {
            let source = &action["source"];
            for field in ["channel", "app", "action"] {
                string(source, field)?;
            }
            if actions.insert(string(action, "name")?, source).is_some() {
                return Err("invalid: duplicate Service member".into());
            }
        }
        let mut peer = self.peer(&conn).await?;
        let mut tools = vec![];
        let mut pages = 0;
        for remote in peer.definitions(&mut pages).await? {
            let name = string(&remote, "name")?;
            if name.is_empty()
                || name.len() > 64
                || !name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
                || matches!(name, "code_exec" | "goal_complete")
            {
                return Err(format!(
                    "invalid: Service {sid} published unsupported model function name {name}"
                ));
            }
            let source = actions
                .remove(name)
                .ok_or_else(|| format!("conflict: {sid}.{name} absent from Service directory"))?;
            let schema = &remote["inputSchema"];
            if !schema.is_object() {
                return Err(format!("invalid: {sid}.{name} input schema"));
            }
            jsonschema::validator_for(schema).map_err(err)?;
            let binding = json!({"transport":"mcp","instance":instance,"service":sid,"member":name,"source":source});
            tools.push(json!({"id":format!("{set}.{name}"), "source":"oi","instance":instance,"toolset":set,
                "binding":binding,"local_id":name,"model_name":name,"input_schema":schema,
                "description":remote["description"].as_str().unwrap_or(name),"direct":true,"effectful":true,
                "instance_revision":inst["revision"],"settings_revision":settings["revision"]}));
        }
        if !actions.is_empty() {
            return Err(format!("conflict: incomplete signatures for {sid}"));
        }
        Ok(tools)
    }

    async fn oi_snapshot(&self, inst: &Value, settings: &Value) -> R<Vec<ServiceSnapshot>> {
        let catalog = connection(
            settings,
            string(settings, "catalog_service")?,
            string(settings, "catalog_credential_ref")?,
        )?;
        let mut peer = self.peer(&catalog).await?;
        let reply = peer
            .message(
                "tools/call",
                json!({"name":"list_services","arguments":{}}),
                true,
            )
            .await?;
        if reply["isError"] == true {
            return Err("OI directory refused list_services".into());
        }
        let data = &reply["structuredContent"];
        if !data["next_cursor"].is_null() {
            return Err("invalid: incomplete OI service directory".into());
        }
        let services = data["items"]
            .as_array()
            .ok_or("invalid: OI directory requires items")?;
        let mut seen = BTreeSet::new();
        let mut snapshots = vec![];
        for service in services {
            let key = id(string(service, "id")?)?;
            if !seen.insert(key) {
                return Err(format!("invalid: duplicate OI Service {key}"));
            }
            snapshots.push(ServiceSnapshot {
                id: key.into(),
                definitions: self.oi_definitions(inst, settings, service).await,
            });
        }
        Ok(snapshots)
    }

    pub async fn refresh_oi(&self, key: &str) -> R<Value> {
        let inst = self.oi_instance(key)?;
        if inst["enabled"] != true {
            return Err("forbidden: internal-tools instance is disabled".into());
        }
        let settings = self.get("settings", key)?;
        let job = format!("oi/{key}");
        let generation: i64 = self.db()?.query_row(
            "INSERT INTO catalog_refresh VALUES(?1,1) ON CONFLICT(id) DO UPDATE SET generation=generation+1 RETURNING generation",
            [&job], |r|r.get(0)).map_err(err)?;
        let result = self.oi_snapshot(&inst, &settings).await;
        let mut db = self.db()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(err)?;
        let current: i64 = tx
            .query_row(
                "SELECT generation FROM catalog_refresh WHERE id=?1",
                [&job],
                |r| r.get(0),
            )
            .map_err(err)?;
        for (kind, expected) in [("plugin", &inst), ("settings", &settings)] {
            if current != generation
                || rev(&tx, kind, key)?.as_deref() != expected["revision"].as_str()
            {
                return Err("conflict: stale OI discovery".into());
            }
        }
        let old_sets = owned(&tx, "toolset", key)?;
        let mut remaining_tools = owned(&tx, "tool", key)?;
        let mut remaining_sets: BTreeSet<_> = old_sets.keys().cloned().collect();
        let error = result.as_ref().err().cloned();
        let services = match result {
            Ok(services) => services,
            Err(e) => old_sets
                .values()
                .filter_map(|set| set["service"].as_str())
                .map(|id| ServiceSnapshot {
                    id: id.into(),
                    definitions: Err(e.clone()),
                })
                .collect(),
        };
        let mut summary = vec![];
        for service in services {
            let set_id = scoped(key, &service.id);
            remaining_sets.remove(&set_id);
            let mut set = json!({"id":set_id,"source":"oi","instance":key,"service":service.id,"instance_revision":inst["revision"],"settings_revision":settings["revision"]});
            match service.definitions {
                Ok(tools) => {
                    set["ready"] = json!(true);
                    let mut members = vec![];
                    for tool in tools {
                        let name = string(&tool, "id")?.to_owned();
                        remaining_tools.remove(&name);
                        put(&tx, "tool", &name, tool)?;
                        members.push(name);
                    }
                    set["members"] = json!(members);
                }
                Err(e) => {
                    set["ready"] = json!(false);
                    set["error"] = json!(e);
                    let members = old_sets
                        .get(&set_id)
                        .and_then(|s| s["members"].as_array())
                        .cloned()
                        .unwrap_or_default();
                    for name in members.iter().filter_map(Value::as_str) {
                        remaining_tools.remove(name);
                    }
                    set["members"] = json!(members);
                }
            }
            summary.push(put(&tx, "toolset", &set_id, set)?);
        }
        for (kind, keys) in [
            ("tool", remaining_tools.into_keys().collect::<Vec<_>>()),
            ("toolset", remaining_sets.into_iter().collect()),
        ] {
            for key in keys {
                tx.execute(
                    "DELETE FROM catalog_current WHERE kind=?1 AND id=?2",
                    params![kind, key],
                )
                .map_err(err)?;
            }
        }
        let status = json!({"id":key,"healthy":error.is_none() && summary.iter().all(|s|s["ready"]==true),"error":error,"services":summary,
            "source":"oi","instance_revision":inst["revision"],"settings_revision":settings["revision"]});
        put(&tx, "oi-discovery", key, status.clone())?;
        tx.commit().map_err(err)?;
        self.oi_status(key)
    }

    pub fn oi_status(&self, key: &str) -> R<Value> {
        let instance = self.oi_instance(key)?;
        let mut status = self
            .get("oi-discovery", key)
            .unwrap_or_else(|_| json!({"id":key,"services":[],"healthy":false}));
        let settings = self.get("settings", key).unwrap_or(Value::Null);
        status["enabled"] = instance["enabled"].clone();
        if instance["enabled"] != true
            || status["instance_revision"] != instance["revision"]
            || status["settings_revision"] != settings["revision"]
        {
            status["healthy"] = json!(false);
            status["stale"] = json!(true);
        }
        let mut conflicts = Vec::new();
        for agent in self.all("agent")? {
            if let Err(error) = self.selected_tools(&agent)
                && error.starts_with("conflict: model tool ")
                && error.contains(&scoped(key, ""))
            {
                conflicts.push(json!({"agent_id":agent["id"],"error":error}));
            }
        }
        status["conflicts"] = json!(conflicts);
        Ok(status)
    }

    pub async fn sync_oi_instances(self) {
        let mut last = BTreeMap::<String, (Value, Instant, tokio::task::JoinHandle<()>)>::new();
        loop {
            if let Ok(instances) = self.all("plugin") {
                for inst in instances.into_iter().filter(|i| {
                    i["enabled"] == true && i["version"] == "1.0.0" && i["package"] == PACKAGE
                }) {
                    let Some(key) = inst["id"].as_str() else {
                        continue;
                    };
                    let Ok(settings) = self.get("settings", key) else {
                        continue;
                    };
                    let signature = json!([inst["revision"], settings["revision"]]);
                    if last.get(key).is_some_and(|(sig, next, job)| {
                        *sig == signature && (*next > Instant::now() || !job.is_finished())
                    }) {
                        continue;
                    }
                    let interval = settings["refresh_interval_secs"]
                        .as_u64()
                        .unwrap_or(30)
                        .clamp(1, 86400);
                    let catalog = self.clone();
                    let id = key.to_owned();
                    let job = tokio::spawn(async move {
                        let _ = catalog.refresh_oi(&id).await;
                    });
                    last.insert(
                        key.to_owned(),
                        (
                            signature,
                            Instant::now() + Duration::from_secs(interval),
                            job,
                        ),
                    );
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}

fn rev(db: &Connection, kind: &str, key: &str) -> R<Option<String>> {
    db.query_row(
        "SELECT rev FROM catalog_current WHERE kind=?1 AND id=?2",
        params![kind, key],
        |r| r.get(0),
    )
    .optional()
    .map_err(err)
}

fn owned(db: &Connection, kind: &str, instance: &str) -> R<BTreeMap<String, Value>> {
    let mut q=db.prepare("SELECT id, document FROM catalog_current JOIN catalog_versions USING(kind,id,rev) WHERE kind=?1 AND json_extract(document,'$.source')='oi' AND json_extract(document,'$.instance')=?2").map_err(err)?;
    let rows = q
        .query_map(params![kind, instance], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(err)?;
    rows.map(|r| {
        let (key, doc) = r.map_err(err)?;
        Ok((key, serde_json::from_str(&doc).map_err(err)?))
    })
    .collect()
}

fn put(db: &Connection, kind: &str, key: &str, mut value: Value) -> R<Value> {
    let hash = sha(&value);
    value["revision"] = json!(hash);
    let previous:Option<String>=db.query_row("SELECT document FROM catalog_current JOIN catalog_versions USING(kind,id,rev) WHERE kind=?1 AND id=?2",params![kind,key],|r|r.get(0)).optional().map_err(err)?;
    if let Some(doc) = previous {
        let old: Value = serde_json::from_str(&doc).map_err(err)?;
        if old["source"] != "oi" || old["instance"] != value["instance"] {
            return Err(format!("conflict: {kind} {key} belongs to another source"));
        }
        if old["revision"] == hash {
            return Ok(old);
        }
    }
    db.execute(
        "INSERT OR IGNORE INTO catalog_versions VALUES(?1,?2,?3,?4)",
        params![kind, key, hash, value.to_string()],
    )
    .map_err(err)?;
    db.execute(
        "INSERT OR REPLACE INTO catalog_current VALUES(?1,?2,?3)",
        params![kind, key, hash],
    )
    .map_err(err)?;
    Ok(value)
}
