//! Local tagging through **Ollama** (AI-PIPELINE §2 `LocalTagger`): a loopback HTTP call to a
//! model already installed on the machine. Nothing leaves the box, so this stays inside the
//! local-first invariant — unlike the cloud tagger, it needs no opt-in.
//!
//! One model serves both shapes. A vision model is handed the asset's preview (`images`) and
//! describes what it sees; a text-only model is handed the asset's **name**, which in a game
//! library is real signal (`SM_Env_Wall_01_Alt`). The prompt and the parse are identical, so the
//! difference is one JSON field.

use std::time::Duration;

use lodestash_core::catalog::TagAssignment;

use crate::tagger::{TagInput, Tagger, MODEL_FACETS};

/// Default Ollama endpoint. Loopback only — a remote host would put asset previews on the network,
/// which the local-first invariant forbids without an explicit opt-in path.
pub const DEFAULT_HOST: &str = "http://127.0.0.1:11434";

/// A tagger backed by an Ollama model.
pub struct OllamaTagger {
    host: String,
    model: String,
    vision: bool,
    timeout: Duration,
}

impl OllamaTagger {
    /// Build a tagger for `model` (e.g. `moondream`, `qwen2.5vl:3b`, `qwen3:8b`).
    /// `vision` says whether to send pixels; a text-only model given an image answers nonsense.
    pub fn new(model: impl Into<String>, vision: bool) -> Self {
        Self {
            host: DEFAULT_HOST.to_string(),
            model: model.into(),
            vision,
            timeout: Duration::from_secs(120),
        }
    }

    pub fn with_host(mut self, host: impl Into<String>) -> Self {
        self.host = host.into();
        self
    }

    /// Models Ollama currently has installed, or `Err` when the daemon isn't reachable. Used to
    /// decide whether local tagging is available at all — an absent Ollama is a normal state, not
    /// an error the user should see as a failure.
    pub fn installed_models(host: &str) -> Result<Vec<String>, String> {
        let body: serde_json::Value = ureq::get(&format!("{host}/api/tags"))
            .timeout(Duration::from_secs(5))
            .call()
            .map_err(|e| e.to_string())?
            .into_json()
            .map_err(|e| e.to_string())?;
        Ok(body["models"]
            .as_array()
            .map(|models| {
                models
                    .iter()
                    .filter_map(|m| m["name"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default())
    }

    /// One blocking generate call, returning the model's raw response text.
    fn generate(&self, prompt: &str, image_png: Option<&[u8]>) -> Result<String, String> {
        let mut body = serde_json::json!({
            "model": self.model,
            "prompt": prompt,
            "stream": false,
            // `format: json` makes Ollama constrain decoding to valid JSON, which removes the
            // single most common failure of small models: prose wrapped around the answer.
            "format": "json",
            "options": { "temperature": 0.0 },
        });
        if let Some(png) = image_png {
            body["images"] = serde_json::json!([base64(png)]);
        }
        let resp: serde_json::Value = ureq::post(&format!("{}/api/generate", self.host))
            .timeout(self.timeout)
            .send_json(body)
            .map_err(|e| e.to_string())?
            .into_json()
            .map_err(|e| e.to_string())?;
        resp["response"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| "ollama response had no `response` field".to_string())
    }
}

/// The instruction both shapes share. The facet list is injected from [`MODEL_FACETS`] so the
/// prompt can't drift from what `validate` will accept.
fn build_prompt(input: &TagInput<'_>, vision: bool) -> String {
    let facets = MODEL_FACETS.join(", ");
    let subject = if vision {
        "Look at this game asset preview.".to_string()
    } else {
        format!(
            "A game asset file is named \"{}\". Infer only what the name actually supports.",
            input.name
        )
    };
    format!(
        "{subject}\n\
         It is a {} asset in a game-asset library.\n\
         Reply with JSON only: an object whose keys are facets and whose values are arrays of \
         short lowercase terms.\n\
         Allowed facets: {facets}.\n\
         Use at most 4 terms per facet, and omit a facet entirely rather than guessing. \
         Do not describe colours, resolution, polygon counts or file formats.",
        media_word(input.media_code)
    )
}

fn media_word(code: &str) -> &str {
    match code {
        "model_3d" => "3D model",
        "audio" => "audio",
        "material" => "material",
        "font" => "font",
        _ => "2D image",
    }
}

/// Parse `{"subject": ["rock"], "style": ["low-poly"]}` into assignments. Unknown facets and
/// non-string values are dropped here; [`crate::tagger::validate`] still runs afterwards, since
/// this is the model's output and the rule is validate-then-write.
fn parse_facets(json: &str, confidence: f64) -> Vec<TagAssignment> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(obj) = value.as_object() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (facet, values) in obj {
        let facet = facet.trim().to_lowercase();
        if !MODEL_FACETS.contains(&facet.as_str()) {
            continue;
        }
        // Tolerate a bare string where the schema asks for an array — small models do it often
        // enough that rejecting the whole answer over it would be the wrong trade.
        let items: Vec<&str> = match values {
            serde_json::Value::Array(a) => a.iter().filter_map(|v| v.as_str()).collect(),
            serde_json::Value::String(s) => vec![s.as_str()],
            _ => continue,
        };
        for item in items {
            let tag = TagAssignment {
                namespace: facet.clone(),
                value: item.trim().to_string(),
                confidence: Some(confidence),
            };
            if !tag.value.is_empty() && !out.contains(&tag) {
                out.push(tag);
            }
        }
    }
    out
}

impl Tagger for OllamaTagger {
    fn model_id(&self) -> &str {
        &self.model
    }

    fn supports(&self, media_code: &str) -> bool {
        // Audio needs the CLAP/PANNs path, not a language model guessing from a filename.
        media_code != "audio"
    }

    fn tag(&self, input: &TagInput<'_>) -> Vec<TagAssignment> {
        let png = match (self.vision, input.image) {
            (true, Some(img)) => match encode_png(img) {
                Ok(bytes) => Some(bytes),
                Err(e) => {
                    tracing::warn!(%e, "ollama tagger: preview encode failed; falling back to name");
                    None
                }
            },
            _ => None,
        };
        let prompt = build_prompt(input, png.is_some());
        match self.generate(&prompt, png.as_deref()) {
            // A name-only inference is a weaker claim than one made from pixels, and the
            // confidence has to say so — the UI hides low-confidence tags rather than deleting
            // them (IDENTIFICATION-AND-SEARCH §2).
            Ok(text) => parse_facets(&text, if png.is_some() { 0.7 } else { 0.4 }),
            Err(e) => {
                tracing::warn!(model = %self.model, %e, "ollama tagging failed");
                Vec::new()
            }
        }
    }
}

fn encode_png(img: &image::RgbaImage) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgba8(img.clone())
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .map_err(|e| e.to_string())?;
    Ok(bytes)
}

/// Standard base64. ponytail: ~15 lines against a new dependency for one call site.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = u32::from_be_bytes([0, b[0], b[1], b[2]]);
        let idx = [(n >> 18) & 63, (n >> 12) & 63, (n >> 6) & 63, n & 63];
        for (i, &c) in idx.iter().enumerate() {
            // The tail is padded: 1 input byte → 2 chars + "==", 2 → 3 chars + "=".
            if i <= chunk.len() {
                out.push(ALPHABET[c as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_rfc_vectors() {
        // RFC 4648 §10 — the padding cases are exactly what a hand-rolled encoder gets wrong.
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        // Bytes above 0x7f exercise the 62/63 alphabet entries.
        assert_eq!(base64(&[0xfb, 0xff, 0xfe]), "+//+");
    }

    #[test]
    fn parse_facets_keeps_the_schema_and_drops_the_rest() {
        let json = r#"{
            "subject": ["rock", "boulder", "rock"],
            "style": "low-poly",
            "colour": ["grey"],
            "technical": ["4k"],
            "usage": [1, "environment"]
        }"#;
        let tags = parse_facets(json, 0.7);
        let got: Vec<(&str, &str)> = tags
            .iter()
            .map(|t| (t.namespace.as_str(), t.value.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("style", "low-poly"),
                ("subject", "rock"),
                ("subject", "boulder"),
                ("usage", "environment"),
            ],
            "unknown facet dropped, `technical` refused, duplicate collapsed, \
             bare string accepted, non-string item skipped"
        );
        assert!(tags.iter().all(|t| t.confidence == Some(0.7)));
    }

    #[test]
    fn malformed_output_is_not_an_error() {
        assert!(parse_facets("I think it's a rock!", 0.7).is_empty());
        assert!(
            parse_facets("[\"rock\"]", 0.7).is_empty(),
            "array, not object"
        );
        assert!(parse_facets("", 0.7).is_empty());
    }

    #[test]
    fn the_prompt_offers_only_facets_validate_accepts() {
        let input = TagInput {
            media_code: "model_3d",
            name: "SM_Env_Rock_01",
            image: None,
        };
        let prompt = build_prompt(&input, false);
        assert!(
            prompt.contains("SM_Env_Rock_01"),
            "text mode leans on the name"
        );
        assert!(prompt.contains("3D model"));
        for facet in MODEL_FACETS {
            assert!(prompt.contains(facet), "{facet} missing from the prompt");
        }
        for banned in ["technical", "color"] {
            assert!(
                !prompt
                    .split("Allowed facets:")
                    .nth(1)
                    .unwrap()
                    .contains(banned),
                "{banned} must not be offered"
            );
        }
    }
}
