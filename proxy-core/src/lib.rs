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
pub use proxy::{build_router, fetch_models, fetch_models_from};
pub use settings::{
    validate_endpoint_url, validate_listen_addr, ApiKind, RuntimeConfig, DEFAULT_LISTEN_ADDR,
};
pub use state::{AppState, RequestLog};
pub use ui_state::UiStateFile;

use std::future::Future;
use std::sync::Arc;

/// Runs the proxy server on the address from the current runtime config, binding
/// the listener itself. Convenience wrapper over [`serve_with`] that never
/// shuts down on its own (the future stays pending) — used where the caller
/// does not manage a separate shutdown signal.
pub async fn serve(state: Arc<AppState>) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(&state.listen_addr()).await?;
    serve_with(listener, state, std::future::pending()).await
}

/// Runs the proxy server on an already-bound listener, shutting down gracefully
/// when `shutdown` resolves. Binding outside this function lets the host
/// surface bind errors (e.g. address in use) before tearing down a running
/// server, and the graceful shutdown lets the old server release its port
/// before a replacement is spawned.
pub async fn serve_with(
    listener: tokio::net::TcpListener,
    state: Arc<AppState>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    warn_on_insecure_config(&state);
    if let Ok(addr) = listener.local_addr() {
        tracing::info!("proxy listening on http://{addr}");
    }
    let router = build_router(state);
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await
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
