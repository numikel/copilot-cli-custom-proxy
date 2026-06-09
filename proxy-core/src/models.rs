//! Model catalog types shared by the proxy, the tray menu, and the settings UI.
//!
//! The upstream `/models` endpoint only reports ids, so the kind (chat vs the
//! non-chat families) is inferred from the id with [`classify_model`]. Keeping
//! this here — rather than in the front-end — means the native tray and the
//! webview agree on what counts as a "chat" model.

use serde::Serialize;

/// A model offered by the upstream, with its inferred classification.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModelInfo {
    /// The model id as returned by the upstream (substituted into requests).
    pub id: String,
    /// True for chat/completion models — the ones agents actually use.
    pub chat: bool,
    /// The non-chat family, when this is not a chat model.
    pub kind: Option<ModelKind>,
}

/// Non-chat model families, tagged in the UI (`embed`, `image`, …).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelKind {
    Embed,
    Image,
    Audio,
    Rerank,
    Moderation,
}

impl ModelKind {
    /// Lowercase tag used by the UI (matches the `cp-kindtag--*` CSS classes).
    pub fn as_str(self) -> &'static str {
        match self {
            ModelKind::Embed => "embed",
            ModelKind::Image => "image",
            ModelKind::Audio => "audio",
            ModelKind::Rerank => "rerank",
            ModelKind::Moderation => "moderation",
        }
    }
}

/// Infers a model's classification from its id.
///
/// Heuristic, case-insensitive match against the id's *word tokens* (split on
/// `-`, `_`, `.`, `/`, `:`, space). Anything that doesn't look like a known
/// non-chat family is treated as a chat model (`chat: true`, `kind: None`) —
/// the common case.
pub fn classify_model(id: &str) -> ModelInfo {
    let lower = id.to_ascii_lowercase();
    let kind = infer_kind(&lower);
    ModelInfo {
        id: id.to_string(),
        chat: kind.is_none(),
        kind,
    }
}

/// Whether any word token of `lower` matches `marker` — equal to it, or
/// starting with it (so `embedding` matches `embed`, `reranker` matches
/// `rerank`). Matching on token boundaries instead of raw substrings stops
/// false hits like `watts` for `tts` or `vanguard` for `guard`, which would
/// otherwise hide perfectly good chat models from the tray.
fn has_marker(lower: &str, marker: &str) -> bool {
    lower
        .split(['-', '_', '.', '/', ':', ' '])
        .any(|token| token == marker || token.starts_with(marker))
}

/// Returns the non-chat family for a lowercased id, or `None` for chat models.
/// Order matters: more specific markers are checked before broader ones.
fn infer_kind(lower: &str) -> Option<ModelKind> {
    // Reranking (e.g. "rerank-english-v3", "bge-reranker").
    if has_marker(lower, "rerank") {
        return Some(ModelKind::Rerank);
    }
    // Content moderation / safety classifiers (e.g. "omni-moderation", "llama-guard").
    if has_marker(lower, "moderation") || has_marker(lower, "guard") {
        return Some(ModelKind::Moderation);
    }
    // Text embeddings (e.g. "text-embedding-3-large", "bge-embedding").
    if has_marker(lower, "embed") {
        return Some(ModelKind::Embed);
    }
    // Speech/audio (e.g. "whisper-1", "tts-1", "gpt-4o-transcribe").
    if has_marker(lower, "whisper")
        || has_marker(lower, "tts")
        || has_marker(lower, "audio")
        || has_marker(lower, "speech")
        || has_marker(lower, "transcribe")
    {
        return Some(ModelKind::Audio);
    }
    // Image generation (e.g. "dall-e-3", "gpt-image-1", "stable-diffusion").
    // `dall-e` straddles the token split, so it is matched as a raw substring;
    // the rest go through the token matcher.
    if lower.contains("dall-e")
        || has_marker(lower, "dalle")
        || has_marker(lower, "image")
        || has_marker(lower, "diffusion")
    {
        return Some(ModelKind::Image);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind_of(id: &str) -> Option<ModelKind> {
        classify_model(id).kind
    }

    #[test]
    fn classifies_embeddings() {
        assert_eq!(kind_of("text-embedding-3-large"), Some(ModelKind::Embed));
        assert_eq!(kind_of("bge-embedding"), Some(ModelKind::Embed));
    }

    #[test]
    fn classifies_images() {
        assert_eq!(kind_of("dall-e-3"), Some(ModelKind::Image));
        assert_eq!(kind_of("gpt-image-1"), Some(ModelKind::Image));
        assert_eq!(kind_of("stable-diffusion-xl"), Some(ModelKind::Image));
    }

    #[test]
    fn classifies_audio() {
        assert_eq!(kind_of("whisper-1"), Some(ModelKind::Audio));
        assert_eq!(kind_of("tts-1"), Some(ModelKind::Audio));
        assert_eq!(kind_of("gpt-4o-transcribe"), Some(ModelKind::Audio));
    }

    #[test]
    fn classifies_rerank() {
        assert_eq!(kind_of("rerank-english-v3.0"), Some(ModelKind::Rerank));
        assert_eq!(kind_of("bge-reranker-large"), Some(ModelKind::Rerank));
    }

    #[test]
    fn classifies_moderation() {
        assert_eq!(kind_of("omni-moderation-latest"), Some(ModelKind::Moderation));
        assert_eq!(kind_of("llama-guard-3-8b"), Some(ModelKind::Moderation));
    }

    #[test]
    fn treats_unknown_as_chat() {
        for id in ["gpt-4o", "claude-3-7-sonnet", "o1-preview", "gemini-2.0-flash"] {
            let m = classify_model(id);
            assert!(m.chat, "{id} should be a chat model");
            assert_eq!(m.kind, None);
        }
    }

    #[test]
    fn token_boundaries_dont_swallow_chat_models() {
        // Regression: a raw substring scan misread these as non-chat and hid
        // them from the tray. "watts" contains "tts", "vanguard" contains
        // "guard" — but neither is a token match.
        for id in ["watts-3b", "vanguard-instruct", "seaguard"] {
            let m = classify_model(id);
            assert!(m.chat, "{id} should be a chat model");
            assert_eq!(m.kind, None, "{id} should have no non-chat kind");
        }
        // ...while the genuine short-marker models still classify correctly.
        assert_eq!(kind_of("tts-1"), Some(ModelKind::Audio));
        assert_eq!(kind_of("llama-guard-3-8b"), Some(ModelKind::Moderation));
    }

    #[test]
    fn classification_is_case_insensitive() {
        assert_eq!(kind_of("Text-Embedding-3-Large"), Some(ModelKind::Embed));
        assert_eq!(kind_of("WHISPER-1"), Some(ModelKind::Audio));
    }

    #[test]
    fn model_info_carries_original_id() {
        let m = classify_model("GPT-4o");
        assert_eq!(m.id, "GPT-4o", "original casing is preserved in the id");
        assert!(m.chat);
    }
}
