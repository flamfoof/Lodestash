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

/// A classified failure of one API call (AI-PIPELINE §5.1). The distinction drives the drain: a
/// **`Throttled`** call is worth waiting and retrying (free tiers cap ~15 req/min), a **`Hard`**
/// call (bad key, quota, non-vision model, network) is not — it falls back to the local model or
/// leaves the asset queued. Neither is ever written as "done, no tags."
#[derive(Debug, Clone)]
pub enum ApiError {
    /// `429`/`503` — retry after backing off. Carries the server's `Retry-After` when it sent one.
    Throttled { retry_after: Option<Duration> },
    /// Anything else: the request will not succeed by retrying as-is.
    Hard(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Throttled { retry_after } => {
                write!(f, "rate limited (retry-after: {retry_after:?})")
            }
            ApiError::Hard(m) => write!(f, "{m}"),
        }
    }
}

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
    /// [`parse_facets`] then reads as the facet object. Errors are classified ([`ApiError`]) so the
    /// caller can back off a throttle but not a dead key.
    fn complete(&self, prompt: &str, image_png: &[u8]) -> Result<String, ApiError> {
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
        let resp = ureq::post(&url)
            .timeout(self.timeout)
            .set("Authorization", &format!("Bearer {}", self.api_key))
            .send_json(body)
            .map_err(classify_ureq)?;
        let resp: serde_json::Value = resp.into_json().map_err(|e| ApiError::Hard(e.to_string()))?;
        resp["choices"][0]["message"]["content"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| ApiError::Hard("no message content in response".to_string()))
    }

    /// One attempt at facet tagging. Returns the proposed tags, `Ok(empty)` when there is nothing to
    /// look at, or a classified [`ApiError`]. **No retry here** — backoff/resume and local fallback
    /// are the drain's job (AI-PIPELINE §5.1), where cancellation and the queue flag live.
    pub fn try_tag(&self, input: &TagInput<'_>) -> Result<TagResult, ApiError> {
        let Some(img) = input.image else {
            return Ok(TagResult::default()); // nothing to look at ⇒ nothing said (not a failure)
        };
        let png = encode_png(img).map_err(|e| ApiError::Hard(format!("png encode: {e}")))?;
        let prompt = build_prompt(input, None, &self.teach);
        let text = self.complete(&prompt, &png)?;
        Ok(TagResult::from_tags(parse_facets(&text, 0.85)))
    }
}

/// Map a `ureq` failure to the retry/no-retry split. `429`/`503` are worth backing off; everything
/// else (auth, 4xx config errors, transport) will not fix itself by retrying the same request.
fn classify_ureq(e: ureq::Error) -> ApiError {
    match e {
        ureq::Error::Status(429 | 503, resp) => ApiError::Throttled {
            retry_after: resp
                .header("Retry-After")
                .and_then(|v| v.trim().parse::<u64>().ok())
                .map(Duration::from_secs),
        },
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            let snippet: String = body.chars().take(200).collect();
            ApiError::Hard(format!("HTTP {code}: {snippet}"))
        }
        ureq::Error::Transport(t) => ApiError::Hard(t.to_string()),
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
    ///
    /// This trait method flattens an error to an empty result for generic callers; the drain uses
    /// [`Self::try_tag`] directly so it can tell a throttle/failure apart from an honest empty answer
    /// (AI-PIPELINE §5.1) and never clear a queued asset it didn't actually tag.
    fn tag(&self, input: &TagInput<'_>, _caption: Option<&str>) -> TagResult {
        // A cloud VLM is a stronger claim than a local name-only guess: 0.85, still < 1.0 (only a
        // user-set tag is exact). validate() runs on the caller's side as always.
        self.try_tag(input).unwrap_or_else(|e| {
            tracing::warn!(%e, "api tag request failed");
            TagResult::default()
        })
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

    #[test]
    fn classify_splits_retry_worthy_from_hard() {
        // 429/503 are throttles the drain should back off and resume; 401/400 are hard.
        let throttle =
            classify_ureq(ureq::Error::Status(429, ureq::Response::new(429, "x", "").unwrap()));
        assert!(matches!(throttle, ApiError::Throttled { .. }), "429 must be a throttle");
        let unavailable =
            classify_ureq(ureq::Error::Status(503, ureq::Response::new(503, "x", "").unwrap()));
        assert!(matches!(unavailable, ApiError::Throttled { .. }), "503 must be a throttle");

        let bad_key =
            classify_ureq(ureq::Error::Status(401, ureq::Response::new(401, "x", "nope").unwrap()));
        match bad_key {
            ApiError::Hard(m) => assert!(m.contains("401"), "hard error keeps the status code"),
            _ => panic!("401 must be a hard error, not retried forever"),
        }
    }
}
