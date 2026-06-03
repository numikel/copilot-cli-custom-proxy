use serde::Deserialize;
use std::path::Path;

/// Konfiguracja wczytywana z `config.toml`.
///
/// Uwaga: klucz API NIE jest tutaj przechowywany — użytkownik wprowadza go
/// w UI, a aplikacja trzyma go wyłącznie w pamięci.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Adres lokalny, na którym nasłuchuje proxy (np. "127.0.0.1:8080").
    pub listen_addr: String,
    /// Bazowy URL korporacyjnego endpointu OpenAI-compatible.
    pub corporate_base_url: String,
    /// Model aktywny przy starcie.
    pub default_model: String,
    /// Lista modeli dostępnych do przełączania w menu tray.
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
            ConfigError::Io(e) => write!(f, "nie udało się odczytać config.toml: {e}"),
            ConfigError::Parse(e) => write!(f, "błąd parsowania config.toml: {e}"),
            ConfigError::Invalid(msg) => write!(f, "niepoprawna konfiguracja: {msg}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// Wczytuje i waliduje konfigurację z pliku TOML.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let contents = std::fs::read_to_string(path).map_err(ConfigError::Io)?;
        let config: Config = toml::from_str(&contents).map_err(ConfigError::Parse)?;
        config.validate()?;
        Ok(config)
    }

    /// Parsuje konfigurację z tekstu (przydatne w testach).
    pub fn from_str(contents: &str) -> Result<Self, ConfigError> {
        let config: Config = toml::from_str(contents).map_err(ConfigError::Parse)?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.models.is_empty() {
            return Err(ConfigError::Invalid("lista `models` jest pusta".into()));
        }
        if !self.models.iter().any(|m| m == &self.default_model) {
            return Err(ConfigError::Invalid(format!(
                "`default_model` (\"{}\") nie znajduje się na liście `models`",
                self.default_model
            )));
        }
        Ok(())
    }

    /// Bazowy URL bez końcowego ukośnika — gotowy do doklejenia ścieżki żądania.
    pub fn base_url_trimmed(&self) -> &str {
        self.corporate_base_url.trim_end_matches('/')
    }
}
