use crate::config::Config;
use secrecy::SecretString;
use std::sync::Mutex;

/// Stan współdzielony między serwerem proxy a interfejsem (tray / okno ustawień).
///
/// Przechowywany jako `Arc<AppState>`: jedna kopia trafia do routera Axum,
/// druga do menedżera stanu Tauri, dzięki czemu zmiana modelu lub klucza w UI
/// jest natychmiast widoczna dla proxy.
pub struct AppState {
    pub config: Config,
    /// Aktualnie wybrany model (podstawiany do pola `model` w żądaniach).
    selected_model: Mutex<String>,
    /// Klucz API — wyłącznie w pamięci, owinięty w `SecretString`
    /// (zeroizacja przy zwalnianiu, redagowany w logach).
    api_key: Mutex<Option<SecretString>>,
    /// Współdzielony klient HTTP do forwardowania żądań na upstream.
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

    /// Ustawia aktywny model, jeśli znajduje się na liście z konfiguracji.
    /// Zwraca `false`, gdy model jest nieznany.
    pub fn set_selected_model(&self, model: impl Into<String>) -> bool {
        let model = model.into();
        if !self.config.models.iter().any(|m| m == &model) {
            return false;
        }
        *self.selected_model.lock().unwrap() = model;
        true
    }

    /// Zapisuje klucz API w pamięci. Pusty ciąg czyści klucz.
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

    /// Klonuje klucz API do wykorzystania w nagłówku `Authorization`.
    /// Zwraca `None`, gdy klucz nie został ustawiony.
    pub fn api_key(&self) -> Option<SecretString> {
        self.api_key.lock().unwrap().clone()
    }
}
