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

use crate::tagger::{TagInput, TagResult, Tagger, MODEL_FACETS};

/// Default Ollama endpoint. Loopback only — a remote host would put asset previews on the network,
/// which the local-first invariant forbids without an explicit opt-in path.
pub const DEFAULT_HOST: &str = "http://127.0.0.1:11434";

/// A tagger backed by Ollama: a **text** model that emits the facet JSON, optionally preceded by a
/// **vision** model that captions the preview.
///
/// The split is not architecture for its own sake, it is what measurement forced. Asked for facet
/// JSON directly, `moondream` degenerates into arrays of numbers (its JSON mode is trained for
/// grounding boxes, not objects), while it captions the same preview accurately in prose. A text
/// model then turns that caption plus the filename into the schema reliably. This is also what
/// AI-PIPELINE section 2 describes for `LocalTagger`: "Ollama VLM captions".
pub struct OllamaTagger {
    host: String,
    /// Emits the facet JSON. Required.
    text_model: String,
    /// Captions the preview first, when the asset has one. `None` = name-only tagging.
    vision_model: Option<String>,
    timeout: Duration,
}

impl OllamaTagger {
    /// Text-only: facets are inferred from the asset's name alone.
    pub fn new(text_model: impl Into<String>) -> Self {
        Self {
            host: DEFAULT_HOST.to_string(),
            text_model: text_model.into(),
            vision_model: None,
            timeout: Duration::from_secs(180),
        }
    }

    /// Add the captioning stage (e.g. `moondream`), used whenever an asset has a preview.
    pub fn with_vision(mut self, vision_model: impl Into<String>) -> Self {
        self.vision_model = Some(vision_model.into());
        self
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

    /// One blocking generate call, returning the model's raw response text. `json` asks Ollama to
    /// constrain decoding, which is right for the facet stage and wrong for captioning.
    fn generate(
        &self,
        model: &str,
        prompt: &str,
        image_png: Option<&[u8]>,
        json: bool,
    ) -> Result<String, String> {
        let mut body = serde_json::json!({
            "model": model,
            "prompt": prompt,
            "stream": false,
            "options": { "temperature": 0.0 },
            // Keep the weights resident between calls. Ollama evicts on its own schedule, and a
            // reload of a multi-GB model costs more than the inference does — the drain measured
            // ~50 s/asset while two models took turns, and ~8 s once each stayed loaded.
            "keep_alive": "10m",
        });
        if json {
            // Constrained decoding removes the commonest failure of small models on the facet
            // stage: prose wrapped around the answer.
            body["format"] = serde_json::json!("json");
        }
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

/// The facet instruction. The facet list is injected from [`MODEL_FACETS`] so the prompt cannot
/// drift from what `validate` will accept.
fn build_prompt(input: &TagInput<'_>, caption: Option<&str>) -> String {
    let facets = MODEL_FACETS.join(", ");
    let evidence = match caption {
        Some(c) => format!(
            "A {} asset in a game-asset library is named \"{}\".\n\
             A vision model describes it as: \"{}\".",
            media_word(input.media_code),
            input.name,
            c
        ),
        None => format!(
            "A {} asset in a game-asset library is named \"{}\". \
             Infer only what the name actually supports.",
            media_word(input.media_code),
            input.name
        ),
    };
    format!(
        "{evidence}\n\
         Reply with JSON only: an object whose keys are facets and whose values are arrays of \
         short lowercase terms.\n\
         Allowed facets: {facets}.\n\
         Use at most 4 terms per facet, and omit a facet entirely rather than guessing. \
         Do not describe colours, resolution, polygon counts or file formats."
    )
}

/// The captioning instruction: plain prose, no schema. Small VLMs caption well and follow
/// structure badly, so this stage asks for the thing they are good at.
fn caption_prompt(media_code: &str) -> String {
    format!(
        "Describe this {} from a game-asset library in one short sentence: what it is, its style, \
         and what it is made of.",
        media_word(media_code)
    )
}

/// Trim the noise small captioners prepend (`moondream` emits a literal `!!!IMPORTANT!!!`) and cap
/// the length, since a caption is a description and not an essay.
fn clean_caption(raw: &str) -> Option<String> {
    let text = raw
        .trim()
        .trim_start_matches(|c: char| c == '!' || c == '*' || c == '#' || c.is_whitespace());
    let text = text.strip_prefix("IMPORTANT").unwrap_or(text);
    let text = text.trim_start_matches([':', '!', '-', ' ']).trim();
    if text.is_empty() {
        return None;
    }
    Some(text.chars().take(300).collect())
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
        &self.text_model
    }

    fn supports(&self, media_code: &str) -> bool {
        // Audio needs the CLAP/PANNs path, not a language model guessing from a filename.
        media_code != "audio"
    }

    fn caption(&self, input: &TagInput<'_>) -> Option<String> {
        let model = self.vision_model.as_ref()?;
        let img = input.image?;
        let png = encode_png(img)
            .map_err(|e| tracing::warn!(%e, "preview encode failed; tagging from the name"))
            .ok()?;
        self.generate(model, &caption_prompt(input.media_code), Some(&png), false)
            .map_err(|e| tracing::warn!(%model, %e, "captioning failed; tagging from the name"))
            .ok()
            .and_then(|c| clean_caption(&c))
    }

    fn tag(&self, input: &TagInput<'_>, caption: Option<&str>) -> TagResult {
        let prompt = build_prompt(input, caption);
        let tags = match self.generate(&self.text_model, &prompt, None, true) {
            // Pixels make a stronger claim than a filename, and the confidence has to say so:
            // the UI hides low-confidence tags rather than deleting them.
            Ok(text) => parse_facets(&text, if caption.is_some() { 0.7 } else { 0.4 }),
            Err(e) => {
                tracing::warn!(model = %self.text_model, %e, "ollama tagging failed");
                Vec::new()
            }
        };
        TagResult {
            tags,
            caption: caption.map(str::to_string),
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
    fn the_caption_becomes_evidence_in_the_facet_prompt() {
        let input = TagInput {
            media_code: "model_3d",
            name: "asset_0192",
            image: None,
        };
        let prompt = build_prompt(&input, Some("A rusty metal barrel with a dented lid."));
        assert!(
            prompt.contains("rusty metal barrel"),
            "caption is passed through"
        );
        assert!(
            prompt.contains("asset_0192"),
            "the name stays as corroborating evidence"
        );
    }

    #[test]
    fn captions_are_cleaned_of_model_noise() {
        // moondream really does prefix this.
        assert_eq!(
            clean_caption("!!!IMPORTANT!!! A red background with a white border."),
            Some("A red background with a white border.".to_string())
        );
        assert_eq!(
            clean_caption("  **A stone wall.**  "),
            Some("A stone wall.**".to_string())
        );
        assert_eq!(clean_caption("   !!!  "), None);
        assert_eq!(
            clean_caption(&"x".repeat(400)).map(|c| c.len()),
            Some(300),
            "a caption is a description, not an essay"
        );
    }

    #[test]
    fn the_prompt_offers_only_facets_validate_accepts() {
        let input = TagInput {
            media_code: "model_3d",
            name: "SM_Env_Rock_01",
            image: None,
        };
        let prompt = build_prompt(&input, None);
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
