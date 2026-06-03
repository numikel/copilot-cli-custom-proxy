use axum::{
    body::{Body, Bytes},
    http::HeaderMap,
    response::Response,
    routing::{any, get, post},
    Json, Router,
};
use proxy_core::{build_router, AppState, Config};
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
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    Json(json!({
        "model": parsed.get("model"),
        "authorization": headers.get("authorization").and_then(|v| v.to_str().ok()),
        "x_test": headers.get("x-test").and_then(|v| v.to_str().ok()),
        "host": headers.get("host").and_then(|v| v.to_str().ok()),
    }))
}

fn config_for(upstream: &str) -> Config {
    Config::from_str(&format!(
        r#"
listen_addr = "127.0.0.1:0"
corporate_base_url = "{upstream}"
default_model = "model-a"
models = ["model-a", "model-b"]
"#
    ))
    .unwrap()
}

#[tokio::test]
async fn replaces_model_and_injects_auth() {
    let upstream = spawn(Router::new().route("/chat/completions", post(echo))).await;

    let state = Arc::new(AppState::new(config_for(&upstream)));
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

    // Config without a static `models` list — relies entirely on fetching.
    let config = Config::from_str(&format!(
        r#"
listen_addr = "127.0.0.1:0"
corporate_base_url = "{upstream}"
"#
    ))
    .unwrap();
    let state = AppState::new(config);
    state.set_api_key("k");

    let fetched = proxy_core::fetch_models(&state).await.unwrap();
    assert_eq!(fetched, vec!["alpha".to_string(), "beta".to_string()]);

    // set_models adopts the first model as the selection when none is set.
    state.set_models(fetched);
    assert_eq!(state.selected_model(), "alpha");
    assert!(state.set_selected_model("beta"));
    assert!(!state.set_selected_model("unknown"));
}

#[tokio::test]
async fn fetch_models_requires_api_key() {
    let upstream = spawn(Router::new()).await;
    let config = Config::from_str(&format!(
        r#"
listen_addr = "127.0.0.1:0"
corporate_base_url = "{upstream}"
"#
    ))
    .unwrap();
    let state = AppState::new(config);

    let err = proxy_core::fetch_models(&state).await.unwrap_err();
    assert!(err.contains("API key"));
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
