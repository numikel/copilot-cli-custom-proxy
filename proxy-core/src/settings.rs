//! Runtime configuration entered in the settings window and persisted to
//! `config.json` (next to the executable). Unlike `config.toml` — which is now
//! only an optional one-time seed (see [`crate::Config::into_runtime`]) — this
//! is the live source of truth the proxy reads on every request.
//!
//! The endpoint is stored as a **full URL including the API suffix**
//! (e.g. `https://openrouter.ai/api/v1/responses`). The wire API is derived
//! from that suffix, so the URL must NOT stop at `/v1` — otherwise chat and
//! responses are indistinguishable.
//!
//! The API key is NOT stored here — it stays in memory only ([`crate::AppState`]).

use serde::{Deserialize, Serialize};
use std::path::Path;

/// OpenAI-compatible wire API an endpoint speaks, derived from the endpoint URL.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApiKind {
    Chat,
    Responses,
}

impl ApiKind {
    /// All known APIs.
    pub const ALL: &'static [ApiKind] = &[ApiKind::Chat, ApiKind::Responses];

    /// The URL path suffix that identifies this API.
    pub fn suffix(self) -> &'static str {
        match self {
            ApiKind::Chat => "/chat/completions",
            ApiKind::Responses => "/responses",
        }
    }

    /// Stable lowercase id used in the JS↔Rust contract ("chat" / "responses").
    pub fn as_str(self) -> &'static str {
        match self {
            ApiKind::Chat => "chat",
            ApiKind::Responses => "responses",
        }
    }
}

/// Default local address the proxy listens on.
pub const DEFAULT_LISTEN_ADDR: &str = "127.0.0.1:8080";

/// Live configuration, persisted to `config.json`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeConfig {
    /// Local address the proxy listens on (e.g. "127.0.0.1:8080").
    #[serde(default = "default_listen_addr")]
    pub listen_addr: String,
    /// Full upstream endpoint URL, including the API suffix
    /// (e.g. "https://openrouter.ai/api/v1/responses"). Empty = not configured.
    #[serde(default)]
    pub endpoint_url: String,
    /// Model active at startup. Optional — the first available model is used
    /// otherwise.
    #[serde(default)]
    pub default_model: Option<String>,
}

fn default_listen_addr() -> String {
    DEFAULT_LISTEN_ADDR.to_string()
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            listen_addr: DEFAULT_LISTEN_ADDR.to_string(),
            endpoint_url: String::new(),
            default_model: None,
        }
    }
}

impl RuntimeConfig {
    /// The wire API implied by the endpoint URL's suffix, or `None` when the URL
    /// is empty or ends in an unrecognized path.
    pub fn active_api(&self) -> Option<ApiKind> {
        let trimmed = self.endpoint_url.trim_end_matches('/');
        ApiKind::ALL
            .iter()
            .copied()
            .find(|api| trimmed.ends_with(api.suffix()))
    }

    /// The endpoint base — the URL minus the known API suffix and any trailing
    /// slash — ready for `/models` or a request path to be appended. `None`
    /// when the URL is empty or its suffix is unrecognized.
    pub fn base_url(&self) -> Option<String> {
        let trimmed = self.endpoint_url.trim_end_matches('/');
        let api = self.active_api()?;
        let base = trimmed.strip_suffix(api.suffix())?;
        Some(base.trim_end_matches('/').to_string())
    }

    /// URL of the model catalog (`{base}/models`), or `None` when unconfigured.
    pub fn models_url(&self) -> Option<String> {
        self.base_url().map(|base| format!("{base}/models"))
    }

    /// Whether a usable endpoint is configured (URL present and suffix known).
    pub fn is_configured(&self) -> bool {
        self.base_url().is_some()
    }

    /// Reads `config.json`, returning `None` if it is missing or unreadable so
    /// the caller can fall back to a seed / defaults.
    pub fn load(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Writes the config to disk (pretty-printed for easy hand-editing).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, text)
    }
}

/// Validates a candidate endpoint URL: it must be http(s) and end in a known
/// API suffix so the wire API can be derived. Returns the detected API.
pub fn validate_endpoint_url(url: &str) -> Result<ApiKind, String> {
    let trimmed = url.trim();
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return Err("Endpoint URL must start with http:// or https://".to_string());
    }
    let stripped = trimmed.trim_end_matches('/');
    ApiKind::ALL
        .iter()
        .copied()
        .find(|api| stripped.ends_with(api.suffix()))
        .ok_or_else(|| {
            "Endpoint URL must end with /chat/completions or /responses \
             (do not stop at /v1, or the API type is ambiguous)"
                .to_string()
        })
}

/// Validates a `host:port` listen address. Hostnames are allowed (so plain
/// `SocketAddr` parsing is too strict); only the port is parsed strictly.
pub fn validate_listen_addr(addr: &str) -> Result<(), String> {
    let addr = addr.trim();
    let (host, port) = addr
        .rsplit_once(':')
        .ok_or("Listen address must be in host:port form (e.g. 127.0.0.1:8080)")?;
    if host.is_empty() {
        return Err("Listen address host must not be empty".to_string());
    }
    port.parse::<u16>()
        .map_err(|_| "Listen address port must be a number between 1 and 65535".to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(url: &str) -> RuntimeConfig {
        RuntimeConfig {
            listen_addr: DEFAULT_LISTEN_ADDR.to_string(),
            endpoint_url: url.to_string(),
            default_model: None,
        }
    }

    #[test]
    fn detects_responses_api() {
        let c = cfg("https://openrouter.ai/api/v1/responses");
        assert_eq!(c.active_api(), Some(ApiKind::Responses));
        assert_eq!(c.base_url().unwrap(), "https://openrouter.ai/api/v1");
        assert_eq!(c.models_url().unwrap(), "https://openrouter.ai/api/v1/models");
    }

    #[test]
    fn detects_chat_api() {
        let c = cfg("https://openrouter.ai/api/v1/chat/completions");
        assert_eq!(c.active_api(), Some(ApiKind::Chat));
        assert_eq!(c.base_url().unwrap(), "https://openrouter.ai/api/v1");
    }

    #[test]
    fn trailing_slash_is_tolerated() {
        let c = cfg("https://e.example.com/v1/responses/");
        assert_eq!(c.active_api(), Some(ApiKind::Responses));
        assert_eq!(c.base_url().unwrap(), "https://e.example.com/v1");
    }

    #[test]
    fn url_stopping_at_v1_is_unrecognized() {
        let c = cfg("https://openrouter.ai/api/v1");
        assert_eq!(c.active_api(), None);
        assert_eq!(c.base_url(), None);
        assert!(!c.is_configured());
    }

    #[test]
    fn empty_url_is_unconfigured() {
        let c = cfg("");
        assert_eq!(c.active_api(), None);
        assert!(!c.is_configured());
    }

    #[test]
    fn validate_endpoint_accepts_known_suffixes() {
        assert_eq!(
            validate_endpoint_url("https://e.example.com/v1/responses").unwrap(),
            ApiKind::Responses
        );
        assert_eq!(
            validate_endpoint_url("http://localhost:1234/v1/chat/completions").unwrap(),
            ApiKind::Chat
        );
    }

    #[test]
    fn validate_endpoint_rejects_v1_only_and_non_http() {
        assert!(validate_endpoint_url("https://e.example.com/v1").is_err());
        assert!(validate_endpoint_url("ftp://e.example.com/v1/responses").is_err());
        assert!(validate_endpoint_url("e.example.com/v1/responses").is_err());
    }

    #[test]
    fn validate_listen_addr_checks_port() {
        assert!(validate_listen_addr("127.0.0.1:8080").is_ok());
        assert!(validate_listen_addr("localhost:65535").is_ok());
        assert!(validate_listen_addr("127.0.0.1").is_err());
        assert!(validate_listen_addr("127.0.0.1:notaport").is_err());
        assert!(validate_listen_addr(":8080").is_err());
    }

    #[test]
    fn config_json_roundtrips() {
        let path = std::env::temp_dir().join("copilot_proxy_config_roundtrip_test.json");
        let _ = std::fs::remove_file(&path);
        let c = cfg("https://e.example.com/v1/responses");
        c.save(&path).unwrap();
        let loaded = RuntimeConfig::load(&path).unwrap();
        assert_eq!(loaded.endpoint_url, c.endpoint_url);
        assert_eq!(loaded.listen_addr, c.listen_addr);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_config_json_loads_none() {
        let missing = std::env::temp_dir().join("copilot_proxy_config_absent_zzz.json");
        let _ = std::fs::remove_file(&missing);
        assert!(RuntimeConfig::load(&missing).is_none());
    }
}
