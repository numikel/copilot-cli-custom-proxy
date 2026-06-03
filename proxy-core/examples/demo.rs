//! Uruchamialny pokaz działania proxy bez GUI.
//!
//! Startuje atrapę upstreamu (odsyła to, co zobaczyła), uruchamia proxy
//! z modelem "model-b" i kluczem "DEMO-KEY", po czym wysyła żądanie z
//! `model: "gpt-original"` i wypisuje, co dotarło na upstream.
//!
//! Uruchom: `cargo run -p proxy-core --example demo`

use axum::{http::HeaderMap, routing::post, Json, Router};
use proxy_core::{build_router, AppState, Config};
use serde_json::json;
use std::sync::Arc;

async fn echo(headers: HeaderMap, body: axum::body::Bytes) -> Json<serde_json::Value> {
    let parsed: serde_json::Value =
        serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    Json(json!({
        "model_widziany_przez_endpoint": parsed.get("model"),
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
    // 1) Atrapa korporacyjnego endpointu.
    let upstream = spawn(Router::new().route("/chat/completions", post(echo))).await;
    println!("Atrapa endpointu (CORPORATE_URL): {upstream}");

    // 2) Proxy z modelem i kluczem ustawionymi tak, jak zrobiłoby to UI.
    let config = Config::from_str(&format!(
        r#"
listen_addr = "127.0.0.1:0"
corporate_base_url = "{upstream}"
default_model = "model-a"
models = ["model-a", "model-b"]
"#
    ))
    .unwrap();
    let state = Arc::new(AppState::new(config));
    state.set_api_key("DEMO-KEY");
    state.set_selected_model("model-b");
    let proxy = spawn(build_router(state)).await;
    println!("Proxy (COPILOT_PROVIDER_BASE_URL): {proxy}");

    // 3) Żądanie jak z Copilot CLI — model "gpt-original".
    println!("\n--> Wysyłam do proxy: {{ \"model\": \"gpt-original\", ... }}");
    let resp = reqwest::Client::new()
        .post(format!("{proxy}/chat/completions"))
        .json(&json!({ "model": "gpt-original", "messages": [] }))
        .send()
        .await
        .unwrap();

    let seen: serde_json::Value = resp.json().await.unwrap();
    println!("\n<-- Co naprawdę dotarło na endpoint:");
    println!("{}", serde_json::to_string_pretty(&seen).unwrap());
}
