//! Core of the local proxy for GitHub Copilot CLI.
//!
//! This crate is independent of Tauri/GUI — it contains all the request
//! forwarding logic (model swap, API key injection, response streaming),
//! so it can be fully tested on any platform.

mod config;
mod proxy;
mod state;

pub use config::{Config, ConfigError};
pub use proxy::build_router;
pub use state::{AppState, RequestLog};

use std::sync::Arc;

/// Runs the proxy server on the address from the configuration. Blocks until it ends.
pub async fn serve(state: Arc<AppState>) -> std::io::Result<()> {
    let addr = state.config.listen_addr.clone();
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("proxy listening on http://{addr}");
    let router = build_router(state);
    axum::serve(listener, router).await
}
