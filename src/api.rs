use crate::{App, config::Identity, context, store::StoreError};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, OriginalUri, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::collections::HashMap;
mod openapi;
mod stream;

type ApiResult = Result<Value, ApiError>;
struct ApiError(StatusCode, String);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":self.1}))).into_response()
    }
}
impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> Self {
        let status = match e {
            StoreError::NotFound => StatusCode::NOT_FOUND,
            StoreError::Conflict(_) | StoreError::StaleLease => StatusCode::CONFLICT,
            StoreError::Invalid(_) => StatusCode::BAD_REQUEST,
            StoreError::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self(status, e.to_string())
    }
}
fn bad(msg: impl Into<String>) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, msg.into())
}
fn deny() -> ApiError {
    ApiError(StatusCode::FORBIDDEN, "permission denied".into())
}
fn catalog_err(s: String) -> ApiError {
    let v = s.to_lowercase();
    let code = if v.contains("forbidden") || v.contains("permission") || v.contains("authorized") {
        StatusCode::FORBIDDEN
    } else if v.contains("not found") || v.contains("not_found") {
        StatusCode::NOT_FOUND
    } else if v.contains("conflict") || v.contains("already exists") {
        StatusCode::CONFLICT
    } else {
        StatusCode::BAD_REQUEST
    };
    ApiError(code, s)
}
pub fn router(app: App) -> Router {
    Router::new()
        .fallback(handle)
        .layer(DefaultBodyLimit::max(12 * 1024 * 1024))
        .with_state(app)
}
async fn handle(
    State(app): State<App>,
    OriginalUri(uri): OriginalUri,
    method: Method,
    headers: HeaderMap,
    bytes: Bytes,
) -> Result<Response, ApiError> {
    let path = uri.path();
    if path == "/health" && method == Method::GET {
        return Ok(Json(json!({"status":"ok"})).into_response());
    }
    let token = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "));
    let who = token.and_then(|s| app.cfg.tokens.get(s)).ok_or_else(|| {
        ApiError(
            StatusCode::UNAUTHORIZED,
            "bearer credential required".into(),
        )
    })?;
    let body = if bytes.is_empty() {
        json!({})
    } else {
        serde_json::from_slice::<Value>(&bytes).map_err(|_| bad("invalid JSON body"))?
    };
    if !body.is_object() {
        return Err(bad("expected JSON object"));
    }
    let path = path
        .strip_prefix("/api/")
        .ok_or(ApiError(StatusCode::NOT_FOUND, "unknown route".into()))?;
    let parts = path.trim_end_matches('/').split('/').collect::<Vec<_>>();
    let q: HashMap<String, String> = serde_urlencoded::from_str(uri.query().unwrap_or(""))
        .map_err(|_| bad("invalid query parameters"))?;
    let key = headers.get("idempotency-key").and_then(|h| h.to_str().ok());
    if method == Method::GET
        && let ["runs", run, "stream"] = parts.as_slice()
    {
        return stream::open(&app, who, run, &headers, q.get("after"));
    }
    let response = route(&app, who, method.as_str(), &parts, &q, key, body).await?;
    let status =
        if method == Method::POST && matches!(parts.as_slice(), ["events", _, "deliveries"]) {
            StatusCode::ACCEPTED
        } else {
            StatusCode::OK
        };
    Ok((status, Json(response)).into_response())
}
async fn route(
    app: &App,
    who: &Identity,
    method: &str,
    p: &[&str],
    q: &HashMap<String, String>,
    key: Option<&str>,
    mut body: Value,
) -> ApiResult {
    let o = who.owner.as_str();
    let db = &app.db;
    if matches!(p.first(), Some(&"dispatches") | Some(&"replies"))
        && let Some(v) = crate::dispatch::routes(app, who, method, p, body.clone(), key).await?
    {
        return Ok(v);
    }
    match (method, p) {
        ("GET", ["openapi.json"]) => Ok(openapi::document(
            app.catalog
                .plugin_routes(o, who.admin)
                .map_err(catalog_err)?,
            who,
        )),
        ("GET", ["plugins", instance, "subscriptions", sub, "events", event]) => {
            db.get(o, "event", event)?;
            let group = crate::bus::subscription_group(o, instance, sub);
            Ok(db
                .subscription_result(&group, event)?
                .unwrap_or(Value::Null))
        }
        ("GET", ["models"]) => Ok(json!(
            app.cfg
                .models
                .iter()
                .map(|(k, v)| json!({"id":k,"model":v.model,"images":v.images}))
                .collect::<Vec<_>>()
        )),
        ("GET", ["agent-templates"] | ["templates"]) => {
            app.catalog.templates_for(o, who.admin).map_err(catalog_err)
        }
        ("POST", ["attachments"]) => {
            crate::attachments::put(&app.cfg.home, o, &body).map_err(|e| bad(e.to_string()))
        }
        ("GET", ["attachments", id, "content"]) => crate::attachments::read(&app.cfg.home, o, id)
            .map_err(|_| ApiError(StatusCode::NOT_FOUND, "attachment not accessible".into())),
        ("DELETE", ["attachments", id]) => {
            crate::attachments::remove(&app.cfg.home, o, id)
                .map_err(|_| ApiError(StatusCode::NOT_FOUND, "attachment not accessible".into()))?;
            Ok(json!({"id":id,"deleted":true}))
        }
        ("GET", ["agents"]) => app.catalog.list_agents(o, who.admin).map_err(catalog_err),
        ("POST", ["agents"]) => app
            .catalog
            .create_agent(o, who.admin, body)
            .map_err(catalog_err),
        ("GET", ["agents", id]) => app.catalog.get_agent(o, who.admin, id).map_err(catalog_err),
        ("PATCH", ["agents", id]) => app
            .catalog
            .patch_agent(o, who.admin, id, body)
            .map_err(catalog_err),
        ("GET", ["agents", id, "tools"]) => {
            app.catalog.tools(o, who.admin, id).map_err(catalog_err)
        }
        ("POST", ["sessions"]) => {
            if let Some(a) = body["agent_id"].as_str() {
                app.catalog.get_agent(o, false, a).map_err(catalog_err)?;
            }
            Ok(db.create_session(o, body)?)
        }
        ("POST", ["events"]) => {
            if body.get("type").and_then(Value::as_str).is_some_and(|t| {
                t == "agent.system_prompt_updated"
                    || t.split('.').next().is_some_and(|s| {
                        matches!(
                            s,
                            "run" | "tool" | "model" | "delivery" | "goal" | "approval"
                        )
                    })
            }) || body["source"] == "loom"
            {
                return Err(bad("reserved internal event"));
            }
            if !who.source && !who.admin {
                if body["source"].as_str().is_some_and(|s| s != "user") {
                    return Err(deny());
                }
                body["source"] = json!("user");
            }
            if who.source
                && let Some(id) = who.source_id.as_deref()
            {
                if body["source"].as_str().is_some_and(|name| name != id) {
                    return Err(deny());
                }
                body["source"] = json!(id);
            }
            Ok(db.post_event(o, body, key)?)
        }
        ("POST", ["events", eid, "deliveries"]) => {
            body.as_object_mut().unwrap().remove("agent_snapshot");
            let session = body["target"]["session_id"]
                .as_str()
                .map(|s| db.get(o, "session", s))
                .transpose()?;
            let target = body["target"]["agent_id"]
                .as_str()
                .or_else(|| session.as_ref()?.get("agent_id")?.as_str())
                .or(body["agent_id"].as_str());
            if let Some(id) = target {
                let mut agent = app.catalog.get_agent(o, false, id).map_err(catalog_err)?;
                if body["action"] == "start_run" {
                    agent["_max_rounds"] = json!(app.cfg.max_rounds);
                    body["agent_snapshot"] = agent;
                }
            }
            if !body["goal"].is_null()
                && body["goal"]["objective"]
                    .as_str()
                    .filter(|s| !s.trim().is_empty())
                    .is_none()
            {
                return Err(bad("goal requires an objective"));
            }
            if let Some(url) = body["delivery_contract"]["target"].as_str()
                && !app.cfg.notification_targets.iter().any(|t| t == url)
            {
                return Err(deny());
            }
            let result = db.deliver(o, eid, body, key)?;
            app.ready.notify_one();
            Ok(result)
        }
        ("GET", ["events"] | ["runs"] | ["sessions"]) => {
            let mut v = match (p[0], q.get("system_prompt_event_id")) {
                ("runs", Some(id)) => db.runs_for_prompt(o, id)?,
                _ => db.list(o, p[0], None)?,
            };
            if let Some(items) = v.as_array_mut() {
                items.retain(|i| {
                    ["type", "source", "run_id", "session_id", "agent_id"]
                        .iter()
                        .all(|k| {
                            q.get(*k)
                                .is_none_or(|expected| i[*k].as_str() == Some(expected.as_str()))
                        })
                });
            }
            Ok(v)
        }
        (
            "GET",
            ["events", id]
            | ["runs", id]
            | ["sessions", id]
            | ["model-calls", id]
            | ["tool-calls", id]
            | ["event-deliveries", id],
        ) => {
            let mut item = db.get(o, p[0], id)?;
            if p[0] == "sessions" {
                item["version"] = json!(db.history(o, id)?.as_array().map(Vec::len));
            }
            Ok(item)
        }
        ("GET", ["sessions", s, "runs"]) => {
            db.get(o, "session", s)?;
            Ok(db.list(o, "run", Some(s))?)
        }
        ("GET", ["sessions", s, "context", "items"]) => Ok(db.history(o, s)?),
        ("POST", ["sessions", s, "branches"]) => {
            let at = body["at"]
                .as_u64()
                .or_else(|| body["at"].as_str()?.parse().ok())
                .ok_or_else(|| bad("at must be a history cursor"))?;
            Ok(db.branch(o, s, at as usize, body)?)
        }
        ("POST", ["sessions", s, "context", "previews"]) => {
            let session = db.get(o, "session", s)?;
            crate::engine::preview(
                app,
                o,
                s,
                session["agent_id"]
                    .as_str()
                    .ok_or_else(|| bad("session has no agent"))?,
            )
            .map_err(|e| bad(e.to_string()))
        }
        ("GET", ["runs", r, kind @ ("model-calls" | "tool-calls")]) => {
            db.get(o, "run", r)?;
            let mut entries = db.list(o, kind, Some(r))?;
            if *kind == "model-calls"
                && let Some(id) = q.get("system_prompt_event_id")
                && let Some(items) = entries.as_array_mut()
            {
                items.retain(|call| call["system_prompt_event_id"].as_str() == Some(id.as_str()));
            }
            Ok(entries)
        }
        ("GET", ["runs", r, "steps"]) => Ok(db.run_steps(o, r)?),
        ("GET", ["runs", r, "tools"]) => {
            let run = db.get(o, "run", r)?;
            app.catalog
                .tools(o, false, run["agent_id"].as_str().unwrap_or(""))
                .map_err(catalog_err)
        }
        ("GET", ["events", e, "deliveries"]) => {
            db.get(o, "event", e)?;
            Ok(db.list(o, "delivery", Some(e))?)
        }
        ("POST", ["tool-calls", cid, "results"]) => {
            if !who.source {
                return Err(deny());
            }
            let key = key.ok_or_else(|| bad("tool receipt requires Idempotency-Key"))?;
            let call = db.get(o, "tool_call", cid)?;
            let bound_source = call["directory"]
                .as_array()
                .and_then(|items| items.iter().find(|t| t["id"] == call["tool_id"]))
                .and_then(|tool| tool["receipt_source"].as_str());
            if bound_source != who.source_id.as_deref() {
                return Err(deny());
            }
            let tool = call["directory"]
                .as_array()
                .and_then(|a| a.iter().find(|t| t["id"] == call["tool_id"]));
            let dispatched = call["external_dispatch"].is_object()
                && tool.is_some_and(|t| t["source"] == "mcp" || t["effectful"] == true);
            if !dispatched
                && call["job_id"].is_null()
                && call.get("receipt_event_id").is_none()
                && !matches!(
                    call["status"].as_str(),
                    Some("accepted" | "progress" | "running")
                )
            {
                return Err(ApiError(
                    StatusCode::CONFLICT,
                    "tool has no external receipt pending".into(),
                ));
            }
            if call["job_id"]
                .as_str()
                .zip(body["job_id"].as_str())
                .is_some_and(|(a, b)| a != b)
            {
                return Err(ApiError(
                    StatusCode::CONFLICT,
                    "receipt refers to another job".into(),
                ));
            }
            if matches!(
                call["status"].as_str(),
                Some("awaiting_approval" | "pending_approval" | "awaiting_input")
            ) {
                return Err(ApiError(
                    StatusCode::CONFLICT,
                    "call is awaiting approval or user input".into(),
                ));
            }
            let terminal = matches!(
                body["status"].as_str().unwrap_or("completed"),
                "completed" | "success" | "succeeded" | "final" | "failed" | "error" | "cancelled"
            );
            let result_key = if body.get("result").is_some() {
                "result"
            } else {
                "output"
            };
            if terminal && let Some(result) = body.get_mut(result_key) {
                *result = crate::engine::store_parts(app, &call, result.take())
                    .map_err(|e| bad(e.to_string()))?;
            }
            let v = db.receive_tool_result(o, cid, body, key)?;
            app.ready.notify_one();
            Ok(v)
        }
        ("POST", ["tool-calls", cid, "approval"]) => {
            if !who.approve {
                return Err(deny());
            }
            let allow = body["allow"]
                .as_bool()
                .ok_or_else(|| bad("allow must be boolean"))?;
            let v = db.decide_approval(
                o,
                cid,
                o,
                allow,
                &body["arguments"],
                key.ok_or_else(|| bad("approval requires Idempotency-Key"))?,
            )?;
            app.ready.notify_one();
            Ok(v)
        }
        ("GET", ["tool-calls", cid, "results", part, "content"]) => {
            let index = part
                .parse::<usize>()
                .map_err(|_| bad("invalid result part"))?;
            let call = db.get(o, "tool_call", cid)?;
            let data = &call["result"];
            let data = if let Some(blocks) = data["content"].as_array() {
                blocks.get(index)
            } else if index == 0 {
                Some(data)
            } else {
                None
            }
            .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "unknown result part".into()))?;
            if let Some(reference) = data["note_ref"].as_str() {
                let read = app
                    .notes
                    .read(
                        call["session_id"].as_str().unwrap_or(""),
                        reference,
                        None,
                        None,
                        usize::MAX,
                    )
                    .map_err(|e| bad(e.to_string()))?;
                return Ok(json!(read));
            }
            Ok(data.clone())
        }
        ("POST", ["sessions", s, "suggestions"]) => suggestion(app, who, s, &body).await,
        _ => app
            .catalog
            .manage(o, who.admin, method, &format!("/api/{}", p.join("/")), body)
            .await
            .map_err(catalog_err),
    }
}
async fn suggestion(app: &App, who: &Identity, s: &str, body: &Value) -> ApiResult {
    let owner = who.owner.as_str();
    let subject = who.subject.as_deref().unwrap_or(owner);
    let history = app.db.history(owner, s)?;
    let items = history
        .as_array()
        .ok_or_else(|| bad("invalid session history"))?;
    let version = items.len();
    let empty = json!({"based_on_version":version,"suggestion":null});
    if body["expected_version"] != json!(version)
        || !items.last().is_some_and(|m| {
            m["role"] == "assistant"
                && !body["after_reply_id"].is_null()
                && m["run_id"] == body["after_reply_id"]
        })
    {
        return Ok(empty);
    }
    let Ok(Some(plugin)) = app.catalog.suggestion_prompt() else {
        return Ok(empty);
    };
    let Ok(access) = app.catalog.suggestion_access(owner, subject) else {
        return Ok(empty);
    };
    if access["enabled"] != true {
        return Ok(empty);
    }
    let Ok(_slot) = app.suggestion_slots.try_acquire() else {
        return Ok(empty);
    };
    let Some(model) = app
        .cfg
        .default_model
        .as_ref()
        .and_then(|k| app.cfg.models.get(k))
    else {
        return Ok(empty);
    };
    if !app.db.reserve_suggestion(
        owner,
        subject,
        access["max_samples_per_minute"].as_u64().unwrap_or(0),
    )? {
        return Ok(empty);
    }
    let visible = Value::Array(
        items
            .iter()
            .filter(|m| {
                (m["event"]["visibility"] != "agent" || access["read_private"] == true)
                    && m["role"] != "tool"
            })
            .cloned()
            .collect(),
    );
    let route = model.route();
    let messages = crate::engine::chat_history(&visible);
    let timeout =
        std::time::Duration::from_millis(access["timeout_ms"].as_u64().unwrap_or(15000).min(60000));
    let guess = tokio::time::timeout(
        timeout,
        context::suggest(
            &app.http,
            &route,
            &messages,
            plugin["prompt"].as_str().unwrap_or(""),
        ),
    )
    .await
    .ok()
    .and_then(Result::ok)
    .flatten();
    let fresh = app
        .db
        .history(owner, s)?
        .as_array()
        .is_some_and(|a| a.len() == version)
        && app
            .catalog
            .suggestion_prompt()
            .ok()
            .flatten()
            .is_some_and(|p| p["revision"] == plugin["revision"])
        && app.catalog.suggestion_access(owner, subject).ok().as_ref() == Some(&access);
    Ok(json!({"based_on_version":version,"suggestion":guess.filter(|_|fresh)}))
}
