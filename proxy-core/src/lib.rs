//! Core of the local proxy for GitHub Copilot CLI.
//!
//! This crate is independent of Tauri/GUI — it contains all the request
//! forwarding logic (model swap, API key injection, response streaming),
//! so it can be fully tested on any platform.

mod config;
mod models;
mod proxy;
mod settings;
mod state;
mod ui_state;

pub use config::{Config, ConfigError};
pub use models::{classify_model, ModelInfo, ModelKind};
pub use proxy::{build_router, fetch_models};
pub use settings::{
    validate_endpoint_url, validate_listen_addr, ApiKind, RuntimeConfig, DEFAULT_LISTEN_ADDR,
};
pub use state::{AppState, RequestLog};
pub use ui_state::UiStateFile;

use std::sync::Arc;

/// Runs the proxy server on the address from the current runtime config. Blocks
/// until it ends (or is aborted by the host on a listen-address change).
pub async fn serve(state: Arc<AppState>) -> std::io::Result<()> {
    warn_on_insecure_config(&state);
    let addr = state.listen_addr();
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("proxy listening on http://{addr}");
    let router = build_router(state);
    axum::serve(listener, router).await
}

/// Logs warnings for configurations that could leak the API key. The proxy
/// injects the corporate key into every forwarded request, so binding beyond
/// loopback effectively shares that key with anything that can reach the port.
fn warn_on_insecure_config(state: &AppState) {
    let listen_addr = state.listen_addr();
    let host = listen_addr
        .rsplit_once(':')
        .map(|(h, _)| h)
        .unwrap_or(listen_addr.as_str());
    let loopback =
        host == "127.0.0.1" || host == "::1" || host.eq_ignore_ascii_case("localhost");
    if !loopback {
        tracing::warn!(
            addr = %listen_addr,
            "proxy is NOT bound to loopback — it will inject your API key for ANY client \
             that can reach this address; use 127.0.0.1 unless you really mean to expose it"
        );
    }
    let endpoint = state.endpoint_url();
    if !endpoint.is_empty() && !endpoint.starts_with("https://") {
        tracing::warn!(
            url = %endpoint,
            "endpoint URL is not HTTPS — the API key would be sent unencrypted"
        );
    }
}
