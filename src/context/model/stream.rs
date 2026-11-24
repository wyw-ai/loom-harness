use super::{ModelError, ModelReply, ModelTiming, parse_reply};
use serde_json::{Value, json};
use std::collections::BTreeMap;

// SSE lines and UTF-8 code points may straddle any number of HTTP chunks.
#[derive(Default)]
struct SseDecoder {
    line: Vec<u8>,
    data: Vec<String>,
    event_type: String,
    after_cr: bool,
    started: bool,
}

impl SseDecoder {
    fn push(
        &mut self,
        bytes: &[u8],
        mut event: impl FnMut(&str, &str) -> Result<bool, ModelError>,
    ) -> Result<bool, ModelError> {
        for &byte in bytes {
            if std::mem::take(&mut self.after_cr) && byte == b'\n' {
                continue;
            }
            if !matches!(byte, b'\r' | b'\n') {
                self.line.push(byte);
                continue;
            }
            self.after_cr = byte == b'\r';
            let line = std::str::from_utf8(&self.line)
                .map_err(|_| ModelError::invalid("invalid UTF-8 in model stream"))?;
            let line = if std::mem::replace(&mut self.started, true) {
                line
            } else {
                line.strip_prefix('\u{feff}').unwrap_or(line)
            };
            if line.is_empty() {
                if !self.data.is_empty() && event(&self.event_type, &self.data.join("\n"))? {
                    return Ok(true);
                }
                self.data.clear();
                self.event_type.clear();
            } else {
                let (field, value) = line.split_once(':').unwrap_or((line, ""));
                let value = value.strip_prefix(' ').unwrap_or(value);
                match field {
                    "data" => self.data.push(value.to_owned()),
                    "event" => self.event_type = value.to_owned(),
                    _ => {}
                }
            }
            self.line.clear();
        }
        Ok(false)
    }
}

#[derive(Default)]
struct ToolParts {
    id: String,
    name: String,
    arguments: String,
}

fn fragment<'a>(value: &'a Value, name: &str) -> Result<Option<&'a str>, ModelError> {
    match &value[name] {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(s)),
        _ => Err(ModelError::invalid(format!("invalid model stream {name}"))),
    }
}

#[derive(Default)]
struct Choice {
    content: Option<String>,
    reasoning: Option<String>,
    tools: BTreeMap<u64, ToolParts>,
    finish: Option<String>,
    usage: Value,
}

impl Choice {
    fn ingest(
        &mut self,
        kind: &str,
        data: &str,
        now: i64,
        timing: &mut ModelTiming,
        on_text: &mut impl FnMut(&str),
    ) -> Result<bool, ModelError> {
        if kind == "error" {
            return Err(ModelError::invalid(
                "model provider reported a stream error",
            ));
        }
        if data.trim() == "[DONE]" {
            return Ok(true);
        }
        let chunk: Value = serde_json::from_str(data)
            .map_err(|_| ModelError::invalid("invalid model stream JSON"))?;
        if !chunk["error"].is_null() {
            return Err(ModelError::invalid(
                "model provider reported a stream error",
            ));
        }
        if !chunk["usage"].is_null() {
            self.usage = chunk["usage"].clone();
        }
        let choices = chunk["choices"]
            .as_array()
            .ok_or_else(|| ModelError::invalid("missing model stream choices"))?;
        for choice in choices {
            if choice
                .get("index")
                .is_some_and(|value| value.as_u64() != Some(0))
            {
                continue;
            }
            if self.finish.is_some() {
                return Err(ModelError::invalid("received model choice after finish"));
            }
            let delta = &choice["delta"];
            if !delta.is_object()
                || fragment(delta, "role")?.is_some_and(|role| role != "assistant")
            {
                return Err(ModelError::invalid("invalid model stream delta"));
            }
            if let Some(reasoning) = fragment(delta, "reasoning_content")? {
                self.reasoning
                    .get_or_insert_with(String::new)
                    .push_str(reasoning);
                if !reasoning.is_empty() {
                    timing.first_reasoning_us.get_or_insert(now);
                }
            }
            if let Some(content) = fragment(delta, "content")? {
                self.content
                    .get_or_insert_with(String::new)
                    .push_str(content);
                if !content.is_empty() {
                    timing.first_content_us.get_or_insert(now);
                    on_text(content);
                }
            }
            if !delta["tool_calls"].is_null() {
                let tools = delta["tool_calls"]
                    .as_array()
                    .ok_or_else(|| ModelError::invalid("invalid model stream tool_calls"))?;
                for tool in tools {
                    if fragment(tool, "type")?.is_some_and(|kind| kind != "function") {
                        return Err(ModelError::invalid("invalid model stream tool type"));
                    }
                    let index = tool["index"]
                        .as_u64()
                        .ok_or_else(|| ModelError::invalid("missing model stream tool index"))?;
                    let parts = self.tools.entry(index).or_default();
                    if let Some(id) = fragment(tool, "id")? {
                        parts.id.push_str(id);
                    }
                    let function = &tool["function"];
                    if let Some(name) = fragment(function, "name")? {
                        parts.name.push_str(name);
                    }
                    if let Some(arguments) = fragment(function, "arguments")? {
                        parts.arguments.push_str(arguments);
                    }
                }
            }
            self.finish = fragment(choice, "finish_reason")?.map(str::to_owned);
        }
        Ok(false)
    }

    fn finish(self, timing: ModelTiming) -> Result<ModelReply, ModelError> {
        let reason = self
            .finish
            .ok_or_else(|| ModelError::invalid("model stream ended without finish_reason"))?;
        let mut message = json!({"role":"assistant","content":self.content});
        if let Some(reasoning) = self.reasoning {
            message["reasoning_content"] = Value::String(reasoning);
        }
        if !self.tools.is_empty() {
            message["tool_calls"] = Value::Array(self.tools.into_values().map(|tool| {
                json!({"id":tool.id,"type":"function","function":{"name":tool.name,"arguments":tool.arguments}})
            }).collect());
        }
        parse_reply(
            &json!({"choices":[{"finish_reason":reason,"message":message}],"usage":self.usage}),
            timing,
        )
    }
}

pub(super) async fn receive(
    mut response: reqwest::Response,
    mut timing: ModelTiming,
    mut on_text: impl FnMut(&str),
) -> Result<ModelReply, ModelError> {
    let mut decoder = SseDecoder::default();
    let mut choice = Choice::default();
    loop {
        let bytes = response
            .chunk()
            .await
            .ok()
            .flatten()
            .ok_or_else(|| ModelError {
                message: "model stream interrupted before [DONE]".into(),
                retryable: timing.first_content_us.is_none(),
            })?;
        let now = chrono::Utc::now().timestamp_micros();
        if decoder.push(&bytes, |kind, data| {
            choice.ingest(kind, data, now, &mut timing, &mut on_text)
        })? {
            return choice.finish(timing);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timing() -> ModelTiming {
        ModelTiming {
            request_started_us: 5,
            first_content_us: None,
            first_reasoning_us: None,
        }
    }

    #[test]
    fn split_utf8_lines_usage_and_live_content() {
        let mut decoder = SseDecoder::default();
        let mut choice = Choice::default();
        let mut times = timing();
        let mut outputs = Vec::new();
        let mut delivered = false;
        let samples = [
            (8, "\u{feff}: ping\r\n\r\n"),
            (
                9,
                "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"\"}}]}\n\n",
            ),
            (
                12,
                "data:{\"choices\":[{\"delta\":{\"reasoning_content\":\"thinking\"}}]}\r\r",
            ),
            (
                18,
                "data: {\"choices\":[\r\ndata: {\"delta\":{\"content\":\"你好\"}}]}\r\n\r\n",
            ),
            (
                42,
                "data: {\"choices\":[{\"delta\":{\"content\":\"!\"},\"finish_reason\":\"stop\"}]}\n\n",
            ),
            (
                50,
                "data: {\"usage\":{\"total_tokens\":5},\"choices\":[]}\n\n",
            ),
            (60, "data: [DONE]\n\n"),
        ];
        for (now, frame) in samples {
            for byte in frame.as_bytes().chunks(1) {
                delivered = decoder
                    .push(byte, |kind, data| {
                        choice.ingest(kind, data, now, &mut times, &mut |s| {
                            outputs.push(s.to_owned())
                        })
                    })
                    .unwrap();
            }
            if now == 18 {
                assert_eq!(outputs, ["你好"]);
                assert!(!delivered);
            }
        }
        assert!(delivered);
        let reply = choice.finish(times).unwrap();
        assert_eq!(outputs, ["你好", "!"]);
        assert_eq!(reply.content.as_deref(), Some("你好!"));
        assert_eq!(reply.message["reasoning_content"], "thinking");
        assert_eq!(reply.timing.first_content_us, Some(18));
        assert_eq!(reply.timing.first_reasoning_us, Some(12));
        assert_eq!(reply.usage["total_tokens"], 5);
    }

    #[test]
    fn reconstructs_interleaved_tool_calls_in_index_order() {
        let mut c = Choice::default();
        let mut t = timing();
        for delta in [
            json!({"role":"assistant","tool_calls":[{"index":1,"type":"function","id":"b","function":{"name":"write","arguments":"{\"v\":"}}]}),
            json!({"tool_calls":[{"index":0,"type":"function","id":"id-","function":{"name":"re","arguments":"{"}},{"index":1,"function":{"arguments":"2}"}}]}),
            json!({"tool_calls":[{"index":0,"id":"a","function":{"name":"ad","arguments":"}"}}]}),
        ] {
            let frame = json!({"choices":[{"index":0,"delta":delta}]}).to_string();
            assert!(
                !c.ingest("", &frame, 10, &mut t, &mut |_| panic!(
                    "tool leaked as text"
                ))
                .unwrap()
            );
        }
        let frame =
            json!({"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}).to_string();
        c.ingest("", &frame, 11, &mut t, &mut |_| {}).unwrap();
        let reply = c.finish(t).unwrap();
        assert_eq!(reply.tool_calls.len(), 2);
        assert_eq!(reply.tool_calls[0].name, "read");
        assert_eq!(reply.tool_calls[0].id, "id-a");
        assert_eq!(reply.tool_calls[1].arguments, json!({"v":2}));
        assert!(reply.timing.first_content_us.is_none());
    }

    #[tokio::test]
    async fn truncated_stream_preserves_no_partial_tool_and_does_not_retry_visible_text() {
        use axum::{Router, http::header::CONTENT_TYPE, routing::get};
        let routes = Router::new().route("/text",get(||async {
            ([(CONTENT_TYPE,"text/event-stream")],"data: {\"choices\":[{\"delta\":{\"content\":\"hello\"},\"finish_reason\":\"stop\"}]}\n\n")
        })).route("/tool",get(||async {
            ([(CONTENT_TYPE,"text/event-stream")],"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"type\":\"function\",\"id\":\"x\",\"function\":{\"name\":\"push\",\"arguments\":\"{}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n")
        })).route("/early-done",get(||async {
            ([(CONTENT_TYPE,"text/event-stream")],"data: [DONE]\n\n")
        }));
        let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        let server = tokio::spawn(axum::serve(socket, routes).into_future());
        let client = reqwest::Client::new();
        for (path, expected_text, retryable) in [
            ("text", "hello", false),
            ("tool", "", true),
            ("early-done", "", false),
        ] {
            let response = client
                .get(format!("http://{addr}/{path}"))
                .send()
                .await
                .unwrap();
            let mut text = String::new();
            let err = receive(response, timing(), |part| text.push_str(part))
                .await
                .unwrap_err();
            assert_eq!(text, expected_text);
            assert_eq!(err.retryable, retryable);
        }
        server.abort();
    }
}
