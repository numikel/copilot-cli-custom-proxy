use crate::config::Config;
use secrecy::SecretString;
use std::sync::Mutex;

/// State shared between the proxy server and the UI (tray / settings window).
///
/// Held as `Arc<AppState>`: one clone goes to the Axum router, another to
/// Tauri's state manager, so a model or key change in the UI is immediately
/// visible to the proxy.
pub struct AppState {
    pub config: Config,
    /// Currently selected model (substituted into the `model` field of requests).
    selected_model: Mutex<String>,
    /// API key — in memory only, wrapped in `SecretString`
    /// (zeroized on drop, redacted in logs).
    api_key: Mutex<Option<SecretString>>,
    /// Shared HTTP client used to forward requests upstream.
    pub http: reqwest::Client,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        let selected_model = config.default_model.clone();
        Self {
            config,
            selected_model: Mutex::new(selected_model),
            api_key: Mutex::new(None),
            http: reqwest::Client::new(),
        }
    }

    pub fn selected_model(&self) -> String {
        self.selected_model.lock().unwrap().clone()
    }

    /// Sets the active model if it is present in the configured list.
    /// Returns `false` when the model is unknown.
    pub fn set_selected_model(&self, model: impl Into<String>) -> bool {
        let model = model.into();
        if !self.config.models.iter().any(|m| m == &model) {
            return false;
        }
        *self.selected_model.lock().unwrap() = model;
        true
    }

    /// Stores the API key in memory. An empty string clears the key.
    pub fn set_api_key(&self, key: impl Into<String>) {
        let key = key.into();
        let mut guard = self.api_key.lock().unwrap();
        *guard = if key.is_empty() {
            None
        } else {
            Some(SecretString::from(key))
        };
    }

    pub fn has_api_key(&self) -> bool {
        self.api_key.lock().unwrap().is_some()
    }

    /// Clones the API key for use in the `Authorization` header.
    /// Returns `None` when no key has been set.
    pub fn api_key(&self) -> Option<SecretString> {
        self.api_key.lock().unwrap().clone()
    }
}
