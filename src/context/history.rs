use super::model::ModelError;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct GoalInput {
    pub objective: String,
    pub completion_condition: String,
    #[serde(default)]
    pub constraints: Vec<String>,
}

/// History already has the Session's selected ordering and authority scope.
/// Derived Goal messages are regenerated for this decision, never inherited.
pub fn build_messages(
    system: &str,
    sections: &[String],
    history: &[Value],
    goal: Option<&GoalInput>,
    continuation: bool,
) -> Vec<Value> {
    build_messages_with_goal_prompt(
        system,
        sections,
        history,
        goal,
        continuation.then_some("继续当前目标。先核对已有进度、完成条件和证据，再执行剩余工作。"),
    )
}

pub fn build_messages_with_goal_prompt(
    system: &str,
    sections: &[String],
    history: &[Value],
    goal: Option<&GoalInput>,
    continuation_prompt: Option<&str>,
) -> Vec<Value> {
    let mut instructions: Vec<_> = std::iter::once(system)
        .chain(sections.iter().map(String::as_str))
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    if let Some(g) = goal {
        instructions.push(format!(
            "Active goal:\n{}\nCompletion condition:\n{}",
            g.objective, g.completion_condition
        ));
        if !g.constraints.is_empty() {
            instructions.push(format!(
                "Current constraints:\n{}",
                g.constraints.join("\n")
            ));
        }
    }
    let mut messages = Vec::with_capacity(history.len() + 2);
    if !instructions.is_empty() {
        messages.push(json!({"role":"system","content":instructions.join("\n\n")}));
    }
    messages.extend(history.iter().filter(|m| m["source"] != "goal").cloned());
    if let Some(prompt) = continuation_prompt.filter(|s| goal.is_some() && !s.trim().is_empty()) {
        messages.push(json!({"role":"user","source":"goal","content":prompt}));
    }
    messages
}

#[derive(Clone, Debug, Serialize)]
pub struct CompactionPlan {
    pub input: [Value; 2],
    pub range: (usize, usize),
}

fn text_view(message: &Value) -> std::borrow::Cow<'_, Value> {
    use std::borrow::Cow;
    let Some(parts) = message["content"].as_array() else {
        return Cow::Borrowed(message);
    };
    if !parts.iter().any(|p| p["type"] == "image_url") {
        return Cow::Borrowed(message);
    }
    let mut result: serde_json::Map<_, _> = message
        .as_object()
        .unwrap()
        .iter()
        .filter(|(k, _)| k.as_str() != "content")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let text = parts
        .iter()
        .map(|part| {
            if part["type"] == "image_url" {
                json!({"type":"text","text":"[image attachment]"})
            } else {
                part.clone()
            }
        })
        .collect();
    result.insert("content".into(), Value::Array(text));
    Cow::Owned(Value::Object(result))
}

/// Budget the textual message projection. Encoded image payloads are sent as
/// native vision input; treating base64 as prompt text would discard ordinary screenshots.
pub fn input_bytes(messages: &[Value]) -> usize {
    2 + messages.len().saturating_sub(1)
        + messages
            .iter()
            .map(|m| text_view(m).to_string().len())
            .sum::<usize>()
}

fn compressible_range(messages: &[Value]) -> Option<(usize, usize)> {
    let last_user = messages
        .iter()
        .rposition(|m| m["role"] == "user" && m["source"] != "goal");
    let first_user = messages.iter().position(|m| m["role"] == "user");
    let mut pending = HashSet::new();
    let mut best = None;
    let mut run_start = 0;
    let mut best_size = 0;
    let mut run_size = 0;
    for (i, m) in messages.iter().enumerate() {
        let role = m["role"].as_str();
        let protected = i == messages.len() - 1
            || Some(i) == last_user
            || Some(i) == first_user
            || role == Some("system")
            || m["source"] == "goal"
            || m["context_protected"] == true;
        if protected {
            run_start = i + 1;
            run_size = 0;
            pending.clear();
            continue;
        }
        if let Some(calls) = m["tool_calls"].as_array() {
            for id in calls.iter().filter_map(|c| c["id"].as_str()) {
                pending.insert(id);
            }
        }
        if role == Some("tool")
            && !m["tool_call_id"]
                .as_str()
                .is_some_and(|id| pending.remove(id))
        {
            // Its initiating assistant message was protected or absent.
            run_start = i + 1;
            run_size = 0;
            pending.clear();
            continue;
        }
        run_size += input_bytes(std::slice::from_ref(m));
        if pending.is_empty() && run_size > best_size {
            best = Some((run_start, i + 1));
            best_size = run_size;
        }
    }
    best
}

/// The limit is the input-message byte allowance after the caller reserves
/// space for tools and output. Mark unsatisfied waits and required evidence
/// with context_protected before calling. This never invokes a business tool.
pub fn plan_compaction(messages: &[Value], prompt: &str) -> Result<CompactionPlan, ModelError> {
    if prompt.trim().is_empty() {
        return Err(ModelError::invalid(
            "compaction plugin supplied no summary instruction",
        ));
    }
    let (start, end) = compressible_range(messages)
        .ok_or_else(|| ModelError::invalid("required model input exceeds its budget"))?;
    let material: Vec<_> = messages[start..end]
        .iter()
        .map(|m| {
            let mut item = text_view(m).into_owned();
            if let Some(obj) = item.as_object_mut() {
                obj.remove("reasoning_content");
            }
            item
        })
        .collect();
    let input = [
        json!({"role":"system","content":prompt}),
        json!({"role":"user","content":serde_json::to_string(&material).unwrap()}),
    ];
    Ok(CompactionPlan {
        input,
        range: (start, end),
    })
}

pub fn apply_compaction(
    messages: &[Value],
    (start, end): (usize, usize),
    summary: &str,
    max_input_bytes: usize,
) -> Result<Vec<Value>, ModelError> {
    let mut output = messages.to_vec();
    output.splice(start..end,[json!({"role":"assistant","source":"compaction","content":format!("此前历史摘要：{summary}")})]);
    if input_bytes(&output) > max_input_bytes {
        return Err(ModelError::invalid(
            "compacted model input still exceeds its budget",
        ));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuation_is_only_shown_once_for_the_right_trigger() {
        let g = GoalInput {
            objective: "20 modules".into(),
            completion_condition: "tests pass".into(),
            constraints: vec!["read-only".into()],
        };
        let history = vec![
            json!({"role":"user","content":"inspect"}),
            json!({"role":"user","source":"goal","content":"old instruction"}),
        ];
        for (continuation, count) in [(true, 1), (false, 0)] {
            let msgs = build_messages("system", &[], &history, Some(&g), continuation);
            assert!(msgs[0]["content"].as_str().unwrap().contains("read-only"));
            assert_eq!(msgs.iter().filter(|m| m["source"] == "goal").count(), count);
            assert!(msgs.iter().all(|m| m["content"] != "old instruction"));
        }
    }

    #[test]
    fn only_closed_tool_batches_can_be_replaced() {
        let h = vec![
            json!({"role":"user","content":"goal"}),
            json!({"role":"assistant","tool_calls":[{"id":"a"},{"id":"b"}]}),
            json!({"role":"tool","tool_call_id":"a","content":"short"}),
            json!({"role":"tool","tool_call_id":"b","content":"short"}),
            json!({"role":"assistant","tool_calls":[{"id":"c"}]}),
            json!({"role":"user","content":"current"}),
        ];
        assert_eq!(compressible_range(&h), Some((1, 4)));
    }

    #[test]
    fn compaction_accounts_for_image_references_without_transcribing_base64() {
        let image = json!({"role":"user","source_event_id":"evt","content":[
            {"type":"text","text":"Screen capture"},{"type":"image_url","image_url":{"url":format!("data:image/png;base64,{}","A".repeat(300_000))}}]});
        let projected = text_view(&image);
        assert_eq!(projected["source_event_id"], "evt");
        assert_eq!(projected["content"][0]["text"], "Screen capture");
        assert_eq!(projected["content"][1]["text"], "[image attachment]");
        assert!(!projected.to_string().contains("base64"));
        assert!(input_bytes(&[image]) < 256);
    }
}
