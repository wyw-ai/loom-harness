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
    "input",
    "inputs",
    "step",
    "version",
    "created_at",
    "status",
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
    }
    Ok(ready)
}
pub(super) fn queue_awakened(c: &Connection, run: &mut Value) -> Result<()> {
    let rid = field(run, "id")?.to_owned();
    run["status"] = json!("queued");
    run["version"] = json!(run["version"].as_u64().unwrap_or(0) + 1);
    run["wake_reason"] = json!("tool_result");
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
            merge(&mut run, &patch, RUN_FIXED)?;
            run["status"] = json!(status);
            run["step"] = json!(run["step"].as_u64().unwrap_or(0) + 1);
            run["version"] = json!(run["version"].as_u64().unwrap_or(0) + 1);
            clear_lease(c, &rid, status == "queued")?;
            save(c, "run", &run)?;
            if status == "waiting" && check_wait(c, &run)? {
                queue_awakened(c, &mut run)?
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
                json!({"call_id":cid,"run_id":rid,"session_id":run["session_id"]}),
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
            if terminal(&call["status"]) && kind=="tool_call" {
                tool_history(c,owner,&run,&call,call.get("result").or(call.get("output")).or(call.get("error")).unwrap_or(&Value::Null))?;
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
        self.tx(|c|{
            let mut call=load(c,owner,"tool_call",cid)?;
            let op=format!("receipt:{cid}");
            if replay(c,owner,&op,Some(key),&body)?.is_some(){return Ok(call)}
            let previous=call["status"].clone();
            if !terminal(&previous){
                let status=body["status"].as_str().unwrap_or("completed");
                let status=match status{
                    "accepted"|"progress"|"running"|"pending"|"waiting"|"completed"|"failed"|"cancelled"=>status,
                    "succeeded"|"success"|"final"=>"completed","error"=>"failed",
                    _=>return Err(StoreError::Invalid("unknown tool receipt status".into()))
                };
                call["status"]=json!(status);
                call["receipt"]=body.clone();
                if let Some(job)=body.get("job_id"){call["job_id"]=job.clone()}
                if terminal(&call["status"]){
                    call["result"]=body.get("result").or(body.get("output")).unwrap_or(&body).clone();
                    if let Some(err)=body.get("error"){call["error"]=err.clone()}
                }
                save(c,"tool_call",&call)?;
                let mut run=load(c,owner,"run",field(&call,"run_id")?)?;
                let content=if terminal(&call["status"]){&call["result"]}else{&body};
                tool_history(c,owner,&run,&call,content)?;
                if run["status"]=="waiting" && check_wait(c,&run)?{queue_awakened(c,&mut run)?}
            }
            remember(c,owner,&op,Some(key),&body,cid)?;
            Ok(call)
        })
    }
}

pub(super) fn tool_history(c:&Connection,owner:&str,run:&Value,call:&Value,content:&Value)->Result<()>{
    let note=content.get("note_ref");
    let mut evidence=json!({"tool_call_id":call["id"],"run_id":run["id"],"session_id":run["session_id"],
        "status":call["status"],"result":content});
    if let Some(note)=note{evidence["note_ref"]=note.clone()}
    let event=emit(c,owner,if terminal(&call["status"]){"tool.result_received"}else{"tool.progress"},evidence)?;
    let mut item=json!({"role":"tool","tool_call_id":call["id"],"run_id":run["id"],"status":call["status"],
        "event_id":event["id"],"content":content});
    if let Some(note)=note{item["note_ref"]=note.clone()}
    append_history(c,owner,field(run,"session_id")?,field(&event,"id")?,&item)
}
