//! Shared zero-shot scoring for the joint-space taggers (AI-PIPELINE §2.1/§6): CLAP audio
//! ([`crate::clap_tags`]) and SigLIP image ([`crate::image_tags`]). One asset embedding is scored
//! against per-facet candidate label vectors — softmax within each facet, emit the top label when it
//! clears a floor. Pure arithmetic, unit-tested here, so both modalities share one correct core.
//!
//! The vectors are L2-normalised, so a cosine is a dot product.

use lodestash_core::catalog::TagAssignment;

/// Softmax temperature. Contrastive models (CLIP/CLAP/SigLIP) learn a `logit_scale` (~100) because
/// raw cosines sit in a narrow band — softmaxing them directly is nearly uniform and hides the
/// winner. Scaling the logits before softmax restores the contrast the model actually learned.
///
/// ponytail: one shared scale for both modalities; if SigLIP and CLAP need different temperatures in
/// practice, split this into per-caller args — the score fn already takes everything else by value.
pub const LOGIT_SCALE: f32 = 30.0;

/// Minimum per-facet softmax probability before a label is trusted. Below this the asset didn't
/// clearly match any candidate in that facet, and silence beats a guess (the "omit rather than
/// guess" rule the VLM prompt also follows). Above 0.5 so a facet split evenly between two candidates
/// (no real match) stays silent.
pub const MIN_PROB: f32 = 0.55;

/// Score one embedding against `(facet, label, unit_vector)` candidates, grouped by facet: softmax
/// the scaled cosines within each facet and emit the top label when it clears [`MIN_PROB`]. A zero
/// asset vector (undecodable input) scores every dot at 0 → uniform softmax → nothing clears the
/// floor, which is the correct "said nothing" answer.
pub fn score_facets<'a>(
    v: &[f32],
    labels: impl IntoIterator<Item = (&'a str, &'a str, &'a [f32])>,
) -> Vec<TagAssignment> {
    use std::collections::BTreeMap;
    // Collect (label, cosine) per facet; BTreeMap gives a stable facet order for deterministic output.
    let mut groups: BTreeMap<&'a str, Vec<(&'a str, f32)>> = BTreeMap::new();
    for (facet, label, lv) in labels {
        let cos = dot(v, lv);
        groups.entry(facet).or_default().push((label, cos));
    }
    let mut out = Vec::new();
    for (facet, scores) in groups {
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

    /// Adapter so the tests can pass owned label tuples like the real callers do.
    fn score(v: &[f32], labels: &[(&str, &str, Vec<f32>)]) -> Vec<TagAssignment> {
        score_facets(v, labels.iter().map(|(f, l, vec)| (*f, *l, vec.as_slice())))
    }

    #[test]
    fn nearest_label_per_facet_wins_and_weak_matches_are_dropped() {
        // Vector points exactly at "explosion" (subject) and sits evenly between the mood labels.
        let labels = vec![
            ("subject", "explosion", vec![1.0, 0.0, 0.0, 0.0]),
            ("subject", "footsteps", vec![0.0, 1.0, 0.0, 0.0]),
            ("mood", "tense", vec![0.0, 0.0, 1.0, 0.0]),
            ("mood", "calm", vec![0.0, 0.0, 0.0, 1.0]),
        ];
        let v = vec![1.0, 0.0, 0.05, 0.05];
        let tags = score(&v, &labels);
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
        // Two candidates, zero vector ⇒ 0.5/0.5 softmax ⇒ below the floor.
        assert!(score(&[0.0, 0.0], &labels).is_empty());
    }
}
