use crate::{App,context::{self,GoalInput},store::Work};
use anyhow::{Context,Result,anyhow,bail};
use serde_json::{Value,json};
use std::time::Duration;

mod tools;

fn s<'a>(obj:&'a Value,key:&str)->Result<&'a str>{obj[key].as_str().with_context(||format!("missing {key}"))}
fn objects(v:&Value)->Vec<Value>{v.as_array().cloned().unwrap_or_default()}
pub fn chat_history(v:&Value)->Vec<Value>{
    objects(v).into_iter().filter_map(|m|{
        let role=m["role"].as_str()?;
        if !matches!(role,"user"|"assistant"|"tool"){return None}
        let content=if m["content"].is_string()||m["content"].is_array(){m["content"].clone()}else{json!(m["content"].to_string())};
        // A historic tool receipt can outlive the ModelCall that issued it.
        // Render it as source material unless the provider-native pair is present.
        if role=="tool"{Some(json!({"role":"user","content":format!("Stored tool result: {content}")}))}
        else{Some(json!({"role":role,"content":content}))}
    }).collect()
}
pub fn externalize(app:&App,call:&Value,text:&str)->Result<Value>{
    let description=format!("{} {}",call["tool_id"].as_str().unwrap_or("tool"),call["arguments"]["job_name"].as_str().or(call["job_id"].as_str()).unwrap_or("result"));
    let meta=app.notes.externalize(s(call,"session_id")?,s(call,"id")?,0,&description,text)?;
    let mut val=json!(meta);
    val["preview"]=json!(text.chars().take(180).collect::<String>());
    Ok(val)
}
pub async fn worker(app:App,id:String){
    loop{
        match app.db.claim(&id,45) {
            Ok(Some(work))=>{
                let renewal=async{
                    loop{tokio::time::sleep(Duration::from_secs(12)).await;if !app.db.renew(&work,45).unwrap_or(false){break}}
                };
                tokio::select! {
                    result=step(&app,&work)=>if let Err(e)=result {
                        if e.downcast_ref::<crate::store::StoreError>().is_none_or(|err|!matches!(err,crate::store::StoreError::StaleLease)){
                            let _=app.db.finish(&work,"failed",json!({"error":e.to_string()}));
                        }
                    },
                    _=renewal=>{}
                }
            }
            Ok(None)=>{tokio::select!{_=app.ready.notified()=>{},_=tokio::time::sleep(Duration::from_millis(150))=>{}}}
            Err(e)=>{eprintln!("worker store: {e}");tokio::time::sleep(Duration::from_millis(500)).await;}
        }
    }
}
fn goal_input(run:&Value)->Option<GoalInput>{
    let g=&run["goal"];
    Some(GoalInput{objective:g["objective"].as_str()?.to_owned(),completion_condition:g["completion_condition"].as_str().unwrap_or("").to_string(),
        constraints:objects(&run["inputs"]).iter().skip(1).filter_map(|e|e["content"].as_str().map(str::to_owned)).collect()})
}
fn routes(app:&App,agent:&Value)->Vec<String>{
    let mut names=Vec::new();
    if let Some(first)=agent["model"]["default"].as_str().or(agent["model"].as_str()).or(app.cfg.default_model.as_deref()){names.push(first.into());}
    for v in objects(&agent["model"]["fallbacks"]){if let Some(id)=v.as_str(){if !names.iter().any(|s|s==id){names.push(id.to_string())}}}
    names
}
fn schemas(catalog:&[Value],goal:bool)->Vec<Value>{
    let mut tools=Vec::new();
    let mut members=Vec::new();
    for tool in catalog {
        if tool["direct"]==true {
            if let Some(id)=tool["id"].as_str(){tools.push(function(&tools::wire_name(id),tool["description"].as_str().unwrap_or(""),tool["input_schema"].clone()));}
        }else{ members.push(json!({"id":tool["id"],"description":tool["description"],"parameters":tool["input_schema"]}));}
    }
    if !members.is_empty(){
        tools.push(function("code_exec",&format!("Run a Lua program. Use return tools.call('id', {{key=value}}), or return tools.parallel({{{{id='tool',args={{}}}}}}). Return values become the tool result. Available operations: {}",json!(members)),
            json!({"type":"object","properties":{"code":{"type":"string"}},"required":["code"],"additionalProperties":false})));
    }
    if goal{
        tools.push(function("goal_complete","Mark the current goal completed with evidence from this run.",json!({"type":"object","properties":{"evidence":{"type":"string"}},"required":["evidence"]})));
    }
    tools
}
fn function(name:&str,description:&str,parameters:Value)->Value{json!({"type":"function","function":{"name":name,"description":description,"parameters":parameters}})}

async fn step(app:&App,w:&Work)->Result<()>{
    let run=&w.run;
    let owner=s(run,"owner")?;
    let agent_id=s(run,"agent_id")?;
    let rid=s(run,"id")?;
    let sid=s(run,"session_id")?;
    let agent=&run["agent_snapshot"];
    let step=run["step"].as_u64().unwrap_or_default();
    let goal=goal_input(run);
    let round=run["round"].as_u64().unwrap_or(0) as usize;
    let budget=run["goal"]["max_rounds"].as_u64().map(|v|v as usize).unwrap_or(app.cfg.max_rounds).min(app.cfg.max_rounds);
    let mut transcript=objects(&run["transcript"]);
    // Finish an explicitly parked asynchronous tool batch before requesting another model turn.
    if let Some(pending)=run["pending_batch"].as_array() {
        for pair in pending {
            let call=app.db.get(owner,"tool_call",s(pair,"id")?)?;
            let result=tools::resume(app,w,&call).await?;
            if let Some(wait)=result.get("waiting_for") {
                app.db.finish(w,"waiting",json!({"waiting_for":wait}))?;return Ok(())
            }
            transcript.push(json!({"role":"tool","tool_call_id":pair["provider_id"],"content":result.to_string()}));
        }
    }
    if round>=budget{
        app.db.finish(w,"failed",json!({"stop_reason":"model round budget exhausted","partial_result":run["progress"],"transcript":transcript}))?;
        return Ok(())
    }
    let directory=app.catalog.tools(owner,false,agent_id).map_err(anyhow::Error::msg)?;
    let directory=objects(&directory);
    let schema=schemas(&directory,goal.is_some() && run["goal_completed"]!=true);
    let all_history=app.db.history(owner,sid)?;
    let history_items=objects(&all_history);
    // Build a fixed start-of-run prefix. Later independent work is outside this Run's input.
    let mut base=Vec::new();
    for item in history_items{
        if item["run_id"]==rid{break}
        base.push(item);
    }
    let mut conversation=chat_history(&Value::Array(base));
    let inputs=objects(&run["inputs"]);
    if let Some(initial)=inputs.first(){conversation.push(input_message(initial));}
    conversation.extend(transcript.clone());
    conversation.extend(inputs.iter().skip(1).map(input_message));
    let system=agent["system_prompt"].as_str().unwrap_or("You are a helpful assistant.");
    let mut sections=directory.iter().filter_map(|t|t["section"].as_str().map(str::to_owned)).collect::<Vec<_>>();
    sections.extend(app.catalog.sections(owner,false,agent_id).map_err(anyhow::Error::msg)?.iter().filter_map(|section|section["text"].as_str().map(str::to_owned)));
    let continuation=run["next_reason"]=="goal" && run["wake_reason"]!="tool_result";
    let mut messages=context::build_messages(system,&sections,&conversation,goal.as_ref(),continuation);
    let names=routes(app,agent);
    if names.is_empty(){bail!("no available model configured for this agent")}
    let mut last_error=String::from("no compatible model");
    let mut accepted=None;
    let reason=if continuation{"goal"}else if round==0{"input"}else{"tool_result"};
    for name in names{
        let Some(provider)=app.cfg.models.get(&name) else{continue};
        if messages.iter().any(|m|m["content"].as_array().is_some_and(|parts|parts.iter().any(|p|p["type"]=="image_url"))) && !provider.images{continue}
        let limit=agent["context_bytes"].as_u64().unwrap_or(256_000) as usize;
        let mut covered=Vec::new();
        if serde_json::to_vec(&messages)?.len()>limit{
            let summary=context::compact(&app.http,&provider.route(),&messages,limit).await.map_err(|e|anyhow!(e.to_string()))?;
            messages=summary.messages;covered=summary.covered;
        }
        let req=json!({"call_key":format!("model:{step}:{}:{name}",inputs.len()),"model":name,"provider_model":provider.model,"reason":reason,"messages":messages,"compressed_ranges":covered,"tools":schema,"tool_directory":directory,"status":"pending"});
        let call=app.db.record_call(w,"model_call",req)?;
        if call["status"]=="completed"{accepted=Some(call["message"].clone());break}
        if call["status"]=="failed"{last_error=call["error"].as_str().unwrap_or("model failed").into();continue}
        let cid=s(&call,"id")?;
        let response=context::complete(&app.http,&provider.route(),&messages,&schema,None).await;
        match response{
            Ok(reply) if matches!(reply.finish_reason.as_str(),"stop"|"tool_calls")=>{
                app.db.update_call(w,"model_call",cid,json!({"status":"completed","message":reply.message,"finish_reason":reply.finish_reason,"usage":reply.usage}))?;
                accepted=Some(reply.message);break
            }
            Ok(reply)=>{last_error=format!("model stopped with {}",reply.finish_reason);app.db.update_call(w,"model_call",cid,json!({"status":"failed","error":last_error}))?;}
            Err(e)=>{last_error=e.to_string();app.db.update_call(w,"model_call",cid,json!({"status":"failed","error":last_error}))?;if !e.retryable{break}}
        }
    }
    let message=accepted.ok_or_else(||anyhow!(last_error))?;
    let requests=objects(&message["tool_calls"]);
    let answer=message["content"].as_str().unwrap_or("").to_owned();
    if requests.is_empty(){
        if answer.trim().is_empty(){bail!("empty final response")}
        if goal.is_some() && run["goal_completed"]!=true{
            transcript.push(message);
            app.db.finish(w,"queued",json!({"round":round+1,"transcript":transcript,"pending_batch":null,"progress":answer,"next_reason":"goal","wake_reason":null}))?;
        }else{
            let result=if run["result_schema"].is_object(){
                let value:Value=serde_json::from_str(&answer).context("final answer is not JSON")?;
                let validator=jsonschema::validator_for(&run["result_schema"]).map_err(|e|anyhow!("invalid result schema: {e}"))?;
                if !validator.is_valid(&value){bail!("final answer failed result schema validation")} value
            }else{json!(answer)};
            let contract=&run["delivery_contract"];
            if let Some(target)=contract["target"].as_str().filter(|_|contract["when"]!="failure" || result["success"]==false || result["status"]=="failed"){
                if !app.cfg.notification_targets.iter().any(|t|t==target){bail!("delivery target is not configured")}
                app.db.prepare_reply(w,json!({"target":target,"content":result,"capability":contract["capability"]}))?;
            }else{
                app.db.finish(w,"completed",json!({"output":result,"round":round+1,"pending_batch":null}))?;
            }
        }
        return Ok(())
    }
    transcript.push(message);
    let mut pending=vec![];let mut waits=vec![];
    let mut marked=run["goal_completed"]==true;
    for (i,req) in requests.iter().enumerate(){
        let name=s(&req["function"],"name")?;
        let args=serde_json::from_str(req["function"]["arguments"].as_str().unwrap_or("{}")).unwrap_or(Value::Null);
        let provider_id=s(req,"id")?;
        if name=="goal_complete" && goal.is_some() && args["evidence"].as_str().is_some_and(|v|!v.trim().is_empty()){
            marked=true;transcript.push(json!({"role":"tool","tool_call_id":provider_id,"content":format!("Completion recorded: {}",args["evidence"])}));continue
        }
        let (call_id,out)=tools::top(app,w,&directory,name,args,&format!("tool:{step}:{}:{i}",inputs.len())).await?;
        if let Some(list)=out["waiting_for"].as_array(){waits.extend(list.iter().cloned());pending.push(json!({"id":call_id,"provider_id":provider_id}));}
        else{transcript.push(json!({"role":"tool","tool_call_id":provider_id,"content":out.to_string()}));}
    }
    let patch=json!({"transcript":transcript,"round":round+1,"goal_completed":marked,"pending_batch":if pending.is_empty(){Value::Null}else{json!(pending)},"waiting_for":waits,"next_reason":"tool_result","wake_reason":null});
    app.db.finish(w,if waits.is_empty(){"queued"}else{"waiting"},patch)?;
    Ok(())
}
fn input_message(event:&Value)->Value {
    let v=event.get("content").or(event.get("text")).or(event.get("data")).cloned().unwrap_or_default();
    json!({"role":"user","content":if v.is_string()||v.is_array(){v}else{json!(v.to_string())}})
}
