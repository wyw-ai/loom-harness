use super::*;
use axum::{
    http::{HeaderValue, header},
    response::Redirect,
};

const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data: blob:; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

pub(super) async fn serve(State(app): State<App>, OriginalUri(uri): OriginalUri) -> Response {
    let path = uri.path();
    let asset = match path {
        "/webui" | "/webui/" => "index.html",
        _ => path.strip_prefix("/webui/").unwrap_or(""),
    };
    let mut response = match app.catalog.webui_asset(asset) {
        Ok(Some(_)) if path == "/webui" => Redirect::temporary("/webui/").into_response(),
        Ok(Some((content_type, body))) => (
            [
                (header::CONTENT_TYPE, content_type),
                (header::CONTENT_SECURITY_POLICY, CSP),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
                (header::REFERRER_POLICY, "no-referrer"),
            ],
            body,
        )
            .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) if e.starts_with("not found:") => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
