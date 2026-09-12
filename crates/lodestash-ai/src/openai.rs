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

/// An endpoint preset for the CLOUD settings card (AI-PIPELINE §5.1 "Endpoint presets"). Selecting
/// one fills `base_url` so a new user never has to know that "OpenAI-compatible" is a wire format,
/// not an OpenAI account. `Custom…` (handled in the UI) exposes the raw field.
pub struct Preset {
    pub label: &'static str,
    pub base_url: &'static str,
    /// A hint shown in the Model field — guidance, not a committed value (verify the live id).
    pub model_hint: &'static str,
    /// Free/local tiers skip the budget-guard confirm; a paid endpoint gets one.
    pub free: bool,
}

/// The presets, in menu order. Local first (fully private, zero cost), then the free hosted tiers,
/// then a paid aggregator. `Custom…` is a UI-only entry that reveals the raw base_url field.
///
/// Model ids are intentionally *hints*, not values — providers rename models often, so the UI
/// prefills only `base_url` (stable) and shows the hint as placeholder text (AI-PIPELINE §5 note:
/// verify ids against the provider). URLs are the well-known OpenAI-compatible bases.
pub const PRESETS: &[Preset] = &[
    Preset {
        label: "Local server",
        base_url: "http://localhost:1234/v1",
        model_hint: "your local model id (LM Studio / vLLM)",
        free: true,
    },
    Preset {
        label: "Gemini (free)",
        base_url: DEFAULT_BASE_URL,
        model_hint: "a Gemini flash vision model",
        free: true,
    },
    Preset {
        label: "Groq (free)",
        base_url: "https://api.groq.com/openai/v1",
        model_hint: "a Groq vision model",
        free: true,
    },
    Preset {
        label: "OpenRouter",
        base_url: "https://openrouter.ai/api/v1",
        model_hint: "provider/model (e.g. a free vision model)",
        free: false,
    },
];

/// The preset whose base_url matches (trailing slash ignored), if any — else `None` = Custom.
pub fn preset_for(base_url: &str) -> Option<&'static Preset> {
    let want = base_url.trim().trim_end_matches('/');
    PRESETS
        .iter()
        .find(|p| p.base_url.trim_end_matches('/') == want)
}

/// Whether an endpoint **bills**, so the budget-guard confirm can be skipped. A self-hosted server
/// costs nothing, and a preset known to be free costs nothing; an unknown remote is treated as paid
/// — the safe default when we cannot prove otherwise.
///
/// Price is not locality (v1 plan S10). A free remote endpoint is still remote, and whether bytes
/// may go there at all is [`crate::endpoint::locality`] plus the user's consent, never this.
pub fn is_free_endpoint(base_url: &str) -> bool {
    if crate::endpoint::locality(base_url) == crate::endpoint::Locality::Loopback {
        return true;
    }
    preset_for(base_url).is_some_and(|p| p.free)
}

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
    /// Where this tagger sends bytes — what the egress gate is asked about (v1 plan S10).
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

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

    /// One round trip against a generated sample image — the CLOUD card's **Test** action
    /// (AI-PIPELINE §5.1), "the single cheapest fix for the #1 failure mode".
    ///
    /// A wrong `base_url`, a non-vision model id or a dead key all produce *nothing* from a bulk
    /// run and no signal at all, and a full pass is hours long. This exercises the real path — same
    /// prompt, same `image_url` body, same error classification — so the answer is the whole
    /// configuration's verdict rather than a reachability ping. The raw assistant text comes back
    /// unparsed on purpose: a model that replies in prose instead of the facet object is
    /// *reachable but useless here*, and only the raw reply shows that.
    ///
    /// Deliberately short-timeout: a preflight the user is watching must not hang for two minutes.
    pub fn probe(&self) -> Result<String, ApiError> {
        let png =
            encode_png(&probe_image()).map_err(|e| ApiError::Hard(format!("png encode: {e}")))?;
        let input = TagInput {
            media_code: "image",
            name: "sample",
            image: None,
        };
        let prompt = build_prompt(&input, None, &self.teach);
        // A preflight the user is watching must not hang for the tagging timeout.
        let quick = Self {
            timeout: Duration::from_secs(30),
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
            model: self.model.clone(),
            teach: self.teach.clone(),
        };
        quick.complete(&prompt, &png)
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
        // **No redirects** (v1 plan S10). The request carries the key and the image; a 3xx would
        // hand both to whatever origin the response names, after the fact. A moved endpoint is a
        // configuration change the user makes knowingly, not one a server announces mid-upload.
        let agent = ureq::builder().redirects(0).timeout(self.timeout).build();
        let resp = agent
            .post(&url)
            .set("Authorization", &format!("Bearer {}", self.api_key))
            .send_json(body)
            .map_err(classify_ureq)?;
        if (300..400).contains(&resp.status()) {
            let to = resp.header("location").unwrap_or("elsewhere").to_string();
            return Err(ApiError::Hard(format!(
                "endpoint redirected to {to}; nothing was followed — point the setting at the                  final URL if that is where it should go"
            )));
        }
        let resp: serde_json::Value = resp
            .into_json()
            .map_err(|e| ApiError::Hard(e.to_string()))?;
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

/// The Test action's subject: a red disc on white.
///
/// Generated rather than bundled — no asset to ship, no path to resolve, and it works on a fresh
/// install with an empty library. A flat colour would not do: a model can answer "white" about a
/// broken decode, so the picture carries a shape *and* a colour that a working vision model names
/// and a text-only one cannot.
fn probe_image() -> image::RgbaImage {
    const N: u32 = 256;
    let (c, r2) = (N as f32 / 2.0, (N as f32 * 0.35).powi(2));
    image::RgbaImage::from_fn(N, N, |x, y| {
        let (dx, dy) = (x as f32 - c, y as f32 - c);
        if dx * dx + dy * dy <= r2 {
            image::Rgba([200, 40, 40, 255])
        } else {
            image::Rgba([255, 255, 255, 255])
        }
    })
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

    fn producer_id(&self) -> String {
        format!(
            "openai-compatible@{}",
            crate::tagger::endpoint_host(&self.base_url)
        )
    }

    fn supports(&self, media_code: &str) -> bool {
        // No cloud-audio path (AI-PIPELINE §5) — audio is described locally, never uploaded.
        media_code != "audio"
    }

    /// This tagger sends the pixels itself, so the generic two-phase batch path — which never fills
    /// `TagInput::image` — cannot drive it. `drains::drain_api_tags` is the drain that can.
    fn needs_image(&self) -> bool {
        true
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

    /// A redirect is not a route (v1 plan S10): the request carries the API key and the image, so a
    /// `302` is refused and the origin it names never hears from us.
    #[test]
    fn a_redirect_is_refused_and_the_new_origin_is_never_contacted() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::atomic::{AtomicUsize, Ordering};

        // Where the redirect would send the key and the picture.
        let elsewhere = TcpListener::bind("127.0.0.1:0").expect("bind");
        let elsewhere_port = elsewhere.local_addr().unwrap().port();
        let reached = std::sync::Arc::new(AtomicUsize::new(0));
        let counter = reached.clone();
        elsewhere.set_nonblocking(true).expect("nonblocking");
        let watcher = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while std::time::Instant::now() < deadline {
                if elsewhere.accept().is_ok() {
                    counter.fetch_add(1, Ordering::Relaxed);
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        });

        let endpoint = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = endpoint.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            if let Ok((mut sock, _)) = endpoint.accept() {
                // Drain the whole request before answering: closing on a half-sent upload is a
                // transport error, and this test is about what the *response* does.
                let _ = sock.set_read_timeout(Some(std::time::Duration::from_millis(150)));
                let mut buf = [0u8; 8192];
                while let Ok(n) = sock.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                }
                let _ = sock.write_all(
                    format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{elsewhere_port}/v1/chat/completions\r\nContent-Length: 0\r\n\r\n"
                    )
                    .as_bytes(),
                );
                let _ = sock.flush();
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        });

        let tagger = OpenAiTagger::new(format!("http://127.0.0.1:{port}/v1"), "secret", "m");
        let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 40, 40, 255]));
        let err = tagger
            .try_tag(&TagInput {
                media_code: "image",
                name: "rock",
                image: Some(&img),
            })
            .expect_err("a redirect is not an answer");
        assert!(
            format!("{err}").contains("redirected"),
            "unexpected error: {err}"
        );
        let _ = server.join();
        let _ = watcher.join();
        assert_eq!(
            reached.load(Ordering::Relaxed),
            0,
            "the redirect target was contacted"
        );
    }

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

    /// The Test preflight's subject must carry a shape *and* a colour.
    ///
    /// A flat image would make the check worthless: a model answering "white" proves nothing about
    /// whether it saw pixels at all, which is the one thing this action exists to establish.
    #[test]
    fn the_probe_image_is_not_a_flat_colour() {
        let img = probe_image();
        let centre = img.get_pixel(img.width() / 2, img.height() / 2);
        let corner = img.get_pixel(0, 0);
        assert_ne!(
            centre, corner,
            "a preflight image a vision model cannot describe tests nothing"
        );
        assert!(
            img.pixels().all(|p| p[3] == 255),
            "fully opaque, or a provider that drops alpha sees a different picture"
        );
    }

    #[test]
    fn audio_is_never_supported() {
        let t = OpenAiTagger::new(DEFAULT_BASE_URL, "k", "m");
        assert!(!t.supports("audio"));
        assert!(t.supports("model_3d"));
        assert!(t.supports("image"));
    }

    #[test]
    fn presets_round_trip_and_free_detection() {
        // Default base URL resolves to the Gemini preset; a trailing slash doesn't break the match.
        assert_eq!(
            preset_for(DEFAULT_BASE_URL).map(|p| p.label),
            Some("Gemini (free)")
        );
        let with_slash = format!("{DEFAULT_BASE_URL}/");
        assert_eq!(
            preset_for(&with_slash).map(|p| p.label),
            Some("Gemini (free)")
        );
        // An unknown remote is Custom (None) and, crucially, treated as *paid* so the budget guard
        // asks before spending; a loopback URL is always free even when it's not a listed preset.
        assert!(preset_for("https://api.example.com/v1").is_none());
        assert!(!is_free_endpoint("https://api.example.com/v1"));
        assert!(is_free_endpoint("http://127.0.0.1:8000/v1"));
        assert!(is_free_endpoint("http://localhost:1234/v1"));
        assert!(is_free_endpoint(DEFAULT_BASE_URL));
    }

    #[test]
    fn classify_splits_retry_worthy_from_hard() {
        // 429/503 are throttles the drain should back off and resume; 401/400 are hard.
        let throttle = classify_ureq(ureq::Error::Status(
            429,
            ureq::Response::new(429, "x", "").unwrap(),
        ));
        assert!(
            matches!(throttle, ApiError::Throttled { .. }),
            "429 must be a throttle"
        );
        let unavailable = classify_ureq(ureq::Error::Status(
            503,
            ureq::Response::new(503, "x", "").unwrap(),
        ));
        assert!(
            matches!(unavailable, ApiError::Throttled { .. }),
            "503 must be a throttle"
        );

        let bad_key = classify_ureq(ureq::Error::Status(
            401,
            ureq::Response::new(401, "x", "nope").unwrap(),
        ));
        match bad_key {
            ApiError::Hard(m) => assert!(m.contains("401"), "hard error keeps the status code"),
            _ => panic!("401 must be a hard error, not retried forever"),
        }
    }
}
