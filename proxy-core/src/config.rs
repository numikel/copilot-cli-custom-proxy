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
}
