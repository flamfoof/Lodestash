//! CLAP **zero-shot** audio tagging (AI-PIPELINE §6 + §110 "CLAP zero-shot description"). No new
//! model and no AudioSet label file: the shipped CLAP embedder already puts audio and text in one
//! joint space, so a clip is tagged by scoring it against a fixed vocabulary of sound-label prompts
//! and keeping the closest per facet. YAMNet/PANNs discrete event tags are the other listed path and
//! remain future work — this is the one that ships on infrastructure already present.
//!
//! Scoring is per-facet softmax over the candidate cosines (the vectors are L2-normalised, so a
//! cosine is a dot product). Softmax within a facet turns "which of these sources is it" into a
//! probability the confidence underbar can show, and keeps one facet's scale from drowning another.

use crate::embedder::Embedder;
use crate::tagger::validate;
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

/// Softmax temperature (CLIP/CLAP zero-shot uses a learned `logit_scale`, ~100). Raw cosines from a
/// contrastive model sit in a narrow band, so softmaxing them directly is nearly uniform and hides
/// the winner. Scaling the logits before softmax restores the contrast the model actually learned.
const LOGIT_SCALE: f32 = 30.0;

/// Minimum per-facet softmax probability before a label is trusted enough to emit. Below this the
/// clip didn't clearly match any candidate in that facet, and silence beats a guess (the same
/// "omit rather than guess" rule the VLM prompt follows). Above 0.5 so a facet split evenly between
/// two candidates (no real match) stays silent.
const MIN_PROB: f32 = 0.55;

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
        let tags = score_facets(&v, &self.labels);
        validate(tags)
    }
}

/// The CLAP caption convention: a clip embeds near a natural description, so the prompt is a phrase,
/// not a bare word.
fn prompt(label: &str) -> String {
    format!("the sound of {label}")
}

/// Score one audio vector against the labels, grouped by facet: softmax the cosines within each
/// facet and emit the top label when it clears [`MIN_PROB`]. Pure so it is unit-testable without a
/// model. A zero audio vector (undecodable clip) scores every dot at 0 → uniform softmax → nothing
/// clears the floor, which is the correct "said nothing" answer.
fn score_facets(
    v: &[f32],
    labels: &[(&'static str, &'static str, Vec<f32>)],
) -> Vec<TagAssignment> {
    use std::collections::BTreeMap;
    // Collect (label, cosine) per facet, preserving first-seen facet order via BTreeMap on a key.
    let mut groups: BTreeMap<&'static str, Vec<(&'static str, f32)>> = BTreeMap::new();
    for (facet, label, lv) in labels {
        let cos = dot(v, lv);
        groups.entry(facet).or_default().push((label, cos));
    }
    let mut out = Vec::new();
    for (facet, scores) in groups {
        // Softmax over the facet's candidates, scaled so the contrastive model's narrow cosine
        // band separates (see LOGIT_SCALE).
        let logit = |s: f32| s * LOGIT_SCALE;
        let max = scores
            .iter()
            .map(|(_, s)| logit(*s))
            .fold(f32::MIN, f32::max);
        let exps: Vec<f32> = scores
            .iter()
            .map(|(_, s)| (logit(*s) - max).exp())
            .collect();
        let sum: f32 = exps.iter().sum();
        if sum <= 0.0 {
            continue;
        }
        // Best candidate in this facet.
        let (best_i, best_prob) = exps
            .iter()
            .enumerate()
            .map(|(i, e)| (i, e / sum))
            .fold((0, 0.0f32), |acc, x| if x.1 > acc.1 { x } else { acc });
        if best_prob >= MIN_PROB {
            out.push(TagAssignment {
                namespace: facet.to_string(),
                value: scores[best_i].0.to_string(),
                confidence: Some(best_prob as f64),
            });
        }
    }
    out
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_label_per_facet_wins_and_weak_matches_are_dropped() {
        // Two facets, hand-built unit vectors. The audio vector points exactly at "explosion"
        // (subject) and sits between the two mood labels (no clear winner → dropped).
        let labels = vec![
            ("subject", "explosion", vec![1.0, 0.0, 0.0, 0.0]),
            ("subject", "footsteps", vec![0.0, 1.0, 0.0, 0.0]),
            ("mood", "tense", vec![0.0, 0.0, 1.0, 0.0]),
            ("mood", "calm", vec![0.0, 0.0, 0.0, 1.0]),
        ];
        // Strongly explosion; mood axes get an equal, tiny projection.
        let v = vec![1.0, 0.0, 0.05, 0.05];
        let tags = score_facets(&v, &labels);
        assert_eq!(tags.len(), 1, "only the confident facet emits");
        assert_eq!(tags[0].namespace, "subject");
        assert_eq!(tags[0].value, "explosion");
        assert!(tags[0].confidence.unwrap() > 0.45);
    }

    #[test]
    fn a_zero_vector_says_nothing() {
        let labels = vec![
            ("subject", "explosion", vec![1.0, 0.0]),
            ("subject", "wind", vec![0.0, 1.0]),
        ];
        // Two candidates, zero audio ⇒ 0.5/0.5 softmax ⇒ below the floor.
        assert!(score_facets(&[0.0, 0.0], &labels).is_empty());
    }
}
