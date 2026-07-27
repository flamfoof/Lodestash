//! SigLIP **zero-shot** image tagging (AI-PIPELINE §2.1/§3). The mirror of [`crate::clap_tags`] for
//! the visual modalities: no new model and no label file — the shipped SigLIP embedder already puts
//! images and text in one joint space, so an image is tagged by scoring it against a fixed vocabulary
//! of label prompts (plus the user's **teaching** traits) and keeping the closest per facet.
//!
//! This is the natural home for teaching-vocabulary facets (AI-PIPELINE §6): a user trait becomes a
//! candidate label, so a taught word can be assigned without a model that was ever trained on it. It
//! **complements** the Ollama VLM caption path — union, not replace — through `validate`+`set_tags`.

use crate::embedder::Embedder;
use crate::tagger::{validate, TeachTrait};
use crate::zeroshot::score_facets;
use lodestash_core::catalog::TagAssignment;
use std::sync::Arc;

/// Base candidate vocabulary: `(facet, label)`. Game-asset shaped and deliberately moderate — a
/// zero-shot list is only as good as its coverage of what the library holds, and a bloated list only
/// dilutes the per-facet softmax. Extend as real libraries show gaps; users extend it themselves via
/// teaching traits, which are merged in at construction.
const LABELS: &[(&str, &str)] = &[
    // subject — what the asset depicts
    ("subject", "character"),
    ("subject", "creature"),
    ("subject", "monster"),
    ("subject", "robot"),
    ("subject", "weapon"),
    ("subject", "sword"),
    ("subject", "shield"),
    ("subject", "armor"),
    ("subject", "building"),
    ("subject", "house"),
    ("subject", "wall"),
    ("subject", "floor"),
    ("subject", "door"),
    ("subject", "tree"),
    ("subject", "rock"),
    ("subject", "plant"),
    ("subject", "barrel"),
    ("subject", "crate"),
    ("subject", "chest"),
    ("subject", "potion"),
    ("subject", "coin"),
    ("subject", "gem"),
    ("subject", "vehicle"),
    ("subject", "ship"),
    ("subject", "terrain"),
    ("subject", "food"),
    ("subject", "icon"),
    ("subject", "user interface"),
    // style — how it's rendered
    ("style", "realistic"),
    ("style", "stylized"),
    ("style", "low-poly"),
    ("style", "pixel art"),
    ("style", "hand-painted"),
    ("style", "cartoon"),
    ("style", "sci-fi"),
    ("style", "fantasy"),
    ("style", "medieval"),
    ("style", "voxel"),
    // material — dominant surface
    ("material", "metal"),
    ("material", "wood"),
    ("material", "stone"),
    ("material", "brick"),
    ("material", "fabric"),
    ("material", "leather"),
    ("material", "gold"),
    ("material", "rusty metal"),
    ("material", "glass"),
    ("material", "dirt"),
    ("material", "sand"),
    ("material", "snow"),
    ("material", "grass"),
    ("material", "crystal"),
    // mood
    ("mood", "dark"),
    ("mood", "bright"),
    ("mood", "colorful"),
    ("mood", "gloomy"),
    ("mood", "cheerful"),
    ("mood", "epic"),
    ("mood", "eerie"),
    // usage — role in a game
    ("usage", "character"),
    ("usage", "environment"),
    ("usage", "prop"),
    ("usage", "weapon"),
    ("usage", "vehicle"),
    ("usage", "user interface"),
    ("usage", "tileable texture"),
    ("usage", "skybox"),
];

/// A SigLIP embedder plus its precomputed label vectors (base vocabulary + the user's teaching
/// traits). Build once per pass; `tag` is then pure arithmetic over a decoded image.
pub struct ImageZeroShot {
    embedder: Arc<dyn Embedder + Send + Sync>,
    /// `(facet, label, unit_vector)` — owned, because teaching labels are runtime strings.
    labels: Vec<(String, String, Vec<f32>)>,
    model_id: String,
}

impl ImageZeroShot {
    /// Precompute the label prompt embeddings, merging the base vocabulary with the user's teaching
    /// traits (a taught trait becomes a candidate label, AI-PIPELINE §6). `None` if the embedder
    /// can't do images (a stub / text-only), so the caller skips zero-shot rather than scoring zeros.
    pub fn new(embedder: Arc<dyn Embedder + Send + Sync>, teach: &[TeachTrait]) -> Option<Self> {
        if !embedder.supports_image() {
            return None;
        }
        // Base vocabulary as owned tuples, then add each visual teaching trait (skip audio-scoped —
        // this pass only ever sees visual assets). Dedup so a taught word that duplicates a base
        // label doesn't get two candidate rows.
        let mut pairs: Vec<(String, String)> =
            LABELS.iter().map(|(f, l)| (f.to_string(), l.to_string())).collect();
        for t in teach {
            if t.scope.as_deref() == Some("audio") {
                continue;
            }
            let pair = (t.namespace.clone(), t.value.clone());
            if !pairs.contains(&pair) {
                pairs.push(pair);
            }
        }
        let labels = pairs
            .into_iter()
            .map(|(facet, label)| {
                let vec = embedder.embed_text(&label);
                (facet, label, vec)
            })
            .collect();
        let model_id = format!("{}-zeroshot", embedder.model_id());
        Some(Self {
            embedder,
            labels,
            model_id,
        })
    }

    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    /// Tag one decoded image. Returns validated facet tags — at most one label per facet, only the
    /// clear ones. A zero image vector (unsupported/blank) yields nothing (see [`score_facets`]).
    pub fn tag(&self, img: &image::RgbaImage) -> Vec<TagAssignment> {
        let v = self.embedder.embed_image(img);
        let tags = score_facets(
            &v,
            self.labels.iter().map(|(f, l, vec)| (f.as_str(), l.as_str(), vec.as_slice())),
        );
        validate(tags)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedder::HashEmbedder;

    /// A HashEmbedder that reports image support and embeds an image by hashing a caption we attach —
    /// enough to prove `new` merges teaching traits and `tag` routes through the shared scorer. The
    /// real semantics are covered by the on-box SigLIP tests; this guards the wiring.
    struct FakeImageEmbedder {
        inner: HashEmbedder,
        image_text: String,
    }
    impl Embedder for FakeImageEmbedder {
        fn model_id(&self) -> &str {
            "fake-img"
        }
        fn dim(&self) -> usize {
            self.inner.dim()
        }
        fn embed_text(&self, text: &str) -> Vec<f32> {
            self.inner.embed_text(text)
        }
        fn embed_image(&self, _img: &image::RgbaImage) -> Vec<f32> {
            // The "image" embeds exactly like the caption text, so it lands on that label.
            self.inner.embed_text(&self.image_text)
        }
        fn supports_image(&self) -> bool {
            true
        }
    }

    #[test]
    fn teaching_trait_becomes_a_candidate_and_can_win() {
        // Image content is the taught word; with it in the vocabulary the tagger can assign it.
        let emb = Arc::new(FakeImageEmbedder {
            inner: HashEmbedder::new(256),
            image_text: "grimdark".to_string(),
        });
        let teach = vec![TeachTrait {
            namespace: "mood".to_string(),
            value: "grimdark".to_string(),
            scope: None,
            usage_note: None,
        }];
        let zs = ImageZeroShot::new(emb, &teach).expect("supports image");
        let img = image::RgbaImage::new(4, 4);
        let tags = zs.tag(&img);
        assert!(
            tags.iter().any(|t| t.namespace == "mood" && t.value == "grimdark"),
            "the taught trait should be assignable via zero-shot: {tags:?}"
        );
    }

    #[test]
    fn a_text_only_embedder_yields_no_tagger() {
        // HashEmbedder doesn't support images → no zero-shot tagger (caller skips the pass).
        assert!(ImageZeroShot::new(Arc::new(HashEmbedder::new(64)), &[]).is_none());
    }
}
