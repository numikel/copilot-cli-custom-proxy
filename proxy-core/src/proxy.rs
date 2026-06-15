use crate::models::{classify_model, ModelInfo};
use crate::settings::RuntimeConfig;
use crate::state::AppState;
use axum::{
    body::{Body, Bytes},
    extract::{ConnectInfo, Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::any,
    Router,
};
use secrecy::{ExposeSecret, SecretString};
use std::net::{IpAddr, SocketAddr};
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
    Router::new().fallback(any(proxy_handler)).with_state(state)
}

/// Whether a client peer may use the proxy. Loopback peers always may (the
/// local CLI launcher and same-machine tools). A non-loopback peer — only
/// reachable when the user opted into network exposure — must present the
/// gateway token as `Authorization: Bearer <token>`. Fails closed: no token
/// configured ⇒ no non-loopback client is allowed.
pub(crate) fn peer_is_authorized(
    peer: IpAddr,
    provided: Option<&str>,
    token: Option<&str>,
) -> bool {
    if peer.is_loopback() {
        return true;
    }
    match token {
        Some(t) if !t.is_empty() => provided == Some(format!("Bearer {t}").as_str()),
        _ => false,
    }
}

/// Axum middleware enforcing [`peer_is_authorized`] before a request reaches the
/// proxy handler. The client's `Authorization` header carries the gateway token
/// (it is replaced with the upstream key downstream, so reusing the header is
/// safe). Added only on the production serve path, where the listener is wired
/// with `ConnectInfo<SocketAddr>`.
pub(crate) async fn gateway_auth(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
    next: Next,
) -> Response {
    let provided = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    let token = state.proxy_token();
    if peer_is_authorized(peer.ip(), provided, token.as_deref()) {
        next.run(req).await
    } else {
        error_response(
            StatusCode::UNAUTHORIZED,
            "Unauthorized: this proxy is exposed to the network and requires a \
             valid token in the Authorization header."
                .to_string(),
        )
    }
}

/// Fetches the available models from the configured endpoint's `/models`,
/// classifying each id. Thin wrapper over [`fetch_models_from`] that reads the
/// live endpoint URL and key from [`AppState`].
pub async fn fetch_models(state: &AppState) -> Result<Vec<ModelInfo>, String> {
    let api_key = state
        .api_key()
        .ok_or("API key is not set — enter it before fetching models")?;
    fetch_models_from(&state.http, &state.endpoint_url(), &api_key).await
}

/// Fetches the available models from `{base}/models` for an explicit endpoint
/// URL, without touching [`AppState`]. This lets a caller probe a *candidate*
/// endpoint before committing it, so the live config and model catalog can be
/// swapped atomically afterwards (avoids a window where the new URL is paired
/// with a stale/empty catalog). `endpoint_url` must include the API suffix.
pub async fn fetch_models_from(
    http: &reqwest::Client,
    endpoint_url: &str,
    api_key: &SecretString,
) -> Result<Vec<ModelInfo>, String> {
    let probe = RuntimeConfig {
        listen_addr: String::new(),
        endpoint_url: endpoint_url.to_string(),
        ..RuntimeConfig::default()
    };
    let url = probe
        .models_url()
        .ok_or("No endpoint configured — set the endpoint URL in the settings window first")?;
    let resp = http
        .get(&url)
        .header(
            header::AUTHORIZATION,
            format!("Bearer {}", api_key.expose_secret()),
        )
        .send()
        .await
        .map_err(|e| format!("failed to reach {url}: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        return Err(format!("endpoint returned {status} for {url}"));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("invalid JSON from {url}: {e}"))?;

    let models: Vec<ModelInfo> = json
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let id = m.get("id").and_then(|id| id.as_str())?;
                    let mut info = classify_model(id);
                    let (prompt, output) = extract_token_limits(m);
                    info.max_prompt_tokens = prompt;
                    info.max_output_tokens = output;
                    Some(info)
                })
                .collect()
        })
        .unwrap_or_default();

    if models.is_empty() {
        return Err(format!("no models returned by {url}"));
    }
    Ok(models)
}

/// Best-effort extraction of a model's advertised token limits from its
/// `/models` entry. Plain OpenAI `/models` reports none, but gateways differ
/// (OpenRouter: `context_length` + `top_provider.max_completion_tokens`;
/// LiteLLM: `max_input_tokens` / `max_output_tokens`; vLLM: `max_model_len`), so
/// several key names are probed in priority order — the first that holds a valid
/// unsigned integer wins. Returns `(max_prompt_tokens, max_output_tokens)`;
/// either is `None` when nothing usable is present.
fn extract_token_limits(model: &serde_json::Value) -> (Option<u32>, Option<u32>) {
    let first_u32 = |keys: &[&str]| -> Option<u32> {
        keys.iter().find_map(|k| {
            model
                .get(*k)
                .and_then(|v| v.as_u64())
                .and_then(|n| u32::try_from(n).ok())
        })
    };
    let prompt = first_u32(&[
        "max_prompt_tokens",
        "max_input_tokens",
        "context_length",
        "context_window",
        "max_model_len",
    ]);
    let output = first_u32(&["max_output_tokens", "max_completion_tokens", "max_tokens"]).or_else(
        || {
            // OpenRouter nests the completion cap under `top_provider`.
            model
                .get("top_provider")
                .and_then(|t| t.get("max_completion_tokens"))
                .and_then(|v| v.as_u64())
                .and_then(|n| u32::try_from(n).ok())
        },
    );
    (prompt, output)
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

    // Resolve the upstream base from the configured endpoint URL.
    let base =
        match state.base_url() {
            Some(b) => b,
            None => return error_response(
                StatusCode::BAD_GATEWAY,
                "No endpoint configured. Open the app's settings window and set the endpoint URL."
                    .to_string(),
            ),
        };

    // Replace the `model` field with the model selected in the tray.
    let model = state.selected_model();
    let outgoing_body = inject_model(&body_bytes, &model);

    let target_url = format!("{base}{path_and_query}");

    // Record + log so you can see exactly what the proxy forwards and to where.
    state.record_request(&model, path_and_query, &target_url);
    tracing::info!(
        method = %method,
        path = %path_and_query,
        model = %model,
        target = %target_url,
        "forwarding request"
    );

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
        Ok(resp) => {
            let status = resp.status();
            state.record_status(status.as_u16());
            tracing::info!(status = %status, model = %model, "upstream responded");
            build_streaming_response(resp)
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
    const LAN: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50));

    #[test]
    fn loopback_peer_never_needs_a_token() {
        assert!(peer_is_authorized(LOOPBACK, None, None));
        assert!(peer_is_authorized(LOOPBACK, None, Some("secret")));
        assert!(peer_is_authorized(
            IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
            None,
            None
        ));
    }

    #[test]
    fn non_loopback_peer_requires_the_matching_token() {
        // No token configured → fail closed.
        assert!(!peer_is_authorized(LAN, Some("Bearer x"), None));
        assert!(!peer_is_authorized(LAN, Some("Bearer x"), Some("")));
        // Missing / wrong header → rejected.
        assert!(!peer_is_authorized(LAN, None, Some("secret")));
        assert!(!peer_is_authorized(LAN, Some("secret"), Some("secret"))); // no "Bearer " prefix
        assert!(!peer_is_authorized(
            LAN,
            Some("Bearer nope"),
            Some("secret")
        ));
        // Correct token → allowed.
        assert!(peer_is_authorized(
            LAN,
            Some("Bearer secret"),
            Some("secret")
        ));
    }

    #[test]
    fn extract_token_limits_reads_openrouter_shape() {
        let m = serde_json::json!({
            "id": "x",
            "context_length": 200000,
            "top_provider": { "max_completion_tokens": 8192 }
        });
        assert_eq!(extract_token_limits(&m), (Some(200_000), Some(8192)));
    }

    #[test]
    fn extract_token_limits_reads_litellm_shape() {
        let m = serde_json::json!({
            "id": "x",
            "max_input_tokens": 128000,
            "max_output_tokens": 16384
        });
        assert_eq!(extract_token_limits(&m), (Some(128_000), Some(16384)));
    }

    #[test]
    fn extract_token_limits_skips_unusable_value_for_a_later_key() {
        // context_length is present but null → fall through to context_window.
        let m = serde_json::json!({
            "id": "x",
            "context_length": null,
            "context_window": 32000
        });
        assert_eq!(extract_token_limits(&m).0, Some(32000));
    }

    #[test]
    fn extract_token_limits_absent_when_bare_id() {
        let m = serde_json::json!({ "id": "x" });
        assert_eq!(extract_token_limits(&m), (None, None));
    }
}
