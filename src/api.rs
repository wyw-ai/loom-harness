use crate::{App,config::Identity,context,store::StoreError};
use axum::{Json,Router,body::Bytes,extract::{DefaultBodyLimit, OriginalUri, State},http::{HeaderMap,Method,StatusCode},response::{IntoResponse,Response}};
use serde_json::{Value,json};
use std::collections::HashMap;

type ApiResult = Result<Value,ApiError>;
struct ApiError(StatusCode,String);
impl IntoResponse for ApiError {
    fn into_response(self)->Response {(self.0,Json(json!({"error":self.1}))).into_response()}
}
impl From<StoreError> for ApiError {
    fn from(e:StoreError)->Self {
        let status=match e {StoreError::NotFound=>StatusCode::NOT_FOUND,StoreError::Conflict(_)|StoreError::StaleLease=>StatusCode::CONFLICT,StoreError::Invalid(_)=>StatusCode::BAD_REQUEST,StoreError::Storage(_)=>StatusCode::INTERNAL_SERVER_ERROR};
        Self(status,e.to_string())
    }
}
fn bad(msg:impl Into<String>)->ApiError{ApiError(StatusCode::BAD_REQUEST,msg.into())}
fn deny()->ApiError{ApiError(StatusCode::FORBIDDEN,"permission denied".into())}
fn catalog_err(s:String)->ApiError{
    let v=s.to_lowercase();
    let code=if v.contains("forbidden")||v.contains("permission")||v.contains("authorized"){StatusCode::FORBIDDEN}else if v.contains("not found")||v.contains("not_found"){StatusCode::NOT_FOUND}else if v.contains("conflict")||v.contains("already exists"){StatusCode::CONFLICT}else{StatusCode::BAD_REQUEST};
    ApiError(code,s)
}
pub fn router(app:App)->Router{
    Router::new().fallback(handle).layer(DefaultBodyLimit::max(12*1024*1024)).with_state(app)
}
async fn handle(State(app):State<App>,OriginalUri(uri):OriginalUri,method:Method,headers:HeaderMap,bytes:Bytes)->Result<Response,ApiError>{
    let path=uri.path();
    if path=="/health" && method==Method::GET{return Ok(Json(json!({"status":"ok"})).into_response())}
    let token=headers.get("authorization").and_then(|h|h.to_str().ok()).and_then(|h|h.strip_prefix("Bearer "));
    let who=token.and_then(|s|app.cfg.tokens.get(s)).ok_or_else(||ApiError(StatusCode::UNAUTHORIZED,"bearer credential required".into()))?;
    let body=if bytes.is_empty(){json!({})}else{serde_json::from_slice::<Value>(&bytes).map_err(|_|bad("invalid JSON body"))?};
    if !body.is_object(){return Err(bad("expected JSON object"))}
    let path=path.strip_prefix("/api/").ok_or(ApiError(StatusCode::NOT_FOUND,"unknown route".into()))?;
    let parts=path.trim_end_matches('/').split('/').collect::<Vec<_>>();
    let q:HashMap<String,String>=serde_urlencoded::from_str(uri.query().unwrap_or("")).map_err(|_|bad("invalid query parameters"))?;
    let key=headers.get("idempotency-key").and_then(|h|h.to_str().ok());
    let response=route(&app,who,method.as_str(),&parts,&q,key,body).await?;
    let status=if method==Method::POST && matches!(parts.as_slice(),["events",_,"deliveries"]){StatusCode::ACCEPTED}else{StatusCode::OK};
    Ok((status,Json(response)).into_response())
}
async fn route(app:&App,who:&Identity,method:&str,p:&[&str],q:&HashMap<String,String>,key:Option<&str>,mut body:Value)->ApiResult{
    let o=who.owner.as_str();
    let db=&app.db;
    if matches!(p.first(),Some(&"dispatches")|Some(&"event-sources")|Some(&"event-source-types")) {
        if let Some(v)=crate::dispatch::routes(app,who,method,p,body.clone(),key).await?{return Ok(v)}
    }
    match (method,p){
        ("GET",["plugins",instance,"subscriptions",sub,"events",event])=>{
            db.get(o,"event",event)?;
            let group=crate::bus::subscription_group(o,instance,sub);
            Ok(db.subscription_result(&group,event)?.unwrap_or(Value::Null))
        }
        ("GET",["models"])=>Ok(json!(app.cfg.models.iter().map(|(k,v)|json!({"id":k,"model":v.model,"images":v.images})).collect::<Vec<_>>())),
        ("GET",["agent-templates"]|["templates"])=>app.catalog.templates().map_err(catalog_err),
        ("GET",["agents"])=>app.catalog.list_agents(o,who.admin).map_err(catalog_err),
        ("POST",["agents"])=>app.catalog.create_agent(o,who.admin,body).map_err(catalog_err),
        ("GET",["agents",id])=>app.catalog.get_agent(o,who.admin,id).map_err(catalog_err),
        ("PATCH",["agents",id])=>app.catalog.patch_agent(o,who.admin,id,body).map_err(catalog_err),
        ("GET",["agents",id,"tools"])=>app.catalog.tools(o,who.admin,id).map_err(catalog_err),
        ("POST",["sessions"])=>{
            if let Some(a)=body["agent_id"].as_str(){app.catalog.get_agent(o,false,a).map_err(catalog_err)?;}
            Ok(db.create_session(o,body)?)
        }
        ("POST",["events"])=>{
            if body.get("type").and_then(Value::as_str).is_some_and(|t|t.split('.').next().is_some_and(|s|matches!(s,"run"|"tool"|"model"|"delivery"|"goal"|"approval"))) || body["source"]=="loom"{return Err(bad("reserved internal event"))}
            if !who.source && !who.admin {if body["source"].as_str().is_some_and(|s|s!="user"){return Err(deny())}body["source"]=json!("user");}
            Ok(db.post_event(o,body,key)?)
        }
        ("POST",["events",eid,"deliveries"])=>{
            body.as_object_mut().unwrap().remove("agent_snapshot");
            let session=body["target"]["session_id"].as_str().map(|s|db.get(o,"session",s)).transpose()?;
            let target=body["target"]["agent_id"].as_str().or_else(|| session.as_ref()?.get("agent_id")?.as_str()).or(body["agent_id"].as_str());
            if let Some(id)=target {let agent=app.catalog.get_agent(o,false,id).map_err(catalog_err)?; if body["action"]=="start_run"{body["agent_snapshot"]=agent;}}
            if !body["goal"].is_null() && body["goal"]["objective"].as_str().filter(|s|!s.trim().is_empty()).is_none(){return Err(bad("goal requires an objective"))}
            if let Some(url)=body["delivery_contract"]["target"].as_str(){if !app.cfg.notification_targets.iter().any(|t|t==url){return Err(deny())}}
            let result=db.deliver(o,eid,body,key)?;app.ready.notify_one();Ok(result)
        }
        ("GET",["events"]|["runs"]|["sessions"])=>{let mut v=db.list(o,p[0],None)?;if let Some(items)=v.as_array_mut(){items.retain(|i|["type","source","run_id","session_id"].iter().all(|k|q.get(*k).is_none_or(|expected|i[*k].as_str()==Some(expected.as_str()))));}Ok(v)}
        ("GET",["events",id]|["runs",id]|["sessions",id]|["model-calls",id]|["tool-calls",id]|["event-deliveries",id])=>{
            let mut item=db.get(o,p[0],id)?;
            if p[0]=="sessions"{item["version"]=json!(db.history(o,id)?.as_array().map(Vec::len));}
            Ok(item)
        }
        ("GET",["sessions",s,"runs"])=>{db.get(o,"session",s)?;Ok(db.list(o,"run",Some(s))?)}
        ("GET",["sessions",s,"context","items"])=>Ok(db.history(o,s)?),
        ("POST",["sessions",s,"branches"])=>{
            let at=body["at"].as_u64().or_else(||body["at"].as_str()?.parse().ok()).ok_or_else(||bad("at must be a history cursor"))?;
            Ok(db.branch(o,s,at as usize,body)?)
        }
        ("POST",["sessions",s,"context","previews"])=>{
            let session=db.get(o,"session",s)?;
            let agent=app.catalog.get_agent(o,false,session["agent_id"].as_str().unwrap_or("")).map_err(catalog_err)?;
            Ok(json!({"messages":context::build_messages(agent["system_prompt"].as_str().unwrap_or(""),&[], &crate::engine::chat_history(&db.history(o,s)?),None,false)}))
        }
        ("GET",["runs",r,kind @ ("model-calls"|"tool-calls")])=>{db.get(o,"run",r)?;Ok(db.list(o,kind,Some(r))?)}
        ("GET",["runs",r,"tools"])=>{let run=db.get(o,"run",r)?;app.catalog.tools(o,false,run["agent_id"].as_str().unwrap_or("")).map_err(catalog_err)}
        ("GET",["events",e,"deliveries"])=>{db.get(o,"event",e)?;Ok(db.list(o,"delivery",Some(e))?)}
        ("POST",["tool-calls",cid,"results"])=>{
            if !who.source {return Err(deny())}
            let key=key.ok_or_else(||bad("tool receipt requires Idempotency-Key"))?;
            let call=db.get(o,"tool_call",cid)?;
            if let Some(text)=body["result"].as_str().filter(|t|t.len()>app.cfg.note_threshold){
                body["result"]=crate::engine::externalize(app,&call,text).map_err(|e|bad(e.to_string()))?;
            }
            let v=db.receive_tool_result(o,cid,body,key)?;app.ready.notify_one();Ok(v)
        }
        ("POST",["tool-calls",cid,"approval"])=>{
            if !who.approve{return Err(deny())}
            let allow=body["allow"].as_bool().ok_or_else(||bad("allow must be boolean"))?;
            let v=db.decide_approval(o,cid,o,allow,&body["arguments"],key.ok_or_else(||bad("approval requires Idempotency-Key"))?)?;
            app.ready.notify_one();Ok(v)
        }
        ("GET",["tool-calls",cid,"results",part,"content"])=>{
            if *part!="0"{return Err(bad("unknown result part"))}
            let call=db.get(o,"tool_call",cid)?;
            let data=&call["result"];
            if let Some(reference)=data["note_ref"].as_str(){
                let read=app.notes.read(call["session_id"].as_str().unwrap_or(""),reference,None,None,usize::MAX).map_err(|e|bad(e.to_string()))?;
                return Ok(json!(read))
            }
            Ok(data.clone())
        }
        ("POST",["sessions",s,"suggestions"])=>suggestion(app,o,s,&body).await,
        _=>app.catalog.manage(o,who.admin,method,&format!("/api/{}",p.join("/")),body).await.map_err(catalog_err),
    }
}
async fn suggestion(app:&App,owner:&str,s:&str,body:&Value)->ApiResult{
    let history=app.db.history(owner,s)?;
    let items=history.as_array().ok_or_else(||bad("invalid session history"))?;
    let version=items.len();
    if body["expected_version"]!=json!(version)||!items.last().is_some_and(|m|m["role"]=="assistant" && !body["after_reply_id"].is_null() && m["run_id"]==body["after_reply_id"]){
        return Ok(json!({"based_on_version":version,"suggestion":null}))
    }
    let model=app.cfg.default_model.as_ref().and_then(|k|app.cfg.models.get(k)).ok_or_else(||bad("no available suggestion model"))?;
    let visible=Value::Array(items.iter().filter(|m|m["event"]["visibility"]!="agent" && m["role"]!="tool").cloned().collect());
    let guess=context::suggest(&app.http,&model.route(),&crate::engine::chat_history(&visible)).await.ok().flatten();
    let fresh=app.db.history(owner,s)?.as_array().is_some_and(|a|a.len()==version);
    Ok(json!({"based_on_version":version,"suggestion":guess.filter(|_|fresh)}))
}
