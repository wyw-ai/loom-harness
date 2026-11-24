use super::*;

pub(super) fn event_content(event: &Value) -> Value {
    for value in [
        &event["content"],
        &event["text"],
        &event["message"],
        &event["data"]["text"],
        &event["data"]["content"],
        &event["data"]["message"],
        &event["data"],
    ] {
        if !value.is_null() {
            return value.clone();
        }
    }
    Value::Null
}
fn input_item(event: &Value, delivery: &str, run: Option<&str>) -> Value {
    json!({"role":"user","content":event_content(event),"event_id":event["id"],"event":event,
        "delivery_id":delivery,"run_id":run})
}
pub(super) fn terminal(status: &Value) -> bool {
    matches!(status.as_str(), Some("completed" | "failed" | "cancelled"))
}
pub(super) fn clear_lease(c: &Connection, rid: &str, ready: bool) -> Result<()> {
    c.execute(
        "UPDATE workq SET ready=?,worker=NULL,token=NULL,expires=NULL WHERE run_id=?",
        params![ready, rid],
    )?;
    Ok(())
}

impl Store {
    pub fn event_by_id(&self, eid: &str) -> Result<Option<Value>> {
        self.read(|c| {
            let raw: Option<String> = c
                .query_row(
                    "SELECT doc FROM objects WHERE kind='event' AND id=?",
                    [eid],
                    |r| r.get(0),
                )
                .optional()?;
            raw.map(|s| serde_json::from_str(&s).map_err(StoreError::from))
                .transpose()
        })
    }
    /// HTTP must resolve and authorize the target agent and replace agent_snapshot with trusted data.
    pub fn deliver(
        &self,
        owner: &str,
        event_id: &str,
        body: Value,
        key: Option<&str>,
    ) -> Result<Value> {
        self.tx(|c| deliver_in(c, owner, event_id, body, key))
    }
}

pub(super) fn deliver_in(
    c: &Connection,
    owner: &str,
    event_id: &str,
    body: Value,
    key: Option<&str>,
) -> Result<Value> {
    expect_object(&body)?;
    let op = format!("delivery:{event_id}");
    // Configuration may have changed when a client retries the original request.
    let mut requested = body.clone();
    requested.as_object_mut().unwrap().remove("agent_snapshot");
    if let Some(k) = replay(c, owner, &op, key, &requested)? {
        return load(c, owner, "delivery", &k);
    }
    let event = load(c, owner, "event", event_id)?;
    let target = &body["target"];
    let asked_session = target["session_id"].as_str();
    let asked_agent = target["agent_id"].as_str();
    if asked_session.is_none() && asked_agent.is_none() {
        return Err(StoreError::Invalid("missing delivery target".into()));
    }
    let action = body["action"].as_str().unwrap_or("attach");
    let did = id("del");
    let mut delivery = json!({"target":target,"event_id":event_id,"action":action,"status":"processed","received":true});
    let session = match asked_session {
        Some(s) => Some(load(c, owner, "session", s)?),
        None if action == "start_run" => Some(create(
            c,
            owner,
            "session",
            &id("ses"),
            None,
            json!({"agent_id":asked_agent}),
        )?),
        None => None,
    };
    let sid = session.as_ref().and_then(|s| s["id"].as_str());
    let bound = session.as_ref().and_then(|s| s["agent_id"].as_str());
    if let (Some(a), Some(b)) = (bound, asked_agent)
        && a != b
    {
        return Err(StoreError::Invalid("agent does not match session".into()));
    }
    if let Some(s) = sid {
        delivery["session_id"] = json!(s)
    }
    match action {
        "attach" => {
            if let Some(s) = sid {
                append_history(c, owner, s, &did, &input_item(&event, &did, None))?
            }
        }
        "start_run" => {
            let s = sid.ok_or_else(|| StoreError::Invalid("missing session".into()))?;
            let agent = bound
                .or(asked_agent)
                .or(body["agent_snapshot"]["id"].as_str())
                .ok_or_else(|| StoreError::Invalid("session has no assigned agent".into()))?;
            let rid = id("run");
            let run = create(
                c,
                owner,
                "run",
                &rid,
                Some(s),
                json!({
                    "session_id":s,"agent_id":agent,"agent_snapshot":body["agent_snapshot"],"agent_revision":body["agent_snapshot"]["revision"],
                    "delivery_id":did,"status":"queued","version":1,"step":0,"input":event,"inputs":[event],
                    "goal":body["goal"],"delivery_contract":body["delivery_contract"],"result_schema":body["result_schema"]
                }),
            )?;
            c.execute(
                "INSERT INTO workq(owner,run_id) VALUES(?,?)",
                params![owner, rid],
            )?;
            append_history(c, owner, s, &did, &input_item(&event, &did, Some(&rid)))?;
            emit(
                c,
                owner,
                "run.started",
                json!({"run_id":rid,"session_id":s,"delivery_id":did}),
            )?;
            delivery["run_id"] = run["id"].clone();
        }
        "add_input" | "request_cancel" => {
            let s = asked_session
                .ok_or_else(|| StoreError::Invalid("control requires the owning session".into()))?;
            let rid = field(&body, "run_id")?;
            let mut run = load(c, owner, "run", rid)?;
            if run["session_id"] != s {
                return Err(StoreError::NotFound);
            }
            delivery["run_id"] = json!(rid);
            if terminal(&run["status"]) {
                delivery["run_status"] = run["status"].clone();
            } else if action == "add_input" && run["status"] == "waiting_delivery" {
                return Err(StoreError::Conflict(
                    "final answer is already committed for delivery".into(),
                ));
            } else {
                run["version"] = json!(run["version"].as_i64().unwrap_or(0) + 1);
                if action == "request_cancel" {
                    super::work::clear_progress_request(&mut run);
                    run["status"] = json!("cancelled");
                    run["stop_reason"] = json!("cancelled by user");
                    clear_lease(c, rid, false)?;
                    if let Some(reply_id) = run["reply_id"].as_str() {
                        c.execute("UPDATE replyq SET ready=0 WHERE reply_id=?", [reply_id])?;
                    }
                } else {
                    let question = body
                        .get("question_id")
                        .or_else(|| event.get("question_id"))
                        .and_then(Value::as_str);
                    if let Some(qid) = question {
                        let mut question_call = load(c, owner, "tool_call", qid)?;
                        if question_call["run_id"] != rid {
                            return Err(StoreError::NotFound);
                        }
                        if question_call["tool_id"] != "ask_user"
                            || question_call["status"] != "awaiting_input"
                        {
                            return Err(StoreError::Conflict(
                                "question is no longer awaiting input".into(),
                            ));
                        }
                        question_call["status"] = json!("completed");
                        question_call["result"] = event_content(&event);
                        question_call["answer_event_id"] = json!(event_id);
                        save(c, "tool_call", &question_call)?;
                        super::work::tool_history(
                            c,
                            owner,
                            &run,
                            &question_call,
                            &question_call["result"],
                        )?;
                        if run["status"] == "waiting" && super::work::check_wait(c, &run)? {
                            super::work::queue_awakened(c, &mut run)?;
                        }
                    }
                    let position = run["transcript"].as_array().map_or(0, Vec::len)
                        + run["pending_batch"].as_array().map_or(0, Vec::len);
                    run["input_positions"][event_id] = json!(position);
                    run["inputs"]
                        .as_array_mut()
                        .ok_or_else(|| StoreError::Storage("invalid run inputs".into()))?
                        .push(event.clone());
                    let progress = question.is_none()
                        && body["respond_now"] == true
                        && run["status"] == "waiting"
                        && super::work::waiting_on_async_tool(c, &run)?;
                    if progress {
                        run["progress_requested"] = json!(true);
                        run["progress_request_id"] = json!(event_id);
                        run["wake_reason"] = json!("progress_request");
                    }
                    if question.is_none()
                        && (progress || run["status"] != "waiting" || run["waiting_input"] == true)
                    {
                        run["waiting_input"] = json!(false);
                        run["status"] = json!("queued");
                        clear_lease(c, rid, true)?;
                    }
                }
                append_history(c, owner, s, &did, &input_item(&event, &did, Some(rid)))?;
                save(c, "run", &run)?;
                emit(
                    c,
                    owner,
                    if action == "request_cancel" {
                        "run.cancelled"
                    } else {
                        "run.input_received"
                    },
                    json!({"run_id":rid,"session_id":s,"delivery_id":did}),
                )?;
            }
        }
        _ => return Err(StoreError::Invalid("unsupported delivery action".into())),
    }
    let delivery = create(c, owner, "delivery", &did, Some(event_id), delivery)?;
    emit(
        c,
        owner,
        "delivery.received",
        json!({"delivery_id":did,"target":target,"source_event_id":event_id}),
    )?;
    remember(c, owner, &op, key, &requested, &did)?;
    Ok(delivery)
}
