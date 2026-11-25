//! SQLite owns the durable records. A worker only owns a short, fenced execution lease.
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};
use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

mod delivery;
mod dispatch;
mod extensions;
mod local_bus;
mod overview;
mod prompt;
mod replies;
mod stream;
#[cfg(test)]
mod tests;
mod usage;
mod work;

pub(crate) use prompt::{
    DEFAULT_SYSTEM_PROMPT, backfill_agent_prompts, initialize_shared_schema, record_agent_prompt,
};

pub type Result<T> = std::result::Result<T, StoreError>;

#[derive(Debug)]
pub enum StoreError {
    NotFound,
    Conflict(String),
    Invalid(String),
    StaleLease,
    Storage(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("resource not found"),
            Self::Conflict(s) | Self::Invalid(s) | Self::Storage(s) => f.write_str(s),
            Self::StaleLease => f.write_str("run lease expired or superseded"),
        }
    }
}
impl std::error::Error for StoreError {}
impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Storage(e.to_string())
    }
}
impl From<serde_json::Error> for StoreError {
    fn from(e: serde_json::Error) -> Self {
        Self::Storage(e.to_string())
    }
}

#[derive(Clone)]
pub struct Store {
    db: Arc<Mutex<Connection>>,
}

#[derive(Debug, Clone)]
pub struct Work {
    pub run: Value,
    pub token: String,
}

fn id(kind: &str) -> String {
    format!("{kind}_{}", Uuid::new_v4().simple())
}
fn field<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| StoreError::Invalid(format!("missing {key}")))
}
fn expect_object(v: &Value) -> Result<()> {
    if v.is_object() {
        Ok(())
    } else {
        Err(StoreError::Invalid("expected JSON object".into()))
    }
}
fn kind_name(kind: &str) -> Result<&str> {
    match kind {
        "session" | "sessions" => Ok("session"),
        "event" | "events" => Ok("event"),
        "delivery" | "deliveries" | "event-deliveries" => Ok("delivery"),
        "run" | "runs" => Ok("run"),
        "model_call" | "model-calls" | "model_calls" => Ok("model_call"),
        "compaction_call" => Ok("compaction_call"),
        "tool_call" | "tool-calls" | "tool_calls" => Ok("tool_call"),
        "dispatch" | "dispatches" => Ok("dispatch"),
        "evaluation" | "evaluations" => Ok("evaluation"),
        "reply" | "replies" => Ok("reply"),
        _ => Err(StoreError::Invalid(format!(
            "unknown resource kind: {kind}"
        ))),
    }
}
fn now(c: &Connection) -> Result<i64> {
    Ok(c.query_row("SELECT unixepoch()", [], |r| r.get(0))?)
}
fn load(c: &Connection, owner: &str, kind: &str, key: &str) -> Result<Value> {
    let raw: String = c
        .query_row(
            "SELECT doc FROM objects WHERE kind=? AND owner=? AND id=?",
            params![kind, owner, key],
            |r| r.get(0),
        )
        .optional()?
        .ok_or(StoreError::NotFound)?;
    Ok(serde_json::from_str(&raw)?)
}
fn create(
    c: &Connection,
    owner: &str,
    kind: &str,
    key: &str,
    parent: Option<&str>,
    mut doc: Value,
) -> Result<Value> {
    expect_object(&doc)?;
    doc["id"] = json!(key);
    doc[format!("{kind}_id")] = json!(key);
    doc["owner"] = json!(owner);
    doc["created_at"] = json!(now(c)?);
    let n = c.execute("INSERT INTO objects(kind,id,owner,parent,doc) VALUES(?,?,?,?,?) ON CONFLICT(kind,id) DO NOTHING",
        params![kind,key,owner,parent,doc.to_string()])?;
    if n == 0 {
        return Err(StoreError::Conflict(format!("{kind} ID already exists")));
    }
    Ok(doc)
}
fn save(c: &Connection, kind: &str, doc: &Value) -> Result<()> {
    let n = c.execute(
        "UPDATE objects SET doc=? WHERE kind=? AND id=? AND owner=?",
        params![
            doc.to_string(),
            kind,
            field(doc, "id")?,
            field(doc, "owner")?
        ],
    )?;
    if n == 0 {
        return Err(StoreError::NotFound);
    }
    Ok(())
}
fn fetch_many(c: &Connection, sql: &str, args: impl rusqlite::Params) -> Result<Vec<Value>> {
    let mut stmt = c.prepare(sql)?;
    let raw = stmt.query_map(args, |r| r.get::<_, String>(0))?;
    let mut docs = vec![];
    for row in raw {
        docs.push(serde_json::from_str(&row?)?);
    }
    Ok(docs)
}
fn user_preview(content: &Value) -> Option<String> {
    let text_parts = content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|part| part["type"] == "text")
        .filter_map(|part| part["text"].as_str());
    let mut preview: String = content
        .as_str()
        .into_iter()
        .chain(text_parts)
        .flat_map(str::split_whitespace)
        .flat_map(|word| std::iter::once(' ').chain(word.chars()))
        .skip(1)
        .take(48)
        .collect();
    if preview.ends_with(' ') {
        preview.pop();
    }
    (!preview.is_empty()).then_some(preview)
}
fn add_session_previews(c: &Connection, owner: &str, sessions: &mut [Value]) -> Result<()> {
    let mut user_items = c.prepare(
        "SELECT doc -> '$.content' FROM history WHERE owner=?1 AND session=?2
         AND json_extract(doc,'$.role')='user'
         AND coalesce(json_extract(doc,'$.event.source'),json_extract(doc,'$.event.source_id'),'user')='user'
         AND json_type(doc,'$.content') IN ('text','array') ORDER BY seq",
    )?;
    for session in sessions {
        if session["title"]
            .as_str()
            .is_some_and(|title| !title.trim().is_empty())
        {
            continue;
        }
        let mut items = user_items.query(params![owner, field(session, "id")?])?;
        while let Some(item) = items.next()? {
            let content = serde_json::from_str(&item.get::<_, String>(0)?)?;
            if let Some(preview) = user_preview(&content) {
                session["first_user_preview"] = json!(preview);
                break;
            }
        }
    }
    Ok(())
}
fn append_history(
    c: &Connection,
    owner: &str,
    session: &str,
    origin: &str,
    item: &Value,
) -> Result<()> {
    c.execute(
        "INSERT OR IGNORE INTO history(owner,session,origin,doc) VALUES(?,?,?,?)",
        params![owner, session, origin, item.to_string()],
    )?;
    Ok(())
}
fn create_event(c: &Connection, owner: &str, body: Value) -> Result<Value> {
    let eid = id("evt");
    let doc = create(c, owner, "event", &eid, None, body)?;
    c.execute("INSERT INTO outbox(event_id) VALUES(?)", [&eid])?;
    Ok(doc)
}
fn emit(c: &Connection, owner: &str, ty: &str, body: Value) -> Result<Value> {
    let mut event = body;
    event["type"] = json!(ty);
    event["source"] = json!("loom");
    create_event(c, owner, event)
}
fn remember(
    c: &Connection,
    owner: &str,
    op: &str,
    key: Option<&str>,
    request: &Value,
    resource: &str,
) -> Result<()> {
    if let Some(k) = key {
        c.execute(
            "INSERT INTO requests(owner,op,key,request,resource) VALUES(?,?,?,?,?)",
            params![owner, op, k, request.to_string(), resource],
        )?;
    }
    Ok(())
}
fn replay(
    c: &Connection,
    owner: &str,
    op: &str,
    key: Option<&str>,
    request: &Value,
) -> Result<Option<String>> {
    let Some(key) = key else { return Ok(None) };
    if key.is_empty() {
        return Err(StoreError::Invalid("empty idempotency key".into()));
    }
    let prior: Option<(String, String)> = c
        .query_row(
            "SELECT request,resource FROM requests WHERE owner=? AND op=? AND key=?",
            params![owner, op, key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    match prior {
        Some((v, k)) if serde_json::from_str::<Value>(&v)? == *request => Ok(Some(k)),
        Some(_) => Err(StoreError::Conflict(
            "idempotency key already bound to different input".into(),
        )),
        None => Ok(None),
    }
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let c = Connection::open(path)?;
        c.busy_timeout(Duration::from_secs(5))?;
        c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        initialize_shared_schema(&c)?;
        c.execute_batch("
            CREATE TABLE IF NOT EXISTS history(seq INTEGER PRIMARY KEY,owner TEXT NOT NULL,session TEXT NOT NULL,
              origin TEXT NOT NULL,doc TEXT NOT NULL,UNIQUE(owner,session,origin));
            CREATE INDEX IF NOT EXISTS history_session_order ON history(owner,session,seq);
            CREATE INDEX IF NOT EXISTS console_recent_objects ON objects(owner,kind,seq)
              WHERE kind='session' OR kind='run';
            CREATE INDEX IF NOT EXISTS console_run_status ON objects(owner,json_extract(doc,'$.status'))
              WHERE kind='run';
            CREATE TABLE IF NOT EXISTS workq(seq INTEGER PRIMARY KEY,run_id TEXT NOT NULL UNIQUE,owner TEXT NOT NULL,
              ready INTEGER NOT NULL DEFAULT 1,token TEXT,worker TEXT,expires INTEGER);
            CREATE INDEX IF NOT EXISTS work_ready ON workq(ready,expires,seq);
            CREATE TABLE IF NOT EXISTS run_stream(seq INTEGER PRIMARY KEY,run_id TEXT NOT NULL,
              model_call_id TEXT NOT NULL,attempt_id TEXT NOT NULL,event TEXT NOT NULL,text TEXT);
            CREATE INDEX IF NOT EXISTS run_stream_cursor ON run_stream(run_id,seq);
            CREATE INDEX IF NOT EXISTS run_stream_call ON run_stream(model_call_id,seq);
            CREATE INDEX IF NOT EXISTS run_attempt_count ON run_stream(model_call_id) WHERE event='attempt_started';
            CREATE INDEX IF NOT EXISTS run_terminal_events ON objects(owner,json_extract(doc,'$.run_id'),json_extract(doc,'$.type'))
              WHERE kind='event' AND json_extract(doc,'$.source')='loom' AND json_extract(doc,'$.type') IN ('run.completed','run.failed','run.cancelled');
            CREATE TABLE IF NOT EXISTS consumed(consumer TEXT NOT NULL,event_id TEXT NOT NULL,
              PRIMARY KEY(consumer,event_id));
            CREATE TABLE IF NOT EXISTS local_bus_groups(consumer TEXT PRIMARY KEY, last_claimed INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS local_bus_pending(consumer TEXT NOT NULL, seq INTEGER NOT NULL,
               lease TEXT NOT NULL, expires_ms INTEGER NOT NULL, PRIMARY KEY(consumer,seq));
            CREATE INDEX IF NOT EXISTS local_bus_expiry ON local_bus_pending(consumer,expires_ms,seq);
            CREATE TABLE IF NOT EXISTS subscription_results(consumer TEXT NOT NULL,event_id TEXT NOT NULL,
               result TEXT NOT NULL,PRIMARY KEY(consumer,event_id));
            CREATE TABLE IF NOT EXISTS replyq(seq INTEGER PRIMARY KEY,reply_id TEXT NOT NULL UNIQUE,
               owner TEXT NOT NULL,ready INTEGER NOT NULL DEFAULT 1,token TEXT,worker TEXT,expires INTEGER);
            CREATE TABLE IF NOT EXISTS run_steps(run_id TEXT NOT NULL,step INTEGER NOT NULL,
               ready_receipt_ms INTEGER,started_ms INTEGER,committed_ms INTEGER,worker TEXT,
               PRIMARY KEY(run_id,step));
            CREATE TABLE IF NOT EXISTS requests(owner TEXT NOT NULL,op TEXT NOT NULL,key TEXT NOT NULL,
              request TEXT NOT NULL,resource TEXT NOT NULL,PRIMARY KEY(owner,op,key));
            CREATE TABLE IF NOT EXISTS instance_settings(name TEXT PRIMARY KEY,value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS suggestion_usage(owner TEXT NOT NULL,subject TEXT NOT NULL,
              minute INTEGER NOT NULL,samples INTEGER NOT NULL,PRIMARY KEY(owner,subject));
        ")?;
        Ok(Self {
            db: Arc::new(Mutex::new(c)),
        })
    }
    fn read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let c = self
            .db
            .lock()
            .map_err(|_| StoreError::Storage("database mutex poisoned".into()))?;
        f(&c)
    }
    pub fn instance_id(&self) -> Result<String> {
        self.tx(|c| {
            c.execute(
                "INSERT OR IGNORE INTO instance_settings(name,value) VALUES('instance_id',?)",
                [Uuid::new_v4().to_string()],
            )?;
            Ok(c.query_row(
                "SELECT value FROM instance_settings WHERE name='instance_id'",
                [],
                |r| r.get(0),
            )?)
        })
    }
    pub fn reserve_suggestion(&self, owner: &str, subject: &str, limit: u64) -> Result<bool> {
        if limit == 0 {
            return Ok(false);
        }
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        self.tx(|c|{
            let written=c.execute("INSERT INTO suggestion_usage(owner,subject,minute,samples) VALUES(?,?,unixepoch()/60,1)
                ON CONFLICT(owner,subject) DO UPDATE SET minute=excluded.minute,
                samples=CASE WHEN minute=excluded.minute THEN samples+1 ELSE 1 END
                WHERE minute<>excluded.minute OR samples<?",params![owner,subject,limit])?;
            Ok(written==1)
        })
    }
    fn tx<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let mut c = self
            .db
            .lock()
            .map_err(|_| StoreError::Storage("database mutex poisoned".into()))?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let val = f(&tx)?;
        tx.commit()?;
        Ok(val)
    }
    pub fn get(&self, owner: &str, kind: &str, key: &str) -> Result<Value> {
        self.read(|c| load(c, owner, kind_name(kind)?, key))
    }
    pub fn list(&self, owner: &str, kind: &str, parent: Option<&str>) -> Result<Value> {
        let k = kind_name(kind)?;
        self.read(|c| {
            Ok(Value::Array(match parent {
                Some(p) => fetch_many(
                    c,
                    "SELECT doc FROM objects WHERE owner=? AND kind=? AND parent=? ORDER BY seq",
                    params![owner, k, p],
                )?,
                None => fetch_many(
                    c,
                    "SELECT doc FROM objects WHERE owner=? AND kind=? ORDER BY seq",
                    params![owner, k],
                )?,
            }))
        })
    }
    pub fn sessions_with_previews(&self, owner: &str, agent_id: Option<&str>) -> Result<Value> {
        self.read(|c| {
            let mut sessions = fetch_many(
                c,
                "SELECT doc FROM objects WHERE owner=?1 AND kind='session'
                 AND (?2 IS NULL OR json_extract(doc,'$.agent_id')=?2) ORDER BY seq",
                params![owner, agent_id],
            )?;
            add_session_previews(c, owner, &mut sessions)?;
            Ok(Value::Array(sessions))
        })
    }
    pub fn history(&self, owner: &str, sid: &str) -> Result<Value> {
        self.read(|c| {
            load(c, owner, "session", sid)?;
            Ok(Value::Array(fetch_many(
                c,
                "SELECT doc FROM history WHERE owner=? AND session=? ORDER BY seq",
                params![owner, sid],
            )?))
        })
    }
    pub fn create_session(&self, owner: &str, body: Value) -> Result<Value> {
        self.tx(|c| {
            let key = body["id"]
                .as_str()
                .or(body["session_id"].as_str())
                .map(str::to_owned)
                .unwrap_or_else(|| id("ses"));
            create(c, owner, "session", &key, None, body)
        })
    }
    pub fn post_event(&self, owner: &str, body: Value, key: Option<&str>) -> Result<Value> {
        self.tx(|c| {
            let source = body["source"]
                .as_str()
                .or(body["source_id"].as_str())
                .unwrap_or("user");
            let op = format!("event:{source}");
            let key = key.or(body["source_event_id"].as_str());
            if let Some(k) = replay(c, owner, &op, key, &body)? {
                return load(c, owner, "event", &k);
            }
            let doc = create_event(c, owner, body.clone())?;
            remember(c, owner, &op, key, &body, field(&doc, "id")?)?;
            Ok(doc)
        })
    }
    pub fn pending_events(&self, limit: usize) -> Result<Vec<Value>> {
        self.read(|c| {
            fetch_many(
                c,
                "SELECT o.doc FROM outbox b JOIN objects o ON o.id=b.event_id AND o.kind='event'
            WHERE b.published=0 ORDER BY b.seq LIMIT ?",
                [limit.min(i64::MAX as usize) as i64],
            )
        })
    }
    pub fn published(&self, event_id: &str) -> Result<()> {
        self.tx(|c| {
            let n = c.execute("UPDATE outbox SET published=1 WHERE event_id=?", [event_id])?;
            if n == 0 {
                return Err(StoreError::NotFound);
            }
            Ok(())
        })
    }
    pub fn consume_event(&self, consumer: &str, event_id: &str) -> Result<bool> {
        self.consume_event_result(consumer, event_id, &Value::Null)
    }
    pub fn consume_event_result(
        &self,
        consumer: &str,
        event_id: &str,
        result: &Value,
    ) -> Result<bool> {
        self.tx(|c| {
            if !c.query_row(
                "SELECT EXISTS(SELECT 1 FROM outbox WHERE event_id=?)",
                [event_id],
                |r| r.get::<_, bool>(0),
            )? {
                return Err(StoreError::NotFound);
            }
            let inserted = c.execute(
                "INSERT OR IGNORE INTO consumed(consumer,event_id) VALUES(?,?)",
                params![consumer, event_id],
            )? == 1;
            if inserted {
                if consumer == "runtime" {
                    work::apply_runtime_event(c, event_id)?;
                }
                c.execute(
                    "INSERT INTO subscription_results(consumer,event_id,result) VALUES(?,?,?)",
                    params![consumer, event_id, result.to_string()],
                )?;
            }
            Ok(inserted)
        })
    }
    pub fn subscription_result(&self, consumer: &str, event_id: &str) -> Result<Option<Value>> {
        self.read(|c| {
            let result: Option<String> = c
                .query_row(
                    "SELECT result FROM subscription_results WHERE consumer=? AND event_id=?",
                    params![consumer, event_id],
                    |r| r.get(0),
                )
                .optional()?;
            result
                .map(|raw| serde_json::from_str(&raw).map_err(StoreError::from))
                .transpose()
        })
    }
}
