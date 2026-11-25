use super::input_bytes;
use serde_json::{Value, json};

pub fn context_limit(agent: &Value) -> u64 {
    agent["context_bytes"].as_u64().unwrap_or(256_000)
}

/// The gauge describes the executed input, measured with the same projection as the compressor.
pub fn session_context(
    agent: &Value,
    run: Option<&Value>,
    call: Option<&Value>,
    compactions: u64,
) -> Value {
    let mut limit = context_limit(agent);
    let mut before = None;
    let mut used = None;
    let mut state = if run.is_none() {
        "empty"
    } else {
        "unavailable"
    };
    if let Some(call) = call {
        let snapshot = &call["ctx_snapshot"];
        if let Some(observed_limit) = snapshot["limit_bytes"].as_u64() {
            limit = observed_limit;
        } else if let Some(matching) = run.filter(|r| r["id"] == call["run_id"]) {
            limit = context_limit(&matching["agent_snapshot"]);
        }
        before = snapshot["before_bytes"].as_u64();
        used = snapshot["used_bytes"].as_u64().or_else(|| {
            call["messages"]
                .as_array()
                .map(|messages| input_bytes(messages) as u64)
        });
        state = if call["reason"] == "compaction" {
            match call["status"].as_str() {
                Some("completed") => "compacted",
                Some("failed" | "cancelled") => "failed",
                _ if run.is_some_and(|r| {
                    r["id"] == call["run_id"]
                        && matches!(r["status"].as_str(), Some("failed" | "cancelled"))
                }) =>
                {
                    "failed"
                }
                _ => "compacting",
            }
        } else if call["compressed_ranges"]
            .as_array()
            .is_some_and(|r| !r.is_empty())
        {
            "compacted"
        } else if used.is_some() {
            "observed"
        } else {
            "unavailable"
        };
    }
    json!({"used_bytes":used,"limit_bytes":limit,"before_bytes":before,"compactions":compactions,
        "state":state,"run_id":call.and_then(|c| c["run_id"].as_str())})
}
