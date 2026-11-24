use super::*;
use super::delivery::{clear_lease,terminal};
use super::work::{check_wait,queue_awakened,run_for_lease};

impl Store {
    /// at is a one-based inclusive cursor in the stored session history.
    pub fn branch(&self,owner:&str,sid:&str,at:usize,body:Value)->Result<Value>{
        expect_object(&body)?;
        self.tx(|c|{
            let source=load(c,owner,"session",sid)?;
            let op=format!("branch:{sid}");
            let req=json!({"at":at,"body":body});
            let key=body["idempotency_key"].as_str();
            if let Some(child)=replay(c,owner,&op,key,&req)?{return load(c,owner,"session",&child)}
            let count:i64=c.query_row("SELECT count(*) FROM history WHERE owner=? AND session=?",params![owner,sid],|r|r.get(0))?;
            if at==0 || at>count as usize{return Err(StoreError::Invalid("branch cursor outside session history".into()))}
            let child=body["id"].as_str().map(str::to_owned).unwrap_or_else(||id("ses"));
            let mut doc=body.clone();
            doc.as_object_mut().unwrap().remove("idempotency_key");
            if doc["agent_id"].is_null(){doc["agent_id"]=source["agent_id"].clone()}
            doc["parent_session_id"]=json!(sid);
            doc["branch_at"]=json!(at);
            let session=create(c,owner,"session",&child,None,doc)?;
            c.execute("INSERT INTO history(owner,session,origin,doc)
                SELECT owner,?,origin,doc FROM history WHERE owner=? AND session=? ORDER BY seq LIMIT ?",
                params![child,owner,sid,at as i64])?;
            emit(c,owner,"branch.created",json!({"parent_session_id":sid,"session_id":child,"at":at}))?;
            remember(c,owner,&op,key,&req,&child)?;
            Ok(session)
        })
    }
    /// Reserve a remote operation before performing any network I/O.
    pub fn begin_external(&self,work:&Work,cid:&str,recovery:Value)->Result<Value>{
        expect_object(&recovery)?;
        self.tx(|c|{
            let run=run_for_lease(c,work)?;
            let owner=field(&run,"owner")?;
            let mut call=load(c,owner,"tool_call",cid)?;
            if call["run_id"]!=run["id"]{return Err(StoreError::NotFound)}
            if let Some(original)=call.get("external_dispatch"){
                if original!=&recovery{return Err(StoreError::Conflict("remote operation already reserved with different parameters".into()))}
                call["_first_dispatch"]=json!(false);
                return Ok(call)
            }
            if !matches!(call["status"].as_str(),Some("pending"|"approved"|"requested")){
                return Err(StoreError::Conflict("call not eligible for dispatch".into()))
            }
            call["external_dispatch"]=recovery;
            call["status"]=json!("dispatched");
            save(c,"tool_call",&call)?;
            call["_first_dispatch"]=json!(true);
            Ok(call)
        })
    }
    /// The HTTP adapter supplies the authenticated approver and checks its delegated scope.
    pub fn decide_approval(&self,owner:&str,cid:&str,approver:&str,allow:bool,expected_args:&Value,key:&str)->Result<Value>{
        self.tx(|c|{
            let op=format!("approval:{cid}");
            let input=json!({"approver":approver,"allow":allow,"arguments":expected_args});
            let mut call=load(c,owner,"tool_call",cid)?;
            if replay(c,owner,&op,Some(key),&input)?.is_some(){return Ok(call)}
            let mut run=load(c,owner,"run",field(&call,"run_id")?)?;
            let expected=call.get("arguments").or(call.get("args")).unwrap_or(&Value::Null);
            if expected!=expected_args || terminal(&run["status"]){return Err(StoreError::Conflict("approval no longer valid".into()))}
            if call.get("approval_decision")==Some(&input){return Ok(call)}
            if !matches!(call["status"].as_str(),Some("awaiting_approval"|"pending_approval")){
                return Err(StoreError::Conflict("tool call is not awaiting approval".into()))
            }
            call["approval_decision"]=input.clone();
            call["status"]=json!(if allow{"approved"}else{"failed"});
            if !allow{call["error"]=json!("approval denied")}
            save(c,"tool_call",&call)?;
            emit(c,owner,"tool.approval_decided",json!({"run_id":run["id"],"tool_call_id":cid,"approved":allow}))?;
            if run["status"]=="waiting" && allow{
                run["wake_reason"]=json!("approval");
                run["approved_tool_call_id"]=json!(cid);
                run["status"]=json!("queued");
                run["version"]=json!(run["version"].as_u64().unwrap_or(0)+1);
                clear_lease(c,field(&run,"id")?,true)?;
                save(c,"run",&run)?;
            }else if run["status"]=="waiting" && check_wait(c,&run)?{queue_awakened(c,&mut run)?}
            remember(c,owner,&op,Some(key),&input,cid)?;
            Ok(call)
        })
    }
}
