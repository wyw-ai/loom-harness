use super::*;
use super::delivery::deliver_in;

impl Store {
    pub fn create_dispatch(&self,owner:&str,mut body:Value)->Result<Value>{
        expect_object(&body)?;
        self.tx(|c|{
            let did=body["id"].as_str().map(str::to_owned).unwrap_or_else(||id("dispatch"));
            body["revision"]=json!(1);
            body["event_cursor"]=json!(c.query_row("SELECT coalesce(max(seq),0) FROM objects WHERE kind='event' AND owner=?",[owner],|r|r.get::<_,i64>(0))?);
            if body["enabled"].is_null(){body["enabled"]=json!(true)}
            create(c,owner,"dispatch",&did,None,body)
        })
    }
    pub fn update_dispatch(&self,owner:&str,did:&str,patch:Value)->Result<Value>{
        expect_object(&patch)?;
        self.tx(|c|{
            let mut doc=load(c,owner,"dispatch",did)?;
            if let Some(expected)=patch.get("expected_revision"){
                if *expected!=doc["revision"]{return Err(StoreError::Conflict("dispatch revision changed".into()))}
            }
            for(k,v) in patch.as_object().unwrap(){
                if !matches!(k.as_str(),"id"|"dispatch_id"|"created_at"|"owner"|"revision"|"expected_revision"|"event_cursor"){
                   doc[k]=v.clone()
                }
            }
            doc["revision"]=json!(doc["revision"].as_u64().unwrap_or(0)+1);
            if patch.get("enabled").is_some()||patch.get("status").is_some()||patch.get("trigger").is_some(){
                doc["event_cursor"]=json!(c.query_row("SELECT coalesce(max(seq),0) FROM objects WHERE kind='event' AND owner=?",[owner],|r|r.get::<_,i64>(0))?);
            }
            save(c,"dispatch",&doc)?;
            Ok(doc)
        })
    }
    /// Evaluate the caller-supplied intents against one trigger and source event.
    /// The caller must authorize the targets and compute the rule against the selected revision.
    pub fn evaluate_dispatch(&self,owner:&str,did:&str,trigger_key:&str,event_id:&str,intents:Value,preview:bool)->Result<Value>{
        self.tx(|c|{
            let dispatch=load(c,owner,"dispatch",did)?;
            if !preview{load(c,owner,"event",event_id)?;}
            let op=format!("dispatch-trigger:{did}");
            let source=json!({"source_event_id":event_id});
            if !preview {
                if let Some(k)=replay(c,owner,&op,Some(trigger_key),&source)?{
                    return load(c,owner,"evaluation",&k)
                }
            }
            if let Some(rev)=intents.get("revision"){
                if *rev!=dispatch["revision"]{return Err(StoreError::Conflict("rule revision changed".into()))}
            }
            let intended=intents.as_array().or_else(||intents["intents"].as_array());
            let error=intents.get("error");
            if intended.is_none() && error.is_none(){return Err(StoreError::Invalid("expected intents array or evaluation error".into()))}
            let paused=dispatch["enabled"]==false || matches!(dispatch["status"].as_str(),Some("paused"|"cancelled"|"disabled"));
            let status=if paused{"paused"}else if error.is_some(){"failed"}else if intended.is_none_or(Vec::is_empty){"skipped"}else{"decided"};
            let mut summary=json!({"dispatch_id":did,"revision":dispatch["revision"],"trigger_key":trigger_key,
                "source_event_id":event_id,"status":status,"error":error,"intents":intended,"deliveries":[]});
            if preview {summary["preview"]=json!(true);return Ok(summary)}
            let eid=id("eval");
            if status=="decided" {
                // Each item has an independent acknowledgement, even if another destination rejects it.
                for(i,v)in intended.unwrap().iter().enumerate(){
                    c.execute_batch("SAVEPOINT delivery_item")?;
                    let result=(|| {
                        let target_event=if let Some(msg)=v.get("message"){
                            let derived=create_event(c,owner,json!({"type":"dispatch.message","source":"loom","content":msg.get("text").unwrap_or(msg),
                               "source_event_id":event_id,"dispatch_id":did}))?;
                            field(&derived,"id")?.to_owned()
                        }else{event_id.to_owned()};
                        deliver_in(c,owner,&target_event,v.clone(),Some(&format!("{eid}:{i}")))
                    })();
                    match result {
                        Ok(d)=>{
                            c.execute_batch("RELEASE delivery_item")?;
                            summary["deliveries"].as_array_mut().unwrap().push(json!({"item":i,"id":d["id"],"status":d["status"]}));
                        }
                        Err(error @ (StoreError::Invalid(_) | StoreError::Conflict(_) | StoreError::NotFound))=>{
                            c.execute_batch("ROLLBACK TO delivery_item; RELEASE delivery_item")?;
                            summary["deliveries"].as_array_mut().unwrap().push(json!({"item":i,"status":"failed","error":error.to_string()}));
                        }
                        Err(other)=>return Err(other)
                    }
                }
            }
            let saved=create(c,owner,"evaluation",&eid,Some(did),summary)?;
            remember(c,owner,&op,Some(trigger_key),&source,&eid)?;
            Ok(saved)
        })
    }
    pub fn dispatch_deliveries(&self,owner:&str,did:&str)->Result<Value>{
        self.read(|c|{
            load(c,owner,"dispatch",did)?;
            let mut result=vec![];
            for eval in fetch_many(c,"SELECT doc FROM objects WHERE kind='evaluation' AND owner=? AND parent=? ORDER BY seq",params![owner,did])?{
                for item in eval["deliveries"].as_array().into_iter().flatten(){
                    result.push(if let Some(k)=item["id"].as_str(){load(c,owner,"delivery",k)?}else{item.clone()});
                }
            }
            Ok(Value::Array(result))
        })
    }
    pub fn active_dispatches(&self)->Result<Vec<Value>>{
        self.read(|c|fetch_many(c,"SELECT doc FROM objects WHERE kind='dispatch'
           AND json_extract(doc,'$.enabled') IS NOT 0
           AND coalesce(json_extract(doc,'$.status'),'') NOT IN('paused','disabled','cancelled') ORDER BY seq",[]))
    }
    pub fn unseen_events(&self,owner:&str,source:&str,after:i64)->Result<Vec<Value>>{
        self.read(|c|fetch_many(c,"SELECT json_set(doc,'$._sequence',seq) FROM objects
           WHERE kind='event' AND owner=? AND seq>? AND json_extract(doc,'$.source')=? ORDER BY seq LIMIT 100",
           params![owner,after,source]))
    }
    pub fn dispatch_progress(&self,owner:&str,did:&str,revision:&Value,field_name:&str,
        expected:&Value,value:&Value)->Result<bool>{
        if !matches!(field_name,"event_cursor"|"next_due"){return Err(StoreError::Invalid("invalid trigger progress".into()))}
        self.tx(|c|{
            let mut d=load(c,owner,"dispatch",did)?;
            if &d["revision"]!=revision || &d[field_name]!=expected{return Ok(false)}
            d[field_name]=value.clone();
            save(c,"dispatch",&d)?;
            Ok(true)
        })
    }
}
