use crate::state::AppState;
use axum::{
    body::{Body, Bytes},
    extract::{Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
    Router,
};
use secrecy::ExposeSecret;
use std::sync::Arc;

/// Maximum buffered request body size (100 MB).
const MAX_BODY_BYTES: usize = 100 * 1024 * 1024;

/// Hop-by-hop headers plus the ones we set ourselves — never forwarded.
const SKIPPED_REQUEST_HEADERS: &[&str] = &[
    "host",
    "content-length",
    "authorization",
    "connection",
    "transfer-encoding",
    "proxy-connection",
    "keep-alive",
];

/// Response headers set by the transport layer — not copied back.
const SKIPPED_RESPONSE_HEADERS: &[&str] = &[
    "content-length",
    "transfer-encoding",
    "connection",
    "keep-alive",
];

/// Builds the Axum router: any path/method goes to the transparent proxy.
pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .fallback(any(proxy_handler))
        .with_state(state)
}

async fn proxy_handler(State(state): State<Arc<AppState>>, req: Request) -> Response {
    let method = req.method().clone();
    let uri = req.uri().clone();
    let req_headers = req.headers().clone();

    let path_and_query = uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or_else(|| uri.path());

    let body_bytes = match axum::body::to_bytes(req.into_body(), MAX_BODY_BYTES).await {
        Ok(b) => b,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                format!("failed to read request body: {e}"),
            )
        }
    };

    // The API key must be set in the UI — kept in memory only.
    let api_key = match state.api_key() {
        Some(k) => k,
        None => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                "API key is not set. Open the app's settings window and enter your key."
                    .to_string(),
            )
        }
    };

    let auth_value = match HeaderValue::from_str(&format!("Bearer {}", api_key.expose_secret())) {
        Ok(v) => v,
        Err(_) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                "API key contains invalid characters.".to_string(),
            )
        }
    };

    // Replace the `model` field with the model selected in the tray.
    let outgoing_body = inject_model(&body_bytes, &state.selected_model());

    let target_url = format!("{}{}", state.config.base_url_trimmed(), path_and_query);

    // Forward the original headers except the skipped ones; override Authorization.
    let mut forward_headers = HeaderMap::new();
    for (name, value) in req_headers.iter() {
        if SKIPPED_REQUEST_HEADERS.contains(&name.as_str()) {
            continue;
        }
        forward_headers.insert(name.clone(), value.clone());
    }
    forward_headers.insert(header::AUTHORIZATION, auth_value);

    let upstream = state
        .http
        .request(method, &target_url)
        .headers(forward_headers)
        .body(outgoing_body)
        .send()
        .await;

    match upstream {
        Ok(resp) => build_streaming_response(resp),
        Err(e) => error_response(
            StatusCode::BAD_GATEWAY,
            format!("failed to reach the endpoint: {e}"),
        ),
    }
}

/// Replaces the `model` field in a JSON body if present. Otherwise (empty body,
/// non-JSON, or no `model` field) passes the body through unchanged.
fn inject_model(body: &[u8], model: &str) -> Bytes {
    if body.is_empty() {
        return Bytes::new();
    }
    match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(mut value) => {
            if let Some(obj) = value.as_object_mut() {
                if obj.contains_key("model") {
                    obj.insert(
                        "model".to_string(),
                        serde_json::Value::String(model.to_string()),
                    );
                    if let Ok(serialized) = serde_json::to_vec(&value) {
                        return Bytes::from(serialized);
                    }
                }
            }
            Bytes::copy_from_slice(body)
        }
        Err(_) => Bytes::copy_from_slice(body),
    }
}

/// Pipes the upstream response back to the client as a stream (SSE/streaming
/// support), preserving the status and headers except the transport-layer ones.
fn build_streaming_response(resp: reqwest::Response) -> Response {
    let status = resp.status();
    let headers = resp.headers().clone();

    let body = Body::from_stream(resp.bytes_stream());
    let mut response = Response::new(body);
    *response.status_mut() = status;

    let out_headers = response.headers_mut();
    for (name, value) in headers.iter() {
        if SKIPPED_RESPONSE_HEADERS.contains(&name.as_str()) {
            continue;
        }
        out_headers.insert(name.clone(), value.clone());
    }

    response
}

fn error_response(status: StatusCode, message: String) -> Response {
    tracing::warn!(%status, "proxy error: {message}");
    let body = serde_json::json!({
        "error": {
            "message": message,
            "type": "proxy_error",
        }
    });
    let mut response = (status, axum::Json(body)).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}
