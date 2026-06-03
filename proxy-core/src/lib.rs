//! Rdzeń lokalnego proxy dla GitHub Copilot CLI.
//!
//! Crate jest niezależny od Tauri/GUI — zawiera całą logikę forwardowania
//! żądań (podmiana modelu, wstrzyknięcie klucza API, streaming odpowiedzi),
//! dzięki czemu da się go w pełni testować na dowolnej platformie.

mod config;
mod proxy;
mod state;

pub use config::{Config, ConfigError};
pub use proxy::build_router;
pub use state::AppState;

use std::sync::Arc;

/// Uruchamia serwer proxy na adresie z konfiguracji. Blokuje do zakończenia.
pub async fn serve(state: Arc<AppState>) -> std::io::Result<()> {
    let addr = state.config.listen_addr.clone();
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("proxy nasłuchuje na http://{addr}");
    let router = build_router(state);
    axum::serve(listener, router).await
}
