//! Lightweight, on-disk UI preferences — stored next to `config.toml` as
//! `ui_state.json`. Unlike the API key (memory only), these are non-secret user
//! choices that should survive a restart.
//!
//! Both the tray-visibility selection and the active-model choice are keyed by
//! endpoint (the endpoint base URL): different upstreams expose different
//! catalogs, so each remembers its own set of models shown in the tray's
//! "Models" submenu and its own active model.

use crate::claude::CcSlot;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// A manual per-endpoint override of the Copilot launch token budget
/// (`COPILOT_PROVIDER_MAX_PROMPT_TOKENS` / `COPILOT_PROVIDER_MAX_OUTPUT_TOKENS`).
/// Either field may be absent, in which case the selected model's advertised
/// limit (or Copilot's own default) is used instead.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<u32>,
}

/// Per-endpoint Claude Code model-slot configuration. Each fixed slot holds an
/// optional upstream catalog id; the subagent slot is either a model or set to
/// inherit (Claude Code resolves it from the other slots). All `None` / `false`
/// is the empty default and is removed from disk rather than stored.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CcSlots {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opus: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sonnet: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub haiku: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub subagent_inherit: bool,
}

impl CcSlots {
    /// The configured catalog id for `slot`, if any.
    pub fn model_for(&self, slot: CcSlot) -> Option<&str> {
        match slot {
            CcSlot::Opus => self.opus.as_deref(),
            CcSlot::Sonnet => self.sonnet.as_deref(),
            CcSlot::Haiku => self.haiku.as_deref(),
            CcSlot::Fable => self.fable.as_deref(),
            CcSlot::Subagent => self.subagent.as_deref(),
        }
    }

    /// Launch gate: the four fixed slots each need a model, and the subagent
    /// needs a model **or** inherit.
    pub fn is_complete(&self) -> bool {
        self.opus.is_some()
            && self.sonnet.is_some()
            && self.haiku.is_some()
            && self.fable.is_some()
            && (self.subagent.is_some() || self.subagent_inherit)
    }

    /// Number of satisfied slots out of five (for the tray status line).
    pub fn configured_count(&self) -> usize {
        let fixed = [&self.opus, &self.sonnet, &self.haiku, &self.fable]
            .into_iter()
            .filter(|s| s.is_some())
            .count();
        fixed + usize::from(self.subagent.is_some() || self.subagent_inherit)
    }

    /// Sets one slot. `inherit` applies only to the subagent slot (ignored
    /// otherwise); selecting a subagent model clears inherit and vice versa.
    pub fn set(&mut self, slot: CcSlot, model_id: Option<String>, inherit: bool) {
        match slot {
            CcSlot::Opus => self.opus = model_id,
            CcSlot::Sonnet => self.sonnet = model_id,
            CcSlot::Haiku => self.haiku = model_id,
            CcSlot::Fable => self.fable = model_id,
            CcSlot::Subagent => {
                if inherit {
                    self.subagent = None;
                    self.subagent_inherit = true;
                } else {
                    self.subagent = model_id;
                    self.subagent_inherit = false;
                }
            }
        }
    }

    /// Drops any slot model not present in `catalog` (e.g. after a Refresh
    /// removed it), keeping the launch gate honest. Returns whether anything
    /// changed. Inherit is left untouched.
    pub fn prune_to_catalog(&mut self, catalog: &[String]) -> bool {
        let mut changed = false;
        for slot in [
            &mut self.opus,
            &mut self.sonnet,
            &mut self.haiku,
            &mut self.fable,
            &mut self.subagent,
        ] {
            if let Some(id) = slot {
                if !catalog.iter().any(|c| c == id) {
                    *slot = None;
                    changed = true;
                }
            }
        }
        changed
    }
}

/// The persisted UI state file (`ui_state.json`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UiStateFile {
    /// endpoint url → ids of the models shown in the tray's Models submenu.
    /// A missing entry means "not curated yet" (all chat models are shown).
    #[serde(default)]
    pub visible_models: HashMap<String, Vec<String>>,
    /// endpoint url → id of the active (selected) model. A missing entry means
    /// "no choice saved yet" → the first available model is used.
    #[serde(default)]
    pub selected_models: HashMap<String, String>,
    /// endpoint url → manual token-limit override for the Copilot launch. A
    /// missing entry means "no override" → the model's advertised limits are used.
    #[serde(default)]
    pub token_overrides: HashMap<String, TokenOverride>,
    /// endpoint url → Claude Code model-slot configuration. A missing entry
    /// means "no slots configured" for that endpoint.
    #[serde(default)]
    pub cc_slot_models: HashMap<String, CcSlots>,
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
        let loaded = UiStateFile::load(&missing);
        assert!(loaded.visible_models.is_empty());
        assert!(loaded.selected_models.is_empty());
    }

    #[test]
    fn roundtrips_selected_models_per_endpoint() {
        let path = std::env::temp_dir().join("copilot_proxy_uistate_selected_test.json");
        let _ = std::fs::remove_file(&path);

        let mut file = UiStateFile::default();
        file.selected_models
            .insert("https://a.example/v1".into(), "m2".into());
        file.selected_models
            .insert("https://b.example/v1".into(), "x".into());
        file.save(&path).unwrap();

        let loaded = UiStateFile::load(&path);
        assert_eq!(
            loaded.selected_models.get("https://a.example/v1").unwrap(),
            "m2"
        );
        assert_eq!(
            loaded.selected_models.get("https://b.example/v1").unwrap(),
            "x"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn roundtrips_token_overrides_per_endpoint() {
        let path = std::env::temp_dir().join("copilot_proxy_uistate_tokens_test.json");
        let _ = std::fs::remove_file(&path);

        let mut file = UiStateFile::default();
        file.token_overrides.insert(
            "https://a.example/v1".into(),
            TokenOverride {
                prompt: Some(128000),
                output: Some(16384),
            },
        );
        file.save(&path).unwrap();

        let loaded = UiStateFile::load(&path);
        assert_eq!(
            loaded.token_overrides.get("https://a.example/v1").copied(),
            Some(TokenOverride {
                prompt: Some(128000),
                output: Some(16384),
            })
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn cc_slots_completeness_and_model_lookup() {
        use crate::claude::CcSlot;
        let mut s = CcSlots::default();
        assert!(!s.is_complete());
        s.set(CcSlot::Opus, Some("vendor/opus".into()), false);
        s.set(CcSlot::Sonnet, Some("vendor/sonnet".into()), false);
        s.set(CcSlot::Haiku, Some("vendor/haiku".into()), false);
        s.set(CcSlot::Fable, Some("vendor/fable".into()), false);
        // Four fixed slots set, subagent still empty → not complete.
        assert!(!s.is_complete());
        // Inherit satisfies the subagent slot.
        s.set(CcSlot::Subagent, None, true);
        assert!(s.is_complete());
        assert!(s.subagent_inherit);
        assert_eq!(s.model_for(CcSlot::Opus), Some("vendor/opus"));
        // Picking a subagent model clears inherit.
        s.set(CcSlot::Subagent, Some("vendor/sub".into()), false);
        assert!(!s.subagent_inherit);
        assert_eq!(s.model_for(CcSlot::Subagent), Some("vendor/sub"));
    }

    #[test]
    fn cc_slots_prune_drops_models_missing_from_catalog() {
        use crate::claude::CcSlot;
        let mut s = CcSlots::default();
        s.set(CcSlot::Opus, Some("gone".into()), false);
        s.set(CcSlot::Sonnet, Some("kept".into()), false);
        let changed = s.prune_to_catalog(&["kept".to_string()]);
        assert!(changed);
        assert_eq!(s.opus, None);
        assert_eq!(s.sonnet.as_deref(), Some("kept"));
        // A second prune with everything present is a no-op.
        assert!(!s.prune_to_catalog(&["kept".to_string()]));
    }

    #[test]
    fn roundtrips_cc_slots_per_endpoint() {
        use crate::claude::CcSlot;
        let path = std::env::temp_dir().join("copilot_proxy_uistate_ccslots_test.json");
        let _ = std::fs::remove_file(&path);

        let mut slots = CcSlots::default();
        slots.set(CcSlot::Opus, Some("vendor/opus".into()), false);
        slots.set(CcSlot::Subagent, None, true);

        let mut file = UiStateFile::default();
        file.cc_slot_models
            .insert("https://openrouter.ai/api/v1".into(), slots.clone());
        file.save(&path).unwrap();

        let loaded = UiStateFile::load(&path);
        assert_eq!(
            loaded.cc_slot_models.get("https://openrouter.ai/api/v1"),
            Some(&slots)
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn old_file_without_selected_models_loads() {
        // A file written by an older version (only `visible_models`) must still
        // parse — `selected_models` defaults to empty via `#[serde(default)]`.
        let path = std::env::temp_dir().join("copilot_proxy_uistate_legacy_test.json");
        std::fs::write(&path, r#"{"visible_models":{"https://a/v1":["m1"]}}"#).unwrap();
        let loaded = UiStateFile::load(&path);
        assert_eq!(
            loaded.visible_models.get("https://a/v1").unwrap(),
            &vec!["m1".to_string()]
        );
        assert!(loaded.selected_models.is_empty());
        let _ = std::fs::remove_file(&path);
    }
}
