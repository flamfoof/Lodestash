//! `ApiTagger` over the **OpenAI-compatible** chat-completions + vision shape (AI-PIPELINE §5).
//!
//! One impl, wide reach: the same `POST {base_url}/chat/completions` with a base64 `image_url`
//! content block is spoken by OpenAI, Google **Gemini** (its OpenAI-compatible endpoint), Groq,
//! OpenRouter, and any local server (LM Studio / llama.cpp / vLLM) — including the free tiers that
//! make cloud tagging affordable. Claude's native Messages API is a different shape and lands later
//! behind the same [`Tagger`] trait. Audio is never sent here (no cloud-audio path); the drain only
//! hands this tagger the visual modalities.
//!
//! A capable cloud VLM holds a schema in one call, so unlike the local two-stage Ollama path this
//! sends the image and asks for the facet JSON directly — the same prompt and the same
//! validate-then-write rule, so the store never sees a facet the grammar doesn't have.

use crate::ollama::{build_prompt, encode_png, parse_facets};
use crate::tagger::{TagInput, TagResult, Tagger, TeachTrait};
use std::time::Duration;

/// Gemini's OpenAI-compatible base URL — the default because its free tier makes cloud tagging
/// affordable at all. Any OpenAI-API host (a paid provider or a local server) is a drop-in.
pub const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta/openai";

/// A tagger backed by any OpenAI-compatible `/chat/completions` vision endpoint.
pub struct OpenAiTagger {
    base_url: String,
    api_key: String,
    model: String,
    teach: Vec<TeachTrait>,
    timeout: Duration,
}

impl OpenAiTagger {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: api_key.into(),
            model: model.into(),
            teach: Vec::new(),
            timeout: Duration::from_secs(120),
        }
    }

    /// Feed the user's teaching traits into the prompt (AI-PIPELINE §6), same as the local path.
    pub fn with_teaching(mut self, teach: Vec<TeachTrait>) -> Self {
        self.teach = teach;
        self
    }

    /// One blocking chat-completions call with the image inline. Returns the assistant's text, which
    /// [`parse_facets`] then reads as the facet object.
    fn complete(&self, prompt: &str, image_png: &[u8]) -> Result<String, String> {
        let data_url = format!("data:image/png;base64,{}", crate::ollama::base64(image_png));
        let body = serde_json::json!({
            "model": self.model,
            "temperature": 0.0,
            // JSON mode where the provider honours it; harmless where it doesn't (parse_facets still
            // salvages the object, and a provider that rejects the field is a config error we surface).
            "response_format": { "type": "json_object" },
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": prompt },
                    { "type": "image_url", "image_url": { "url": data_url } }
                ]
            }]
        });
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let resp: serde_json::Value = ureq::post(&url)
            .timeout(self.timeout)
            .set("Authorization", &format!("Bearer {}", self.api_key))
            .send_json(body)
            .map_err(|e| e.to_string())?
            .into_json()
            .map_err(|e| e.to_string())?;
        resp["choices"][0]["message"]["content"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| "no message content in response".to_string())
    }
}

impl Tagger for OpenAiTagger {
    fn model_id(&self) -> &str {
        &self.model
    }

    fn supports(&self, media_code: &str) -> bool {
        // No cloud-audio path (AI-PIPELINE §5) — audio is described locally, never uploaded.
        media_code != "audio"
    }

    /// Single-call facet tagging: the image goes with the prompt and the model returns the JSON. The
    /// `caption` argument is ignored — a cloud VLM doesn't need the local caption pre-step.
    fn tag(&self, input: &TagInput<'_>, _caption: Option<&str>) -> TagResult {
        let Some(img) = input.image else {
            return TagResult::default(); // nothing to look at ⇒ nothing said
        };
        let png = match encode_png(img) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(%e, "api tagger: png encode failed");
                return TagResult::default();
            }
        };
        let prompt = build_prompt(input, None, &self.teach);
        match self.complete(&prompt, &png) {
            // A cloud VLM is a stronger claim than a local name-only guess: 0.85, still < 1.0 (only
            // a user-set tag is exact). validate() runs on the caller's side as always.
            Ok(text) => TagResult::from_tags(parse_facets(&text, 0.85)),
            Err(e) => {
                tracing::warn!(%e, "api tag request failed");
                TagResult::default()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_image_yields_no_tags_without_a_request() {
        // No network: an imageless input must short-circuit (audio never reaches here anyway).
        let t = OpenAiTagger::new("http://127.0.0.1:0", "k", "m");
        let input = TagInput {
            media_code: "model_3d",
            name: "SM_Rock",
            image: None,
        };
        assert!(t.tag(&input, None).tags.is_empty());
    }

    #[test]
    fn audio_is_never_supported() {
        let t = OpenAiTagger::new(DEFAULT_BASE_URL, "k", "m");
        assert!(!t.supports("audio"));
        assert!(t.supports("model_3d"));
        assert!(t.supports("image"));
    }
}
