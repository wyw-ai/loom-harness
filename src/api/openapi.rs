use serde_json::{Value, json};

const PUBLIC: &[(&str, &str)] = &[
    ("models", "get"),
    ("agent-templates", "get"),
    ("agents", "get post"),
    ("agents/{id}", "get patch"),
    ("agents/{id}/tools", "get"),
    ("sessions", "get post"),
    ("sessions/{id}", "get"),
    ("sessions/{id}/runs", "get"),
    ("sessions/{id}/context/items", "get"),
    ("sessions/{id}/context/previews", "post"),
    ("sessions/{id}/branches", "post"),
    ("sessions/{id}/suggestions", "post"),
    ("events", "get post"),
    ("events/{id}", "get"),
    ("events/{id}/deliveries", "get post"),
    ("event-deliveries/{id}", "get"),
    ("runs", "get"),
    ("runs/{id}", "get"),
    ("runs/{id}/steps", "get"),
    ("runs/{id}/tools", "get"),
    ("runs/{id}/model-calls", "get"),
    ("runs/{id}/tool-calls", "get"),
    ("model-calls/{id}", "get"),
    ("tool-calls/{id}", "get"),
    ("tool-calls/{id}/results/{part}/content", "get"),
    ("attachments", "post"),
    ("attachments/{id}", "delete"),
    ("attachments/{id}/content", "get"),
    ("dispatches", "get post"),
    ("dispatches/{id}", "get patch"),
    ("dispatches/{id}/previews", "post"),
    ("dispatches/{id}/evaluations", "get"),
    ("dispatches/{id}/deliveries", "get"),
    ("replies/{id}", "get"),
    (
        "plugins/{instance}/subscriptions/{subscription}/events/{event}",
        "get",
    ),
    ("docs", "get"),
    ("docs/{reference}", "get"),
    ("openapi.json", "get"),
];
const ADMIN: &[(&str, &str)] = &[
    ("plugins", "get post"),
    ("plugins/{instance}", "get patch"),
    ("plugins/{instance}/versions", "get post"),
    ("plugins/{instance}/versions/{version}", "get"),
    ("settings/{instance}", "get patch"),
    ("settings/{instance}/schema", "get"),
    ("catalog/apply", "post"),
    ("catalog/diagnostics", "get"),
    ("catalog/tools", "get"),
    ("catalog/tools/{tool}/versions/{revision}", "get"),
    ("mcp/connections", "get post"),
    ("mcp/connections/{instance}/{connection}", "get patch"),
    ("mcp/connections/{instance}/{connection}/refresh", "post"),
    ("mcp/connections/{instance}/{connection}/discovery", "get"),
];
pub(super) fn document(mut paths: Value, who: &crate::config::Identity) -> Value {
    let mut routes = PUBLIC.to_vec();
    if who.admin {
        routes.extend_from_slice(ADMIN)
    }
    if who.approve {
        routes.push(("tool-calls/{id}/approval", "post"))
    }
    if who.source {
        routes.push(("tool-calls/{id}/results", "post"))
    }
    for (path, methods) in routes {
        let entry = &mut paths[format!("/api/{path}")];
        let parameters = path
            .split('/')
            .filter_map(|s| s.strip_prefix('{')?.strip_suffix('}'))
            .map(|s| json!({"name":s,"in":"path","required":true,"schema":{"type":"string"}}))
            .collect::<Vec<_>>();
        if !parameters.is_empty() {
            entry["parameters"] = json!(parameters);
        }
        for method in methods.split_whitespace() {
            let status = if path == "events/{id}/deliveries" && method == "post" {
                "202"
            } else {
                "200"
            };
            let mut op = json!({"responses":{status:{"description":"Success","content":{"application/json":{"schema":{}}}}}});
            if matches!(method, "post" | "patch") {
                op["requestBody"] =
                    json!({"content":{"application/json":{"schema":{"type":"object"}}}});
            }
            entry[method] = op;
        }
    }
    json!({"openapi":"3.1.0","info":{"title":"Loom Harness","version":env!("CARGO_PKG_VERSION")},"security":[{"bearer":[]}],"components":{"securitySchemes":{"bearer":{"type":"http","scheme":"bearer"}}},"paths":paths})
}
