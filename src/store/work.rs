use super::delivery::{clear_lease, terminal};
use super::*;

pub(super) fn run_for_lease(c: &Connection, work: &Work) -> Result<Value> {
    let rid = field(&work.run, "id")?;
    let valid:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM workq WHERE run_id=? AND token=? AND ready=1 AND expires>unixepoch())",
        params![rid,work.token],|r|r.get(0))?;
    if !valid {
        return Err(StoreError::StaleLease);
    }
    load(c, field(&work.run, "owner")?, "run", rid)
}
fn merge(doc: &mut Value, patch: &Value, protected: &[&str]) -> Result<()> {
    expect_object(patch)?;
    for (k, v) in patch.as_object().unwrap() {
        if !protected.contains(&k.as_str()) {
            doc[k] = v.clone()
        }
    }
    Ok(())
}
const RUN_FIXED: &[&str] = &[
    "id",
    "run_id",
    "owner",
    "session_id",
    "agent_id",
    "agent_snapshot",
    "agent_revision",
    "system_prompt_event_id",
    "input",
    "inputs",
    "step",
    "version",
    "created_at",
    "status",
    "progress_message",
];
const CALL_FIXED: &[&str] = &[
    "id",
    "tool_call_id",
    "model_call_id",
    "run_id",
    "owner",
    "session_id",
    "created_at",
    "call_key",
    "system_prompt_event_id",
    "base_system_prompt_event_id",
    "_created",
];

pub(super) fn check_wait(c: &Connection, run: &Value) -> Result<bool> {
    let mut pending = Vec::new();
    match &run["waiting_for"] {
        Value::Array(a) => {
            for k in a {
                pending.push(
                    k.as_str()
                        .ok_or_else(|| StoreError::Invalid("expected call ID".into()))?,
                )
            }
        }
        Value::String(s) => pending.push(s),
        _ => return Ok(false),
    }
    if pending.is_empty() {
        return Ok(false);
    }
    let mut ready = true;
    for call in pending {
        let tool = load(c, field(run, "owner")?, "tool_call", call)?;
        if tool["run_id"] != run["id"] {
            return Err(StoreError::Invalid("waiting for another run's call".into()));
        }
        ready &= terminal(&tool["status"]);
        if let Some(eid) = tool["receipt_event_id"].as_str() {
            ready &= c.query_row(
                "SELECT EXISTS(SELECT 1 FROM consumed WHERE consumer='runtime' AND event_id=?)",
                [eid],
                |r| r.get::<_, bool>(0),
            )?;
        }
    }
    Ok(ready)
}

pub(super) fn waiting_on_async_tool(c: &Connection, run: &Value) -> Result<bool> {
    Ok(c.query_row(
        "SELECT EXISTS(SELECT 1 FROM objects WHERE kind='tool_call' AND owner=? AND parent=?
            AND id IN(SELECT value FROM json_each(?))
            AND json_extract(doc,'$.status') IN('accepted','running','progress','unknown','dispatched','waiting','pending')
            AND coalesce(json_extract(doc,'$.tool_id'),'') NOT IN('ask_user','code_exec'))",
        params![field(run, "owner")?, field(run, "id")?, run["waiting_for"].to_string()],
        |row| row.get(0),
    )?)
}

pub(super) fn clear_progress_request(run: &mut Value) {
    if run.get("progress_request_id").is_some() {
        run.as_object_mut().unwrap().remove("progress_request_id");
        run["progress_requested"] = json!(false);
        if run["wake_reason"] == "progress_request" {
            run["wake_reason"] = Value::Null;
        }
    }
}

pub(super) fn queue_awakened(c: &Connection, run: &mut Value) -> Result<()> {
    let rid = field(run, "id")?.to_owned();
    run["status"] = json!("queued");
    run["version"] = json!(run["version"].as_u64().unwrap_or(0) + 1);
    run["wake_reason"] = json!("tool_result");
    clear_progress_request(run);
    c.execute(
        "INSERT OR IGNORE INTO run_steps(run_id,step,ready_receipt_ms) VALUES(?,?,?)",
        params![
            rid,
            run["step"].as_i64(),
            latest_receipt_ms(c, run)?.unwrap_or_else(|| chrono::Utc::now().timestamp_millis())
        ],
    )?;
    clear_lease(c, &rid, true)?;
    save(c, "run", run)?;
    emit(
        c,
        field(run, "owner")?,
        "run.wait_satisfied",
        json!({"run_id":rid,"session_id":run["session_id"],"waiting_for":run["waiting_for"],"version":run["version"]}),
    )?;
    Ok(())
}
fn latest_receipt_ms(c: &Connection, run: &Value) -> Result<Option<i64>> {
    let refs = if run["waiting_for"].is_string() {
        json!([run["waiting_for"]])
    } else {
        run["waiting_for"].clone()
    };
    Ok(c.query_row(
        "SELECT max(json_extract(doc,'$.receipt_at_ms')) FROM objects WHERE kind='tool_call'
        AND parent=? AND id IN(SELECT value FROM json_each(?))",
        params![field(run, "id")?, refs.to_string()],
        |r| r.get(0),
    )?)
}
pub(super) fn apply_runtime_event(c: &Connection, eid: &str) -> Result<()> {
    let fields: (String,Option<String>)=c.query_row("SELECT owner,
        CASE WHEN json_extract(doc,'$.type')='tool.result_received' AND json_extract(doc,'$.source')='loom'
        THEN json_extract(doc,'$.run_id') ELSE NULL END FROM objects WHERE kind='event' AND id=?",
        [eid], |r|Ok((r.get(0)?,r.get(1)?)))?;
    if let (owner, Some(rid)) = fields {
        let mut run = load(c, &owner, "run", &rid)?;
        let before_progress_claim = run["status"] == "queued" && run["progress_requested"] == true;
        if (run["status"] == "waiting" || before_progress_claim) && check_wait(c, &run)? {
            queue_awakened(c, &mut run)?
        }
    }
    Ok(())
}

impl Store {
    pub fn claim(&self, worker: &str, lease_secs: i64) -> Result<Option<Work>> {
        if worker.is_empty() || lease_secs <= 0 {
            return Err(StoreError::Invalid("invalid worker lease".into()));
        }
        self.tx(|c| {
            let picked: Option<(String, String)> = c
                .query_row(
                    "SELECT run_id,owner FROM workq
                WHERE ready=1 AND (token IS NULL OR expires<=unixepoch()) ORDER BY seq LIMIT 1",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((rid, owner)) = picked else {
                return Ok(None);
            };
            let token = id("lease");
            c.execute(
                "UPDATE workq SET token=?,worker=?,expires=unixepoch()+? WHERE run_id=?",
                params![token, worker, lease_secs, rid],
            )?;
            let mut run = load(c, &owner, "run", &rid)?;
            run["status"] = json!("running");
            if run.get("worker_claimed_us").is_none() {
                run["worker_claimed_us"] = json!(chrono::Utc::now().timestamp_micros());
            }
            c.execute("INSERT INTO run_steps(run_id,step,started_ms,worker) VALUES(?,?,?,?)
               ON CONFLICT(run_id,step) DO UPDATE SET started_ms=coalesce(run_steps.started_ms,excluded.started_ms),
               worker=coalesce(run_steps.worker,excluded.worker)",
               params![rid,run["step"].as_i64(),chrono::Utc::now().timestamp_millis(),worker])?;
            save(c, "run", &run)?;
            Ok(Some(Work { run, token }))
        })
    }
    pub fn renew(&self, work: &Work, lease_secs: i64) -> Result<bool> {
        if lease_secs <= 0 {
            return Err(StoreError::Invalid("invalid lease duration".into()));
        }
        self.tx(|c| {
            Ok(c.execute(
                "UPDATE workq SET expires=unixepoch()+?
            WHERE run_id=? AND token=? AND ready=1 AND expires>unixepoch()",
                params![lease_secs, field(&work.run, "id")?, work.token],
            )? == 1)
        })
    }
    pub fn finish(&self, work: &Work, status: &str, patch: Value) -> Result<Value> {
        if !matches!(
            status,
            "queued" | "waiting" | "completed" | "failed" | "cancelled"
        ) {
            return Err(StoreError::Invalid("invalid run transition".into()));
        }
        self.tx(|c| {
            let mut run = run_for_lease(c, work)?;
            let owner = field(&run, "owner")?.to_owned();
            let rid = field(&run, "id")?.to_owned();
            if let Some(message) = patch.get("progress_message") {
                if status != "waiting" || run["progress_requested"] != true {
                    return Err(StoreError::Conflict(
                        "no outstanding progress request".into(),
                    ));
                }
                for key in ["waiting_for", "pending_batch"] {
                    if patch.get(key).is_some_and(|value| value != &run[key]) {
                        return Err(StoreError::Invalid(
                            "progress must preserve pending tool calls".into(),
                        ));
                    }
                }
                let request = field(&run, "progress_request_id")?;
                append_history(
                    c,
                    &owner,
                    field(&run, "session_id")?,
                    &format!("progress:{rid}:{request}"),
                    &json!({"role":"assistant","status":"progress","run_id":rid,
                        "request_event_id":request,"content":message}),
                )?;
            }
            merge(&mut run, &patch, RUN_FIXED)?;
            run["status"] = json!(status);
            if patch.get("progress_message").is_some()
                || patch["progress_requested"] == false
                || terminal(&run["status"])
            {
                clear_progress_request(&mut run);
            }
            commit_step(c, &run)?;
            run["step"] = json!(run["step"].as_u64().unwrap_or(0) + 1);
            run["version"] = json!(run["version"].as_u64().unwrap_or(0) + 1);
            clear_lease(c, &rid, status == "queued")?;
            save(c, "run", &run)?;
            if status == "waiting" {
                if check_wait(c, &run)? {
                    queue_awakened(c, &mut run)?
                } else {
                    super::extensions::resume_approved(c, &mut run)?;
                }
            }
            if terminal(&run["status"]) {
                let content = [
                    "output",
                    "result",
                    "answer",
                    "reply",
                    "response",
                    "error",
                    "stop_reason",
                ]
                .iter()
                .find_map(|k| run.get(*k))
                .cloned()
                .unwrap_or(Value::Null);
                append_history(
                    c,
                    &owner,
                    field(&run, "session_id")?,
                    &format!("final:{rid}"),
                    &json!({"role":"assistant","run_id":rid,"status":status,"content":content}),
                )?;
                emit(
                    c,
                    &owner,
                    &format!("run.{status}"),
                    json!({"run_id":rid,"session_id":run["session_id"]}),
                )?;
            }
            Ok(run)
        })
    }
    pub fn record_call(&self, work: &Work, kind: &str, mut body: Value) -> Result<Value> {
        let kind = kind_name(kind)?;
        if !matches!(kind, "model_call" | "tool_call") {
            return Err(StoreError::Invalid("expected call kind".into()));
        }
        expect_object(&body)?;
        self.tx(|c| {
            let run = run_for_lease(c, work)?;
            let owner = field(&run, "owner")?;
            let rid = field(&run, "id")?;
            let op = format!("record:{rid}:{kind}");
            let key = body["call_key"]
                .as_str()
                .or(body["id"].as_str())
                .map(str::to_owned);
            let mut request = body.clone();
            request.as_object_mut().unwrap().remove("_created");
            if let Some(k) = replay(c, owner, &op, key.as_deref(), &request)? {
                let mut call = load(c, owner, kind, &k)?;
                call["_created"] = json!(false);
                return Ok(call);
            }
            let cid = body["id"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| id(kind));
            body["run_id"] = json!(rid);
            body["session_id"] = run["session_id"].clone();
            body["step"] = run["step"].clone();
            body.as_object_mut().unwrap().remove("_created");
            if body["status"].is_null() {
                body["status"] = json!("pending")
            }
            let mut requested = json!({"call_id":cid,"run_id":rid,"session_id":run["session_id"]});
            if kind == "model_call" {
                super::prompt::bind_model_prompt(c, &run, &mut body)?;
                if let Some(event) = body.get("system_prompt_event_id") {
                    requested["system_prompt_event_id"] = event.clone();
                }
            }
            let mut call = create(c, owner, kind, &cid, Some(rid), body)?;
            remember(c, owner, &op, key.as_deref(), &request, &cid)?;
            emit(
                c,
                owner,
                if kind == "tool_call" {
                    "tool.call_requested"
                } else {
                    "model.inference_requested"
                },
                requested,
            )?;
            call["_created"] = json!(true);
            Ok(call)
        })
    }
    pub fn update_call(&self, work: &Work, kind: &str, cid: &str, patch: Value) -> Result<Value> {
        let kind = kind_name(kind)?;
        if !matches!(kind, "tool_call" | "model_call") {
            return Err(StoreError::Invalid("expected call kind".into()));
        }
        self.tx(|c| {
            let run = run_for_lease(c, work)?;
            let owner = field(&run, "owner")?;
            let mut call = load(c, owner, kind, cid)?;
            if call["run_id"] != run["id"] {
                return Err(StoreError::NotFound);
            }
            if terminal(&call["status"]) {
                if kind == "tool_call"
                    && patch.get("status").is_some()
                    && !terminal(&patch["status"])
                {
                    // A final callback may arrive while the original request's acceptance is in flight.
                    return Ok(call);
                }
                if patch
                    .as_object()
                    .is_some_and(|p| p.iter().all(|(k, v)| call.get(k) == Some(v)))
                {
                    return Ok(call);
                }
                return Err(StoreError::Conflict("call already terminal".into()));
            }
            merge(&mut call, &patch, CALL_FIXED)?;
            save(c, kind, &call)?;
            if terminal(&call["status"]) && kind == "tool_call" {
                tool_history(
                    c,
                    owner,
                    &run,
                    &call,
                    call.get("result")
                        .or(call.get("output"))
                        .or(call.get("error"))
                        .unwrap_or(&Value::Null),
                )?;
            } else if terminal(&call["status"]) {
                emit(
                    c,
                    owner,
                    &format!("model.{}", field(&call, "status")?),
                    json!({"call_id":cid,"run_id":run["id"],"session_id":run["session_id"]}),
                )?;
            }
            Ok(call)
        })
    }
    pub fn receive_tool_result(
        &self,
        owner: &str,
        cid: &str,
        body: Value,
        key: &str,
    ) -> Result<Value> {
        expect_object(&body)?;
        self.tx(|c| {
            let mut call = load(c, owner, "tool_call", cid)?;
            let op = format!("receipt:{cid}");
            if replay(c, owner, &op, Some(key), &body)?.is_some() {
                return Ok(call);
            }
            let previous = call["status"].clone();
            if matches!(
                previous.as_str(),
                Some("awaiting_approval" | "pending_approval" | "awaiting_input")
            ) {
                return Err(StoreError::Conflict(
                    "receipt cannot resolve this approval or question".into(),
                ));
            }
            if call["job_id"]
                .as_str()
                .zip(body["job_id"].as_str())
                .is_some_and(|(a, b)| a != b)
            {
                return Err(StoreError::Conflict("receipt refers to another job".into()));
            }
            if !terminal(&previous) {
                let status = body["status"].as_str().unwrap_or("completed");
                let status = match status {
                    "accepted" | "progress" | "running" | "pending" | "waiting" | "completed"
                    | "failed" | "cancelled" => status,
                    "succeeded" | "success" | "final" => "completed",
                    "error" => "failed",
                    _ => return Err(StoreError::Invalid("unknown tool receipt status".into())),
                };
                call["status"] = json!(status);
                call["receipt"] = body.clone();
                if let Some(job) = body.get("job_id") {
                    call["job_id"] = job.clone()
                }
                if terminal(&call["status"]) {
                    call["result"] = body
                        .get("result")
                        .or(body.get("output"))
                        .unwrap_or(&body)
                        .clone();
                    if let Some(err) = body.get("error") {
                        call["error"] = err.clone()
                    }
                }
                let run = load(c, owner, "run", field(&call, "run_id")?)?;
                let content = if terminal(&call["status"]) {
                    &call["result"]
                } else {
                    &body
                };
                let event = tool_history(c, owner, &run, &call, content)?;
                if terminal(&call["status"]) {
                    call["receipt_event_id"] = json!(event);
                    call["receipt_at_ms"] = json!(chrono::Utc::now().timestamp_millis());
                }
                save(c, "tool_call", &call)?;
            }
            remember(c, owner, &op, Some(key), &body, cid)?;
            Ok(call)
        })
    }
}

pub(super) fn commit_step(c: &Connection, run: &Value) -> Result<()> {
    c.execute(
        "UPDATE run_steps SET committed_ms=? WHERE run_id=? AND step=? AND committed_ms IS NULL",
        params![
            chrono::Utc::now().timestamp_millis(),
            field(run, "id")?,
            run["step"].as_i64()
        ],
    )?;
    Ok(())
}
impl Store {
    pub fn run_steps(&self, owner: &str, rid: &str) -> Result<Value> {
        self.read(|c|{
            load(c,owner,"run",rid)?;
            Ok(Value::Array(fetch_many(c,"SELECT json_object('run_id',run_id,'step',step,'ready_receipt_ms',ready_receipt_ms,
                'started_ms',started_ms,'committed_ms',committed_ms,'worker',worker) FROM run_steps WHERE run_id=? ORDER BY step",[rid])?))
        })
    }
}

pub(super) fn tool_history(
    c: &Connection,
    owner: &str,
    run: &Value,
    call: &Value,
    content: &Value,
) -> Result<String> {
    let note = content.get("note_ref");
    let mut evidence = json!({"tool_call_id":call["id"],"run_id":run["id"],"session_id":run["session_id"],
        "status":call["status"],"result":content});
    if let Some(note) = note {
        evidence["note_ref"] = note.clone()
    }
    let event = emit(
        c,
        owner,
        if terminal(&call["status"]) {
            "tool.result_received"
        } else {
            "tool.progress"
        },
        evidence,
    )?;
    let mut item = json!({"role":"tool","tool_call_id":call["id"],"tool_id":call["tool_id"],"run_id":run["id"],"status":call["status"],
        "event_id":event["id"],"content":content});
    if let Some(note) = note {
        item["note_ref"] = note.clone()
    }
    append_history(
        c,
        owner,
        field(run, "session_id")?,
        field(&event, "id")?,
        &item,
    )?;
    Ok(field(&event, "id")?.to_owned())
}
