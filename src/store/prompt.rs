use super::*;

pub(crate) const DEFAULT_SYSTEM_PROMPT: &str = "You are a helpful assistant.";

// Both the catalog writer and Run admission use these tables on the same database.
pub(crate) fn initialize_shared_schema(c: &Connection) -> Result<()> {
    c.execute_batch("
        CREATE TABLE IF NOT EXISTS objects(seq INTEGER PRIMARY KEY,kind TEXT NOT NULL,id TEXT NOT NULL,
            owner TEXT NOT NULL,parent TEXT,doc TEXT NOT NULL,UNIQUE(kind,id));
        CREATE INDEX IF NOT EXISTS objects_scope ON objects(owner,kind,parent,seq);
        CREATE INDEX IF NOT EXISTS runs_by_system_prompt ON objects(owner,json_extract(doc,'$.system_prompt_event_id'),seq) WHERE kind='run';
        CREATE TABLE IF NOT EXISTS outbox(seq INTEGER PRIMARY KEY,event_id TEXT NOT NULL UNIQUE,
            published INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE IF NOT EXISTS catalog_current(kind TEXT NOT NULL,id TEXT NOT NULL,rev TEXT NOT NULL,PRIMARY KEY(kind,id));
        CREATE TABLE IF NOT EXISTS catalog_versions(kind TEXT NOT NULL,id TEXT NOT NULL,rev TEXT NOT NULL,document TEXT NOT NULL,PRIMARY KEY(kind,id,rev));
        CREATE TABLE IF NOT EXISTS agent_prompt_heads(owner TEXT NOT NULL,agent_id TEXT NOT NULL,event_id TEXT NOT NULL,
            PRIMARY KEY(owner,agent_id));")?;
    Ok(())
}

// Called under the same write transaction that activates this Agent revision.
pub(crate) fn record_agent_prompt(c: &Connection, agent: &Value) -> Result<()> {
    let owner = field(agent, "owner")?;
    let agent_id = field(agent, "id")?;
    let content = agent["system_prompt"]
        .as_str()
        .unwrap_or(DEFAULT_SYSTEM_PROMPT);
    let previous: Option<(String, String)> = c
        .query_row(
            "SELECT h.event_id,json_extract(e.doc,'$.content') FROM agent_prompt_heads h
         JOIN objects e ON e.kind='event' AND e.owner=h.owner AND e.id=h.event_id
         WHERE h.owner=? AND h.agent_id=?",
            params![owner, agent_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if previous.as_ref().is_some_and(|(_, text)| text == content) {
        return Ok(());
    }
    let event = emit(
        c,
        owner,
        "agent.system_prompt_updated",
        json!({
            "agent_id":agent_id, "agent_revision":agent["revision"], "content":content,
            "previous_event_id":previous.map(|(id,_)| id)
        }),
    )?;
    c.execute(
        "INSERT INTO agent_prompt_heads(owner,agent_id,event_id) VALUES(?,?,?)
         ON CONFLICT(owner,agent_id) DO UPDATE SET event_id=excluded.event_id",
        params![owner, agent_id, field(&event, "id")?],
    )?;
    Ok(())
}

// An old HOME may have an effective Agent whose YAML is temporarily invalid or
// absent. Seed its actual catalog revision before applying any newly edited YAML.
pub(crate) fn backfill_agent_prompts(c: &Connection) -> Result<()> {
    let agents = fetch_many(c,
        "SELECT v.document FROM catalog_current a JOIN catalog_versions v USING(kind,id,rev)
         LEFT JOIN agent_prompt_heads h ON h.agent_id=a.id AND h.owner=json_extract(v.document,'$.owner')
         WHERE a.kind='agent' AND h.event_id IS NULL", [])?;
    for agent in agents {
        record_agent_prompt(c, &agent)?;
    }
    Ok(())
}

pub(super) fn agent_for_run(
    c: &Connection,
    owner: &str,
    agent_id: &str,
    max_rounds: Option<&Value>,
) -> Result<(Value, String)> {
    // An API handler or a dispatch can have prepared its intent before a
    // concurrent Agent update. Capture the effective spec and head together.
    let (document, event_id): (String, String) = c.query_row(
        "SELECT v.document,h.event_id FROM catalog_current a JOIN catalog_versions v USING(kind,id,rev)
         JOIN agent_prompt_heads h ON h.agent_id=a.id AND h.owner=json_extract(v.document,'$.owner')
         WHERE a.kind='agent' AND a.id=? AND h.owner=?",
        params![agent_id, owner], |row| Ok((row.get(0)?, row.get(1)?))
    ).optional()?.ok_or(StoreError::NotFound)?;
    let mut snapshot: Value = serde_json::from_str(&document)?;
    if let Some(limit) = max_rounds {
        snapshot["_max_rounds"] = limit.clone();
    }
    Ok((snapshot, event_id))
}

/// Bind the exact system message after hooks, Goal instructions and context assembly.
/// Only a committed ModelCall can supply a previous effective event for this Run.
pub(super) fn bind_model_prompt(c: &Connection, run: &Value, call: &mut Value) -> Result<()> {
    let owner = field(run, "owner")?;
    let rid = field(run, "id")?;
    if let Some(base) = run["system_prompt_event_id"].as_str() {
        call["base_system_prompt_event_id"] = json!(base);
    }
    let Some(messages) = call["messages"].as_array() else {
        return Ok(());
    };
    // The builder omits the system role when the effective prompt is empty.
    let content = messages
        .iter()
        .find(|m| m["role"] == "system")
        .and_then(|m| m["content"].as_str())
        .unwrap_or("");
    let last: Option<String> = c
        .query_row(
            "SELECT json_extract(doc,'$.system_prompt_event_id') FROM objects
         WHERE owner=? AND kind='model_call' AND parent=? ORDER BY seq DESC LIMIT 1",
            params![owner, rid],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    let previous_id = last.as_deref().or(run["system_prompt_event_id"].as_str());
    if let Some(previous) = previous_id
        && load(c, owner, "event", previous)?["content"].as_str() == Some(content)
    {
        call["system_prompt_event_id"] = json!(previous);
        return Ok(());
    }
    let event = emit(
        c,
        owner,
        "run.system_prompt_updated",
        json!({
            "run_id":rid,"session_id":run["session_id"],"agent_id":run["agent_id"],
            "base_system_prompt_event_id":run["system_prompt_event_id"],
            "previous_event_id":previous_id,"content":content,
        }),
    )?;
    call["system_prompt_event_id"] = event["id"].clone();
    Ok(())
}

impl Store {
    pub fn runs_for_prompt(&self, owner: &str, event_id: &str) -> Result<Value> {
        self.read(|c| {
            let derived_run: Option<String> = c.query_row(
                "SELECT json_extract(doc,'$.run_id') FROM objects WHERE owner=? AND kind='event' AND id=?
                 AND json_extract(doc,'$.type')='run.system_prompt_updated' AND json_extract(doc,'$.source')='loom'",
                params![owner, event_id], |row| row.get(0),
            ).optional()?;
            let runs = match derived_run {
                Some(id) => vec![load(c, owner, "run", &id)?],
                None => fetch_many(c, "SELECT doc FROM objects WHERE owner=? AND kind='run'
                         AND json_extract(doc,'$.system_prompt_event_id')=? ORDER BY seq",
                         params![owner, event_id])?,
            };
            Ok(Value::Array(runs))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;

    #[test]
    fn registration_uses_the_committed_head_even_if_the_handler_read_an_older_revision() {
        let home = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(home.path()).unwrap();
        let store = Store::open(home.path().join("state/loom.db")).unwrap();
        let mut stale = catalog
            .create_agent("u", false, json!({"id":"reader","system_prompt":"old"}))
            .unwrap();
        stale["_max_rounds"] = json!(5);
        let current = catalog
            .patch_agent("u", false, "reader", json!({"system_prompt":"new"}))
            .unwrap();
        let input = store
            .post_event("u", json!({"content":"go"}), None)
            .unwrap();
        let delivery = store
            .deliver(
                "u",
                input["id"].as_str().unwrap(),
                json!({"target":{"agent_id":"reader"},"action":"start_run","agent_snapshot":stale}),
                None,
            )
            .unwrap();
        let run = store
            .get("u", "run", delivery["run_id"].as_str().unwrap())
            .unwrap();
        let event = store
            .get(
                "u",
                "event",
                run["system_prompt_event_id"].as_str().unwrap(),
            )
            .unwrap();
        assert_eq!(run["agent_revision"], current["revision"]);
        assert_eq!(run["agent_snapshot"]["_max_rounds"], 5);
        assert_eq!(run["agent_snapshot"]["system_prompt"], event["content"]);
        assert_eq!(event["content"], "new");
    }

    #[test]
    fn old_catalog_without_any_yaml_gains_a_single_baseline_with_its_existing_revision() {
        let home = tempfile::tempdir().unwrap();
        let state = home.path().join("state");
        std::fs::create_dir(&state).unwrap();
        let db = Connection::open(state.join("loom.db")).unwrap();
        db.execute_batch("CREATE TABLE catalog_current(kind TEXT,id TEXT,rev TEXT,PRIMARY KEY(kind,id));
            CREATE TABLE catalog_versions(kind TEXT,id TEXT,rev TEXT,document TEXT,PRIMARY KEY(kind,id,rev));
            INSERT INTO catalog_current VALUES('agent','reader','legacy');").unwrap();
        db.execute(
            "INSERT INTO catalog_versions VALUES('agent','reader','legacy',?)",
            [json!({"id":"reader","owner":"u","revision":"legacy"}).to_string()],
        )
        .unwrap();
        drop(db);
        Catalog::open(home.path()).unwrap();
        Catalog::open(home.path()).unwrap();
        let store = Store::open(state.join("loom.db")).unwrap();
        let events = store.pending_events(10).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["agent_revision"], "legacy");
        assert_eq!(events[0]["content"], DEFAULT_SYSTEM_PROMPT);
    }
}
