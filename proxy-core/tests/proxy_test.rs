use axum::{
    body::{Body, Bytes},
    http::HeaderMap,
    response::Response,
    routing::{any, get, post},
    Json, Router,
};
use proxy_core::{build_router, classify_model, AppState, ModelKind, RuntimeConfig};
use serde_json::json;
use std::sync::Arc;

/// Runs the given router on a random port and returns its base URL.
async fn spawn(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://{addr}")
}

/// Upstream stub: echoes back what it saw (model, headers).
async fn echo(headers: HeaderMap, body: Bytes) -> Json<serde_json::Value> {
    let parsed: serde_json::Value =
        serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    Json(json!({
        "model": parsed.get("model"),
        "authorization": headers.get("authorization").and_then(|v| v.to_str().ok()),
        "x_test": headers.get("x-test").and_then(|v| v.to_str().ok()),
        "host": headers.get("host").and_then(|v| v.to_str().ok()),
    }))
}

/// A runtime config whose endpoint points at the given upstream's
/// `/chat/completions` (so the derived base is exactly `upstream`).
fn config_for(upstream: &str) -> RuntimeConfig {
    RuntimeConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        endpoint_url: format!("{upstream}/chat/completions"),
        ..RuntimeConfig::default()
    }
}

/// A runtime config used by the model-fetch tests (base derived from the chat
/// endpoint, so `/models` resolves to `{upstream}/models`).
fn fetch_config_for(upstream: &str) -> RuntimeConfig {
    RuntimeConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        endpoint_url: format!("{upstream}/chat/completions"),
        ..RuntimeConfig::default()
    }
}

#[tokio::test]
async fn replaces_model_and_injects_auth() {
    let upstream = spawn(Router::new().route("/chat/completions", post(echo))).await;

    let state = Arc::new(AppState::new(config_for(&upstream)));
    state.set_models(vec![classify_model("model-a"), classify_model("model-b")]);
    state.set_api_key("test-key");
    assert!(state.set_selected_model("model-b"));

    let proxy = spawn(build_router(state)).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{proxy}/chat/completions"))
        .header("x-test", "hello")
        .json(&json!({ "model": "original", "messages": [] }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let v: serde_json::Value = resp.json().await.unwrap();

    // model replaced with the one selected in the tray
    assert_eq!(v["model"], "model-b");
    // Authorization overridden with the in-memory key
    assert_eq!(v["authorization"], "Bearer test-key");
    // original (non-filtered) headers are forwarded
    assert_eq!(v["x_test"], "hello");
    // Host points at the upstream, not the proxy (original Host not sent)
    let upstream_authority = upstream.trim_start_matches("http://");
    assert_eq!(v["host"], upstream_authority);
}

#[tokio::test]
async fn missing_api_key_returns_502() {
    let upstream = spawn(Router::new().route("/chat/completions", post(echo))).await;

    // API key intentionally not set
    let state = Arc::new(AppState::new(config_for(&upstream)));
    let proxy = spawn(build_router(state)).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{proxy}/chat/completions"))
        .json(&json!({ "model": "original", "messages": [] }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 502);
    let v: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(v["error"]["type"], "proxy_error");
}

#[tokio::test]
async fn forwards_non_json_body_unchanged() {
    async fn echo_raw(body: Bytes) -> Vec<u8> {
        body.to_vec()
    }
    let upstream = spawn(Router::new().fallback(any(echo_raw))).await;

    let state = Arc::new(AppState::new(config_for(&upstream)));
    state.set_api_key("k");
    let proxy = spawn(build_router(state)).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{proxy}/anything"))
        .body("not-json-payload")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "not-json-payload");
}

#[tokio::test]
async fn fetches_models_from_endpoint() {
    async fn models() -> Json<serde_json::Value> {
        Json(json!({ "object": "list", "data": [{ "id": "alpha" }, { "id": "beta" }] }))
    }
    let upstream = spawn(Router::new().route("/models", get(models))).await;

    let state = AppState::new(fetch_config_for(&upstream));
    state.set_api_key("k");

    let fetched = proxy_core::fetch_models(&state).await.unwrap();
    assert_eq!(fetched.len(), 2);
    assert_eq!(fetched[0].id, "alpha");
    assert!(fetched[0].chat);
    assert_eq!(fetched[1].id, "beta");
    assert!(fetched[1].chat);

    // set_models adopts the first model as the selection when none is set.
    state.set_models(fetched);
    assert_eq!(state.selected_model(), "alpha");
    assert!(state.set_selected_model("beta"));
    assert!(!state.set_selected_model("unknown"));
}

#[tokio::test]
async fn fetch_models_requires_api_key() {
    let upstream = spawn(Router::new()).await;
    let state = AppState::new(fetch_config_for(&upstream));

    let err = proxy_core::fetch_models(&state).await.unwrap_err();
    assert!(err.contains("API key"));
}

#[tokio::test]
async fn fetch_models_classifies_mixed_kinds() {
    // Upstream returns a mix: one chat model, one embedding model, one audio model.
    async fn models_mixed() -> Json<serde_json::Value> {
        Json(json!({
            "object": "list",
            "data": [
                { "id": "gpt-4o" },
                { "id": "text-embedding-3-large" },
                { "id": "whisper-1" },
            ]
        }))
    }
    let upstream = spawn(Router::new().route("/models", get(models_mixed))).await;

    let state = AppState::new(fetch_config_for(&upstream));
    state.set_api_key("k");

    let fetched = proxy_core::fetch_models(&state).await.unwrap();
    assert_eq!(fetched.len(), 3);

    // gpt-4o → chat model
    let gpt = fetched.iter().find(|m| m.id == "gpt-4o").unwrap();
    assert!(gpt.chat, "gpt-4o should be a chat model");
    assert_eq!(gpt.kind, None);

    // text-embedding-3-large → embed (non-chat)
    let emb = fetched
        .iter()
        .find(|m| m.id == "text-embedding-3-large")
        .unwrap();
    assert!(
        !emb.chat,
        "text-embedding-3-large should not be a chat model"
    );
    assert_eq!(emb.kind, Some(ModelKind::Embed));

    // whisper-1 → audio (non-chat)
    let aud = fetched.iter().find(|m| m.id == "whisper-1").unwrap();
    assert!(!aud.chat, "whisper-1 should not be a chat model");
    assert_eq!(aud.kind, Some(ModelKind::Audio));

    // Confirm helper: only gpt-4o in chat_model_ids after set_models.
    state.set_models(fetched);
    let chat_ids = state.chat_model_ids();
    assert_eq!(chat_ids, vec!["gpt-4o".to_string()]);
}

#[tokio::test]
async fn forwards_to_base_derived_from_responses_endpoint() {
    // Endpoint ends in /responses → base is the upstream root, and a request to
    // the proxy's /responses path forwards to {upstream}/responses.
    let upstream = spawn(Router::new().route("/responses", post(echo))).await;

    let config = RuntimeConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        endpoint_url: format!("{upstream}/responses"),
        ..RuntimeConfig::default()
    };
    let state = Arc::new(AppState::new(config));
    state.set_models(vec![classify_model("model-a")]);
    state.set_api_key("k");
    let proxy = spawn(build_router(state)).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{proxy}/responses"))
        .json(&json!({ "model": "original", "input": "hi" }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let v: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(v["model"], "model-a");
    assert_eq!(v["authorization"], "Bearer k");
}

#[tokio::test]
async fn unconfigured_endpoint_returns_502() {
    let state = Arc::new(AppState::new(RuntimeConfig::default()));
    state.set_api_key("k");
    let proxy = spawn(build_router(state)).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{proxy}/chat/completions"))
        .json(&json!({ "model": "x", "messages": [] }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 502);
    let v: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(v["error"]["type"], "proxy_error");
}

#[tokio::test]
async fn reconfigures_endpoint_and_forwards_to_new_upstream() {
    // Two independent upstreams; the echo reports the Host it saw, so we can tell
    // which one a request actually reached.
    let upstream_a = spawn(Router::new().route("/chat/completions", post(echo))).await;
    let upstream_b = spawn(Router::new().route("/chat/completions", post(echo))).await;

    let state = Arc::new(AppState::new(config_for(&upstream_a)));
    state.set_models(vec![classify_model("model-a"), classify_model("model-b")]);
    state.set_api_key("k");
    assert!(state.set_selected_model("model-b"));

    let proxy = spawn(build_router(state.clone())).await;
    let client = reqwest::Client::new();

    // Before reconfiguration: request hits upstream A.
    let resp = client
        .post(format!("{proxy}/chat/completions"))
        .json(&json!({ "model": "original", "messages": [] }))
        .send()
        .await
        .unwrap();
    let v: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(v["host"], upstream_a.trim_start_matches("http://"));
    assert_eq!(v["model"], "model-b");

    // Live swap to upstream B; model-b is still in the new catalog → selection
    // is preserved (never reset to an empty model).
    state.swap_endpoint(
        format!("{upstream_b}/chat/completions"),
        vec![classify_model("model-b"), classify_model("model-c")],
    );
    assert_eq!(state.selected_model(), "model-b");

    // After reconfiguration: the same proxy now forwards to upstream B.
    let resp = client
        .post(format!("{proxy}/chat/completions"))
        .json(&json!({ "model": "original", "messages": [] }))
        .send()
        .await
        .unwrap();
    let v: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(v["host"], upstream_b.trim_start_matches("http://"));
    assert_eq!(v["model"], "model-b");
    // Crucially, never an empty model string mid-swap.
    assert_ne!(v["model"], "");
}

#[tokio::test]
async fn serve_with_lets_loopback_through_without_a_token() {
    // The production serve path adds gateway auth + ConnectInfo. A loopback
    // client must still pass even when exposure + a token are configured.
    let upstream = spawn(Router::new().route("/chat/completions", post(echo))).await;

    let mut config = config_for(&upstream);
    config.expose_to_network = true;
    config.proxy_token = Some("super-secret".to_string());
    let state = Arc::new(AppState::new(config));
    state.set_models(vec![classify_model("model-a")]);
    state.set_api_key("k");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        proxy_core::serve_with(listener, state, std::future::pending())
            .await
            .unwrap();
    });
    let proxy = format!("http://{addr}");

    let client = reqwest::Client::new();
    // No Authorization header at all — loopback is exempt from the token.
    let resp = client
        .post(format!("{proxy}/chat/completions"))
        .json(&json!({ "model": "original", "messages": [] }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let v: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(v["model"], "model-a");
    // The upstream still receives the injected key, not the gateway token.
    assert_eq!(v["authorization"], "Bearer k");
}

#[tokio::test]
async fn streams_response_through() {
    async fn stream_sse() -> Response {
        let chunks: Vec<Result<Bytes, std::io::Error>> = vec![
            Ok(Bytes::from("data: chunk-1\n\n")),
            Ok(Bytes::from("data: chunk-2\n\n")),
            Ok(Bytes::from("data: [DONE]\n\n")),
        ];
        let stream = futures_util::stream::iter(chunks);
        Response::builder()
            .header("content-type", "text/event-stream")
            .body(Body::from_stream(stream))
            .unwrap()
    }

    let upstream = spawn(Router::new().route("/chat/completions", post(stream_sse))).await;

    let state = Arc::new(AppState::new(config_for(&upstream)));
    state.set_api_key("k");
    let proxy = spawn(build_router(state)).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{proxy}/chat/completions"))
        .json(&json!({ "model": "x", "messages": [] }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get("content-type").unwrap(),
        "text/event-stream"
    );
    let text = resp.text().await.unwrap();
    assert_eq!(text, "data: chunk-1\n\ndata: chunk-2\n\ndata: [DONE]\n\n");
}

#[tokio::test]
async fn messages_maps_proxy_cc_label_and_strips_v1() {
    use proxy_core::CcSlot;

    // Upstream exposes /v1/messages (Claude Code targets /v1/messages; the proxy
    // strips the duplicate /v1 because the base already ends in /v1).
    let upstream = spawn(Router::new().route("/v1/messages", post(echo))).await;

    let config = RuntimeConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        endpoint_url: format!("{upstream}/v1/messages"),
        ..RuntimeConfig::default()
    };
    let state = Arc::new(AppState::new(config));
    state.set_models(vec![classify_model("vendor/opus-x")]);
    state.set_api_key("k");
    state
        .set_cc_slot(CcSlot::Opus, Some("vendor/opus-x".into()), false)
        .unwrap();

    let proxy = spawn(build_router(state)).await;
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{proxy}/v1/messages"))
        .header("x-api-key", "client-secret")
        .json(&json!({ "model": "proxy-cc/opus", "messages": [] }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let v: serde_json::Value = resp.json().await.unwrap();
    // Label mapped to the configured catalog id.
    assert_eq!(v["model"], "vendor/opus-x");
    // Upstream auth is the injected key, not the client's x-api-key.
    assert_eq!(v["authorization"], "Bearer k");
    assert_eq!(v["host"], upstream.trim_start_matches("http://"));
}

#[tokio::test]
async fn messages_unconfigured_slot_returns_502() {
    let upstream = spawn(Router::new().route("/v1/messages", post(echo))).await;
    let config = RuntimeConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        endpoint_url: format!("{upstream}/v1/messages"),
        ..RuntimeConfig::default()
    };
    let state = Arc::new(AppState::new(config));
    state.set_api_key("k");

    let proxy = spawn(build_router(state)).await;
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{proxy}/v1/messages"))
        .json(&json!({ "model": "proxy-cc/opus", "messages": [] }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 502);
    let v: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(v["error"]["type"], "proxy_error");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Opus slot not configured")
    );
}
