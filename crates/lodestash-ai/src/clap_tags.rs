//! CLAP **zero-shot** audio tagging (AI-PIPELINE §6 + §110 "CLAP zero-shot description"). No new
//! model and no AudioSet label file: the shipped CLAP embedder already puts audio and text in one
//! joint space, so a clip is tagged by scoring it against a fixed vocabulary of sound-label prompts
//! and keeping the closest per facet. YAMNet/PANNs discrete event tags are the other listed path and
//! remain future work — this is the one that ships on infrastructure already present.
//!
//! Scoring is per-facet softmax over the candidate cosines, shared with the image path in
//! [`crate::zeroshot`] (the vectors are L2-normalised, so a cosine is a dot product).

use crate::embedder::Embedder;
use crate::tagger::validate;
use crate::zeroshot::score_facets;
use lodestash_core::catalog::TagAssignment;
use std::sync::Arc;

/// The candidate vocabulary: `(facet, label)`. Kept small and game-audio-shaped on purpose — a
/// zero-shot list is only as good as its coverage of what the library actually contains, and a
/// bloated list only dilutes the softmax. Extend as real libraries show gaps.
const LABELS: &[(&str, &str)] = &[
    // subject — the sound's source
    ("subject", "explosion"),
    ("subject", "gunshot"),
    ("subject", "footsteps"),
    ("subject", "wind"),
    ("subject", "rain"),
    ("subject", "thunder"),
    ("subject", "fire"),
    ("subject", "water"),
    ("subject", "sword clash"),
    ("subject", "magic spell"),
    ("subject", "engine"),
    ("subject", "bird"),
    ("subject", "crowd"),
    ("subject", "door"),
    ("subject", "glass breaking"),
    ("subject", "coin"),
    ("subject", "punch"),
    ("subject", "bell"),
    // usage — what role it plays
    ("usage", "background music"),
    ("usage", "sound effect"),
    ("usage", "ambient loop"),
    ("usage", "voice line"),
    ("usage", "user interface click"),
    // mood
    ("mood", "tense"),
    ("mood", "cheerful"),
    ("mood", "ominous"),
    ("mood", "calm"),
    ("mood", "epic"),
];

/// A CLAP embedder plus its precomputed label vectors. Build once per pass; `tag` is then pure
/// arithmetic over a mel spectrogram.
pub struct AudioZeroShot {
    embedder: Arc<dyn Embedder + Send + Sync>,
    /// `(facet, label, unit_vector)`.
    labels: Vec<(&'static str, &'static str, Vec<f32>)>,
    model_id: String,
}

impl AudioZeroShot {
    /// Precompute the label prompt embeddings. `None` if the embedder can't do audio (a stub), so
    /// the caller skips the audio tag pass rather than writing zeros.
    pub fn new(embedder: Arc<dyn Embedder + Send + Sync>) -> Option<Self> {
        if !embedder.supports_audio() {
            return None;
        }
        let labels = LABELS
            .iter()
            .map(|(facet, label)| {
                let vec = embedder.embed_text(&prompt(label));
                (*facet, *label, vec)
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

    /// Tag a clip from its log-mel spectrogram (`[frames][mels]`, the same shape the CLAP embed
    /// drain feeds). Returns validated facet tags — at most one label per facet, only the clear ones.
    pub fn tag(&self, mel: &[f32], frames: usize) -> Vec<TagAssignment> {
        let v = self.embedder.embed_audio(mel, frames);
        let tags = score_facets(
            &v,
            self.labels
                .iter()
                .map(|(f, l, vec)| (*f, *l, vec.as_slice())),
        );
        validate(tags)
    }
}

/// The CLAP caption convention: a clip embeds near a natural description, so the prompt is a phrase,
/// not a bare word.
fn prompt(label: &str) -> String {
    format!("the sound of {label}")
}
