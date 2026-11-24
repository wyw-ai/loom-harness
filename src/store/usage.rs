use super::*;

#[derive(Default)]
struct Totals {
    attempts: u64,
    reported: u64,
    input: Option<u64>,
    output: Option<u64>,
    total: Option<u64>,
    hit: Option<u64>,
    miss: Option<u64>,
    cache_samples: u64,
    cached_input: u64,
    sampled_input: u64,
    speed_samples: u64,
    timed_output: u64,
    model_micros: u64,
    decode_samples: u64,
    decode_output: u64,
    decode_micros: u64,
}

fn add(sum: &mut Option<u64>, value: Option<u64>) {
    if let Some(n) = value {
        *sum = Some(sum.unwrap_or(0).saturating_add(n));
    }
}

impl Totals {
    fn observe(&mut self, call: &Value, attempts: u64) {
        self.attempts += attempts.max(1);
        let usage = &call["usage"];
        let input = usage["prompt_tokens"].as_u64();
        let output = usage["completion_tokens"].as_u64();
        let total = usage["total_tokens"]
            .as_u64()
            .or_else(|| input?.checked_add(output?));
        add(&mut self.input, input);
        add(&mut self.output, output);
        add(&mut self.total, total);
        self.reported += u64::from(total.is_some());

        let hit = usage["prompt_cache_hit_tokens"]
            .as_u64()
            .or_else(|| usage["prompt_tokens_details"]["cached_tokens"].as_u64());
        let miss = usage["prompt_cache_miss_tokens"].as_u64();
        let hit = hit.or_else(|| input?.checked_sub(miss?));
        let miss = miss.or_else(|| input?.checked_sub(hit?));
        add(&mut self.hit, hit);
        add(&mut self.miss, miss);
        if let (Some(hits), Some(misses)) = (hit, miss) {
            self.cached_input = self.cached_input.saturating_add(hits);
            self.sampled_input = self
                .sampled_input
                .saturating_add(hits.saturating_add(misses));
            self.cache_samples += 1;
        }

        let timing = &call["timing"];
        if let (Some(tokens), Some(first), Some(end)) = (
            output,
            timing["first_output_us"].as_i64(),
            timing["generation_ended_us"].as_i64(),
        ) && let Some(duration) = end.checked_sub(first)
            && duration > 0
        {
            self.decode_samples += 1;
            self.decode_output = self.decode_output.saturating_add(tokens);
            self.decode_micros = self.decode_micros.saturating_add(duration as u64);
        }

        if let (Some(tokens), Some(start), Some(end)) = (
            output,
            timing["request_started_us"].as_i64(),
            timing["response_completed_us"].as_i64(),
        ) && let Some(duration) = end.checked_sub(start)
            && duration > 0
        {
            self.speed_samples += 1;
            self.timed_output = self.timed_output.saturating_add(tokens);
            self.model_micros = self.model_micros.saturating_add(duration as u64);
        }
    }

    fn tokens(&self) -> Value {
        if self.attempts == 0 {
            return json!({"input":0,"output":0,"total":0});
        }
        json!({"input":self.input,"output":self.output,"total":self.total})
    }

    fn calls(&self) -> Value {
        json!({"total":self.attempts,"reported":self.reported})
    }

    fn speed(&self) -> Value {
        let rate = (self.model_micros > 0)
            .then(|| self.timed_output as f64 * 1_000_000.0 / self.model_micros as f64);
        let decode = (self.decode_micros > 0)
            .then(|| self.decode_output as f64 * 1_000_000.0 / self.decode_micros as f64);
        json!({"output_tokens_per_second":rate,"sampled_calls":self.speed_samples,
            "decode_tokens_per_second":decode,"decode_sampled_calls":self.decode_samples})
    }

    fn cache(&self) -> Value {
        let rate =
            (self.sampled_input > 0).then(|| self.cached_input as f64 / self.sampled_input as f64);
        json!({"hit_tokens":self.hit,"miss_tokens":self.miss,"hit_rate":rate,"sampled_calls":self.cache_samples})
    }
}

fn run_elapsed(
    c: &Connection,
    owner: &str,
    run: &Value,
    as_of_us: i64,
) -> Result<(Option<i64>, u64)> {
    let admitted = run["admitted_us"].as_i64();
    let start = admitted.or_else(|| run["created_at"].as_i64()?.checked_mul(1_000_000));
    let mut precision = if admitted.is_some() { 1 } else { 1000 };
    let end = if !super::delivery::terminal(&run["status"]) {
        Some(as_of_us)
    } else if let Some(end) = run["finished_us"].as_i64() {
        Some(end)
    } else {
        // Regular local completions commit their final step and terminal Run in one transaction.
        // A user cancellation or external delivery can occur well after the last step.
        let step_ms: Option<i64> = if run["status"] != "cancelled" && run["reply_id"].is_null() {
            c.query_row(
                "SELECT max(committed_ms) FROM run_steps WHERE run_id=?1",
                [field(run, "id")?],
                |r| r.get(0),
            )?
        } else {
            None
        };
        if let Some(ms) = step_ms {
            ms.checked_mul(1000)
        } else {
            let sec: Option<i64> = c.query_row(
                "SELECT max(json_extract(doc,'$.created_at')) FROM objects WHERE owner=?1 AND kind='event'
                 AND json_extract(doc,'$.source')='loom' AND json_extract(doc,'$.type') IN ('run.completed','run.failed','run.cancelled')
                 AND json_extract(doc,'$.run_id')=?2 AND json_extract(doc,'$.type')=?3",
                params![owner,field(run,"id")?,format!("run.{}",field(run,"status")?)],|r|r.get(0))?;
            precision = 1000;
            sec.and_then(|s| s.checked_mul(1_000_000))
        }
    };
    let elapsed = start.zip(end).map(|(s, e)| {
        if precision == 1000 {
            (e / 1_000_000).saturating_sub(s / 1_000_000).max(0) * 1000
        } else {
            e.saturating_sub(s).max(0) / 1000
        }
    });
    Ok((elapsed, precision))
}

struct Report {
    runs: Vec<Value>,
    totals: Totals,
    context_run: Option<Value>,
    context_call: Option<Value>,
    compactions: u64,
}

fn report(c: &Connection, owner: &str, id: &str, session_scope: bool) -> Result<Report> {
    let scope = if session_scope { "parent" } else { "id" };
    let runs = fetch_many(c,&format!("SELECT json_object('id',id,'agent_id',doc->>'agent_id','status',doc->>'status',
                'agent_snapshot',json_object('context_bytes',doc->>'$.agent_snapshot.context_bytes'),
                'admitted_us',doc->>'admitted_us','created_at',doc->>'created_at','finished_us',doc->>'finished_us',
                'reply_id',doc->>'reply_id') FROM objects WHERE kind='run' AND owner=?1 AND {scope}=?2 ORDER BY seq"),
                params![owner,id])?;
    let mut report = Report {
        runs: Vec::with_capacity(runs.len()),
        totals: Totals::default(),
        context_run: runs.last().cloned(),
        context_call: None,
        compactions: 0,
    };
    let as_of = chrono::Utc::now().timestamp_micros();
    // Never copy stored message lists or tool payloads into a statistics response.
    let mut calls = c.prepare_cached(
        "SELECT seq,json_object('id',id,'run_id',parent,'model',doc->>'model','reason',doc->>'reason',
          'status',doc->>'status','usage',doc->'usage','timing',doc->'timing','ctx_snapshot',doc->'ctx_snapshot',
          'compressed_ranges',doc->'compressed_ranges','compaction_revision',doc->>'compaction_revision',
          'preflight_only',doc->'preflight_only'),
          max(coalesce(doc->>'provider_attempts',0),
              (SELECT count(*) FROM run_stream WHERE model_call_id=objects.id AND event='attempt_started'))
          FROM objects WHERE owner=?1 AND kind IN ('model_call','compaction_call') AND parent=?2 ORDER BY seq")?;
    let mut latest_seq = 0;
    for run in runs {
        let run_id = field(&run, "id")?;
        let mut rows = calls.query(params![owner, run_id])?;
        let mut totals = Totals::default();
        while let Some(row) = rows.next()? {
            let seq: i64 = row.get(0)?;
            let call: Value = serde_json::from_str(&row.get::<_, String>(1)?)?;
            let attempts = row.get(2)?;
            if call["preflight_only"] != true {
                report.totals.observe(&call, attempts);
                totals.observe(&call, attempts);
            }
            let has_legacy_summary = call["ctx_snapshot"].is_null()
                && call["compressed_ranges"]
                    .as_array()
                    .is_some_and(|r| !r.is_empty());
            if has_legacy_summary {
                // The old worker actually issued a separate summary request, but did not
                // persist its provider receipt. Its cost is unknown, never counted as zero.
                report.totals.attempts += 1;
                totals.attempts += 1;
            }
            if (call["reason"] == "compaction" && call["status"] == "completed")
                || has_legacy_summary
            {
                report.compactions += 1;
            }
            if session_scope && seq > latest_seq {
                latest_seq = seq;
                report.context_call = Some(call);
                report.context_run = Some(run.clone());
            }
        }
        let (elapsed, precision) = run_elapsed(c, owner, &run, as_of)?;
        report.runs.push(json!({"run_id":run_id,"status":run["status"],"elapsed_ms":elapsed,"elapsed_precision_ms":precision,
            "tokens":totals.tokens(),"calls":totals.calls()}));
    }
    if let Some(call) = &mut report.context_call
        && call["ctx_snapshot"].is_null()
        && call["reason"] != "compaction"
    {
        // Only legacy recent calls lack a byte snapshot. Reuse the compactor's size rules
        // on this one saved request, with no material from other calls or other tenants.
        let messages: Option<String> = c.query_row(
            "SELECT doc->'messages' FROM objects WHERE owner=?1 AND kind='model_call' AND id=?2",
            params![owner, field(call, "id")?],
            |r| r.get(0),
        )?;
        if let Some(raw) = messages {
            call["messages"] = serde_json::from_str(&raw)?;
        }
    }
    Ok(report)
}

impl Store {
    pub fn start_suggestion_usage(&self, owner: &str, session_id: &str) -> Result<String> {
        self.tx(|c| {
            load(c, owner, "session", session_id)?;
            let key = id("sample");
            create(
                c,
                owner,
                "suggestion_usage",
                &key,
                Some(session_id),
                json!({"status":"pending"}),
            )?;
            Ok(key)
        })
    }

    pub fn finish_suggestion_usage(
        &self,
        owner: &str,
        id: &str,
        usage: Value,
        timing: Value,
    ) -> Result<()> {
        // A read-only suggestion can sample private material. Only numeric metering fields
        // are retained, even if a compatible provider adds unrelated response metadata.
        let counts = json!({
            "prompt_tokens":usage["prompt_tokens"].as_u64(),
            "completion_tokens":usage["completion_tokens"].as_u64(),
            "total_tokens":usage["total_tokens"].as_u64(),
            "prompt_cache_hit_tokens":usage["prompt_cache_hit_tokens"].as_u64(),
            "prompt_cache_miss_tokens":usage["prompt_cache_miss_tokens"].as_u64(),
            "prompt_tokens_details":{"cached_tokens":usage["prompt_tokens_details"]["cached_tokens"].as_u64()}
        });
        let times = json!({
            "request_started_us":timing["request_started_us"].as_i64(),
            "response_completed_us":timing["response_completed_us"].as_i64(),
            "first_output_us":timing["first_output_us"].as_i64(),"generation_ended_us":timing["generation_ended_us"].as_i64()
        });
        self.tx(|c| {
            let mut sample = load(c, owner, "suggestion_usage", id)?;
            if sample["status"] == "settled" {
                if sample["usage"] == counts && sample["timing"] == times {
                    return Ok(());
                }
                return Err(StoreError::Conflict(
                    "suggestion usage already settled".to_owned(),
                ));
            }
            sample["status"] = json!("settled");
            sample["usage"] = counts;
            sample["timing"] = times;
            save(c, "suggestion_usage", &sample)
        })
    }

    pub fn run_usage(&self, owner: &str, id: &str) -> Result<Value> {
        self.read(|c| {
            let tx = c.unchecked_transaction()?;
            let mut rows = report(&tx, owner, id, false)?.runs;
            rows.pop().ok_or(StoreError::NotFound)
        })
    }

    pub fn session_usage(&self, owner: &str, session: &Value, agent: &Value) -> Result<Value> {
        if session["owner"].as_str() != Some(owner) {
            return Err(StoreError::NotFound);
        }
        self.read(|c| {
            let tx = c.unchecked_transaction()?;
            let id = field(session, "id")?;
            let data = report(&tx, owner, id, true)?;
            let mut totals = data.totals;
            let mut suggestions = Totals::default();
            let mut query = tx.prepare_cached(
                "SELECT doc FROM objects WHERE kind='suggestion_usage' AND owner=?1 AND parent=?2",
            )?;
            let mut samples = query.query(params![owner, id])?;
            while let Some(row) = samples.next()? {
                let sample = serde_json::from_str(&row.get::<_, String>(0)?)?;
                totals.observe(&sample, 1);
                suggestions.observe(&sample, 1);
            }
            let context = crate::context::session_context(
                agent,
                data.context_run.as_ref(),
                data.context_call.as_ref(),
                data.compactions,
            );
            Ok(json!({
                "session_id": id,
                "tokens": totals.tokens(),
                "calls": totals.calls(),
                "cache": totals.cache(),
                "speed": totals.speed(),
                "runs": data.runs,
                "context": context,
                "suggestions": {"calls": suggestions.calls(), "tokens": suggestions.tokens()}
            }))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregates_measured_requests_once_and_preserves_unknowns_across_retries_and_branches() {
        let store = Store::open(":memory:").unwrap();
        let session = store.create_session("u", json!({"id":"s"})).unwrap();
        store.tx(|db| {
            create(db,"u","run","r",Some("s"),json!({"status":"completed","admitted_us":1_000_000,"finished_us":5_560_000}))?;
            create(db,"u","model_call","m",Some("r"),json!({"status":"completed","ctx_snapshot":{"used_bytes":50,"limit_bytes":100},
                "usage":{"prompt_tokens":100,"completion_tokens":20,"total_tokens":120,"prompt_cache_hit_tokens":90,"prompt_cache_miss_tokens":10},
                "timing":{"request_started_us":1_000_000,"first_output_us":1_200_000,"generation_ended_us":1_450_000,"response_completed_us":1_500_000}}))?;
            for attempt in ["lost", "accepted"] {
                db.execute("INSERT INTO run_stream(run_id,model_call_id,attempt_id,event) VALUES('r','m',?1,'attempt_started')",[attempt])?;
            }
            create(db,"u","compaction_call","summary",Some("r"),json!({"reason":"compaction","status":"completed",
                "usage":{"prompt_tokens":20,"completion_tokens":10,"prompt_tokens_details":{"cached_tokens":5}},
                "timing":{"request_started_us":2_000_000,"response_completed_us":3_000_000},
                "ctx_snapshot":{"used_bytes":40,"before_bytes":150,"limit_bytes":100}}))?;
            Ok(())
        }).unwrap();
        let usage = store.session_usage("u", &session, &Value::Null).unwrap();
        assert_eq!(
            usage["tokens"],
            json!({"input":120,"output":30,"total":150})
        );
        assert_eq!(usage["calls"], json!({"reported":2,"total":3}));
        assert_eq!(
            usage["speed"],
            json!({"sampled_calls":2,"output_tokens_per_second":20.0,"decode_sampled_calls":1,"decode_tokens_per_second":80.0})
        );
        assert_eq!(
            usage["cache"],
            json!({"hit_tokens":95,"miss_tokens":25,"hit_rate":95.0/120.0,"sampled_calls":2})
        );
        assert_eq!(usage["context"]["compactions"], 1);
        assert_eq!(usage["context"]["used_bytes"], 40);
        let run = store.run_usage("u", "r").unwrap();
        assert_eq!(run, usage["runs"][0]);
        assert_eq!(run["elapsed_ms"], 4560);
        assert!(matches!(
            store.run_usage("other", "r"),
            Err(StoreError::NotFound)
        ));
        let branch = store
            .create_session("u", json!({"id":"b","parent_session_id":"s"}))
            .unwrap();
        let empty = store.session_usage("u", &branch, &Value::Null).unwrap();
        assert_eq!(empty["tokens"]["total"], 0);
        assert!(empty["cache"]["hit_rate"].is_null());
        assert!(empty["speed"]["output_tokens_per_second"].is_null());

        // Recommendations are session-only. Settling a sample twice must not duplicate
        // billing or preserve provider-specific free-text metadata about private materials.
        let sample = store.start_suggestion_usage("u", "s").unwrap();
        assert!(matches!(
            store.start_suggestion_usage("other", "s"),
            Err(StoreError::NotFound)
        ));
        let raw = json!({"prompt_tokens":9,"completion_tokens":3,"secret_text":"private"});
        let timing = json!({"request_started_us":3_000_000,"response_completed_us":4_000_000,"debug":"private"});
        for _ in 0..2 {
            store
                .finish_suggestion_usage("u", &sample, raw.clone(), timing.clone())
                .unwrap();
        }
        let with_sample = store.session_usage("u", &session, &Value::Null).unwrap();
        assert_eq!(with_sample["tokens"]["total"], 162);
        assert_eq!(with_sample["suggestions"]["tokens"]["total"], 12);
        assert_eq!(with_sample["runs"], usage["runs"]);
        assert_eq!(with_sample["context"], usage["context"]);
        assert_eq!(store.history("u", "s").unwrap(), json!([]));
        let saved = store
            .read(|c| load(c, "u", "suggestion_usage", &sample))
            .unwrap();
        assert!(!saved.to_string().contains("private"));
    }

    #[test]
    fn legacy_summary_and_provider_without_receipt_are_marked_as_partial() {
        let store = Store::open(":memory:").unwrap();
        let session = store.create_session("u", json!({"id":"s"})).unwrap();
        store
            .tx(|db| {
                create(
                    db,
                    "u",
                    "run",
                    "r",
                    Some("s"),
                    json!({"status":"completed","admitted_us":1_000_000}),
                )?;
                db.execute(
                    "INSERT INTO run_steps(run_id,step,committed_ms) VALUES('r',1,2100)",
                    [],
                )?;
                create(
                    db,
                    "u",
                    "model_call",
                    "a",
                    Some("r"),
                    json!({"status":"failed","usage":{"total_tokens":10},
                "messages":[{"role":"user","content":"legacy"}],"compressed_ranges":[[0,1]]}),
                )?;
                create(
                    db,
                    "u",
                    "model_call",
                    "b",
                    Some("r"),
                    json!({"status":"failed"}),
                )?;
                Ok(())
            })
            .unwrap();
        let usage = store.session_usage("u", &session, &Value::Null).unwrap();
        assert_eq!(usage["calls"], json!({"reported":1,"total":3}));
        assert_eq!(
            usage["tokens"],
            json!({"input":null,"output":null,"total":10})
        );
        assert_eq!(usage["context"]["compactions"], 1);
        assert!(usage["speed"]["output_tokens_per_second"].is_null());
        assert_eq!(usage["runs"][0]["elapsed_ms"], 1100);
    }
}
