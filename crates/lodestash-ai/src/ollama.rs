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

use crate::tagger::{TagInput, TagResult, Tagger, TeachTrait, MODEL_FACETS};

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
    /// User traits (`teach=1`) offered to the model as preferred terms (AI-PIPELINE §6).
    teach: Vec<TeachTrait>,
    timeout: Duration,
}

impl OllamaTagger {
    /// Text-only: facets are inferred from the asset's name alone.
    pub fn new(text_model: impl Into<String>) -> Self {
        Self {
            host: DEFAULT_HOST.to_string(),
            text_model: text_model.into(),
            vision_model: None,
            teach: Vec::new(),
            timeout: Duration::from_secs(180),
        }
    }

    /// Add the captioning stage (e.g. `moondream`), used whenever an asset has a preview.
    pub fn with_vision(mut self, vision_model: impl Into<String>) -> Self {
        self.vision_model = Some(vision_model.into());
        self
    }

    /// Feed the user's teaching traits into the facet prompt (AI-PIPELINE §6): future scans prefer
    /// the user's own vocabulary. Scoped per modality at prompt-build time.
    pub fn with_teaching(mut self, teach: Vec<TeachTrait>) -> Self {
        self.teach = teach;
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
pub(crate) fn build_prompt(
    input: &TagInput<'_>,
    caption: Option<&str>,
    teach: &[TeachTrait],
) -> String {
    let facets = MODEL_FACETS.join(", ");
    let evidence = match caption {
        Some(c) => format!(
            "A {} asset in a game-asset library is named \"{}\".\n\
             A vision model describes it as: \"{}\".{}",
            media_word(input.media_code),
            input.name,
            c,
            flat_art_caveat(input.media_code)
        ),
        None => format!(
            "A {} asset in a game-asset library is named \"{}\". \
             Infer only what the name actually supports.",
            media_word(input.media_code),
            input.name
        ),
    };
    // Teaching block (AI-PIPELINE §6): the user's own vocabulary, scoped to this modality. Only the
    // traits that apply, so a model-only trait never nudges an audio scan. Never a fabricated line:
    // omitted entirely when the user has taught nothing relevant.
    let teaching = {
        let relevant: Vec<String> = teach
            .iter()
            .filter(|t| t.applies_to(input.media_code))
            .map(|t| match &t.usage_note {
                Some(note) if !note.trim().is_empty() => {
                    format!("{}:{} (use when {})", t.namespace, t.value, note.trim())
                }
                _ => format!("{}:{}", t.namespace, t.value),
            })
            .collect();
        if relevant.is_empty() {
            String::new()
        } else {
            format!(
                "\nPrefer these of the user's own terms whenever they genuinely apply: {}.",
                relevant.join("; ")
            )
        }
    };
    format!(
        "{evidence}{teaching}\n\
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

/// Trim the noise small captioners prepend (`moondream` emits a literal `!!!IMPORTANT!!!`, and
/// `IMAGE!!!`) and cap the length, since a caption is a description and not an essay.
///
/// Also **rejects an answer that is not prose**. A caption is written to `asset.description`,
/// which FTS indexes, so a bad one is not merely useless — it is searchable noise sitting on the
/// asset for ever. Observed on the reference library: asked to describe a pile of sacks,
/// `moondream` answered `ids: [0.17, 0.13, 0.83, 0.37]` — bounding boxes, not a sentence. The
/// same failure fed `mood:"0 71"`-style tags before [`is_label`] started rejecting those.
fn clean_caption(raw: &str) -> Option<String> {
    let text = raw
        .trim()
        .trim_start_matches(|c: char| c == '!' || c == '*' || c == '#' || c.is_whitespace());
    // Small captioners announce themselves before answering; the words vary, the shape does not.
    let text = ["IMPORTANT", "IMAGE", "ANSWER", "CAPTION"]
        .iter()
        .find_map(|p| text.strip_prefix(p))
        .unwrap_or(text);
    let text = text.trim_start_matches([':', '!', '-', ' ']).trim();
    if !reads_as_prose(text) {
        return None;
    }
    Some(text.chars().take(300).collect())
}

/// Does this look like a sentence about an asset, rather than a machine's scratch output?
///
/// Two words of actual letters, and not majority digits. Deliberately crude: the job is to catch
/// coordinate dumps and empty answers, not to grade English.
fn reads_as_prose(text: &str) -> bool {
    let words = text
        .split_whitespace()
        .filter(|w| w.chars().any(char::is_alphabetic))
        .count();
    let digits = text.chars().filter(char::is_ascii_digit).count();
    words >= 2 && digits * 2 < text.len()
}

/// What a flat asset's pixels can and cannot settle.
///
/// A texture is usually an **atlas**: `round crates.png` in the Artificer pack is wood planks
/// and two rope strips laid out for UVs, with no crate anywhere in the image. A vision model
/// reads that honestly and answers "a wooden door with horizontal planks and rope accents" —
/// and the facet model, handed the name *and* that caption, took the caption's word for what
/// the object **is** and wrote `subject:wooden door`. Four of five samples failed exactly that
/// way (`sacks` → seed packet, `wheelbarow` and `bucket` → wooden crate), while the one 3D
/// model in the set — whose preview is a render of the thing itself — came out right.
///
/// So this is not model quality and a bigger VLM does not fix it: for flat art the pixels are
/// evidence about the **surface** and the filename is the evidence about the **subject**. A
/// game library names its textures after what they clothe, which is exactly the fact the
/// picture cannot carry.
fn flat_art_caveat(code: &str) -> &'static str {
    match code {
        "texture" | "image" | "sprite" => {
            " This is flat art and may be a texture atlas or an unwrapped sheet rather than a picture of the object, so trust the name for what the asset is, and the description for what it is made of and how it looks."
        }
        _ => "",
    }
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
/// Is this string a label a person would search for?
///
/// A small model asked for facets sometimes answers with its own confidence numbers instead —
/// the reference catalog carried `mood:"0 71"`, `style:"0 67"`, `subject:"0 73"` and
/// `material:"01"` from exactly that, which are unsearchable and pollute every autocomplete.
/// A tag has to contain a letter; digits are fine *inside* one (`4k`, `pbr2`).
fn is_label(value: &str) -> bool {
    value.chars().any(char::is_alphabetic)
}

pub(crate) fn parse_facets(json: &str, confidence: f64) -> Vec<TagAssignment> {
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
            if is_label(&tag.value) && !out.contains(&tag) {
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

    fn producer_id(&self) -> String {
        format!("ollama@{}", crate::tagger::endpoint_host(&self.host))
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
        let prompt = build_prompt(input, caption, &self.teach);
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

pub(crate) fn encode_png(img: &image::RgbaImage) -> Result<Vec<u8>, String> {
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
pub(crate) fn base64(bytes: &[u8]) -> String {
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

    /// A caption lands in `asset.description`, which FTS indexes — so a non-answer is not
    /// harmless, it is permanent searchable noise. Every string here came off the reference
    /// library or its model.
    #[test]
    fn a_caption_has_to_read_as_prose() {
        // Real `moondream` output when asked to describe a pile of sacks.
        assert_eq!(clean_caption("ids: [0.17, 0.13, 0.83, 0.37]"), None);
        assert_eq!(clean_caption("   "), None);
        assert_eq!(clean_caption("!!!"), None);
        assert_eq!(clean_caption("0.42"), None);
        // The announcement prefixes get trimmed, and the sentence survives.
        assert_eq!(
            clean_caption("IMAGE!!! is a 2D image of a wooden board").as_deref(),
            Some("is a 2D image of a wooden board"),
        );
        assert_eq!(
            clean_caption("!!!IMPORTANT!!!: a rusty metal barrel").as_deref(),
            Some("a rusty metal barrel"),
        );
        // A caption with numbers in it is still a caption.
        assert_eq!(
            clean_caption("a 4k texture of 2 wooden crates").as_deref(),
            Some("a 4k texture of 2 wooden crates"),
        );
    }

    /// A model that answers with its own confidence numbers must not seed the vocabulary.
    /// Six such rows existed in the reference catalog before this guard.
    #[test]
    fn numeric_answers_are_not_labels() {
        let json =
            r#"{"subject":["0 73","crate"],"mood":"0 71","style":["01"],"material":["4k pbr"]}"#;
        let tags = parse_facets(json, 0.7);
        let got: Vec<(&str, &str)> = tags
            .iter()
            .map(|t| (t.namespace.as_str(), t.value.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![("material", "4k pbr"), ("subject", "crate")],
            "only the values a person could search for survive: {tags:?}"
        );
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

    /// A texture atlas does not picture the object it clothes, so the caption cannot settle
    /// what the asset is — the prompt has to say so. This is the whole reason
    /// `subject:wooden door` was written onto an asset named `round crates`.
    #[test]
    fn flat_art_tells_the_model_the_name_owns_the_subject() {
        let flat = TagInput {
            media_code: "image",
            name: "round crates",
            image: None,
        };
        let p = build_prompt(&flat, Some("a wooden door with rope accents"), &[]);
        assert!(p.contains("trust the name"), "flat art gets the caveat");

        // A 3D model's preview *is* a render of the object, so its pixels are the better
        // evidence and the caveat must not appear — it would teach the model to discount them.
        let solid = TagInput {
            media_code: "model_3d",
            name: "SM_Wep_BrokenSword_01",
            image: None,
        };
        let p = build_prompt(&solid, Some("a broken sword"), &[]);
        assert!(
            !p.contains("trust the name"),
            "a rendered model keeps its pixels"
        );
    }

    #[test]
    fn the_caption_becomes_evidence_in_the_facet_prompt() {
        let input = TagInput {
            media_code: "model_3d",
            name: "asset_0192",
            image: None,
        };
        let prompt = build_prompt(&input, Some("A rusty metal barrel with a dented lid."), &[]);
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
    fn teaching_traits_enter_the_prompt_only_when_in_scope() {
        let teach = vec![
            TeachTrait {
                namespace: "material".into(),
                value: "bronze".into(),
                scope: Some("model_3d".into()),
                usage_note: Some("the greenish metal".into()),
            },
            TeachTrait {
                namespace: "mood".into(),
                value: "ominous".into(),
                scope: None, // any modality
                usage_note: None,
            },
            TeachTrait {
                namespace: "audio".into(),
                value: "sfx".into(),
                scope: Some("audio".into()),
                usage_note: None,
            },
        ];
        let input = TagInput {
            media_code: "model_3d",
            name: "SM_Statue",
            image: None,
        };
        let prompt = build_prompt(&input, None, &teach);
        assert!(prompt.contains("material:bronze"), "in-scope trait offered");
        assert!(prompt.contains("the greenish metal"), "usage note included");
        assert!(prompt.contains("mood:ominous"), "any-scope trait offered");
        assert!(
            !prompt.contains("audio:sfx"),
            "an audio-scoped trait never steers a 3D scan"
        );
        // No teaching at all ⇒ no teaching sentence.
        assert!(!build_prompt(&input, None, &[]).contains("user's own terms"));
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
        // Real words, because a 400-character single token is not a long caption — it is not
        // a caption at all, and `reads_as_prose` now says so.
        assert_eq!(
            clean_caption(&"a wooden crate ".repeat(40)).map(|c| c.len()),
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
        let prompt = build_prompt(&input, None, &[]);
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
