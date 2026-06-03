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
        Ok(())
    }

    /// Base URL without a trailing slash — ready for the request path to be appended.
    pub fn base_url_trimmed(&self) -> &str {
        self.corporate_base_url.trim_end_matches('/')
    }
}
