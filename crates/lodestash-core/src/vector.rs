//! Brute-force vector search primitives for hybrid search (DATA-MODEL §3, §2 vec_asset;
//! IDENTIFICATION-AND-SEARCH §5.B). Pure `std` — the embeddings themselves are produced
//! elsewhere (`lodestash-ai`); this is just the KNN retriever.
//!
//! ponytail: O(n·d) brute force, comfortably fast to ~100k vectors per DATA-MODEL §3.
//! The [`VectorIndex`] trait is the seam a disk-based ANN (LanceDB/HNSW) swaps in behind
//! past ~1M vectors; nothing above this module knows which index it's talking to.

/// Cosine similarity of two equal-length vectors, in `[-1.0, 1.0]`. Returns `0.0` on a
/// length mismatch or a zero-magnitude vector — never `NaN`/`inf`.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

/// K-nearest-neighbour retrieval seam. Returns `(asset_id, similarity)` most-similar-first,
/// at most `k` items.
pub trait VectorIndex {
    fn knn(&self, query: &[f32], k: usize) -> Vec<(i64, f32)>;
}

/// In-memory brute-force [`VectorIndex`]: scores every candidate by [`cosine_similarity`].
/// Candidates are `(asset_id, embedding)` — typically the pre-filtered set loaded via
/// `Catalog::embeddings_for_knn`.
pub struct BruteForceIndex {
    items: Vec<(i64, Vec<f32>)>,
}

impl BruteForceIndex {
    pub fn new(items: Vec<(i64, Vec<f32>)>) -> Self {
        Self { items }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

impl VectorIndex for BruteForceIndex {
    fn knn(&self, query: &[f32], k: usize) -> Vec<(i64, f32)> {
        if k == 0 || self.items.is_empty() {
            return Vec::new();
        }
        // Dimension-mismatched candidates score 0 (cosine_similarity guards it); a real
        // pipeline never mixes dims per model, but a stale row must not panic the search.
        let mut scored: Vec<(i64, f32)> = self
            .items
            .iter()
            .map(|(id, v)| (*id, cosine_similarity(query, v)))
            .collect();
        // Similarity DESC, ties broken by ascending id for determinism.
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        scored.truncate(k);
        scored
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn cosine_edge_cases() {
        assert!(approx(cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]), 1.0));
        assert!(approx(cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]), 0.0));
        assert!(approx(cosine_similarity(&[1.0, 0.0], &[-1.0, 0.0]), -1.0));
        assert_eq!(cosine_similarity(&[0.0, 0.0], &[1.0, 1.0]), 0.0, "zero mag");
        assert_eq!(cosine_similarity(&[1.0], &[1.0, 0.0]), 0.0, "len mismatch");
        // Magnitude-invariant.
        assert!(approx(cosine_similarity(&[2.0, 0.0], &[9.0, 0.0]), 1.0));
    }

    #[test]
    fn knn_orders_by_similarity() {
        let idx = BruteForceIndex::new(vec![
            (10, vec![1.0, 0.0]), // identical to query
            (20, vec![0.0, 1.0]), // orthogonal
            (30, vec![0.9, 0.1]), // close
        ]);
        let hits = idx.knn(&[1.0, 0.0], 2);
        assert_eq!(hits.len(), 2, "respects k");
        assert_eq!(hits[0].0, 10, "identical is closest");
        assert_eq!(hits[1].0, 30, "then the near one");
    }

    #[test]
    fn knn_tie_break_is_deterministic() {
        // Two identical vectors → equal score → ascending id wins.
        let idx = BruteForceIndex::new(vec![(50, vec![1.0, 0.0]), (40, vec![1.0, 0.0])]);
        assert_eq!(idx.knn(&[1.0, 0.0], 2), vec![(40, 1.0), (50, 1.0)]);
    }

    #[test]
    fn knn_empty_and_zero_k() {
        assert!(BruteForceIndex::new(vec![]).knn(&[1.0], 5).is_empty());
        let idx = BruteForceIndex::new(vec![(1, vec![1.0])]);
        assert!(idx.knn(&[1.0], 0).is_empty(), "k==0 → empty");
        assert!(!idx.is_empty() && idx.len() == 1);
    }

    #[test]
    fn knn_excludes_wrong_dimension_without_panic() {
        let idx = BruteForceIndex::new(vec![
            (1, vec![1.0, 0.0]),
            (2, vec![1.0, 0.0, 0.0]), // wrong dim → scores 0, sorts last
        ]);
        let hits = idx.knn(&[1.0, 0.0], 5);
        assert_eq!(hits[0].0, 1);
        assert_eq!(hits[1], (2, 0.0), "mismatched-dim scored 0, not panicked");
    }
}
