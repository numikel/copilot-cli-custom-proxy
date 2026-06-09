//! Lightweight, on-disk UI preferences — stored next to `config.toml` as
//! `ui_state.json`. Unlike the API key (memory only), these are non-secret user
//! choices that should survive a restart.
//!
//! The tray-visibility selection is keyed by endpoint (the endpoint base URL):
//! different upstreams expose different catalogs, so each remembers its own set
//! of models shown in the tray's "Models" submenu.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// The persisted UI state file (`ui_state.json`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UiStateFile {
    /// endpoint url → ids of the models shown in the tray's Models submenu.
    /// A missing entry means "not curated yet" (all chat models are shown).
    #[serde(default)]
    pub visible_models: HashMap<String, Vec<String>>,
}

impl UiStateFile {
    /// Reads the file, returning an empty state if it is missing or unreadable.
    /// Persisted preferences are best-effort — a corrupt file must never block
    /// startup — but a *corrupt* file (present, invalid JSON) is logged at
    /// `warn` so a silently reset preference set is at least diagnosable.
    pub fn load(path: &Path) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                tracing::warn!("could not read ui_state at {}: {e}", path.display());
                return Self::default();
            }
        };
        match serde_json::from_str(&text) {
            Ok(state) => state,
            Err(e) => {
                tracing::warn!(
                    "ui_state at {} is corrupt — resetting preferences: {e}",
                    path.display()
                );
                Self::default()
            }
        }
    }

    /// Writes the state to disk (pretty-printed for easy hand-editing), via an
    /// atomic temp-write + rename so a crash mid-write can't truncate it.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        crate::atomic_io::write_atomic(path, text.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_visible_models_per_endpoint() {
        let path = std::env::temp_dir().join("copilot_proxy_uistate_roundtrip_test.json");
        let _ = std::fs::remove_file(&path);

        let mut file = UiStateFile::default();
        file.visible_models.insert(
            "https://a.example/v1".into(),
            vec!["m1".into(), "m2".into()],
        );
        file.visible_models
            .insert("https://b.example/v1".into(), vec!["x".into()]);
        file.save(&path).unwrap();

        let loaded = UiStateFile::load(&path);
        assert_eq!(
            loaded.visible_models.get("https://a.example/v1").unwrap(),
            &vec!["m1".to_string(), "m2".to_string()]
        );
        assert_eq!(
            loaded
                .visible_models
                .get("https://b.example/v1")
                .unwrap()
                .len(),
            1
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_or_corrupt_file_loads_empty() {
        let missing = std::env::temp_dir().join("copilot_proxy_uistate_absent_xyz.json");
        let _ = std::fs::remove_file(&missing);
        assert!(UiStateFile::load(&missing).visible_models.is_empty());
    }
}
