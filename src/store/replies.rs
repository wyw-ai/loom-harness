use super::delivery::{clear_lease, terminal};
use super::work::run_for_lease;
use super::*;

impl Store {
    /// Persist the immutable answer and destination together before the first network attempt.
    pub fn prepare_reply(&self, work: &Work, mut body: Value) -> Result<Value> {
        expect_object(&body)?;
        if body["target"].is_null() || body.get("content").is_none() {
            return Err(StoreError::Invalid(
                "reply requires target and content".into(),
            ));
        }
        self.tx(|c| {
            let mut run = run_for_lease(c, work)?;
            let rid = field(&run, "id")?.to_owned();
            let owner = field(&run, "owner")?.to_owned();
            super::work::commit_step(c, &run)?;
            let reply_id = id("reply");
            body["run_id"] = json!(rid);
            body["session_id"] = run["session_id"].clone();
            body["status"] = json!("ready");
            body["attempt"] = json!(0);
            let reply = create(c, &owner, "reply", &reply_id, Some(&rid), body)?;
            c.execute(
                "INSERT INTO replyq(reply_id,owner) VALUES(?,?)",
                params![reply_id, owner],
            )?;
            run["status"] = json!("waiting_delivery");
            run["delivery_status"] = json!("ready");
            run["reply_id"] = json!(reply_id);
            run["version"] = json!(run["version"].as_u64().unwrap_or(0) + 1);
            clear_lease(c, &rid, false)?;
            save(c, "run", &run)?;
            emit(
                c,
                &owner,
                "run.reply_ready",
                json!({"run_id":rid,"reply_id":reply_id,"session_id":run["session_id"]}),
            )?;
            Ok(reply)
        })
    }
    pub fn claim_reply(&self, worker: &str, lease_secs: i64) -> Result<Option<Work>> {
        if worker.is_empty() || lease_secs <= 0 {
            return Err(StoreError::Invalid("invalid reply lease".into()));
        }
        self.tx(|c| {
            let record: Option<(String, String)> = c
                .query_row(
                    "SELECT reply_id,owner FROM replyq WHERE ready=1
                AND (token IS NULL OR expires<=unixepoch()) ORDER BY seq LIMIT 1",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((reply_id, owner)) = record else {
                return Ok(None);
            };
            let mut reply = load(c, &owner, "reply", &reply_id)?;
            let token = id("lease");
            c.execute(
                "UPDATE replyq SET token=?,worker=?,expires=unixepoch()+? WHERE reply_id=?",
                params![token, worker, lease_secs, reply_id],
            )?;
            reply["attempt"] = json!(reply["attempt"].as_u64().unwrap_or(0) + 1);
            reply["status"] = json!("sending");
            save(c, "reply", &reply)?;
            Ok(Some(Work { run: reply, token }))
        })
    }
    pub fn settle_reply(&self, work: &Work, status: &str, detail: Value) -> Result<Value> {
        if !matches!(status, "confirmed" | "unknown" | "failed" | "retry") {
            return Err(StoreError::Invalid("invalid reply outcome".into()));
        }
        self.tx(|c|{
            let reply_id=field(&work.run,"id")?;
            let owner=field(&work.run,"owner")?;
            let updated=c.execute("UPDATE replyq SET token=NULL,worker=NULL,expires=NULL,ready=?
               WHERE reply_id=? AND token=? AND expires>unixepoch()",params![status=="retry",reply_id,work.token])?;
            if updated==0{return Err(StoreError::StaleLease)}
            let mut reply=load(c,owner,"reply",reply_id)?;
            reply["status"]=json!(status);
            reply["outcome"]=detail;
            save(c,"reply",&reply)?;
            let rid=field(&reply,"run_id")?;
            let mut run=load(c,owner,"run",rid)?;
            run["delivery_status"]=json!(status);
            run["version"]=json!(run["version"].as_u64().unwrap_or(0)+1);
            if !terminal(&run["status"]){
                match status{
                    "confirmed"=>{
                        run["status"]=json!("completed");run["result"]=reply["content"].clone();
                        append_history(c,owner,field(&run,"session_id")?,&format!("final:{rid}"),
                            &json!({"role":"assistant","content":reply["content"],"run_id":rid,"reply_id":reply_id}))?;
                        emit(c,owner,"run.completed",json!({"run_id":rid,"reply_id":reply_id}))?;
                    }
                    "failed"=>{
                        run["status"]=json!("failed");run["error"]=reply["outcome"].clone();
                        emit(c,owner,"run.failed",json!({"run_id":rid,"reply_id":reply_id}))?;
                    }
                    _=>{}
                }
            }
            save(c,"run",&run)?;
            Ok(reply)
        })
    }
}
