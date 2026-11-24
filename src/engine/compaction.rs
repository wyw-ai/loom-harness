use super::*;
use crate::config::Provider;
use sha2::{Digest, Sha256};

pub(super) fn plan(
    app: &App,
    w: &Work,
    request: &Value,
    messages: &[Value],
) -> Result<(Value, context::CompactionPlan)> {
    let strategy = app
        .catalog
        .compaction_prompt()
        .map_err(anyhow::Error::msg)?
        .context("required input exceeds budget and compaction is disabled");
    let planned = strategy.and_then(|strategy| {
        let plan = context::plan_compaction(messages, s(&strategy, "prompt")?)?;
        Ok((strategy["revision"].clone(), plan))
    });
    if let Err(error) = &planned {
        let mut failed = request.clone();
        failed["status"] = json!("failed");
        failed["preflight_only"] = json!(true);
        failed["error"] = json!(error.to_string());
        app.db.record_call(w, "compaction_call", failed)?;
    }
    planned
}

/// Checkpoint the auxiliary provider request separately from the Run's business model calls.
/// A reclaimed worker can reapply a completed summary without charging for it again.
pub(super) async fn run(
    app: &App,
    w: &Work,
    provider: &Provider,
    mut request: Value,
    plan: &context::CompactionPlan,
    messages: &[Value],
    limit: usize,
) -> Result<Vec<Value>> {
    // Keep the short receipt and an identity of the exact source material. The duplicated
    // original history (which may include attachments) need not be copied into telemetry.
    let digest = Sha256::digest(serde_json::to_vec(&plan.input)?);
    request["input_hash"] = json!(format!("{digest:x}"));
    let call = app.db.record_call(w, "compaction_call", request)?;
    let cid = s(&call, "id")?;
    if call["status"] == "completed" {
        return Ok(context::apply_compaction(
            messages,
            plan.range,
            s(&call, "summary")?,
            limit,
        )?);
    }
    if call["status"] == "failed" {
        bail!("{}", call["error"].as_str().unwrap_or("compaction failed"));
    }
    // A worker can lose its lease after the provider accepted the request. Meter a
    // recovered attempt separately even if the first receipt never reached Loom.
    let attempts = call["provider_attempts"]
        .as_u64()
        .unwrap_or(0)
        .saturating_add(1);
    app.db.update_call(
        w,
        "compaction_call",
        cid,
        json!({"provider_attempts":attempts}),
    )?;
    let reply = match context::complete(&app.http, &provider.route(), &plan.input, &[], None).await
    {
        Ok(reply) => reply,
        Err(error) => {
            app.db.update_call(
                w,
                "compaction_call",
                cid,
                json!({"status":"failed","error":error.to_string()}),
            )?;
            return Err(error.into());
        }
    };
    let mut patch =
        json!({"usage":reply.usage,"timing":reply.timing,"finish_reason":reply.finish_reason});
    let shortened = if reply.finish_reason == "stop" && reply.tool_calls.is_empty() {
        context::apply_compaction(
            messages,
            plan.range,
            reply.content.as_deref().unwrap_or(""),
            limit,
        )
        .map_err(anyhow::Error::from)
    } else {
        Err(anyhow!("compaction did not produce a complete summary"))
    };
    match &shortened {
        Ok(output) => {
            patch["status"] = json!("completed");
            patch["summary"] = json!(reply.content.unwrap_or_default());
            patch["ctx_snapshot"] = call["ctx_snapshot"].clone();
            patch["ctx_snapshot"]["used_bytes"] = json!(context::input_bytes(output));
        }
        Err(e) => {
            patch["status"] = json!("failed");
            patch["error"] = json!(e.to_string());
        }
    }
    app.db.update_call(w, "compaction_call", cid, patch)?;
    shortened
}
