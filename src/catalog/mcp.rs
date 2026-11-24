use super::resources::scoped;
use super::*;
use rusqlite::{OptionalExtension, TransactionBehavior, params};

struct Peer<'a> {
    catalog: &'a Catalog,
    connection: &'a Value,
    session: Option<String>,
    version: String,
}
impl Peer<'_> {
    async fn message(&mut self, method: &str, params: Value, request: bool) -> R<Value> {
        let mut body = json!({"jsonrpc":"2.0","method":method,"params":params});
        if request {
            body["id"] = json!(uuid::Uuid::new_v4().to_string());
        }
        let mut post = self
            .catalog
            .http
            .post(string(self.connection, "url")?)
            .header("Accept", "application/json, text/event-stream")
            .json(&body);
        if method == "tools/call"
            && let Some(k) = body
                .pointer("/params/_meta/loom.idempotency_key")
                .and_then(Value::as_str)
        {
            post = post.header("Idempotency-Key", k)
        }
        if method != "initialize" {
            post = post.header("MCP-Protocol-Version", &self.version)
        }
        if let Some(s) = &self.session {
            post = post.header("Mcp-Session-Id", s)
        }
        if let Some(reference) = self.connection["credential_ref"].as_str() {
            id(reference)?;
            let key = format!(
                "LOOM_MCP_CREDENTIAL_{}",
                reference.replace('-', "_").to_ascii_uppercase()
            );
            let secret = std::env::var(key).map_err(|_| "missing MCP credential")?;
            post = post.bearer_auth(secret);
        }
        let mut response = post
            .send()
            .await
            .map_err(|e| format!("MCP transport: {}", e.without_url()))?;
        if !response.status().is_success() {
            return Err(format!("MCP HTTP status {}", response.status()));
        }
        if method == "initialize" {
            self.session = response
                .headers()
                .get("mcp-session-id")
                .and_then(|s| s.to_str().ok())
                .map(String::from);
        }
        if !request {
            return Ok(Value::Null);
        }
        let sse = response
            .headers()
            .get("content-type")
            .and_then(|s| s.to_str().ok())
            .is_some_and(|s| s.starts_with("text/event-stream"));
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| format!("MCP response: {}", e.without_url()))?
        {
            if bytes.len() + chunk.len() > 8 * 1024 * 1024 {
                return Err("MCP response too large".into());
            }
            bytes.extend_from_slice(&chunk);
            if sse && let Some(msg) = sse_response(&bytes, &body["id"]) {
                return decode_reply(&body["id"], msg);
            }
        }
        let reply: Value = if sse {
            sse_response(&bytes, &body["id"]).ok_or("MCP missing SSE response")?
        } else {
            serde_json::from_slice(&bytes).map_err(err)?
        };
        decode_reply(&body["id"], reply)
    }
}
fn sse_response(bytes: &[u8], id: &Value) -> Option<Value> {
    let text = std::str::from_utf8(bytes).ok()?.replace("\r\n", "\n");
    for part in text.split("\n\n") {
        let payload: Vec<_> = part
            .lines()
            .filter_map(|l| l.strip_prefix("data:").map(str::trim_start))
            .collect();
        if let Ok(v) = serde_json::from_str::<Value>(&payload.join("\n"))
            && v["id"] == *id
        {
            return Some(v);
        }
    }
    None
}
fn decode_reply(id: &Value, v: Value) -> R<Value> {
    if v["jsonrpc"] != "2.0" || &v["id"] != id {
        return Err("invalid: mismatched MCP response".into());
    }
    if let Some(e) = v.get("error") {
        return Err(format!("MCP RPC error: {e}"));
    }
    v.get("result")
        .cloned()
        .ok_or("invalid: missing MCP result".into())
}
impl Catalog {
    async fn peer<'a>(&'a self, c: &'a Value) -> R<Peer<'a>> {
        let version = c["protocol_version"]
            .as_str()
            .unwrap_or("2025-11-25")
            .to_string();
        let mut p = Peer {
            catalog: self,
            connection: c,
            session: None,
            version: version.clone(),
        };
        let handshake=p.message("initialize",json!({"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"loom-harness","version":env!("CARGO_PKG_VERSION")}}),true).await?;
        if handshake.pointer("/capabilities/tools").is_none() {
            return Err("MCP tools capability missing".into());
        }
        let negotiated = string(&handshake, "protocolVersion")?;
        if !["2025-03-26", "2025-06-18", "2025-11-25", "2026-07-28"].contains(&negotiated) {
            return Err("unsupported MCP version".into());
        }
        p.version = negotiated.into();
        p.message("notifications/initialized", json!({}), false)
            .await?;
        Ok(p)
    }
    pub(super) async fn remote_call(
        &self,
        tool: &Value,
        args: &Value,
        tracking: &Value,
        authorize: impl FnOnce() -> R<()>,
    ) -> R<Value> {
        let c = self.get("connection", string(tool, "connection_key")?)?;
        if c["revision"] != tool["connection_revision"] {
            return Err("conflict: MCP connection changed".into());
        }
        let mut peer = self.peer(&c).await?;
        authorize()?;
        if !self.source_enabled(tool)?
            || self.get("tool", string(tool, "id")?)?["revision"] != tool["revision"]
        {
            return Err("conflict: MCP binding changed".into());
        }
        let mut req = json!({"name":tool["local_id"],"arguments":args});
        if tool["effectful"] == true {
            req["_meta"] = json!({"loom.tool_call_id":string(tracking,"tool_call_id")?,"loom.idempotency_key":string(tracking,"idempotency_key")?});
        }
        let result = peer.message("tools/call", req, true).await.map_err(|e| {
            if e.starts_with("MCP transport:")
                || e.starts_with("MCP response:")
                || e.starts_with("MCP HTTP status 5")
            {
                format!("external outcome unknown: {e}")
            } else {
                e
            }
        })?;
        if !result["content"].is_array() {
            return Err("invalid: MCP content".into());
        }
        if result["isError"] == true {
            return Err(format!("MCP tool error: {}", result["content"]));
        }
        Ok(result)
    }
    pub async fn recover(
        &self,
        scope: ToolScope<'_>,
        name: &str,
        args: Value,
        tracking: &Value,
    ) -> R<Value> {
        let unknown = || json!({"status":"unknown"});
        let ToolScope {
            owner,
            agent,
            directory: snapshot,
        } = scope;
        if self.authorize(owner, false, agent, name, snapshot).is_err() {
            return Ok(unknown());
        }
        let t = self.get("tool", name)?;
        if t["source"] != "mcp" || t["effectful"] != true {
            return Ok(unknown());
        }
        let rec = &t["recovery"];
        let key = string(tracking, "idempotency_key")?;
        if tracking.get("arguments") != Some(&args) {
            return Ok(unknown());
        }
        let result = if let Some(lookup) = rec["lookup_tool"].as_str() {
            let arg = if let (Some(job), Some(field)) = (
                tracking["job_id"].as_str(),
                rec["lookup_handle_arg"].as_str(),
            ) {
                Some((field, job))
            } else {
                rec["lookup_key_arg"].as_str().map(|field| (field, key))
            };
            let Some((field, value)) = arg else {
                return Ok(unknown());
            };
            let c = self.get("connection", string(&t, "connection_key")?)?;
            if c["revision"] != t["connection_revision"] {
                return Ok(unknown());
            }
            let mut peer = self.peer(&c).await?;
            let mut input = json!({});
            input[field] = json!(value);
            if self.authorize(owner, false, agent, name, snapshot).is_err() {
                return Ok(unknown());
            }
            peer.message("tools/call",json!({"name":lookup,"arguments":input,"_meta":{"loom.tool_call_id":tracking["tool_call_id"]}}),true).await?
        } else if rec["idempotent_resubmit"] == true {
            let then = tracking["dispatched_at_ms"].as_i64();
            let ttl = rec["ttl_ms"].as_i64();
            let now = chrono::Utc::now().timestamp_millis();
            if !then
                .zip(ttl)
                .is_some_and(|(x, t)| t > 0 && x <= now && now.saturating_sub(x) < t)
            {
                return Ok(unknown());
            }
            self.remote_call(&t, &args, tracking, || {
                self.authorize(owner, false, agent, name, snapshot)
                    .map(|_| ())
            })
            .await?
        } else {
            return Ok(unknown());
        };
        if result["isError"] == true {
            return Ok(unknown());
        }
        let data = &result["structuredContent"];
        if data["status"] == "completed" || data["status"] == "failed" {
            return Ok(json!({"status":"completed","result":result}));
        }
        if let Some(job) = data["job_id"].as_str() {
            return Ok(json!({"status":"waiting","job_id":job,"result":result}));
        }
        Ok(unknown())
    }
    pub async fn discover(&self, inst: &str, name: &str) -> R<Value> {
        let key = scoped(id(inst)?, id(name)?);
        let c = self.get("connection", &key)?;
        let i = self.get("plugin", inst)?;
        if c["enabled"] == false || i["enabled"] != true {
            return Err("forbidden: disabled MCP connection".into());
        }
        let generation=self.db()?.query_row("INSERT INTO catalog_refresh VALUES(?1,1) ON CONFLICT(id) DO UPDATE SET generation=generation+1 RETURNING generation",[&key],|r|r.get::<_,i64>(0)).map_err(err)?;
        let mut pages = 0;
        let result=async{
            let mut peer=self.peer(&c).await?;let mut cursor=None;let mut cursors=BTreeSet::new();let mut seen=BTreeSet::new();let mut defs=vec![];
            loop{
                let args=cursor.take().map(|s:String|json!({"cursor":s})).unwrap_or(json!({}));let page=peer.message("tools/list",args,true).await?;pages+=1;
                let list=page["tools"].as_array().ok_or("invalid: MCP list")?;
                for t in list{let n=string(t,"name")?;
                    if !seen.insert(n.to_owned()){return Err("invalid: duplicate MCP tool name".into())}
                    let import=array(&c["imports"]).find(|v|*v==n||v["name"]==n);
                    if c["imports"].is_array()&&import.is_none(){continue}
                    let direct=import.is_some_and(|v|v["exposure"]=="direct");let fq=format!("{key}.{}",n.replace('%',"%25").replace('.',"%2E"));
                    let mut d=json!({"id":fq,"description":t["description"].as_str().unwrap_or(n),"local_id":n,"direct":direct,"input_schema":t["inputSchema"],"source":"mcp","instance":inst,"instance_revision":i["revision"],"connection_key":key,"connection_revision":c["revision"],"allowed_agents":c["allowed_agents"]});
                    d["requires_approval"]=json!(false);
                    if let Some(source)=import.and_then(|v|v["receipt_source"].as_str()).or(c["receipt_source"].as_str()){
                        d["receipt_source"]=json!(id(source)?);
                    }
                    if let Some(v)=import&& v.is_object(){
                        d["effectful"]=json!(v["effectful"]==true);
                        d["requires_approval"]=json!(v["requires_approval"]==true || v["approval"]=="required");
                        if let Some(r)=v.get("recovery"){d["recovery"]=r.clone()}
                    }
                    if !d["input_schema"].is_object(){return Err("invalid: missing tool input schema".into())}
                    if let Some(s)=t.get("outputSchema"){d["output_schema"]=s.clone()}
                    defs.push(d);
                }
                let Some(next)=page["nextCursor"].as_str()else{break};if !cursors.insert(next.to_owned())||pages>1024{return Err("invalid: MCP pagination".into())}cursor=Some(next.into());
            }Ok::<_,String>(defs)
        }.await;
        let mut db = self.db()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(err)?;
        let current: i64 = tx
            .query_row(
                "SELECT generation FROM catalog_refresh WHERE id=?1",
                [&key],
                |r| r.get(0),
            )
            .map_err(err)?;
        let actual: String = tx
            .query_row(
                "SELECT rev FROM catalog_current WHERE kind='connection' AND id=?1",
                [&key],
                |r| r.get(0),
            )
            .map_err(err)?;
        if current != generation || c["revision"] != actual {
            return Err("conflict: stale MCP discovery".into());
        }
        let put = |kind: &str, key: &str, mut d: Value| -> R<Value> {
            let rev = sha(&d);
            d["revision"] = json!(rev);
            tx.execute(
                "INSERT OR IGNORE INTO catalog_versions VALUES(?1,?2,?3,?4)",
                params![kind, key, rev, d.to_string()],
            )
            .map_err(err)?;
            tx.execute(
                "INSERT OR REPLACE INTO catalog_current VALUES(?1,?2,?3)",
                params![kind, key, rev],
            )
            .map_err(err)?;
            Ok(d)
        };
        match result {
            Ok(tools) => {
                let mut members = vec![];
                for t in tools {
                    let n = string(&t, "id")?.to_owned();
                    put("tool", &n, t)?;
                    members.push(n);
                }
                let d = put(
                    "discovery",
                    &key,
                    json!({"members":members,"connection_revision":actual,"healthy":true}),
                )?;
                tx.execute(
                    "DELETE FROM catalog_diagnostics WHERE kind='discovery' AND id=?1",
                    [&key],
                )
                .map_err(err)?;
                tx.commit().map_err(err)?;
                Ok(d)
            }
            Err(e) => {
                if pages == 0 {
                    let prev:Option<String>=tx.query_row("SELECT document FROM catalog_versions JOIN catalog_current USING(kind,id,rev) WHERE kind='discovery' AND id=?1",[&key],|r|r.get(0)).optional().map_err(err)?;
                    let old: Value = prev
                        .and_then(|s| serde_json::from_str(&s).ok())
                        .unwrap_or(json!({}));
                    put(
                        "discovery",
                        &key,
                        json!({"members":old["members"],"connection_revision":actual,"healthy":false}),
                    )?;
                }
                let detail = json!({"kind":"discovery","id":key,"error":e,"completed_pages":pages});
                tx.execute(
                    "INSERT OR REPLACE INTO catalog_diagnostics VALUES('discovery',?1,?2)",
                    params![key, detail.to_string()],
                )
                .map_err(err)?;
                tx.commit().map_err(err)?;
                Err(e)
            }
        }
    }
}
