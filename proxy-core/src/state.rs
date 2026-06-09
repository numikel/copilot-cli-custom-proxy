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
    /// Live runtime configuration (endpoint, listen address, network exposure),
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
    /// Serializes the read-modify-write of `ui_state.json` so two concurrent
    /// `set_visible_models` calls can't lost-update each other's changes.
    ui_io: Mutex<()>,
    /// Shared HTTP client used to forward requests upstream.
    pub http: reqwest::Client,
}

impl AppState {
    pub fn new(config: RuntimeConfig) -> Self {
        // Models are fetched from the endpoint once a key is set; start empty.
        // The active model starts empty too and is restored from `ui_state.json`
        // (per-endpoint) the first time the catalog is loaded — see `set_models`.
        Self {
            config: Mutex::new(config),
            config_path: Mutex::new(None),
            models: Mutex::new(Vec::new()),
            selected_model: Mutex::new(String::new()),
            api_key: Mutex::new(None),
            request_log: Mutex::new(RequestLog::default()),
            visible_models: Mutex::new(None),
            ui_state_path: Mutex::new(None),
            ui_io: Mutex::new(()),
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

    /// Reads the active-model selection persisted for `key` in `ui_state.json`,
    /// if any. Best-effort and lock-free w.r.t. the state mutexes, so it is safe
    /// to call *before* entering a critical section (e.g. in `swap_endpoint`).
    fn persisted_selected(&self, key: &str) -> Option<String> {
        let path = self.ui_state_path.lock().unwrap().clone()?;
        UiStateFile::load(&path).selected_models.get(key).cloned()
    }

    /// Persists the active-model selection for the current endpoint to
    /// `ui_state.json` (best-effort), serialized under `ui_io` exactly like
    /// [`AppState::set_visible_models`] so concurrent writers can't lost-update.
    fn persist_selected_model(&self, model: &str) {
        let path = self.ui_state_path.lock().unwrap().clone();
        if let Some(path) = path {
            let _io = self.ui_io.lock().unwrap();
            let key = self.endpoint_key();
            let mut file = UiStateFile::load(&path);
            file.selected_models.insert(key, model.to_string());
            if let Err(e) = file.save(&path) {
                tracing::warn!("failed to persist ui_state.json: {e}");
            }
        }
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

    /// Atomically swaps the endpoint URL **and** its model catalog in a single
    /// critical section, keeping the current selection when it still exists in
    /// the new catalog. When it does not, the new endpoint's own persisted
    /// selection is restored if present, otherwise the first model (or empty).
    /// This closes the window the old `set_endpoint` + `set_models([])` sequence
    /// left open, in which a request could read the new URL paired with an empty
    /// model.
    ///
    /// The persisted selection is read **before** the critical section (it does
    /// disk I/O and must not run while the state locks are held). Lock order is
    /// `config < models < selected` (consistent with
    /// [`AppState::set_selected_model`]); `persist_config` and
    /// `reload_visible_for_endpoint` run *after* the `config` lock is released
    /// because they re-acquire it (std `Mutex` is not reentrant).
    pub fn swap_endpoint(&self, url: String, models: Vec<ModelInfo>) {
        let restored = self.persisted_selected(&key_for_endpoint(&url));
        {
            let mut config = self.config.lock().unwrap();
            let mut models_guard = self.models.lock().unwrap();
            let mut selected = self.selected_model.lock().unwrap();
            config.endpoint_url = url;
            if !models.iter().any(|m| m.id == *selected) {
                *selected = pick_selection(&models, restored.as_deref());
            }
            *models_guard = models;
        }
        self.persist_config();
        self.reload_visible_for_endpoint();
    }

    /// Replaces the listen address and persists it. The caller is responsible
    /// for restarting the proxy task so the new address takes effect.
    pub fn set_listen_addr(&self, addr: String) {
        self.config.lock().unwrap().listen_addr = addr;
        self.persist_config();
    }

    /// Whether the proxy may bind beyond loopback (the network-exposure opt-in).
    pub fn expose_to_network(&self) -> bool {
        self.config.lock().unwrap().expose_to_network
    }

    /// The gateway token non-loopback clients must present, if one is set.
    pub fn proxy_token(&self) -> Option<String> {
        self.config.lock().unwrap().proxy_token.clone()
    }

    /// Turns network exposure on or off. Enabling it mints a gateway token when
    /// none exists yet (so an exposed proxy is never tokenless); disabling it
    /// leaves the token in place so it survives a later re-enable. Persists.
    pub fn set_expose_to_network(&self, enabled: bool) {
        {
            let mut cfg = self.config.lock().unwrap();
            cfg.expose_to_network = enabled;
            if enabled && cfg.proxy_token.is_none() {
                cfg.proxy_token = Some(crate::settings::generate_proxy_token());
            }
        }
        self.persist_config();
    }

    /// Replaces the gateway token with a freshly generated one and persists it.
    /// Returns the new token so the UI can show it.
    pub fn regenerate_proxy_token(&self) -> String {
        let token = crate::settings::generate_proxy_token();
        self.config.lock().unwrap().proxy_token = Some(token.clone());
        self.persist_config();
        token
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
            None => models
                .iter()
                .filter(|m| m.chat)
                .map(|m| m.id.clone())
                .collect(),
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
    /// persists it (best-effort) to `ui_state.json`. The on-disk
    /// read-modify-write runs under `ui_io` so concurrent calls serialize
    /// rather than clobbering each other's entries.
    pub fn set_visible_models(&self, ids: Vec<String>) {
        *self.visible_models.lock().unwrap() = Some(ids.clone());
        let path = self.ui_state_path.lock().unwrap().clone();
        if let Some(path) = path {
            // Hold the I/O lock across the whole load→insert→save sequence.
            let _io = self.ui_io.lock().unwrap();
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
    /// Keeps the current selection if still present; otherwise restores this
    /// endpoint's persisted choice when it is in the new catalog, falling back
    /// to the first model. This is what re-applies the saved active model after
    /// a restart (the in-memory selection starts empty).
    pub fn set_models(&self, models: Vec<ModelInfo>) {
        let restored = self.persisted_selected(&self.endpoint_key());
        {
            let mut selected = self.selected_model.lock().unwrap();
            if !models.iter().any(|m| m.id == *selected) {
                *selected = pick_selection(&models, restored.as_deref());
            }
        }
        *self.models.lock().unwrap() = models;
    }

    pub fn selected_model(&self) -> String {
        self.selected_model.lock().unwrap().clone()
    }

    /// Sets the active model if it is present in the available list, persisting
    /// the choice per-endpoint to `ui_state.json` so it survives a restart.
    /// Returns `false` when the model is unknown (nothing is persisted then).
    pub fn set_selected_model(&self, model: impl Into<String>) -> bool {
        let model = model.into();
        let known = self.models.lock().unwrap().iter().any(|m| m.id == model);
        if !known {
            return false;
        }
        *self.selected_model.lock().unwrap() = model.clone();
        self.persist_selected_model(&model);
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

/// The `ui_state.json` key for a candidate endpoint URL — its base (suffix
/// stripped), or the raw URL when the suffix is unrecognized. Mirrors
/// [`AppState::endpoint_key`] for a URL not yet committed to the config, so the
/// new endpoint's persisted preferences can be read before the swap.
fn key_for_endpoint(url: &str) -> String {
    let probe = RuntimeConfig {
        endpoint_url: url.to_string(),
        ..RuntimeConfig::default()
    };
    probe.base_url().unwrap_or_else(|| url.to_string())
}

/// Picks the active model for a freshly loaded catalog: the persisted choice if
/// it is present in the catalog, otherwise the first model, otherwise empty.
fn pick_selection(models: &[ModelInfo], persisted: Option<&str>) -> String {
    if let Some(p) = persisted {
        if models.iter().any(|m| m.id == p) {
            return p.to_string();
        }
    }
    models.first().map(|m| m.id.clone()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::classify_model;

    fn state_with(models: &[&str]) -> AppState {
        let config = RuntimeConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            endpoint_url: "https://endpoint.example/v1/chat/completions".to_string(),
            ..RuntimeConfig::default()
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

    #[test]
    fn swap_endpoint_keeps_selection_when_model_survives() {
        let state = state_with(&["gpt-4o", "claude-3"]);
        assert!(state.set_selected_model("claude-3"));
        // New catalog still contains claude-3 → selection preserved.
        state.swap_endpoint(
            "https://other.example/v1/chat/completions".to_string(),
            vec![classify_model("claude-3"), classify_model("gpt-4o-mini")],
        );
        assert_eq!(
            state.endpoint_url(),
            "https://other.example/v1/chat/completions"
        );
        assert_eq!(state.selected_model(), "claude-3");
    }

    #[test]
    fn swap_endpoint_resets_selected_when_model_absent() {
        let state = state_with(&["gpt-4o", "claude-3"]);
        assert!(state.set_selected_model("claude-3"));
        // claude-3 is gone → fall back to the first model in the new catalog.
        state.swap_endpoint(
            "https://other.example/v1/chat/completions".to_string(),
            vec![classify_model("llama-3"), classify_model("mistral")],
        );
        assert_eq!(state.selected_model(), "llama-3");
        // Empty catalog → empty selection (no stale id pointing nowhere).
        state.swap_endpoint("https://third.example/v1/responses".to_string(), vec![]);
        assert_eq!(state.selected_model(), "");
    }

    #[test]
    fn enabling_exposure_mints_a_token_once() {
        let state = state_with(&["gpt-4o"]);
        assert!(!state.expose_to_network());
        assert_eq!(state.proxy_token(), None);

        state.set_expose_to_network(true);
        assert!(state.expose_to_network());
        let token = state.proxy_token().expect("token minted on enable");
        assert!(!token.is_empty());

        // Toggling off keeps the token; toggling back on does not mint a new one.
        state.set_expose_to_network(false);
        assert_eq!(state.proxy_token().as_deref(), Some(token.as_str()));
        state.set_expose_to_network(true);
        assert_eq!(state.proxy_token().as_deref(), Some(token.as_str()));

        // Explicit regeneration replaces it.
        let fresh = state.regenerate_proxy_token();
        assert_ne!(fresh, token);
        assert_eq!(state.proxy_token().as_deref(), Some(fresh.as_str()));
    }

    #[test]
    fn set_visible_models_persists_last_write() {
        let path = std::env::temp_dir().join("copilot_proxy_state_visible_rmw_test.json");
        let _ = std::fs::remove_file(&path);

        let state = state_with(&["gpt-4o", "claude-3"]);
        state.load_ui_state(path.clone());

        state.set_visible_models(vec!["gpt-4o".into()]);
        state.set_visible_models(vec!["claude-3".into()]);

        // Reload from disk through a fresh state: the last write survived intact.
        let reloaded = state_with(&["gpt-4o", "claude-3"]);
        reloaded.load_ui_state(path.clone());
        assert_eq!(reloaded.visible_model_ids(), vec!["claude-3"]);

        let _ = std::fs::remove_file(&path);
    }

    fn chat_state(endpoint: &str) -> AppState {
        AppState::new(RuntimeConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            endpoint_url: endpoint.to_string(),
            ..RuntimeConfig::default()
        })
    }

    #[test]
    fn selected_model_persists_and_restores_per_endpoint() {
        let path = std::env::temp_dir().join("copilot_proxy_selected_restore_test.json");
        let _ = std::fs::remove_file(&path);
        let endpoint = "https://endpoint.example/v1/chat/completions";

        let state = chat_state(endpoint);
        state.load_ui_state(path.clone());
        state.set_models(
            ["gpt-4o", "claude-3"]
                .iter()
                .map(|m| classify_model(m))
                .collect(),
        );
        // Picking a non-default model persists it.
        assert!(state.set_selected_model("claude-3"));

        // A fresh state on the same endpoint restores the persisted selection
        // instead of defaulting to the first model — the in-memory selection
        // starts empty and is re-applied when the catalog loads.
        let reloaded = chat_state(endpoint);
        reloaded.load_ui_state(path.clone());
        assert_eq!(reloaded.selected_model(), "");
        reloaded.set_models(
            ["gpt-4o", "claude-3"]
                .iter()
                .map(|m| classify_model(m))
                .collect(),
        );
        assert_eq!(reloaded.selected_model(), "claude-3");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn swap_endpoint_restores_that_endpoints_persisted_selection() {
        let path = std::env::temp_dir().join("copilot_proxy_selected_swap_test.json");
        let _ = std::fs::remove_file(&path);
        let endpoint_a = "https://a.example/v1/chat/completions";

        let state = chat_state(endpoint_a);
        state.load_ui_state(path.clone());
        state.set_models(
            ["gpt-4o", "claude-3"]
                .iter()
                .map(|m| classify_model(m))
                .collect(),
        );
        assert!(state.set_selected_model("claude-3")); // persisted for endpoint A

        // Switch to endpoint B and persist a choice there.
        state.swap_endpoint(
            "https://b.example/v1/chat/completions".to_string(),
            vec![classify_model("llama-3"), classify_model("mistral")],
        );
        assert!(state.set_selected_model("mistral"));

        // Back to endpoint A: its persisted selection wins over the first model.
        state.swap_endpoint(
            endpoint_a.to_string(),
            vec![classify_model("gpt-4o"), classify_model("claude-3")],
        );
        assert_eq!(state.selected_model(), "claude-3");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn restore_falls_back_to_first_when_persisted_absent_from_catalog() {
        let path = std::env::temp_dir().join("copilot_proxy_selected_fallback_test.json");
        let _ = std::fs::remove_file(&path);
        let endpoint = "https://endpoint.example/v1/chat/completions";

        let state = chat_state(endpoint);
        state.load_ui_state(path.clone());
        state.set_models(
            ["gpt-4o", "claude-3"]
                .iter()
                .map(|m| classify_model(m))
                .collect(),
        );
        assert!(state.set_selected_model("claude-3"));

        // A new catalog without claude-3: the persisted id is gone → first model.
        state.swap_endpoint(
            endpoint.to_string(),
            vec![classify_model("llama-3"), classify_model("mistral")],
        );
        assert_eq!(state.selected_model(), "llama-3");

        let _ = std::fs::remove_file(&path);
    }
}
