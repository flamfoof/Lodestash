//! The embedding seam (AI-PIPELINE §3, IDENTIFICATION-AND-SEARCH §5.B). Real encoders
//! (SigLIP image, CLAP audio, a text encoder) arrive as ONNX `Embedder`s behind this
//! trait once `ort` lands; storage + KNN over the output live in `lodestash-core`.

/// Produces a unit-normalised embedding vector. Text-only for now — image/audio encoders
/// take raw bytes/samples and get their own methods when their ONNX impls land; this is
/// the query-side + text-modality half both of those will share the store/index with.
pub trait Embedder {
    /// Stable id recorded alongside each vector (`asset_embedding.model_id`), so a re-embed
    /// with a new model doesn't collide with the old vectors.
    fn model_id(&self) -> &str;
    /// Output dimensionality — must match what's stored for this model.
    fn dim(&self) -> usize;
    /// Embed text into a `dim`-length, L2-normalised vector.
    fn embed_text(&self, text: &str) -> Vec<f32>;
}

/// Deterministic, dependency-free stand-in used until the ONNX encoders exist: hashes each
/// whitespace token into the vector via a feature-hashing bucket, then L2-normalises. It
/// carries no semantics (synonyms don't cluster), but it exercises the full
/// embed → `Catalog::upsert_embedding` → `BruteForceIndex::knn` path end to end, and makes
/// identical text embed identically (so an exact query self-retrieves).
pub struct HashEmbedder {
    dim: usize,
}

impl HashEmbedder {
    pub fn new(dim: usize) -> Self {
        assert!(dim > 0, "embedding dim must be positive");
        Self { dim }
    }
}

impl Embedder for HashEmbedder {
    fn model_id(&self) -> &str {
        "hash-stub-v1"
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn embed_text(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0.0f32; self.dim];
        for token in text.split_whitespace() {
            let token = token.to_lowercase();
            // FNV-1a → bucket + sign, the classic feature-hashing trick.
            let h = fnv1a(token.as_bytes());
            let bucket = (h % self.dim as u64) as usize;
            let sign = if h & 1 == 0 { 1.0 } else { -1.0 };
            v[bucket] += sign;
        }
        l2_normalise(&mut v);
        v
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Scale `v` to unit length in place (no-op for the zero vector — empty/blank input).
fn l2_normalise(v: &mut [f32]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_is_deterministic_and_normalised() {
        let e = HashEmbedder::new(64);
        assert_eq!(e.dim(), 64);
        let a = e.embed_text("low poly rock");
        let b = e.embed_text("low poly rock");
        assert_eq!(a, b, "identical text → identical vector");
        assert_eq!(a.len(), 64);
        let norm: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "unit length");
    }

    #[test]
    fn different_text_differs_blank_is_zero() {
        let e = HashEmbedder::new(64);
        assert_ne!(e.embed_text("rock"), e.embed_text("sword"));
        // Blank input can't be normalised → stays the zero vector (never NaN).
        let z = e.embed_text("   ");
        assert!(z.iter().all(|x| *x == 0.0));
    }
}
