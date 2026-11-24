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
mod replies;
#[cfg(test)]
mod tests;
mod work;

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
        c.execute_batch("
            PRAGMA journal_mode=WAL;
            PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS objects(seq INTEGER PRIMARY KEY,kind TEXT NOT NULL,id TEXT NOT NULL,
              owner TEXT NOT NULL,parent TEXT,doc TEXT NOT NULL,UNIQUE(kind,id));
            CREATE INDEX IF NOT EXISTS objects_scope ON objects(owner,kind,parent,seq);
            CREATE TABLE IF NOT EXISTS history(seq INTEGER PRIMARY KEY,owner TEXT NOT NULL,session TEXT NOT NULL,
              origin TEXT NOT NULL,doc TEXT NOT NULL,UNIQUE(owner,session,origin));
            CREATE TABLE IF NOT EXISTS workq(seq INTEGER PRIMARY KEY,run_id TEXT NOT NULL UNIQUE,owner TEXT NOT NULL,
              ready INTEGER NOT NULL DEFAULT 1,token TEXT,worker TEXT,expires INTEGER);
            CREATE INDEX IF NOT EXISTS work_ready ON workq(ready,expires,seq);
            CREATE TABLE IF NOT EXISTS outbox(seq INTEGER PRIMARY KEY,event_id TEXT NOT NULL UNIQUE,
              published INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE IF NOT EXISTS consumed(consumer TEXT NOT NULL,event_id TEXT NOT NULL,
              PRIMARY KEY(consumer,event_id));
            CREATE TABLE IF NOT EXISTS subscription_results(consumer TEXT NOT NULL,event_id TEXT NOT NULL,
               result TEXT NOT NULL,PRIMARY KEY(consumer,event_id));
            CREATE TABLE IF NOT EXISTS replyq(seq INTEGER PRIMARY KEY,reply_id TEXT NOT NULL UNIQUE,
               owner TEXT NOT NULL,ready INTEGER NOT NULL DEFAULT 1,token TEXT,worker TEXT,expires INTEGER);
            CREATE TABLE IF NOT EXISTS requests(owner TEXT NOT NULL,op TEXT NOT NULL,key TEXT NOT NULL,
              request TEXT NOT NULL,resource TEXT NOT NULL,PRIMARY KEY(owner,op,key));
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
        self.consume_event_result(consumer,event_id,&Value::Null)
    }
    pub fn consume_event_result(&self, consumer:&str,event_id:&str,result:&Value)->Result<bool>{
        self.tx(|c| {
            if !c.query_row(
                "SELECT EXISTS(SELECT 1 FROM outbox WHERE event_id=?)",
                [event_id],
                |r| r.get::<_, bool>(0),
            )? {
                return Err(StoreError::NotFound);
            }
            let inserted=c.execute(
                "INSERT OR IGNORE INTO consumed(consumer,event_id) VALUES(?,?)",
                params![consumer, event_id],
            )? == 1;
            if inserted{
                c.execute("INSERT INTO subscription_results(consumer,event_id,result) VALUES(?,?,?)",
                   params![consumer,event_id,result.to_string()])?;
            }
            Ok(inserted)
        })
    }
    pub fn subscription_result(&self,consumer:&str,event_id:&str)->Result<Option<Value>>{
        self.read(|c|{
            let result:Option<String>=c.query_row("SELECT result FROM subscription_results WHERE consumer=? AND event_id=?",
                params![consumer,event_id],|r|r.get(0)).optional()?;
            result.map(|raw|serde_json::from_str(&raw).map_err(StoreError::from)).transpose()
        })
    }
    pub fn consumed(&self, consumer: &str, event_id: &str) -> Result<bool> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT EXISTS(SELECT 1 FROM consumed WHERE consumer=? AND event_id=?)",
                params![consumer, event_id],
                |r| r.get(0),
            )?)
        })
    }
}
