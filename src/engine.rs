use crate::{
    App,
    context::{self, GoalInput},
    store::Work,
};
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::time::Duration;

#[cfg(test)]
mod recovery_tests;
mod tools;

fn s<'a>(obj: &'a Value, key: &str) -> Result<&'a str> {
    obj[key].as_str().with_context(|| format!("missing {key}"))
}
fn objects(v: &Value) -> Vec<Value> {
    v.as_array().cloned().unwrap_or_default()
}
pub fn chat_history(v: &Value) -> Vec<Value> {
    objects(v)
        .into_iter()
        .filter_map(|m| {
            let role = m["role"].as_str()?;
            if !matches!(role, "user" | "assistant" | "tool") {
                return None;
            }
            let content = if m["content"].is_string() || m["content"].is_array() {
                m["content"].clone()
            } else {
                json!(m["content"].to_string())
            };
            // A historic tool receipt can outlive the ModelCall that issued it.
            // Render it as source material unless the provider-native pair is present.
            if role == "tool" {
                Some(json!({"role":"user","content":format!("Stored tool result: {content}")}))
            } else {
                Some(json!({"role":role,"content":content}))
            }
        })
        .collect()
}
fn externalize(app: &App, call: &Value, part: usize, text: &str) -> Result<Value> {
    let description = format!(
        "{} {}",
        call["tool_id"].as_str().unwrap_or("tool"),
        call["arguments"]["job_name"]
            .as_str()
            .or(call["job_id"].as_str())
            .unwrap_or("result")
    );
    let meta = app.notes.externalize(
        s(call, "session_id")?,
        s(call, "id")?,
        part,
        &description,
        text,
    )?;
    let mut val = json!(meta);
    val["preview"] = json!(text.chars().take(180).collect::<String>());
    Ok(val)
}
pub fn store_parts(app: &App, call: &Value, mut value: Value) -> Result<Value> {
    if let Some(text) = value.as_str().filter(|t| t.len() > app.cfg.note_threshold) {
        return externalize(app, call, 0, text);
    }
    let Some(blocks) = value.get_mut("content").and_then(Value::as_array_mut) else {
        return Ok(value);
    };
    let single = blocks.len() == 1;
    for (index, block) in blocks.iter_mut().enumerate() {
        if block["type"] == "text"
            && let Some(text) = block["text"]
                .as_str()
                .filter(|t| t.len() > app.cfg.note_threshold)
        {
            let reference = externalize(app, call, index, text)?;
            if single {
                return Ok(reference);
            }
            *block = reference;
            block["type"] = json!("text");
            block["text"] = block["preview"].clone();
        }
    }
    Ok(value)
}
pub async fn worker(app: App, id: String) {
    loop {
        match app.db.claim(&id, app.cfg.lease_seconds) {
            Ok(Some(work)) => {
                let renewal = async {
                    loop {
                        tokio::time::sleep(Duration::from_millis(
                            app.cfg.lease_seconds as u64 * 250,
                        ))
                        .await;
                        if !app.db.renew(&work, app.cfg.lease_seconds).unwrap_or(false) {
                            break;
                        }
                    }
                };
                tokio::select! {
                    result=step(&app,&work)=>if let Err(e)=result
                        && e.downcast_ref::<crate::store::StoreError>().is_none_or(|err|!matches!(err,crate::store::StoreError::StaleLease)){
                            let _=app.db.finish(&work,"failed",json!({"error":e.to_string()}));
                        },
                    _=renewal=>{}
                }
            }
            Ok(None) => {
                tokio::select! {_=app.ready.notified()=>{},_=tokio::time::sleep(Duration::from_millis(150))=>{}}
            }
            Err(e) => {
                eprintln!("worker store: {e}");
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
}
fn goal_input(run: &Value) -> Option<GoalInput> {
    let g = &run["goal"];
    Some(GoalInput {
        objective: g["objective"].as_str()?.to_owned(),
        completion_condition: g["completion_condition"].as_str().unwrap_or("").to_string(),
        constraints: objects(&run["inputs"])
            .iter()
            .skip(1)
            .filter_map(|e| e["content"].as_str().map(str::to_owned))
            .collect(),
    })
}
fn routes(app: &App, agent: &Value) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(first) = agent["model"]["default"]
        .as_str()
        .or(agent["model"].as_str())
        .or(app.cfg.default_model.as_deref())
    {
        names.push(first.into());
    }
    for v in objects(&agent["model"]["fallbacks"]) {
        if let Some(id) = v.as_str()
            && !names.iter().any(|s| s == id)
        {
            names.push(id.to_string())
        }
    }
    names
}
fn schemas(catalog: &[Value], goal: bool) -> Vec<Value> {
    let mut tools = Vec::new();
    let mut members = Vec::new();
    for tool in catalog {
        if tool["direct"] == true {
            if let Some(id) = tool["id"].as_str() {
                tools.push(function(
                    &tools::wire_name(id),
                    tool["description"].as_str().unwrap_or(""),
                    tool["input_schema"].clone(),
                ));
            }
        } else {
            members.push(json!({"id":tool["id"],"description":tool["description"],"parameters":tool["input_schema"]}));
        }
    }
    if !members.is_empty() {
        tools.push(function("code_exec",&format!("Run a Lua program. Use return tools.call('id', {{key=value}}), or return tools.parallel({{{{id='tool',args={{}}}}}}). Return values become the tool result. Available operations: {}",json!(members)),
            json!({"type":"object","properties":{"code":{"type":"string"}},"required":["code"],"additionalProperties":false})));
    }
    if goal {
        tools.push(function("goal_complete","Mark the current goal completed with evidence from this run.",json!({"type":"object","properties":{"evidence":{"type":"string"}},"required":["evidence"]})));
    }
    tools
}
fn function(name: &str, description: &str, parameters: Value) -> Value {
    json!({"type":"function","function":{"name":name,"description":description,"parameters":parameters}})
}

fn select_context(
    app: &App,
    owner: &str,
    agent_id: &str,
    history: Vec<Value>,
    run_id: Option<&str>,
) -> Result<Vec<Value>> {
    let Some(selected) = app
        .catalog
        .apply_hook(
            owner,
            agent_id,
            "context",
            &json!({"messages":history,"run_id":run_id}),
        )
        .map_err(anyhow::Error::msg)?
    else {
        return Ok(history);
    };
    let messages = selected
        .as_array()
        .context("context plugin must return a message array")?;
    if messages
        .iter()
        .any(|m| !matches!(m["role"].as_str(), Some("user" | "assistant" | "tool")))
    {
        bail!("unsupported role from context plugin")
    }
    Ok(messages.clone())
}
fn sections_for(
    app: &App,
    owner: &str,
    agent_id: &str,
    directory: &[Value],
) -> Result<Vec<String>> {
    let mut sections = directory
        .iter()
        .filter_map(|t| t["section"].as_str().map(str::to_owned))
        .collect::<Vec<_>>();
    sections.extend(
        app.catalog
            .sections(owner, false, agent_id)
            .map_err(anyhow::Error::msg)?
            .iter()
            .filter_map(|section| section["text"].as_str().map(str::to_owned)),
    );
    Ok(sections)
}
pub fn preview(app: &App, owner: &str, sid: &str, agent_id: &str) -> Result<Value> {
    let agent = app
        .catalog
        .get_agent(owner, false, agent_id)
        .map_err(anyhow::Error::msg)?;
    let directory = objects(
        &app.catalog
            .tools(owner, false, agent_id)
            .map_err(anyhow::Error::msg)?,
    );
    let conversation = select_context(
        app,
        owner,
        agent_id,
        chat_history(&app.db.history(owner, sid)?),
        None,
    )?;
    let sections = sections_for(app, owner, agent_id, &directory)?;
    Ok(
        json!({"messages":context::build_messages(agent["system_prompt"].as_str().unwrap_or("You are a helpful assistant."),&sections,&conversation,None,false),"tools":schemas(&directory,false)}),
    )
}

async fn step(app: &App, w: &Work) -> Result<()> {
    let run = &w.run;
    let owner = s(run, "owner")?;
    let agent_id = s(run, "agent_id")?;
    let rid = s(run, "id")?;
    let sid = s(run, "session_id")?;
    let agent = &run["agent_snapshot"];
    let step = run["step"].as_u64().unwrap_or_default();
    let mut goal = goal_input(run);
    let round = run["round"].as_u64().unwrap_or(0) as usize;
    let limit = agent["_max_rounds"]
        .as_u64()
        .unwrap_or(app.cfg.max_rounds as u64);
    let budget = run["goal"]["max_rounds"]
        .as_u64()
        .unwrap_or(limit)
        .min(limit) as usize;
    let policy = if goal.is_some() {
        app.catalog.goal_strategy(&json!({"completed":run["goal_completed"]==true,"cancelled":false,"budget_exhausted":round>=budget,"unrecoverable":false,"waiting":false})).map_err(anyhow::Error::msg)?
    } else {
        None
    };
    if policy.is_none() {
        goal = None;
    }
    let mut transcript = objects(&run["transcript"]);
    let mut received = Vec::new();
    // Finish an explicitly parked asynchronous tool batch before requesting another model turn.
    if let Some(pending) = run["pending_batch"].as_array() {
        for pair in pending {
            let call = app.db.get(owner, "tool_call", s(pair, "id")?)?;
            let result = tools::resume(app, w, &call).await?;
            if let Some(wait) = result.get("waiting_for") {
                app.db.finish(w, "waiting", json!({"waiting_for":wait}))?;
                return Ok(());
            }
            let receipt = json!({"role":"tool","tool_call_id":pair["provider_id"],"content":result.to_string()});
            received.push(receipt.clone());
            transcript.push(receipt);
        }
    }
    if round >= budget {
        let partial = run
            .get("progress")
            .filter(|p| !p.is_null())
            .cloned()
            .or_else(|| {
                transcript
                    .iter()
                    .rev()
                    .find_map(|m| m.get("content").filter(|c| !c.is_null()).cloned())
            });
        app.db.finish(w,"failed",json!({"stop_reason":"model round budget exhausted","partial_result":partial,"transcript":transcript}))?;
        return Ok(());
    }
    let directory = app
        .catalog
        .tools(owner, false, agent_id)
        .map_err(anyhow::Error::msg)?;
    let mut directory = objects(&directory);
    let schema = schemas(&directory, goal.is_some() && run["goal_completed"] != true);
    let all_history = app.db.history(owner, sid)?;
    let history_items = objects(&all_history);
    // Build a fixed start-of-run prefix. Later independent work is outside this Run's input.
    let mut base = Vec::new();
    for item in history_items {
        if item["run_id"] == rid {
            break;
        }
        base.push(item);
    }
    let mut conversation = chat_history(&Value::Array(base));
    let inputs = objects(&run["inputs"]);
    let trigger = app
        .catalog
        .apply_hook(
            owner,
            agent_id,
            "trigger",
            &json!({"run_id":rid,"inputs":inputs,"step":step,"reason":run["wake_reason"]}),
        )
        .map_err(anyhow::Error::msg)?;
    if trigger == Some(Value::Bool(false)) {
        app.db
            .finish(w, "waiting", json!({"waiting_input":true,"waiting_for":[]}))?;
        return Ok(());
    }
    if let Some(initial) = inputs.first() {
        conversation.push(input_message(initial));
    }
    let mut offset = 0;
    for input in inputs.iter().skip(1) {
        let position = input["id"]
            .as_str()
            .and_then(|id| run["input_positions"][id].as_u64())
            .map_or(transcript.len(), |n| n as usize)
            .clamp(offset, transcript.len());
        conversation.extend_from_slice(&transcript[offset..position]);
        conversation.push(input_message(input));
        offset = position;
    }
    conversation.extend_from_slice(&transcript[offset..]);
    conversation = select_context(app, owner, agent_id, conversation, Some(rid))?;
    if goal.is_some() {
        for receipt in received {
            if let Some(item) = conversation
                .iter_mut()
                .find(|m| m["role"] == "tool" && m["tool_call_id"] == receipt["tool_call_id"])
            {
                item["context_protected"] = json!(true);
            } else {
                conversation.push(json!({"role":"user","source":"tool_evidence","context_protected":true,"content":format!("Received tool result for {}: {}",receipt["tool_call_id"],receipt["content"])}));
            }
        }
    }
    let system = agent["system_prompt"]
        .as_str()
        .unwrap_or("You are a helpful assistant.");
    let mut sections = sections_for(app, owner, agent_id, &directory)?;
    if run["result_schema"].is_object() {
        sections.push(format!("Return the final answer as a JSON value matching this schema. Intermediate tool results retain their own types.\n{}",run["result_schema"]));
    }
    let continuation = run["next_reason"] == "goal" && run["wake_reason"] != "tool_result";
    let prompt = policy
        .as_ref()
        .and_then(|p| p["instruction"].as_str())
        .filter(|_| continuation);
    let mut messages = context::build_messages_with_goal_prompt(
        system,
        &sections,
        &conversation,
        goal.as_ref(),
        prompt,
    );
    crate::attachments::resolve(&app.cfg.home, owner, &mut messages)?;
    let names = routes(app, agent);
    if names.is_empty() {
        bail!("no available model configured for this agent")
    }
    let mut last_error = String::from("no compatible model");
    let mut accepted = None;
    let prior_calls = objects(&app.db.list(owner, "model_call", Some(rid))?);
    let reason = if continuation {
        "goal"
    } else if round == 0 {
        "input"
    } else {
        "tool_result"
    };
    for name in names {
        let Some(provider) = app.cfg.models.get(&name) else {
            continue;
        };
        if messages.iter().any(|m| {
            m["content"]
                .as_array()
                .is_some_and(|parts| parts.iter().any(|p| p["type"] == "image_url"))
        }) && !provider.images
        {
            continue;
        }
        let key = format!("model:{step}:{}:{name}", inputs.len());
        let previous = prior_calls.iter().find(|c| c["call_key"] == key);
        let limit = agent["context_bytes"].as_u64().unwrap_or(256_000) as usize;
        let mut covered = Vec::new();
        if previous.is_none() && serde_json::to_vec(&messages)?.len() > limit {
            let summary = context::compact(&app.http, &provider.route(), &messages, limit)
                .await
                .map_err(|e| anyhow!(e.to_string()))?;
            messages = summary.messages;
            covered = summary.covered;
        }
        let call = if let Some(old) = previous {
            old.clone()
        } else {
            let req = json!({"call_key":key,"model":name,"provider_model":provider.model,"reason":reason,"messages":messages,"compressed_ranges":covered,"tools":schema,"tool_directory":directory,"goal_policy":policy,"status":"pending"});
            app.db.record_call(w, "model_call", req)?
        };
        if call["status"] == "completed" {
            directory = objects(&call["tool_directory"]);
            accepted = Some(call["message"].clone());
            break;
        }
        if call["status"] == "failed" {
            last_error = call["error"].as_str().unwrap_or("model failed").into();
            continue;
        }
        let cid = s(&call, "id")?;
        let response = context::complete(
            &app.http,
            &provider.route(),
            &objects(&call["messages"]),
            &objects(&call["tools"]),
            None,
        )
        .await;
        match response {
            Ok(reply) if matches!(reply.finish_reason.as_str(), "stop" | "tool_calls") => {
                app.db.update_call(w,"model_call",cid,json!({"status":"completed","message":reply.message,"finish_reason":reply.finish_reason,"usage":reply.usage}))?;
                directory = objects(&call["tool_directory"]);
                accepted = Some(reply.message);
                break;
            }
            Ok(reply) => {
                last_error = format!("model stopped with {}", reply.finish_reason);
                app.db.update_call(
                    w,
                    "model_call",
                    cid,
                    json!({"status":"failed","error":last_error}),
                )?;
            }
            Err(e) => {
                last_error = e.to_string();
                app.db.update_call(
                    w,
                    "model_call",
                    cid,
                    json!({"status":"failed","error":last_error}),
                )?;
                if !e.retryable {
                    break;
                }
            }
        }
    }
    let message = accepted.ok_or_else(|| anyhow!(last_error))?;
    let requests = objects(&message["tool_calls"]);
    let answer = message["content"].as_str().unwrap_or("").to_owned();
    if requests.is_empty() {
        if answer.trim().is_empty() {
            bail!("empty final response")
        }
        let continue_goal=goal.is_some() && run["goal_completed"]!=true && app.catalog.goal_strategy(&json!({"completed":false,"cancelled":false,"waiting":false,"budget_exhausted":false,"unrecoverable":false})).map_err(anyhow::Error::msg)?.is_some_and(|p|p["decision"]=="continue");
        if continue_goal {
            transcript.push(message);
            app.db.finish(w,"queued",json!({"round":round+1,"transcript":transcript,"pending_batch":null,"progress":answer,"next_reason":"goal","wake_reason":null}))?;
        } else {
            let result = if run["result_schema"].is_object() {
                let value: Value =
                    serde_json::from_str(&answer).context("final answer is not JSON")?;
                let validator = jsonschema::validator_for(&run["result_schema"])
                    .map_err(|e| anyhow!("invalid result schema: {e}"))?;
                if !validator.is_valid(&value) {
                    bail!("final answer failed result schema validation")
                }
                value
            } else {
                json!(answer)
            };
            let contract = &run["delivery_contract"];
            if let Some(target) = contract["target"].as_str().filter(|_| {
                contract["when"] != "failure"
                    || result["success"] == false
                    || result["status"] == "failed"
            }) {
                if !app.cfg.notification_targets.iter().any(|t| t == target) {
                    bail!("delivery target is not configured")
                }
                app.db.prepare_reply(
                    w,
                    json!({"target":target,"content":result,"capability":contract["capability"]}),
                )?;
            } else {
                app.db.finish(
                    w,
                    "completed",
                    json!({"output":result,"round":round+1,"pending_batch":null}),
                )?;
            }
        }
        return Ok(());
    }
    transcript.push(message);
    let mut pending = vec![];
    let mut waits = vec![];
    let mut marked = run["goal_completed"] == true;
    for (i, req) in requests.iter().enumerate() {
        let name = s(&req["function"], "name")?;
        let args = serde_json::from_str(req["function"]["arguments"].as_str().unwrap_or("{}"))
            .unwrap_or(Value::Null);
        let provider_id = s(req, "id")?;
        if name == "goal_complete"
            && goal.is_some()
            && args["evidence"]
                .as_str()
                .is_some_and(|v| !v.trim().is_empty())
        {
            marked = true;
            transcript.push(json!({"role":"tool","tool_call_id":provider_id,"content":format!("Completion recorded: {}",args["evidence"])}));
            continue;
        }
        let (call_id, out) = tools::top(
            app,
            w,
            &directory,
            name,
            args,
            &format!("tool:{step}:{}:{i}", inputs.len()),
        )
        .await?;
        if let Some(list) = out["waiting_for"].as_array() {
            waits.extend(list.iter().cloned());
            pending.push(json!({"id":call_id,"provider_id":provider_id}));
        } else {
            transcript
                .push(json!({"role":"tool","tool_call_id":provider_id,"content":out.to_string()}));
        }
    }
    let patch = json!({"transcript":transcript,"round":round+1,"goal_completed":marked,"pending_batch":if pending.is_empty(){Value::Null}else{json!(pending)},"waiting_for":waits,"next_reason":"tool_result","wake_reason":null});
    app.db.finish(
        w,
        if waits.is_empty() {
            "queued"
        } else {
            "waiting"
        },
        patch,
    )?;
    Ok(())
}
fn input_message(event: &Value) -> Value {
    let v = event
        .get("content")
        .or(event.get("text"))
        .or(event.get("data"))
        .cloned()
        .unwrap_or_default();
    json!({"role":"user","content":if v.is_string()||v.is_array(){v}else{json!(v.to_string())}})
}
