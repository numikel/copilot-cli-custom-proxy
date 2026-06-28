//! Runtime configuration entered in the settings window and persisted to
//! `config.json` (next to the executable). Unlike `config.toml` — which is now
//! only an optional one-time seed (see [`crate::Config::into_runtime`]) — this
//! is the live source of truth the proxy reads on every request.
//!
//! The endpoint is stored as a **full URL including the API suffix**
//! (e.g. `https://openrouter.ai/api/v1/responses`). The wire API is derived
//! from that suffix, so the URL must NOT stop at `/v1` — otherwise chat and
//! responses are indistinguishable. The suffix must form the final segments of
//! the URL's *path* — a host that merely looks like one (`https://responses`)
//! does not count — and the URL must contain a host.
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
    Messages,
}

impl ApiKind {
    /// All known APIs.
    pub const ALL: &'static [ApiKind] = &[ApiKind::Chat, ApiKind::Responses, ApiKind::Messages];

    /// The URL path suffix that identifies this API.
    pub fn suffix(self) -> &'static str {
        match self {
            ApiKind::Chat => "/chat/completions",
            ApiKind::Responses => "/responses",
            ApiKind::Messages => "/messages",
        }
    }

    /// Stable lowercase id used in the JS↔Rust contract.
    pub fn as_str(self) -> &'static str {
        match self {
            ApiKind::Chat => "chat",
            ApiKind::Responses => "responses",
            ApiKind::Messages => "messages",
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
    /// Opt-in to binding the proxy beyond loopback. Off by default: the proxy
    /// injects the API key into every request, so a non-loopback bind would
    /// share that key with the whole network. When off, a non-loopback
    /// `listen_addr` is rejected / reset to loopback.
    #[serde(default)]
    pub expose_to_network: bool,
    /// Gateway access token required from non-loopback clients once
    /// [`Self::expose_to_network`] is on. Distinct from the upstream API key
    /// (which stays in memory only) — this is a low-sensitivity, self-generated
    /// credential, persisted so a remote device need not re-pair every restart.
    #[serde(default)]
    pub proxy_token: Option<String>,
}

fn default_listen_addr() -> String {
    DEFAULT_LISTEN_ADDR.to_string()
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            listen_addr: DEFAULT_LISTEN_ADDR.to_string(),
            endpoint_url: String::new(),
            expose_to_network: false,
            proxy_token: None,
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
    ///
    /// Assumes `endpoint_url` has passed [`validate_endpoint_url`] (every write
    /// path validates, and the app's `sanitize_config` clears an invalid URL on
    /// load) — the method itself stays liberal and just strips the suffix.
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
    /// the caller can fall back to a seed / defaults. A *corrupt* file (present
    /// but not valid JSON) is logged at `warn` so a lost configuration is
    /// diagnosable — a missing file stays silent (the normal first-run case).
    pub fn load(path: &Path) -> Option<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(e) => {
                tracing::warn!("could not read config at {}: {e}", path.display());
                return None;
            }
        };
        match serde_json::from_str(&text) {
            Ok(cfg) => Some(cfg),
            Err(e) => {
                tracing::warn!(
                    "config at {} is corrupt and was ignored: {e}",
                    path.display()
                );
                None
            }
        }
    }

    /// Writes the config to disk (pretty-printed for easy hand-editing), via an
    /// atomic temp-write + rename so a crash mid-write can't truncate it.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        crate::atomic_io::write_atomic(path, text.as_bytes())
    }
}

/// Validates a candidate endpoint URL: it must be http(s), contain a host, and
/// its *path* must end in a known API suffix so the wire API can be derived —
/// a host that merely looks like a suffix does not count. Returns the
/// detected API.
pub fn validate_endpoint_url(url: &str) -> Result<ApiKind, String> {
    let trimmed = url.trim();
    let rest = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"))
        .ok_or("Endpoint URL must start with http:// or https://")?;
    // The authority is everything up to the first path/query/fragment delimiter.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return Err(
            "Endpoint URL must contain a host (e.g. https://api.example.com/v1/responses)"
                .to_string(),
        );
    }
    // Reject credentials embedded in the authority (`user:pass@host`). Splitting
    // on the first path/query/fragment delimiter keeps any later `@` (e.g. in a
    // query string) from triggering a false positive.
    if authority.contains('@') {
        return Err(
            "Endpoint URL must not contain credentials (user:pass@host) — \
                    enter the API key in the settings window instead"
                .to_string(),
        );
    }
    // Match the suffix against the path only, so it cannot be satisfied by the
    // host (e.g. `https://chat/completions`, whose path is just `/completions`).
    let path = rest[authority.len()..].trim_end_matches('/');
    ApiKind::ALL
        .iter()
        .copied()
        .find(|api| path.ends_with(api.suffix()))
        .ok_or_else(|| {
            "Endpoint URL must end with /chat/completions, /responses, or /messages \
             (do not stop at /v1, or the API type is ambiguous)"
                .to_string()
        })
}

/// Validates a `host:port` listen address. Hostnames are allowed (so plain
/// `SocketAddr` parsing is too strict), but the host is restricted to a strict
/// character whitelist so the value is safe to interpolate into a launched
/// CLI's command line (the address becomes the proxy's `base_url`). Bracketed
/// IPv6 literals (`[::1]:8080`) are supported. The port must be 1–65535.
pub fn validate_listen_addr(addr: &str) -> Result<(), String> {
    let addr = addr.trim();

    // Split host from port, handling bracketed IPv6 literals separately so the
    // colons inside the address are not mistaken for the port separator.
    let (host, port, is_ipv6) = if let Some(after_bracket) = addr.strip_prefix('[') {
        let (host, rest) = after_bracket
            .split_once(']')
            .ok_or("IPv6 listen address must be enclosed in brackets (e.g. [::1]:8080)")?;
        let port = rest
            .strip_prefix(':')
            .ok_or("Listen address must be in host:port form (e.g. [::1]:8080)")?;
        (host, port, true)
    } else {
        let (host, port) = addr
            .rsplit_once(':')
            .ok_or("Listen address must be in host:port form (e.g. 127.0.0.1:8080)")?;
        (host, port, false)
    };

    if host.is_empty() {
        return Err("Listen address host must not be empty".to_string());
    }

    let host_ok = if is_ipv6 {
        host.chars().all(|c| c.is_ascii_hexdigit() || c == ':')
    } else {
        // RFC 1123 host characters only. This rejects shell metacharacters
        // (`;`, backtick, `$`, `|`, `&`, quotes, whitespace, ...) outright.
        host.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
    };
    if !host_ok {
        return Err("Listen address host contains invalid characters \
                    (only letters, digits, '-' and '.' are allowed)"
            .to_string());
    }

    match port.parse::<u16>() {
        Ok(0) | Err(_) => {
            Err("Listen address port must be a number between 1 and 65535".to_string())
        }
        Ok(_) => Ok(()),
    }
}

/// Extracts the host portion of a `host:port` listen address, handling
/// bracketed IPv6 literals (`[::1]:8080` → `::1`).
fn listen_host(addr: &str) -> Option<&str> {
    let addr = addr.trim();
    if let Some(after_bracket) = addr.strip_prefix('[') {
        after_bracket.split_once(']').map(|(host, _)| host)
    } else {
        addr.rsplit_once(':').map(|(host, _)| host)
    }
}

/// Whether a `host:port` listen address binds to loopback only (reachable from
/// the local machine, not the network). Covers the whole `127.0.0.0/8` range,
/// `::1`, and `localhost`. A non-loopback bind shares the injected API key with
/// anything that can reach the port, so it is gated behind an explicit opt-in.
pub fn is_loopback_listen_addr(addr: &str) -> bool {
    match listen_host(addr) {
        Some(host) => match host.parse::<std::net::IpAddr>() {
            Ok(ip) => ip.is_loopback(),
            Err(_) => host.eq_ignore_ascii_case("localhost"),
        },
        None => false,
    }
}

/// Generates a fresh gateway token (a random 32-char hex string). Used to
/// protect the proxy when it is exposed beyond loopback.
pub fn generate_proxy_token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(url: &str) -> RuntimeConfig {
        RuntimeConfig {
            endpoint_url: url.to_string(),
            ..RuntimeConfig::default()
        }
    }

    #[test]
    fn detects_responses_api() {
        let c = cfg("https://openrouter.ai/api/v1/responses");
        assert_eq!(c.active_api(), Some(ApiKind::Responses));
        assert_eq!(c.base_url().unwrap(), "https://openrouter.ai/api/v1");
        assert_eq!(
            c.models_url().unwrap(),
            "https://openrouter.ai/api/v1/models"
        );
    }

    #[test]
    fn detects_chat_api() {
        let c = cfg("https://openrouter.ai/api/v1/chat/completions");
        assert_eq!(c.active_api(), Some(ApiKind::Chat));
        assert_eq!(c.base_url().unwrap(), "https://openrouter.ai/api/v1");
    }

    #[test]
    fn detects_messages_api() {
        let c = cfg("https://api.anthropic.com/v1/messages");
        assert_eq!(c.active_api(), Some(ApiKind::Messages));
        assert_eq!(c.base_url().unwrap(), "https://api.anthropic.com/v1");
        assert_eq!(
            c.models_url().unwrap(),
            "https://api.anthropic.com/v1/models"
        );
    }

    #[test]
    fn validate_endpoint_accepts_messages_suffix() {
        assert_eq!(
            validate_endpoint_url("https://api.anthropic.com/v1/messages").unwrap(),
            ApiKind::Messages
        );
        // The suffix must live in the path, not the host.
        assert!(validate_endpoint_url("https://messages").is_err());
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
        // The suffix may be the entire path (base = the bare authority).
        assert_eq!(
            validate_endpoint_url("https://h/responses").unwrap(),
            ApiKind::Responses
        );
    }

    #[test]
    fn validate_endpoint_rejects_empty_host() {
        assert!(validate_endpoint_url("https:///chat/completions").is_err());
        assert!(validate_endpoint_url("http:///responses").is_err());
        let err = validate_endpoint_url("https:///chat/completions").unwrap_err();
        assert!(err.contains("host"));
    }

    #[test]
    fn validate_endpoint_rejects_suffix_inside_host() {
        // The suffix must live in the path: `https://chat/completions` has host
        // "chat" and path "/completions"; `https://responses` has no path at all.
        assert!(validate_endpoint_url("https://chat/completions").is_err());
        assert!(validate_endpoint_url("https://responses").is_err());
        // ...while the same segments after a real host validate fine.
        assert_eq!(
            validate_endpoint_url("https://h/chat/completions").unwrap(),
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
    fn validate_endpoint_rejects_userinfo() {
        // Credentials in the authority must be rejected (they would leak into
        // logs and config.json).
        assert!(validate_endpoint_url("https://user:pass@evil.com/v1/responses").is_err());
        assert!(validate_endpoint_url("https://user@evil.com/v1/responses").is_err());
        // A normal credential-free URL still validates.
        assert!(validate_endpoint_url("https://e.example.com/v1/responses").is_ok());
        // The authority split means an `@` is only flagged in the authority, not
        // later in the URL — the rejection above is specifically the userinfo
        // guard (not the suffix check, which would also fail on a stray query).
        let err = validate_endpoint_url("https://user:pass@evil.com/v1/responses").unwrap_err();
        assert!(err.contains("credentials"));
    }

    #[test]
    fn validate_listen_addr_checks_port() {
        assert!(validate_listen_addr("127.0.0.1:8080").is_ok());
        assert!(validate_listen_addr("localhost:65535").is_ok());
        assert!(validate_listen_addr("127.0.0.1").is_err());
        assert!(validate_listen_addr("127.0.0.1:notaport").is_err());
        assert!(validate_listen_addr(":8080").is_err());
        // Port 0 would let the OS pick a random port — reject it.
        assert!(validate_listen_addr("127.0.0.1:0").is_err());
    }

    #[test]
    fn validate_listen_addr_accepts_ipv6_and_hostnames() {
        assert!(validate_listen_addr("[::1]:8080").is_ok());
        assert!(validate_listen_addr("[2001:db8::1]:443").is_ok());
        assert!(validate_listen_addr("example-host.local:80").is_ok());
        // Brackets without a port, or a non-hex IPv6 body, are rejected.
        assert!(validate_listen_addr("[::1]").is_err());
        assert!(validate_listen_addr("[gggg::1]:80").is_err());
    }

    #[test]
    fn validate_listen_addr_rejects_shell_metachars() {
        // The host feeds the launched CLI's command line — reject anything that
        // could break out of it (command-injection regression guard).
        assert!(validate_listen_addr("127.0.0.1;calc:8080").is_err());
        assert!(validate_listen_addr("127.0.0.1`whoami`:8080").is_err());
        assert!(validate_listen_addr("$(rm -rf /):8080").is_err());
        assert!(validate_listen_addr("a b:8080").is_err());
        assert!(validate_listen_addr("a|b:8080").is_err());
        assert!(validate_listen_addr("a&b:8080").is_err());
        assert!(validate_listen_addr("a'b:8080").is_err());
        assert!(validate_listen_addr("a\"b:8080").is_err());
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

    #[test]
    fn corrupt_config_json_loads_none() {
        // A present-but-unparseable file is ignored (and logged) rather than
        // crashing — the caller falls back to a seed / defaults.
        let path = std::env::temp_dir().join("copilot_proxy_config_corrupt_test.json");
        std::fs::write(&path, b"{ this is not valid json").unwrap();
        assert!(RuntimeConfig::load(&path).is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn config_roundtrips_new_fields() {
        let path = std::env::temp_dir().join("copilot_proxy_config_newfields_test.json");
        let _ = std::fs::remove_file(&path);
        let c = RuntimeConfig {
            expose_to_network: true,
            proxy_token: Some("abc123".to_string()),
            ..cfg("https://e.example.com/v1/responses")
        };
        c.save(&path).unwrap();
        let loaded = RuntimeConfig::load(&path).unwrap();
        assert!(loaded.expose_to_network);
        assert_eq!(loaded.proxy_token.as_deref(), Some("abc123"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn loopback_detection() {
        assert!(is_loopback_listen_addr("127.0.0.1:8080"));
        assert!(is_loopback_listen_addr("127.0.0.5:80"));
        assert!(is_loopback_listen_addr("localhost:8080"));
        assert!(is_loopback_listen_addr("[::1]:8080"));
        assert!(!is_loopback_listen_addr("0.0.0.0:8080"));
        assert!(!is_loopback_listen_addr("192.168.1.10:8080"));
        assert!(!is_loopback_listen_addr("example.com:443"));
    }

    #[test]
    fn generated_token_is_nonempty_and_unique() {
        let a = generate_proxy_token();
        let b = generate_proxy_token();
        assert!(!a.is_empty());
        assert_ne!(a, b);
    }
}
