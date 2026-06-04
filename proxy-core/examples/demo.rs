//! A runnable demonstration of the proxy without a GUI.
//!
//! Starts an upstream stub (echoes what it saw), runs the proxy with model
//! "model-b" and key "DEMO-KEY", then sends a request with `model:
//! "gpt-original"` and prints what arrived at the upstream.
//!
//! Run with: `cargo run -p proxy-core --example demo`

use axum::{http::HeaderMap, routing::post, Json, Router};
use proxy_core::{build_router, classify_model, AppState, RuntimeConfig};
use serde_json::json;
use std::sync::Arc;

async fn echo(headers: HeaderMap, body: axum::body::Bytes) -> Json<serde_json::Value> {
    let parsed: serde_json::Value =
        serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    Json(json!({
        "model_seen_by_endpoint": parsed.get("model"),
        "authorization": headers.get("authorization").and_then(|v| v.to_str().ok()),
    }))
}

async fn spawn(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://{addr}")
}

#[tokio::main]
async fn main() {
    // 1) Stub of the corporate endpoint.
    let upstream = spawn(Router::new().route("/chat/completions", post(echo))).await;
    println!("Endpoint stub (CORPORATE_URL): {upstream}");

    // 2) Proxy with the model and key set as the UI would set them.
    let config = RuntimeConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        endpoint_url: format!("{upstream}/chat/completions"),
        default_model: Some("model-a".to_string()),
    };
    let state = Arc::new(AppState::new(config));
    state.set_models(vec![classify_model("model-a"), classify_model("model-b")]);
    state.set_api_key("DEMO-KEY");
    state.set_selected_model("model-b");
    let proxy = spawn(build_router(state)).await;
    println!("Proxy (COPILOT_PROVIDER_BASE_URL): {proxy}");

    // 3) Request as if from Copilot CLI — model "gpt-original".
    println!("\n--> Sending to proxy: {{ \"model\": \"gpt-original\", ... }}");
    let resp = reqwest::Client::new()
        .post(format!("{proxy}/chat/completions"))
        .json(&json!({ "model": "gpt-original", "messages": [] }))
        .send()
        .await
        .unwrap();

    let seen: serde_json::Value = resp.json().await.unwrap();
    println!("\n<-- What actually arrived at the endpoint:");
    println!("{}", serde_json::to_string_pretty(&seen).unwrap());
}
