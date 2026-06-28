//! Claude Code (Anthropic Messages API) specifics: the five model slots, their
//! stable `proxy-cc/*` request labels, and the env vars Claude Code reads them
//! from. Kept in one module so the slot list is a single source of truth shared
//! by the proxy (label→model mapping), the launcher (env vars), and the UI/tray.

/// One of Claude Code's model slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CcSlot {
    Opus,
    Sonnet,
    Haiku,
    Fable,
    Subagent,
}

impl CcSlot {
    /// Every slot, in display order.
    pub const ALL: &'static [CcSlot] = &[
        CcSlot::Opus,
        CcSlot::Sonnet,
        CcSlot::Haiku,
        CcSlot::Fable,
        CcSlot::Subagent,
    ];

    /// Stable slot id used in the UI, tray menu ids, and the `proxy-cc/<id>`
    /// request label.
    pub fn id(self) -> &'static str {
        match self {
            CcSlot::Opus => "opus",
            CcSlot::Sonnet => "sonnet",
            CcSlot::Haiku => "haiku",
            CcSlot::Fable => "fable",
            CcSlot::Subagent => "subagent",
        }
    }

    /// Capitalized name for status lines and error messages ("Opus", …).
    pub fn display_name(self) -> &'static str {
        match self {
            CcSlot::Opus => "Opus",
            CcSlot::Sonnet => "Sonnet",
            CcSlot::Haiku => "Haiku",
            CcSlot::Fable => "Fable",
            CcSlot::Subagent => "Subagent",
        }
    }

    /// The stable model label Claude Code sends for this slot; the proxy maps it
    /// to the configured catalog id at request time.
    pub fn label(self) -> &'static str {
        match self {
            CcSlot::Opus => "proxy-cc/opus",
            CcSlot::Sonnet => "proxy-cc/sonnet",
            CcSlot::Haiku => "proxy-cc/haiku",
            CcSlot::Fable => "proxy-cc/fable",
            CcSlot::Subagent => "proxy-cc/subagent",
        }
    }

    /// The environment variable Claude Code reads this slot's model from.
    pub fn env_var(self) -> &'static str {
        match self {
            CcSlot::Opus => "ANTHROPIC_DEFAULT_OPUS_MODEL",
            CcSlot::Sonnet => "ANTHROPIC_DEFAULT_SONNET_MODEL",
            CcSlot::Haiku => "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            CcSlot::Fable => "ANTHROPIC_DEFAULT_FABLE_MODEL",
            CcSlot::Subagent => "CLAUDE_CODE_SUBAGENT_MODEL",
        }
    }

    /// Parses a slot id ("opus", …) from the UI / tray.
    pub fn from_id(id: &str) -> Option<CcSlot> {
        CcSlot::ALL.iter().copied().find(|s| s.id() == id)
    }

    /// Parses a `proxy-cc/<id>` request label back to its slot.
    pub fn from_label(label: &str) -> Option<CcSlot> {
        label.strip_prefix("proxy-cc/").and_then(CcSlot::from_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_roundtrips_through_from_label() {
        for &slot in CcSlot::ALL {
            assert_eq!(CcSlot::from_label(slot.label()), Some(slot));
        }
        assert_eq!(CcSlot::from_label("gpt-4o"), None);
        assert_eq!(CcSlot::from_label("proxy-cc/unknown"), None);
    }

    #[test]
    fn ids_and_env_vars_are_distinct() {
        let labels: Vec<_> = CcSlot::ALL.iter().map(|s| s.label()).collect();
        let envs: Vec<_> = CcSlot::ALL.iter().map(|s| s.env_var()).collect();
        for i in 0..labels.len() {
            for j in (i + 1)..labels.len() {
                assert_ne!(labels[i], labels[j]);
                assert_ne!(envs[i], envs[j]);
            }
        }
        // Subagent uses Claude Code's dedicated env var, not the ANTHROPIC_DEFAULT_* family.
        assert_eq!(CcSlot::Subagent.env_var(), "CLAUDE_CODE_SUBAGENT_MODEL");
    }
}
