use crate::models::ModelInfo;
use crate::settings::{ApiKind, RuntimeConfig};
use crate::ui_state::UiStateFile;
use secrecy::SecretString;
use serde::Serialize;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;

/// A snapshot of recent proxy traffic, surfaced in the UI so you can verify
/// what the proxy actually forwards (and to where).
#[derive(Clone, Default, Serialize)]
pub struct RequestLog {
    /// Number of requests forwarded since startup.
    pub count: u64,
    /// Model substituted into the most recent request.
    pub last_model: String,
    /// Path of the most recent request (e.g. "/chat/completions").
    pub last_path: String,
    /// Full upstream URL the most recent request was sent to.
    pub last_target: String,
    /// HTTP status returned by the upstream for the most recent request.
    pub last_status: Option<u16>,
}

/// State shared between the proxy server and the UI (tray / settings window).
///
/// Held as `Arc<AppState>`: one clone goes to the Axum router, another to
/// Tauri's state manager, so a model or key change in the UI is immediately
/// visible to the proxy.
pub struct AppState {
    /// Live runtime configuration (endpoint, listen address, default model),
    /// edited in the settings window and persisted to `config.json`.
    config: Mutex<RuntimeConfig>,
    /// Where `config.json` persists; `None` disables saving (e.g. in tests).
    /// Set by the host app via [`AppState::set_config_path`].
    config_path: Mutex<Option<PathBuf>>,
    /// Available models (fetched from the endpoint once an API key is set),
    /// each classified as chat / non-chat for the tray and settings UI.
    models: Mutex<Vec<ModelInfo>>,
    /// Currently selected model (substituted into the `model` field of requests).
    selected_model: Mutex<String>,
    /// API key — in memory only, wrapped in `SecretString`
    /// (zeroized on drop, redacted in logs).
    api_key: Mutex<Option<SecretString>>,
    /// Rolling record of forwarded requests for live verification in the UI.
    request_log: Mutex<RequestLog>,
    /// Models shown in the tray's "Models" submenu for the current endpoint.
    /// `None` means "not curated" → all chat models are shown by default.
    visible_models: Mutex<Option<Vec<String>>>,
    /// Where UI preferences persist (`ui_state.json`); `None` disables saving
    /// (e.g. in tests). Set by the host app via [`AppState::load_ui_state`].
    ui_state_path: Mutex<Option<PathBuf>>,
    /// Shared HTTP client used to forward requests upstream.
    pub http: reqwest::Client,
}

impl AppState {
    pub fn new(config: RuntimeConfig) -> Self {
        // Models are fetched from the endpoint once a key is set; start empty.
        let selected_model = config.default_model.clone().unwrap_or_default();
        Self {
            config: Mutex::new(config),
            config_path: Mutex::new(None),
            models: Mutex::new(Vec::new()),
            selected_model: Mutex::new(selected_model),
            api_key: Mutex::new(None),
            request_log: Mutex::new(RequestLog::default()),
            visible_models: Mutex::new(None),
            ui_state_path: Mutex::new(None),
            http: reqwest::Client::new(),
        }
    }

    // --- Runtime config accessors -------------------------------------------

    /// Local address the proxy listens on.
    pub fn listen_addr(&self) -> String {
        self.config.lock().unwrap().listen_addr.clone()
    }

    /// Full upstream endpoint URL (empty when unconfigured).
    pub fn endpoint_url(&self) -> String {
        self.config.lock().unwrap().endpoint_url.clone()
    }

    /// Endpoint base (URL minus the API suffix), ready for a path to be appended.
    pub fn base_url(&self) -> Option<String> {
        self.config.lock().unwrap().base_url()
    }

    /// URL of the model catalog (`{base}/models`).
    pub fn models_url(&self) -> Option<String> {
        self.config.lock().unwrap().models_url()
    }

    /// Wire API implied by the endpoint URL, or `None` when unconfigured.
    pub fn active_api(&self) -> Option<ApiKind> {
        self.config.lock().unwrap().active_api()
    }

    /// A snapshot clone of the runtime config (e.g. for persistence).
    pub fn runtime_config(&self) -> RuntimeConfig {
        self.config.lock().unwrap().clone()
    }

    /// Key under which this endpoint's UI preferences are stored: the endpoint
    /// base (stable across the chat/responses switch), or the raw URL as a
    /// fallback when the suffix is unrecognized.
    fn endpoint_key(&self) -> String {
        let cfg = self.config.lock().unwrap();
        cfg.base_url().unwrap_or_else(|| cfg.endpoint_url.clone())
    }

    /// Sets where `config.json` is persisted. Call once at startup.
    pub fn set_config_path(&self, path: PathBuf) {
        *self.config_path.lock().unwrap() = Some(path);
    }

    /// Persists the current runtime config to `config.json` (best-effort).
    fn persist_config(&self) {
        let path = self.config_path.lock().unwrap().clone();
        if let Some(path) = path {
            let cfg = self.config.lock().unwrap().clone();
            if let Err(e) = cfg.save(&path) {
                tracing::warn!("failed to persist config.json: {e}");
            }
        }
    }

    /// Replaces the endpoint URL, persists it, and reloads this endpoint's
    /// tray-visibility selection. The caller is responsible for clearing /
    /// refetching the model list (the catalog changes with the endpoint).
    pub fn set_endpoint(&self, url: String) {
        self.config.lock().unwrap().endpoint_url = url;
        self.persist_config();
        self.reload_visible_for_endpoint();
    }

    /// Replaces the listen address and persists it. The caller is responsible
    /// for restarting the proxy task so the new address takes effect.
    pub fn set_listen_addr(&self, addr: String) {
        self.config.lock().unwrap().listen_addr = addr;
        self.persist_config();
    }

    /// Points the app at `ui_state.json` and loads this endpoint's persisted
    /// tray-visibility selection. Call once at startup (no-op-safe to skip).
    pub fn load_ui_state(&self, path: PathBuf) {
        let key = self.endpoint_key();
        let file = UiStateFile::load(&path);
        *self.visible_models.lock().unwrap() = file.visible_models.get(&key).cloned();
        *self.ui_state_path.lock().unwrap() = Some(path);
    }

    /// Reloads the tray-visibility selection for the current endpoint from disk
    /// (used after the endpoint changes).
    fn reload_visible_for_endpoint(&self) {
        let path = self.ui_state_path.lock().unwrap().clone();
        if let Some(path) = path {
            let key = self.endpoint_key();
            let file = UiStateFile::load(&path);
            *self.visible_models.lock().unwrap() = file.visible_models.get(&key).cloned();
        }
    }

    /// Ids of the models shown in the tray submenu for the current endpoint:
    /// the curated set (intersected with the known chat models, preserving
    /// catalog order) or — when uncurated — every chat model.
    pub fn visible_model_ids(&self) -> Vec<String> {
        let models = self.models.lock().unwrap();
        let visible = self.visible_models.lock().unwrap();
        match &*visible {
            None => models.iter().filter(|m| m.chat).map(|m| m.id.clone()).collect(),
            Some(ids) => {
                let allow: HashSet<&str> = ids.iter().map(String::as_str).collect();
                models
                    .iter()
                    .filter(|m| m.chat && allow.contains(m.id.as_str()))
                    .map(|m| m.id.clone())
                    .collect()
            }
        }
    }

    /// Replaces the tray-visibility selection for the current endpoint and
    /// persists it (best-effort) to `ui_state.json`.
    pub fn set_visible_models(&self, ids: Vec<String>) {
        *self.visible_models.lock().unwrap() = Some(ids.clone());
        let path = self.ui_state_path.lock().unwrap().clone();
        if let Some(path) = path {
            let key = self.endpoint_key();
            let mut file = UiStateFile::load(&path);
            file.visible_models.insert(key, ids);
            if let Err(e) = file.save(&path) {
                tracing::warn!("failed to persist ui_state.json: {e}");
            }
        }
    }

    pub fn models(&self) -> Vec<ModelInfo> {
        self.models.lock().unwrap().clone()
    }

    /// Ids of every available model, in order (convenience for callers that only
    /// need identifiers, e.g. logging).
    pub fn model_ids(&self) -> Vec<String> {
        self.models
            .lock()
            .unwrap()
            .iter()
            .map(|m| m.id.clone())
            .collect()
    }

    /// Ids of the chat models only, in order — the subset the tray and agents
    /// care about.
    pub fn chat_model_ids(&self) -> Vec<String> {
        self.models
            .lock()
            .unwrap()
            .iter()
            .filter(|m| m.chat)
            .map(|m| m.id.clone())
            .collect()
    }

    /// Replaces the available model list (e.g. after fetching from the endpoint).
    /// Keeps the current selection if still present, otherwise picks the first.
    pub fn set_models(&self, models: Vec<ModelInfo>) {
        {
            let mut selected = self.selected_model.lock().unwrap();
            if !models.iter().any(|m| m.id == *selected) {
                *selected = models.first().map(|m| m.id.clone()).unwrap_or_default();
            }
        }
        *self.models.lock().unwrap() = models;
    }

    pub fn selected_model(&self) -> String {
        self.selected_model.lock().unwrap().clone()
    }

    /// Sets the active model if it is present in the available list.
    /// Returns `false` when the model is unknown.
    pub fn set_selected_model(&self, model: impl Into<String>) -> bool {
        let model = model.into();
        let known = self.models.lock().unwrap().iter().any(|m| m.id == model);
        if !known {
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

    /// Records an outgoing request (before the upstream status is known).
    pub fn record_request(&self, model: &str, path: &str, target: &str) {
        let mut log = self.request_log.lock().unwrap();
        log.count += 1;
        log.last_model = model.to_string();
        log.last_path = path.to_string();
        log.last_target = target.to_string();
        log.last_status = None;
    }

    /// Records the upstream HTTP status for the most recent request.
    pub fn record_status(&self, status: u16) {
        self.request_log.lock().unwrap().last_status = Some(status);
    }

    /// Returns a snapshot of recent traffic for display in the UI.
    pub fn request_log(&self) -> RequestLog {
        self.request_log.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::classify_model;

    fn state_with(models: &[&str]) -> AppState {
        let config = RuntimeConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            endpoint_url: "https://endpoint.example/v1/chat/completions".to_string(),
            default_model: None,
        };
        let state = AppState::new(config);
        state.set_models(models.iter().map(|m| classify_model(m)).collect());
        state
    }

    #[test]
    fn visible_defaults_to_all_chat_models_in_order() {
        let state = state_with(&["gpt-4o", "text-embedding-3-large", "claude-3"]);
        // Uncurated → every chat model, non-chat excluded, catalog order kept.
        assert_eq!(state.visible_model_ids(), vec!["gpt-4o", "claude-3"]);
    }

    #[test]
    fn set_visible_curates_and_intersects_chat() {
        let state = state_with(&["gpt-4o", "claude-3", "whisper-1"]);
        // whisper-1 is non-chat → dropped; "ghost" is unknown → dropped.
        state.set_visible_models(vec!["claude-3".into(), "whisper-1".into(), "ghost".into()]);
        assert_eq!(state.visible_model_ids(), vec!["claude-3"]);
    }

    #[test]
    fn curating_to_none_hides_every_model() {
        let state = state_with(&["gpt-4o", "claude-3"]);
        state.set_visible_models(vec![]);
        assert!(state.visible_model_ids().is_empty());
    }
}
