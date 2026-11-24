use super::*;
use mlua::{HookTriggers,Lua,LuaSerdeExt,Table,VmState};
use std::sync::{Arc,atomic::{AtomicUsize,Ordering}};

pub fn wire_name(id:&str)->String{
    // Preserve ordinary function names. Escape dots and underscores separately to avoid collisions.
    id.replace('_',"_u").replace('.',"_d").replace('-',"_h")
}
fn result_of(call:&Value)->Value{
    if call["status"]=="failed" {json!({"error":call["error"].as_str().unwrap_or("tool failed")})} else {call["result"].clone()}
}
fn done(v:&Value)->bool{matches!(v["status"].as_str(),Some("completed"|"failed"|"cancelled"))}
fn pending(id:&str)->Value{json!({"waiting_for":[id]})}

pub async fn top(app:&App,w:&Work,dir:&[Value],name:&str,args:Value,key:&str)->Result<(String,Value)>{
    let id=if name=="code_exec" {name} else {dir.iter().find(|d|d["direct"]==true && d["id"].as_str().is_some_and(|id|wire_name(id)==name)).and_then(|d|d["id"].as_str()).unwrap_or("")};
    let entry=app.db.record_call(w,"tool_call",json!({"call_key":key,"tool_id":id,"arguments":args,"entry":if name=="code_exec"{"code"}else{"direct"},"directory":dir}))?;
    let cid=s(&entry,"id")?.to_owned();
    let output=if done(&entry){result_of(&entry)}else if entry["_created"]!=true{resume(app,w,&entry).await?}else if id=="code_exec" {
        let v=match args["code"].as_str(){
            Some(code)=>code_exec(app,w,dir,&cid,code).await,
            None=>Err(anyhow!("code_exec requires Lua code"))
        };
        settle(app,w,&entry,v)?
    }else if id.is_empty(){settle(app,w,&entry,Err(anyhow!("unavailable native tool")))?}
    else{invoke(app,w,dir,&entry).await?};
    Ok((cid,output))
}
pub async fn resume(app:&App,w:&Work,call:&Value)->Result<Value>{
    if done(call){return Ok(result_of(call))}
    if call["tool_id"]!="code_exec"{return recover_member(app,w,call).await}
    let all=app.db.list(s(&w.run,"owner")?,"tool_call",Some(s(&w.run,"id")?))?;
    let mut children=objects(&all).into_iter().filter(|c|c["parent_id"]==call["id"]).collect::<Vec<_>>();
    if children.is_empty(){return Ok(pending(s(call,"id")?))}
    for child in &mut children {
        if !done(child) {
            recover_member(app,w,child).await?;
            *child=app.db.get(s(&w.run,"owner")?,"tool_call",s(child,"id")?)?;
        }
    }
    let waits=children.iter().filter(|c|!done(c)).map(|c|c["id"].clone()).collect::<Vec<_>>();
    if !waits.is_empty(){return Ok(json!({"waiting_for":waits}))}
    let results=children.iter().map(|c|json!({"tool_id":c["tool_id"],"call_id":c["id"],"result":result_of(c)})).collect::<Vec<_>>();
    settle(app,w,call,Ok(json!(results)))
}
async fn recover_member(app:&App,w:&Work,call:&Value)->Result<Value>{
    let cid=s(call,"id")?;
    let dir=objects(&call["directory"]);
    if call["status"]=="approved"{return invoke(app,w,&dir,call).await}
    if call["external_dispatch"].is_object() && call["status"]!="accepted" {
        let mut tracking=call["external_dispatch"].clone();
        if let Some(job)=call.get("job_id"){tracking["job_id"]=job.clone();}
        let result=app.catalog.recover(s(&w.run,"owner")?,false,s(&w.run,"agent_id")?,s(call,"tool_id")?,call["arguments"].clone(),&call["directory"],&tracking).await;
        match result{
            Ok(v) if v["status"]=="completed"=>return settle(app,w,call,Ok(v["result"].clone())),
            Ok(v) if v["status"]=="waiting"=>{app.db.update_call(w,"tool_call",cid,json!({"status":"accepted","job_id":v["job_id"]}))?;}
            _=>{app.db.update_call(w,"tool_call",cid,json!({"status":"unknown","recovery_status":"awaiting_verification"}))?;}
        }
    }
    Ok(pending(cid))
}
fn settle(app:&App,w:&Work,call:&Value,result:Result<Value>)->Result<Value>{
    let id=s(call,"id")?;
    match result{
        Ok(v) if v.get("waiting_for").is_some()=>{app.db.update_call(w,"tool_call",id,json!({"status":"waiting","waiting_for":v["waiting_for"]}))?;Ok(v)}
        Ok(mut v)=>{
            let tool=call["tool_id"].as_str().unwrap_or("");
            if !matches!(tool,"read_note"|"code_exec"|"search_notes"){
                if let Some(text)=v.as_str().filter(|t|t.len()>app.cfg.note_threshold){v=externalize(app,call,text)?;}
                else if let Some(blocks)=v["content"].as_array(){if blocks.len()==1 && blocks[0]["type"]=="text"{
                    if let Some(text)=blocks[0]["text"].as_str().filter(|s|s.len()>app.cfg.note_threshold){v=externalize(app,call,text)?;}
                }}
            }
            app.db.update_call(w,"tool_call",id,json!({"status":"completed","result":v}))?;Ok(v)
        }
        Err(e)=>{let msg=e.to_string();app.db.update_call(w,"tool_call",id,json!({"status":"failed","error":msg}))?;Ok(json!({"error":msg}))}
    }
}
async fn invoke(app:&App,w:&Work,dir:&[Value],call:&Value)->Result<Value>{
    let owner=s(&w.run,"owner")?;let agent=s(&w.run,"agent_id")?;let sid=s(&w.run,"session_id")?;
    let id=s(call,"tool_id")?;let args=&call["arguments"];
    let outcome:Result<Value>=async {
        let snapshot=json!(dir);
        let tool=app.catalog.authorize(owner,false,agent,id,&snapshot).map_err(anyhow::Error::msg)?;
        if let Some(schema)=tool.get("input_schema").filter(|s|s.is_object()){
            let validator=jsonschema::validator_for(schema).map_err(|e|anyhow!("invalid tool schema: {e}"))?;
            if !validator.is_valid(args){bail!("arguments do not match tool schema")}
        }
        if !app.db.renew(w,45)?{bail!("run is no longer active")}
        if (tool["requires_approval"]==true||tool["approval"]=="required") && call["status"]!="approved"{
            app.db.update_call(w,"tool_call",s(call,"id")?,json!({"status":"awaiting_approval"}))?;
            return Ok(pending(s(call,"id")?))
        }
        match id{
            "agents.get"=>Ok(app.catalog.get_agent(owner,false,agent).map_err(anyhow::Error::msg)?),
            "sessions.history"=>Ok(app.db.history(owner,sid)?),
            "events.search"=>{
                if args["scope"]!="current_agent_inbox"{bail!("unsupported event scope")}
                let deliveries=app.db.list(owner,"delivery",None)?;
                let mut events=vec![];
                for delivery in objects(&deliveries){if delivery["target"]["agent_id"]==agent && delivery["target"]["session_id"].is_null(){events.push(app.db.get(owner,"event",s(&delivery,"event_id")?)?)}}
                Ok(json!(events))
            }
            "write_note"=>Ok(json!(app.notes.write(sid,s(args,"description")?,s(args,"body")?)?)),
            "search_notes"=>Ok(json!(app.notes.search(sid,args["keyword"].as_str(),args["cursor"].as_str(),args["limit"].as_u64().unwrap_or(20) as usize)?)),
            "read_note"=>Ok(json!(app.notes.read(sid,s(args,"note_ref")?,args["start_line"].as_u64().map(|v|v as usize),args["end_line"].as_u64().map(|v|v as usize),app.cfg.note_budget)?)),
            _=>{
                let mut ctx=json!({"agent_id":agent,"session_id":sid,"run_id":w.run["id"],"tool_call_id":call["id"]});
                if let Some(r)=args["note_ref"].as_str(){
                    app.catalog.authorize(owner,false,agent,"read_note",&snapshot).map_err(anyhow::Error::msg)?;
                    ctx["note"]=json!(app.notes.read(sid,r,None,None,app.cfg.note_budget)?);
                }
                let cid=s(call,"id")?;
                let tracked=tool["effectful"]==true;
                let tracking=json!({"tool_call_id":cid,"idempotency_key":cid,"dispatched_at_ms":chrono::Utc::now().timestamp_millis(),"service":tool["connection_key"],"arguments":args,"recovery":tool["recovery"]});
                if tracked{app.db.begin_external(w,cid,tracking.clone())?;}
                let output=app.catalog.execute_tracked(owner,false,agent,id,args.clone(),&snapshot,&ctx,&tracking).await;
                let out=match output{
                    Ok(out)=>out,
                    Err(_) if tracked=>{
                        let existing=app.db.get(owner,"tool_call",cid)?;
                        if done(&existing){return Ok(result_of(&existing))}
                        let recovered=app.catalog.recover(owner,false,agent,id,args.clone(),&snapshot,&tracking).await;
                        if let Ok(v)=recovered {
                            if v["status"]=="completed"{return Ok(v["result"].clone())}
                            if v["status"]=="waiting" {app.db.update_call(w,"tool_call",cid,json!({"status":"accepted","job_id":v["job_id"]}))?;return Ok(pending(cid))}
                        }
                        app.db.update_call(w,"tool_call",cid,json!({"status":"unknown","recovery_status":"awaiting_verification"}))?;return Ok(pending(cid))
                    }
                    Err(e)=>return Err(anyhow!(e))
                };
                let receipt=out.get("structuredContent").unwrap_or(&out);
                if matches!(receipt["status"].as_str(),Some("accepted"|"pending"|"waiting")) || (tracked && !receipt["job_id"].is_null() && receipt["status"]!="completed"){
                    app.db.update_call(w,"tool_call",cid,json!({"status":"accepted","job_id":receipt["job_id"]}))?;
                    return Ok(pending(s(call,"id")?))
                }
                Ok(out)
            }
        }
    }.await;
    if outcome.as_ref().ok().is_some_and(|v|v.get("waiting_for").is_some()){
        let existing=app.db.get(owner,"tool_call",s(call,"id")?)?;
        if done(&existing){return Ok(result_of(&existing))}
        return outcome
    }
    settle(app,w,call,outcome)
}

#[derive(Clone)]
struct Program {app:App,work:Work,dir:Vec<Value>,parent:String,ordinal:Arc<AtomicUsize>}
impl Program {
    async fn call(&self,id:String,args:Value)->Result<Value>{
        let n=self.ordinal.fetch_add(1,Ordering::Relaxed);
        if n>=64{bail!("program tool-call limit exceeded")}
        let entry=self.app.db.record_call(&self.work,"tool_call",json!({"call_key":format!("{}:{n}",self.parent),"parent_id":self.parent,"tool_id":id,"arguments":args,"entry":"program","directory":self.dir}))?;
        if done(&entry){return Ok(result_of(&entry))}
        if entry["_created"]!=true{return Ok(pending(s(&entry,"id")?))}
        if !self.dir.iter().any(|t|t["id"]==id && t["direct"]!=true){return settle(&self.app,&self.work,&entry,Err(anyhow!("not available through Code Mode")))}
        invoke(&self.app,&self.work,&self.dir,&entry).await
    }
}
async fn code_exec(app:&App,w:&Work,dir:&[Value],parent:&str,code:&str)->Result<Value>{
    if code.len()>65536{bail!("program too large")}
    let lua=Lua::new();
    lua.set_memory_limit(8*1024*1024)?;
    let ticks=AtomicUsize::new(0);
    lua.set_global_hook(HookTriggers::new().every_nth_instruction(1000),move |_,_|{
        if ticks.fetch_add(1,Ordering::Relaxed)>1000{return Err(mlua::Error::runtime("Lua instruction budget exhausted"))}Ok(VmState::Continue)
    })?;
    let env=lua.create_table()?;
    let globals=lua.globals();
    for name in ["ipairs","pairs","next","select","tonumber","tostring","type","assert","error","math","table","string","utf8"]{
        env.set(name,globals.get::<mlua::Value>(name)?)?;
    }
    let program=Program{app:app.clone(),work:w.clone(),dir:dir.to_vec(),parent:parent.into(),ordinal:Default::default()};
    let api=lua.create_table()?;
    let one=program.clone();
    api.set("call",lua.create_async_function(move |lua,(id,params):(String,Option<Table>)|{
        let pr=one.clone();async move {
            let args=params.map(|t|lua.from_value::<Value>(mlua::Value::Table(t))).transpose()?.unwrap_or(json!({}));
            let out=pr.call(id,args).await.map_err(mlua::Error::external)?;lua.to_value(&out)
        }})?)?;
    api.set("parallel",lua.create_async_function(move |lua,list:Table|{
        let pr=program.clone();async move{
            let mut jobs=Vec::new();
            for it in list.sequence_values::<Table>(){let item=it?;let id=item.get::<String>("id")?;let args=item.get::<Option<Table>>("args")?.map(|t|lua.from_value::<Value>(mlua::Value::Table(t))).transpose()?.unwrap_or(json!({}));jobs.push(pr.call(id,args));}
            let out=futures_util::future::try_join_all(jobs).await.map_err(mlua::Error::external)?;lua.to_value(&out)
        }})?)?;
    env.set("tools",api)?;
    env.set("wait_for",lua.create_function(|_,v:mlua::Value|Ok(v))?)?;
    let value:mlua::Value=lua.load(code).set_mode(mlua::ChunkMode::Text).set_environment(env).eval_async().await?;
    let all=app.db.list(s(&w.run,"owner")?,"tool_call",Some(s(&w.run,"id")?))?;
    let waits=objects(&all).into_iter().filter(|c|c["parent_id"]==parent && !done(c)).map(|c|c["id"].clone()).collect::<Vec<_>>();
    if !waits.is_empty(){return Ok(json!({"waiting_for":waits}))}
    Ok(lua.from_value(value)?)
}
