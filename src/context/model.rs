use base64::{Engine, engine::general_purpose::STANDARD};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashSet, fmt, time::Duration};

mod stream;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelRoute {
    pub base_url: String,
    pub model: String,
    pub api_key_env: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ModelToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Clone, Debug, Serialize)]
pub struct ModelReply {
    /// Keep this intact for the next turn. Some providers require their
    /// reasoning_content to be returned along with assistant tool calls.
    pub message: Value,
    pub content: Option<String>,
    pub tool_calls: Vec<ModelToolCall>,
    pub finish_reason: String,
    pub usage: Value,
    pub timing: ModelTiming,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct ModelTiming {
    pub request_started_us: i64,
    pub first_content_us: Option<i64>,
    pub first_reasoning_us: Option<i64>,
    pub response_completed_us: Option<i64>,
    /// Stream-only generation bounds. Buffered responses cannot measure decoding speed.
    pub first_output_us: Option<i64>,
    pub generation_ended_us: Option<i64>,
}

#[derive(Debug)]
pub struct ModelError {
    pub message: String,
    pub retryable: bool,
}

impl ModelError {
    pub(super) fn invalid(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
        }
    }
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ModelError {}

fn endpoint(route: &ModelRoute, authenticated: bool) -> Result<Url, ModelError> {
    if route.model.trim().is_empty() {
        return Err(ModelError::invalid("model name is missing"));
    }
    let mut url =
        Url::parse(&route.base_url).map_err(|_| ModelError::invalid("invalid model endpoint"))?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || (authenticated && url.scheme() == "http" && !loopback)
    {
        return Err(ModelError::invalid(
            "invalid model endpoint or insecure credential transport",
        ));
    }
    let path = url.path().trim_end_matches('/');
    if !path.ends_with("/chat/completions") {
        url.set_path(&format!("{path}/chat/completions"));
    }
    Ok(url)
}

pub(super) fn wire_message(message: &Value) -> Value {
    // Loom's source and material references remain in ModelCall evidence.
    // A provider receives only fields for the chat protocol.
    let mut wire = json!({});
    for key in [
        "role",
        "content",
        "name",
        "tool_calls",
        "tool_call_id",
        "reasoning_content",
    ] {
        if let Some(value) = message.get(key) {
            wire[key] = value.clone();
        }
    }
    wire
}

/// Convert attachment bytes already resolved under the caller's authorization.
/// Model providers need the bytes (or a readable URL), not a HOME file path.
pub fn image_part(mime: &str, content: &[u8]) -> Result<Value, ModelError> {
    let matches = match mime {
        "image/png" => content.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/jpeg" => content.starts_with(&[0xff, 0xd8, 0xff]),
        "image/gif" => content.starts_with(b"GIF87a") || content.starts_with(b"GIF89a"),
        "image/webp" => content.starts_with(b"RIFF") && content.get(8..12) == Some(b"WEBP"),
        _ => false,
    };
    if !matches {
        return Err(ModelError::invalid(
            "unsupported image type or image bytes do not match declared type",
        ));
    }
    Ok(
        json!({"type":"image_url","image_url":{"url":format!("data:{mime};base64,{}",STANDARD.encode(content))}}),
    )
}

pub async fn complete(
    http: &Client,
    route: &ModelRoute,
    messages: &[Value],
    tools: &[Value],
    response_format: Option<&Value>,
) -> Result<ModelReply, ModelError> {
    complete_streaming(http, route, messages, tools, response_format, |_| {}).await
}

pub async fn complete_streaming(
    http: &Client,
    route: &ModelRoute,
    messages: &[Value],
    tools: &[Value],
    response_format: Option<&Value>,
    mut on_text: impl FnMut(&str),
) -> Result<ModelReply, ModelError> {
    let url = endpoint(route, route.api_key_env.is_some())?;
    let mut payload = json!({"model": route.model, "messages": messages.iter().map(wire_message).collect::<Vec<_>>(), "stream": true, "stream_options":{"include_usage":true}});
    if !tools.is_empty() {
        payload["tools"] = json!(tools);
    }
    if let Some(format) = response_format {
        payload["response_format"] = format.clone();
    }
    let mut request = http
        .post(url)
        .timeout(Duration::from_secs(120))
        .header(
            reqwest::header::ACCEPT,
            "text/event-stream, application/json",
        )
        .json(&payload);
    if let Some(name) = &route.api_key_env {
        let key = std::env::var(name)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| ModelError::invalid("model credential is not configured"))?;
        request = request.bearer_auth(key);
    }
    let mut timing = ModelTiming {
        request_started_us: chrono::Utc::now().timestamp_micros(),
        first_content_us: None,
        first_reasoning_us: None,
        response_completed_us: None,
        first_output_us: None,
        generation_ended_us: None,
    };
    let response = request.send().await.map_err(|e| ModelError {
        message: if e.is_timeout() {
            "model provider timed out"
        } else {
            "could not reach model provider"
        }
        .into(),
        retryable: e.is_timeout() || e.is_connect() || e.is_request(),
    })?;
    let status = response.status();
    if !status.is_success() {
        // Providers sometimes echo headers or input in their error bodies.
        return Err(ModelError {
            message: format!("model provider returned HTTP {}", status.as_u16()),
            retryable: status.is_server_error() || matches!(status.as_u16(), 408 | 429),
        });
    }
    let is_stream = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/event-stream"));
    if is_stream {
        return stream::receive(response, timing, on_text).await;
    }
    let body: Value = response
        .json()
        .await
        .map_err(|_| ModelError::invalid("invalid model response JSON"))?;
    let now = chrono::Utc::now().timestamp_micros();
    let message = &body["choices"][0]["message"];
    timing.first_content_us = message["content"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|_| now);
    timing.first_reasoning_us = message["reasoning_content"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|_| now);
    let reply = parse_reply(&body, timing)?;
    if let Some(text) = reply.content.as_deref().filter(|text| !text.is_empty()) {
        on_text(text);
    }
    Ok(reply)
}

fn required_text<'a>(v: &'a Value, key: &str) -> Result<&'a str, ModelError> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ModelError::invalid(format!("model response is missing {key}")))
}

fn parse_reply(body: &Value, mut timing: ModelTiming) -> Result<ModelReply, ModelError> {
    timing
        .response_completed_us
        .get_or_insert_with(|| chrono::Utc::now().timestamp_micros());
    let choice = body
        .pointer("/choices/0")
        .ok_or_else(|| ModelError::invalid("model returned no choice"))?;
    let reason = required_text(choice, "finish_reason")?;
    let message = &choice["message"];
    if message["role"] != "assistant" {
        return Err(ModelError::invalid("invalid provider message role"));
    }
    let content = match message.get("content") {
        Some(Value::String(s)) => Some(s.clone()),
        None | Some(Value::Null) => None,
        _ => return Err(ModelError::invalid("invalid provider message content")),
    };
    let mut calls = Vec::new();
    if let Some(raw) = message.get("tool_calls").filter(|v| !v.is_null()) {
        let raw = raw
            .as_array()
            .ok_or_else(|| ModelError::invalid("tool_calls must be an array"))?;
        let mut ids = HashSet::new();
        for call in raw {
            let id = required_text(call, "id")?;
            if call["type"] != "function" || !ids.insert(id) {
                return Err(ModelError::invalid(
                    "invalid tool call type or duplicate call ID",
                ));
            }
            let func = &call["function"];
            let args: Value = serde_json::from_str(required_text(func, "arguments")?)
                .map_err(|_| ModelError::invalid("model tool arguments are not valid JSON"))?;
            if !args.is_object() {
                return Err(ModelError::invalid("tool arguments must be an object"));
            }
            calls.push(ModelToolCall {
                id: id.to_owned(),
                name: required_text(func, "name")?.to_owned(),
                arguments: args,
            });
        }
    }
    if (reason == "tool_calls" && calls.is_empty()) || (reason == "stop" && !calls.is_empty()) {
        return Err(ModelError::invalid("tool calls do not match finish_reason"));
    }
    if reason == "stop" && content.as_deref().unwrap_or_default().trim().is_empty() {
        return Err(ModelError::invalid("model returned an empty final answer"));
    }
    Ok(ModelReply {
        message: wire_message(message),
        content,
        tool_calls: calls,
        finish_reason: reason.to_owned(),
        usage: body["usage"].clone(),
        timing,
    })
}

pub struct SuggestionSample {
    pub text: Option<String>,
    pub usage: Value,
    pub timing: ModelTiming,
}

/// Caller supplies only the history visible to the requesting user and
/// rechecks the Session version, reply, permission and plugin before delivery.
pub async fn suggest(
    http: &Client,
    route: &ModelRoute,
    visible_history: &[Value],
    prompt: &str,
) -> Result<Option<SuggestionSample>, ModelError> {
    if prompt.trim().is_empty() {
        return Ok(None);
    }
    let visible: Vec<_> = visible_history
        .iter()
        .map(|message| json!({"role": message["role"], "content": message["content"]}))
        .collect();
    let request = [
        json!({"role":"system","content":prompt}),
        json!({"role":"user","content":serde_json::to_string(&visible).unwrap()}),
    ];
    let reply = complete(http, route, &request, &[], None).await?;
    let text = reply
        .content
        .filter(|_| reply.finish_reason == "stop" && reply.tool_calls.is_empty())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty() && s != "NONE" && !s.contains('\n') && s.chars().count() <= 400);
    Ok(Some(SuggestionSample {
        text,
        usage: reply.usage,
        timing: reply.timing,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_tool_pair_and_provider_continuation() {
        let message = json!({"role":"assistant","content":null,"reasoning_content":"provider replay data","tool_calls":[{"type":"function","id":"abc","function":{"name":"get","arguments":"{\"x\":2}"}}]});
        let mut payload = json!({"choices":[{"message":message,"finish_reason":"tool_calls"}]});
        let timing = ModelTiming {
            request_started_us: 1,
            first_content_us: None,
            first_reasoning_us: None,
            response_completed_us: None,
            first_output_us: None,
            generation_ended_us: None,
        };
        let r = parse_reply(&payload, timing).unwrap();
        assert_eq!(r.tool_calls[0].arguments["x"], 2);
        assert_eq!(r.message["reasoning_content"], "provider replay data");
        payload["choices"][0]["finish_reason"] = json!("stop");
        assert!(parse_reply(&payload, timing).is_err());
        let m = wire_message(
            &json!({"role":"user","content":"continue","source":"goal","secret":"no"}),
        );
        assert_eq!(m, json!({"role":"user","content":"continue"}));
    }

    #[tokio::test]
    async fn provider_receives_image_bytes_in_order_and_suggestion_cannot_dispatch() {
        use axum::{Json, Router, extract::State, routing::post};
        use std::sync::{Arc, Mutex};
        let log = Arc::new(Mutex::new(Vec::<Value>::new()));
        async fn handler(
            State(log): State<Arc<Mutex<Vec<Value>>>>,
            Json(payload): Json<Value>,
        ) -> Json<Value> {
            let suggest = payload["messages"][0]["content"]
                .as_str()
                .is_some_and(|v| v.contains("Suggest one"));
            log.lock().unwrap().push(payload);
            if suggest {
                return Json(
                    json!({"choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,"tool_calls":[{"type":"function","id":"c1","function":{"name":"push","arguments":"{}"}}]}}]}),
                );
            }
            Json(
                json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"ok"}}]}),
            )
        }
        let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        let server = tokio::spawn(
            axum::serve(
                socket,
                Router::new()
                    .route("/v1/chat/completions", post(handler))
                    .with_state(log.clone()),
            )
            .into_future(),
        );
        let route = ModelRoute {
            base_url: format!("http://{addr}/v1"),
            model: "fixture".into(),
            api_key_env: None,
        };
        let png=STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a25kAAAAASUVORK5CYII=").unwrap();
        let part = image_part("image/png", &png).unwrap();
        let messages = [
            json!({"role":"user","content":[{"type":"text","text":"before"},part,{"type":"text","text":"after"}]}),
        ];
        let client = Client::new();
        assert_eq!(
            complete(&client, &route, &messages, &[], None)
                .await
                .unwrap()
                .content
                .as_deref(),
            Some("ok")
        );
        let visible =
            [json!({"role":"assistant","content":"public","reasoning_content":"private-marker"})];
        assert!(
            suggest(
                &client,
                &route,
                &visible,
                "Suggest one next message based on the transcript."
            )
            .await
            .unwrap()
            .and_then(|sample| sample.text)
            .is_none()
        );
        let calls = log.lock().unwrap();
        assert_eq!(calls[0]["stream"], true);
        assert_eq!(calls[0]["stream_options"]["include_usage"], true);
        let parts = &calls[0]["messages"][0]["content"];
        assert_eq!(parts[0]["text"], "before");
        assert_eq!(parts[2]["text"], "after");
        let b64 = parts[1]["image_url"]["url"]
            .as_str()
            .unwrap()
            .strip_prefix("data:image/png;base64,")
            .unwrap();
        assert_eq!(STANDARD.decode(b64).unwrap(), png);
        assert!(calls[1].get("tools").is_none());
        assert!(!calls[1].to_string().contains("private-marker"));
        assert!(image_part("image/jpeg", &png).is_err());
        server.abort();
    }
}
