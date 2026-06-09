use serde::Deserialize;
use std::path::Path;

/// Configuration loaded from `config.toml`.
///
/// Note: the API key is NOT stored here — the user enters it in the UI and the
/// app keeps it in memory only.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Local address the proxy listens on (e.g. "127.0.0.1:8080").
    pub listen_addr: String,
    /// Base URL of the OpenAI-compatible corporate endpoint.
    pub corporate_base_url: String,
    /// Model that is active at startup. Optional — if omitted, the first
    /// available model (static list or fetched from the endpoint) is used.
    #[serde(default)]
    pub default_model: Option<String>,
    /// Optional static list of models. If empty, the list is fetched from
    /// `{corporate_base_url}/models` once an API key is set.
    #[serde(default)]
    pub models: Vec<String>,
    /// OpenAI-compatible APIs the endpoint serves. Controls which agents the
    /// app offers to launch: "chat" = /chat/completions (e.g. Copilot),
    /// "responses" = /responses (e.g. Codex). Defaults to ["chat"].
    #[serde(default = "default_upstream_apis")]
    pub upstream_apis: Vec<String>,
}

/// Known wire APIs an upstream may serve.
pub const KNOWN_APIS: &[&str] = &["chat", "responses"];

fn default_upstream_apis() -> Vec<String> {
    vec!["chat".to_string()]
}

#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Parse(toml::de::Error),
    Invalid(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io(e) => write!(f, "failed to read config.toml: {e}"),
            ConfigError::Parse(e) => write!(f, "failed to parse config.toml: {e}"),
            ConfigError::Invalid(msg) => write!(f, "invalid configuration: {msg}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// Loads and validates the configuration from a TOML file.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let contents = std::fs::read_to_string(path).map_err(ConfigError::Io)?;
        let config: Config = toml::from_str(&contents).map_err(ConfigError::Parse)?;
        config.validate()?;
        Ok(config)
    }

    /// Parses the configuration from a string (useful in tests).
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(contents: &str) -> Result<Self, ConfigError> {
        let config: Config = toml::from_str(contents).map_err(ConfigError::Parse)?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        // `models` may be empty — the list is then fetched from the endpoint.
        // Only validate `default_model` membership when both are provided.
        if let Some(default_model) = &self.default_model {
            if !self.models.is_empty() && !self.models.iter().any(|m| m == default_model) {
                return Err(ConfigError::Invalid(format!(
                    "`default_model` (\"{default_model}\") is not in the `models` list"
                )));
            }
        }
        if self.upstream_apis.is_empty() {
            return Err(ConfigError::Invalid(
                "`upstream_apis` must list at least one API (e.g. [\"chat\"])".to_string(),
            ));
        }
        if let Some(bad) = self
            .upstream_apis
            .iter()
            .find(|a| !KNOWN_APIS.contains(&a.as_str()))
        {
            return Err(ConfigError::Invalid(format!(
                "unknown value \"{bad}\" in `upstream_apis` (known: {KNOWN_APIS:?})"
            )));
        }
        Ok(())
    }

    /// Base URL without a trailing slash — ready for the request path to be appended.
    pub fn base_url_trimmed(&self) -> &str {
        self.corporate_base_url.trim_end_matches('/')
    }

    /// Converts a legacy `config.toml` into the runtime config the app now uses.
    /// Builds the full endpoint URL from the base + the first declared API's
    /// suffix (the new model encodes the wire API in the URL). Used once to seed
    /// `config.json` on first run; afterwards `config.toml` is ignored.
    pub fn into_runtime(self) -> crate::settings::RuntimeConfig {
        use crate::settings::ApiKind;
        let base = self.base_url_trimmed().to_string();
        let api = self
            .upstream_apis
            .first()
            .map(String::as_str)
            .and_then(|a| match a {
                "responses" => Some(ApiKind::Responses),
                "chat" => Some(ApiKind::Chat),
                _ => None,
            })
            .unwrap_or(ApiKind::Chat);
        crate::settings::RuntimeConfig {
            listen_addr: self.listen_addr,
            endpoint_url: format!("{base}{}", api.suffix()),
            default_model: self.default_model,
            ..crate::settings::RuntimeConfig::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
        listen_addr = "127.0.0.1:8080"
        corporate_base_url = "https://e.example.com/v1"
    "#;

    #[test]
    fn upstream_apis_defaults_to_chat() {
        let config = Config::from_str(MINIMAL).unwrap();
        assert_eq!(config.upstream_apis, vec!["chat".to_string()]);
    }

    #[test]
    fn accepts_known_apis() {
        let toml = format!("{MINIMAL}\nupstream_apis = [\"chat\", \"responses\"]");
        let config = Config::from_str(&toml).unwrap();
        assert_eq!(config.upstream_apis, vec!["chat", "responses"]);
    }

    #[test]
    fn rejects_unknown_api() {
        let toml = format!("{MINIMAL}\nupstream_apis = [\"grpc\"]");
        assert!(Config::from_str(&toml).is_err());
    }

    #[test]
    fn rejects_empty_apis() {
        let toml = format!("{MINIMAL}\nupstream_apis = []");
        assert!(Config::from_str(&toml).is_err());
    }

    #[test]
    fn into_runtime_builds_full_endpoint_url() {
        use crate::settings::ApiKind;

        // Single "chat" API → /chat/completions suffix.
        let chat = Config::from_str(MINIMAL).unwrap().into_runtime();
        assert_eq!(chat.endpoint_url, "https://e.example.com/v1/chat/completions");
        assert_eq!(chat.active_api(), Some(ApiKind::Chat));
        assert_eq!(chat.listen_addr, "127.0.0.1:8080");

        // "responses" first → /responses suffix; trailing slash on base trimmed.
        let toml = "listen_addr = \"127.0.0.1:8080\"\n\
                    corporate_base_url = \"https://e.example.com/v1/\"\n\
                    upstream_apis = [\"responses\", \"chat\"]";
        let resp = Config::from_str(toml).unwrap().into_runtime();
        assert_eq!(resp.endpoint_url, "https://e.example.com/v1/responses");
        assert_eq!(resp.active_api(), Some(ApiKind::Responses));
    }
}
