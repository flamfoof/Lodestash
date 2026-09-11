//! The tagging seam (AI-PIPELINE §2, IDENTIFICATION-AND-SEARCH §2). Real taggers — a local Ollama
//! VLM, a WD-style ONNX classifier, or the Claude vision API — arrive behind this trait; storage,
//! normalization and FTS live in `lodestash-core`.

use lodestash_core::catalog::TagAssignment;

/// What a tagger was shown. The pixels are already decoded and the technical facts already
/// measured, mirroring how [`crate::embedder::Embedder`] takes a decoded image rather than a path:
/// this crate stays models-only, and no tagger re-implements ingest.
pub struct TagInput<'a> {
    /// `media_type.code` — a tagger may refuse a modality it can't see.
    pub media_code: &'a str,
    /// The asset's display name. Filenames carry real signal in game libraries
    /// (`SM_Env_Wall_01`), so a text-only tagger has something to work with.
    pub name: &'a str,
    /// A decoded preview: the raster itself for 2D, a rendered view for 3D, `None` for audio.
    pub image: Option<&'a image::RgbaImage>,
}

/// A user trait fed back into the identifier (AI-PIPELINE §6, UI-SPEC §9). A trait the user marked
/// `teach=1` becomes a preferred candidate term in the tagger's prompt, scoped per modality so a
/// model-only trait never steers an audio scan.
#[derive(Debug, Clone, PartialEq)]
pub struct TeachTrait {
    pub namespace: String,
    pub value: String,
    /// `media_type.code` the trait applies to; `None` = any modality.
    pub scope: Option<String>,
    /// When-to-apply guidance shown to the model (the trait's `usage_note`).
    pub usage_note: Option<String>,
}

impl TeachTrait {
    /// Whether this trait should be offered to a scan of `media_code` (matches its scope or is any).
    pub fn applies_to(&self, media_code: &str) -> bool {
        self.scope.as_deref().map_or(true, |s| s == media_code)
    }
}

/// What a tagger produced: facet tags plus, when a model can write one, a short caption that
/// becomes `asset.description` (AI-PIPELINE §6 "caption fills asset.description, FTS-searchable").
#[derive(Debug, Default, Clone, PartialEq)]
pub struct TagResult {
    pub tags: Vec<TagAssignment>,
    pub caption: Option<String>,
}

impl TagResult {
    pub fn from_tags(tags: Vec<TagAssignment>) -> Self {
        Self {
            tags,
            caption: None,
        }
    }
}

/// Produces facet tags for one asset (IDENTIFICATION-AND-SEARCH §2).
///
/// The trait deliberately returns *proposals*, not writes: validation (unknown namespaces dropped,
/// per-facet caps, confidence clamped) and persistence are the caller's, so a hallucinating model
/// can never write a facet the grammar doesn't have — the same "validate then write" rule the
/// query parser follows.
pub trait Tagger {
    /// Stable id recorded on every tag it produces (`asset_tag.model_id`).
    fn model_id(&self) -> &str;
    /// Who produced the tags — the `ai:<producer>` association source (v1 plan S04). An endpoint
    /// tagger names its kind and host, so two endpoints serving the same bare model alias stay
    /// apart; never a key or a credential. The default serves test fakes; real taggers override.
    fn producer_id(&self) -> String {
        format!("model-{}", self.model_id())
    }
    /// Whether this tagger can say anything about a given `media_type.code`. A vision tagger
    /// returns false for audio rather than guessing from the filename.
    fn supports(&self, media_code: &str) -> bool;
    /// Describe the asset's preview in prose, when this tagger has a model that can.
    ///
    /// Split from [`Self::tag`] so a caller can run all the captions first and all the facet
    /// passes second. That is not a style choice: alternating two Ollama models per asset makes
    /// the daemon evict and reload multi-GB weights on every call, which measured **~50 s/asset**
    /// against a few seconds when each model stays resident for a batch.
    fn caption(&self, _input: &TagInput<'_>) -> Option<String> {
        None
    }

    /// Propose tags, optionally given a caption produced earlier by [`Self::caption`]. An empty
    /// result is a valid answer — "nothing recognisable" is information, and the caller still
    /// clears the queue flag so the asset isn't re-run forever.
    fn tag(&self, input: &TagInput<'_>, caption: Option<&str>) -> TagResult;

    /// Does this tagger read `TagInput::image` directly, rather than through a caption it produced
    /// in [`Self::caption`]?
    ///
    /// The two-phase shape above means the *generic* batch path never fills `image` — the caption
    /// is supposed to carry the picture. A tagger that reads the pixels itself is therefore unusable
    /// on that path, and saying so is the difference between "refused" and "silently tagged
    /// nothing": the API tagger was routed through it for weeks, returned an empty result without
    /// making one HTTP request, and the caller wrote that emptiness to the catalog as a finished
    /// answer. Such a tagger needs the drain built for it (`drains::drain_api_tags`).
    fn needs_image(&self) -> bool {
        false
    }
}

/// The facet namespaces a *model* is allowed to fill (IDENTIFICATION-AND-SEARCH §2).
///
/// `color`, `technical` and `audio` are excluded on purpose: colour and technical facts are
/// derived deterministically from the file, and audio facets come from the audio models — asking
/// a vision model for them only invites confident nonsense.
/// The core boundary's list (`lodestash_core::catalog::AI_NAMESPACES`) — one list, not two.
pub const MODEL_FACETS: &[&str] = lodestash_core::catalog::AI_NAMESPACES;

/// Per-facet value cap (IDENTIFICATION-AND-SEARCH §2 "validate then write").
pub const MAX_VALUES_PER_FACET: usize = lodestash_core::catalog::MAX_AI_PER_NAMESPACE;

/// A value that fills a facet without saying anything.
///
/// The prompt asks a model to *omit* a facet rather than guess, and it mostly does — but not
/// always: a 200-asset run wrote `material:none`, `mood:none` and `free:unspecified` (14 rows
/// of 1,498). Those are worse than absent. A tag is a claim, they claim nothing, and they land
/// in the trait cloud as facets a user can click and filter by — `none` is not a material.
/// Dropped here rather than in one tagger, so every backend is covered by the same rule.
fn says_nothing(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "none" | "n/a" | "na" | "null" | "unknown" | "unspecified" | "undefined" | "no" | ""
    )
}

/// Drop what a model isn't allowed to say and cap what it says too much of, keeping the
/// highest-confidence values per facet. Runs between [`Tagger::tag`] and the catalog write.
pub fn validate(tags: Vec<TagAssignment>) -> Vec<TagAssignment> {
    let mut kept: Vec<TagAssignment> = tags
        .into_iter()
        .filter(|t| MODEL_FACETS.contains(&t.namespace.as_str()) && !says_nothing(&t.value))
        // Drop whatever the catalog boundary would refuse — a score in the label slot, a
        // non-finite confidence, an overlong label — so a drain never hands it a batch it rejects
        // and leaves the asset queued for ever. Same rules, one place (v1 plan S04).
        .filter(|t| lodestash_core::catalog::assignment_problem("ai:model", t).is_none())
        .collect();
    // Highest confidence first so the cap keeps the model's best guesses; `None` sorts last but
    // a model-produced tag normally carries a score.
    kept.sort_by(|a, b| {
        b.confidence
            .unwrap_or(0.0)
            .total_cmp(&a.confidence.unwrap_or(0.0))
    });
    let mut per_facet: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    kept.retain(|t| {
        let n = per_facet.entry(t.namespace.clone()).or_default();
        *n += 1;
        *n <= MAX_VALUES_PER_FACET
    });
    // Highest confidence first (above), so the total cap keeps the best guesses too.
    kept.truncate(lodestash_core::catalog::MAX_AI_ASSIGNMENTS);
    kept
}

/// `host[:port]` of an endpoint URL — scheme, userinfo, path and query stripped. Enough to tell two
/// endpoints apart in a producer id; never a key or a credential (v1 plan S04).
pub fn endpoint_host(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    host.to_ascii_lowercase()
}

/// Deterministic, model-free stand-in — the `HashEmbedder` of tagging. It reads the *filename*,
/// which in a game library is genuinely informative (`SM_Env_Wall_01_Alt` → environment, wall),
/// and emits `usage`/`subject` facets from a small keyword table.
///
/// It exists so the tag → validate → `Catalog::set_tags` → FTS path can be exercised end to end
/// before any model is installed, and so the UI has something to render. It is not identification:
/// an asset whose name says nothing gets nothing.
pub struct FilenameTagger;

/// `(keyword, namespace, value)` — deliberately small and boring. A real vocabulary belongs in the
/// P4 mapping library, where the user can see and edit it.
const KEYWORDS: &[(&str, &str, &str)] = &[
    ("wall", "subject", "wall"),
    ("floor", "subject", "floor"),
    ("door", "subject", "door"),
    ("tree", "subject", "tree"),
    ("rock", "subject", "rock"),
    ("chr", "usage", "character"),
    ("char", "usage", "character"),
    ("wep", "usage", "weapon"),
    ("weapon", "usage", "weapon"),
    ("env", "usage", "environment"),
    ("prop", "usage", "prop"),
    ("veh", "usage", "vehicle"),
    ("ui", "usage", "ui"),
    ("fx", "usage", "vfx"),
    ("sfx", "audio", "sfx"),
    ("music", "audio", "music"),
];

impl Tagger for FilenameTagger {
    fn model_id(&self) -> &str {
        "filename-stub-v1"
    }

    fn producer_id(&self) -> String {
        "filename".to_string()
    }

    fn supports(&self, _media_code: &str) -> bool {
        true // a filename exists for every asset
    }

    fn tag(&self, input: &TagInput<'_>, _caption: Option<&str>) -> TagResult {
        // Split on the separators asset names actually use, so `SM_Chr_Ghost_01` yields tokens
        // rather than one long word.
        let lower = input.name.to_lowercase();
        let tokens: Vec<&str> = lower
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|t| !t.is_empty())
            .collect();
        let mut out: Vec<TagAssignment> = Vec::new();
        for (keyword, namespace, value) in KEYWORDS {
            if tokens.iter().any(|t| t == keyword) {
                let tag = TagAssignment {
                    namespace: (*namespace).to_string(),
                    value: (*value).to_string(),
                    // Not a model score: a filename match is a rule firing, and calling it 1.0
                    // would let it outrank a real model's considered guess.
                    confidence: Some(0.5),
                };
                if !out.contains(&tag) {
                    out.push(tag);
                }
            }
        }
        TagResult::from_tags(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_tagger_reads_the_naming_convention() {
        let t = FilenameTagger;
        let input = TagInput {
            media_code: "model_3d",
            name: "SM_Chr_Ghost_01",
            image: None,
        };
        let tags = t.tag(&input, None).tags;
        assert_eq!(
            tags.iter().map(|t| t.value.as_str()).collect::<Vec<_>>(),
            vec!["character"]
        );

        // Two conventions in one name, and the same value twice collapses.
        let tags = t
            .tag(
                &TagInput {
                    name: "SM_Env_Wall_01_Wall_Alt",
                    ..input
                },
                None,
            )
            .tags;
        let mut values: Vec<&str> = tags.iter().map(|t| t.value.as_str()).collect();
        values.sort();
        assert_eq!(values, vec!["environment", "wall"]);

        // A substring is not a match: "character" must not fire on "chrome".
        assert!(t
            .tag(
                &TagInput {
                    name: "chrome_panel",
                    ..input
                },
                None
            )
            .tags
            .is_empty());
    }

    /// A facet filled with `none` is worse than an empty facet: it reads as a claim, and it
    /// shows up in the trait cloud as something a user can click. Seen in a real 200-asset run.
    #[test]
    fn validate_drops_values_that_say_nothing() {
        let kept = validate(vec![
            TagAssignment::exact("material", "none"),
            TagAssignment::exact("mood", "N/A"),
            TagAssignment::exact("style", " Unspecified "),
            TagAssignment::exact("subject", "fog"),
        ]);
        assert_eq!(
            kept.iter()
                .map(|t| (t.namespace.as_str(), t.value.as_str()))
                .collect::<Vec<_>>(),
            vec![("subject", "fog")],
            "only the tag that says something survives"
        );
    }

    #[test]
    fn validate_drops_forbidden_facets_and_caps_the_rest() {
        let mut tags = vec![
            TagAssignment::exact("technical", "2k"),
            TagAssignment::exact("color", "#ff0000"),
            TagAssignment::exact("nonsense", "whatever"),
        ];
        for i in 0..12 {
            tags.push(TagAssignment {
                namespace: "subject".into(),
                value: format!("thing{i}"),
                confidence: Some(i as f64 / 12.0),
            });
        }
        let kept = validate(tags);
        assert!(
            kept.iter().all(|t| t.namespace == "subject"),
            "a model may not write technical/color facets, or invent one"
        );
        assert_eq!(kept.len(), MAX_VALUES_PER_FACET, "per-facet cap applied");
        assert!(
            kept.iter().all(|t| t.confidence.unwrap() >= 4.0 / 12.0),
            "the cap keeps the highest-confidence values"
        );
    }
}
