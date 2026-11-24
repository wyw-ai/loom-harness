//! API and background execution share the same persisted arrangements and evaluation identities.
use crate::{App,config::Identity,store::{Result,StoreError,Work}};
use chrono::{DateTime,Utc};
use serde_json::{json,Value};
use std::{str::FromStr,time::Duration};

fn invalid(msg:impl Into<String>)->StoreError{StoreError::Invalid(msg.into())}
fn next_due(trigger:&Value,after:i64)->Result<Value>{
    if trigger["type"]!="schedule"{return Ok(Value::Null)}
    if let Some(period)=trigger["interval_seconds"].as_i64(){
        if period<=0{return Err(invalid("interval_seconds must be positive"))}
        return Ok(json!(after.checked_add(period).ok_or_else(||invalid("interval overflow"))?))
    }
    if let Some(at)=trigger.get("at"){
        let seconds=at.as_i64().or_else(||at.as_str()?.parse::<DateTime<Utc>>().ok().map(|t|t.timestamp()))
            .ok_or_else(||invalid("at must be epoch seconds or an RFC3339 timestamp"))?;
        return Ok(if seconds>after{json!(seconds)}else{Value::Null})
    }
    let expr=trigger["cron"].as_str().ok_or_else(||invalid("missing cron or at"))?;
    let tz=chrono_tz::Tz::from_str(trigger["timezone"].as_str().unwrap_or("UTC")).map_err(|_|invalid("unknown schedule timezone"))?;
    let parts:Vec<_>=expr.split_whitespace().collect();
    let scheduled=if parts.len()==5{
        // POSIX weekdays are 0=Sun..6=Sat (also 7=Sun). The parser uses 1=Sun..7=Sat.
        let mut dow=String::new();
        let mut digits=String::new();
        let mut step=false;
        for ch in parts[4].chars().chain(std::iter::once(' ')){
            if ch.is_ascii_digit(){digits.push(ch);continue}
            if !digits.is_empty(){
                let n=digits.parse::<u32>().map_err(|_|invalid("invalid cron weekday"))?;
                if !step && n>7{return Err(invalid("invalid cron weekday"))}
                dow.push_str(&(if step{n}else{n%7+1}).to_string());
                digits.clear();
            }
            if ch!=' '{dow.push(ch)}
            step=ch=='/';
        }
        format!("0 {} {} {} {} {}",parts[0],parts[1],parts[2],parts[3],dow)
    }else{expr.to_owned()};
    let sched=cron::Schedule::from_str(&scheduled).map_err(|e|invalid(e.to_string()))?;
    let date=DateTime::from_timestamp(after,0).ok_or_else(||invalid("invalid schedule time"))?.with_timezone(&tz);
    Ok(sched.after(&date).next().map(|v|json!(v.timestamp())).unwrap_or(Value::Null))
}
fn valid_definition(doc:&Value)->Result<()>{
    let trigger=&doc["trigger"];
    match trigger["type"].as_str(){
        Some("source"|"event")=>{if trigger["source"].as_str().or(trigger["source_id"].as_str()).is_none(){
            return Err(invalid("source trigger requires an explicit source ID"))
        }}
        Some("schedule")=>{next_due(trigger,Utc::now().timestamp())?;}
        Some("direct"|"immediate")=>{}
        _=>return Err(invalid("unsupported dispatch trigger"))
    }
    if !doc["rule"].is_object(){return Err(invalid("dispatch requires a rule"))}
    Ok(())
}
fn rule_output(rule:&Value,event:&Value)->Result<Vec<Value>>{
    if rule["type"]=="lua"{
        let code=rule["code"].as_str().or(rule["source"].as_str()).ok_or_else(||invalid("Lua rule requires inline source"))?;
        let result=crate::catalog::evaluate_dispatch_rule(code,&json!({"event":event,"params":rule["params"]})).map_err(invalid)?;
        if result.as_object().is_some_and(|o|o.is_empty()){return Ok(vec![])}
        return result.as_array().cloned().ok_or_else(||invalid("decide must return a list of intents"))
    }
    if let Some(cond)=rule.get("condition"){
        let path=cond["field"].as_str().ok_or_else(||invalid("condition requires field"))?;
        let actual=path.split('.').try_fold(event,|cur,k|cur.get(k));
        if actual!=cond.get("equals"){return Ok(vec![])}
    }
    if let Some(intents)=rule.get("intents"){return intents.as_array().cloned().ok_or_else(||invalid("intents must be a list"))}
    Ok(vec![rule.clone()])
}
fn evaluate(app:&App,d:&Value,event:&Value,trigger:&str,preview:bool)->Result<Value>{
    let owner=d["owner"].as_str().ok_or_else(||invalid("dispatch has no owner"))?;
    let mut intents=rule_output(&d["rule"],event);
    if let Ok(items)=&mut intents{
        for intent in items {
            let checked=(||->Result<()> {
                let obj=intent.as_object_mut().ok_or_else(||invalid("invalid intent"))?;
                obj.remove("agent_snapshot");
                if let Some(e)=intent["event_id"].as_str(){if Some(e)!=event["id"].as_str(){return Err(invalid("rule must reference its authorized source event"))}}
                let agent=if let Some(sid)=intent["target"]["session_id"].as_str(){
                    let s=app.db.get(owner,"session",sid)?;
                    s["agent_id"].as_str().map(str::to_owned)
                }else{intent["target"]["agent_id"].as_str().map(str::to_owned)};
                if let Some(id)=agent{
                    let snapshot=app.catalog.get_agent(owner,false,&id).map_err(invalid)?;
                    if intent["action"]=="start_run"{intent["agent_snapshot"]=snapshot;}
                }else if intent["target"]["session_id"].is_null(){return Err(invalid("missing target"))}
                Ok(())
            })();
            if let Err(e)=checked{intents=Err(e);break}
        }
    }
    let payload=match intents { Ok(i)=>json!({"revision":d["revision"],"intents":i}),
        Err(e)=>json!({"revision":d["revision"],"error":e.to_string()})};
    let eval=app.db.evaluate_dispatch(owner,d["id"].as_str().unwrap(),trigger,event["id"].as_str().unwrap_or("preview"),payload,preview)?;
    app.ready.notify_waiters();
    Ok(eval)
}
pub async fn routes(app:&App,who:&Identity,method:&str,p:&[&str],mut body:Value,key:Option<&str>)->Result<Option<Value>>{
    let owner=&who.owner;
    let db=&app.db;
    let v=match(method,p){
        ("GET",["dispatches"])=>db.list(owner,"dispatch",None)?,
        ("GET",["dispatches",did])=>db.get(owner,"dispatch",did)?,
        ("GET",["dispatches",did,"deliveries"])=>db.dispatch_deliveries(owner,did)?,
        ("GET",["dispatches",did,"evaluations"])=>{db.get(owner,"dispatch",did)?;db.list(owner,"evaluation",Some(did))?}
        ("POST",["dispatches"])=>{
            valid_definition(&body)?;
            body["next_due"]=next_due(&body["trigger"],Utc::now().timestamp())?;
            let d=db.create_dispatch(owner,body)?;
            if matches!(d["trigger"]["type"].as_str(),Some("direct"|"immediate")){
                let event=if let Some(eid)=d["trigger"]["event_id"].as_str(){db.get(owner,"event",eid)?}else{
                    db.post_event(owner,json!({"type":"dispatch.triggered","source":"loom","dispatch_id":d["id"]}),Some(&format!("direct:{}",d["id"])))?};
                evaluate(app,&d,&event,key.unwrap_or("direct"),false)?;
            }
            d
        }
        ("PATCH",["dispatches",did])=>{
            let mut checked=db.get(owner,"dispatch",did)?;
            for(k,v)in body.as_object().ok_or_else(||invalid("invalid dispatch update"))?{checked[k]=v.clone()}
            valid_definition(&checked)?;
            if body.get("enabled").is_some()||body.get("trigger").is_some()||body.get("status").is_some(){
                body["next_due"]=next_due(&checked["trigger"],Utc::now().timestamp())?;
            }else{body.as_object_mut().unwrap().remove("next_due");}
            db.update_dispatch(owner,did,body)?
        }
        ("POST",["dispatches",did,"previews"])=>{
            let mut d=db.get(owner,"dispatch",did)?;
            if let Some(rule)=body.get("rule"){d["rule"]=rule.clone()}
            let sample=body.get("event").or(body.get("input")).unwrap_or(&body);
            evaluate(app,&d,sample,"preview",true)?
        }
        ("GET",["replies",id])=>db.get(owner,"reply",id)?,
        _=>return Ok(None)
    };
    Ok(Some(v))
}
fn process_dispatch(app:&App,d:&Value)->Result<()> {
    let owner=d["owner"].as_str().unwrap();
    let did=d["id"].as_str().unwrap();
    let trigger=&d["trigger"];
    match trigger["type"].as_str(){
        Some("source"|"event")=>{
            let source=trigger["source"].as_str().or(trigger["source_id"].as_str()).unwrap_or("");
            let mut cursor=d["event_cursor"].clone();
            for e in app.db.unseen_events(owner,source,cursor.as_i64().unwrap_or(0))?{
                let event_type=trigger["event_type"].as_str();
                if event_type.is_none()||event_type==e["type"].as_str(){
                    evaluate(app,d,&e,e["id"].as_str().unwrap(),false)?;
                }
                if !app.db.dispatch_progress(owner,did,&d["revision"],"event_cursor",&cursor,&e["_sequence"])?{break}
                cursor=e["_sequence"].clone();
            }
        }
        Some("schedule")=>{
            if let Some(due)=d["next_due"].as_i64().filter(|t|*t<=Utc::now().timestamp()){
                let tick=format!("tick:{did}:{due}");
                let event=app.db.post_event(owner,json!({"type":"dispatch.triggered","source":"loom","dispatch_id":did,"due":due}),Some(&tick))?;
                evaluate(app,d,&event,&tick,false)?;
                app.db.dispatch_progress(owner,did,&d["revision"],"next_due",&d["next_due"],&next_due(trigger,due)?)?;
            }
        }
        _=>{}
    }
    Ok(())
}

pub fn allowed_reply_target(app:&App,url:&str)->bool{
    app.cfg.notification_targets.iter().any(|allowed|allowed==url)
        && reqwest::Url::parse(url).is_ok_and(|parsed|matches!(parsed.scheme(),"https"|"http")&&parsed.host().is_some())
}
async fn send_reply(app:&App,work:&Work)->Result<()>{
    let reply=&work.run;
    let target=reply["target"].as_str().ok_or_else(||invalid("invalid reply destination"))?;
    let id=reply["id"].as_str().unwrap();
    let capability=reply["capability"].as_str().unwrap_or("unknown");
    if !allowed_reply_target(app,target){app.db.settle_reply(work,"failed",json!("destination is not allowed"))?;return Ok(())}
    let client=reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(5))
        .build().map_err(|e|invalid(e.to_string()))?;
    let attempt=reply["attempt"].as_u64().unwrap_or(1);
    if attempt>1 && capability=="query_by_key" {
        match client.get(target).query(&[("delivery_id",id)]).send().await {
            Ok(response) if response.status().is_success()=>{
                let value=response.json::<Value>().await.unwrap_or(Value::Null);
                if value["confirmed"]==true{app.db.settle_reply(work,"confirmed",value)?;return Ok(())}
                app.db.settle_reply(work,"unknown",json!("recipient could not confirm delivery"))?;
                return Ok(())
            }
            Ok(r) if r.status()==reqwest::StatusCode::NOT_FOUND=>{
                // Only an authoritative negative lookup authorizes a fresh delivery.
            }
            _=>{app.db.settle_reply(work,"unknown",json!("recipient lookup unavailable"))?;return Ok(())}
        }
    }else if attempt>1 && capability!="idempotent"{
        app.db.settle_reply(work,"unknown",json!("prior delivery may have reached recipient"))?;
        return Ok(())
    }
    let sent=client.post(target).header("Idempotency-Key",id).json(&json!({"delivery_id":id,"content":reply["content"]})).send().await;
    let outcome=match sent{
        Ok(r) if r.status().is_success()=>("confirmed",json!({"http_status":r.status().as_u16()})),
        Ok(r) if r.status().is_client_error()=>("failed",json!({"http_status":r.status().as_u16()})),
        _ if matches!(capability,"idempotent"|"query_by_key") && attempt<3=>("retry",json!("receiver acknowledgement unavailable")),
        _=>("unknown",json!("receiver acknowledgement unavailable"))
    };
    app.db.settle_reply(work,outcome.0,outcome.1)?;
    Ok(())
}
pub async fn pump(app:App){
    let worker=format!("dispatch-{}",uuid::Uuid::new_v4());
    loop{
        if let Ok(active)=app.db.active_dispatches(){for d in active{
            if let Err(e)=process_dispatch(&app,&d){eprintln!("dispatch {}: {e}",d["id"])}
        }}
        if let Ok(Some(reply))=app.db.claim_reply(&worker,30){
            if let Err(e)=send_reply(&app,&reply).await{eprintln!("reply {}: {e}",reply.run["id"])}
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
