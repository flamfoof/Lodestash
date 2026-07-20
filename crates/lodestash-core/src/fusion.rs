//! Reciprocal Rank Fusion for hybrid search.
//!
//! Spec: docs/DATA-MODEL.md §3 step 3.
//! Fuses several ranked result lists (each already ordered best-first) into a single
//! ranked list using the RRF formula. An id absent from a list contributes nothing;
//! duplicates within a list use the best (smallest) rank.

use std::collections::HashMap;

/// Fuse multiple ranked lists via Reciprocal Rank Fusion.
///
/// Each inner `Vec<i64>` is one retriever's result, ordered best-first (index 0 = rank 1).
/// An id's fused score = Σ 1/(k + rank_i(d)) over every ranking it appears in.
/// Returns ids sorted by fused score DESC, ties broken by ASC asset_id, truncated to `limit`.
/// `limit == 0` returns an empty vec.
///
/// # Examples
///
/// ```
/// use lodestash_core::fusion::rrf_fuse;
/// let rankings = vec![vec![1, 2, 3], vec![3, 1, 2]];
/// let result = rrf_fuse(&rankings, 60.0, 5);
/// assert_eq!(result, vec![1, 3, 2]);
/// ```
pub fn rrf_fuse(rankings: &[Vec<i64>], k: f32, limit: usize) -> Vec<i64> {
    if rankings.is_empty() || limit == 0 {
        return Vec::new();
    }

    // Accumulate scores as f64 for precision.
    let mut scores: HashMap<i64, f64> = HashMap::new();

    for ranking in rankings {
        // Deduplicate: keep the smallest rank (1-based) for each id.
        let mut best_rank: HashMap<i64, usize> = HashMap::new();
        for (pos, id) in ranking.iter().enumerate() {
            let rank = pos + 1; // 1-based rank
            best_rank
                .entry(*id)
                .and_modify(|r| {
                    if rank < *r {
                        *r = rank;
                    }
                })
                .or_insert(rank);
        }

        // Accumulate RRF contribution.
        let k_f64 = k as f64;
        for (id, rank) in best_rank {
            *scores.entry(id).or_insert(0.0) += 1.0 / (k_f64 + rank as f64);
        }
    }

    if scores.is_empty() {
        return Vec::new();
    }

    // Sort by score DESC, then asset_id ASC.
    let mut result: Vec<(i64, f64)> = scores.into_iter().collect();
    result.sort_unstable_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });

    result.into_iter().take(limit).map(|(id, _)| id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classic_rrf_example() {
        // id 1 ranked #1 in first, #2 in second; id 2 the mirror; id 3 last in both.
        let rankings = vec![vec![1, 2, 3], vec![2, 1, 3]];
        let result = rrf_fuse(&rankings, 60.0, 5);
        // id 1 and id 2 tie on score; tie-break by ascending id → 1 before 2.
        assert_eq!(result, vec![1, 2, 3], "classic RRF example");
    }

    #[test]
    fn high_rank_in_both_beats_top_rank_in_one() {
        // id 1 is #1 in list A but absent from list B; id 2 is present in both.
        let rankings = vec![vec![1, 2], vec![2]];
        let result = rrf_fuse(&rankings, 60.0, 5);
        assert_eq!(
            result,
            vec![2, 1],
            "id ranked in both beats id ranked #1 in one"
        );
    }

    #[test]
    fn tie_break_by_id() {
        // Symmetric ranks → genuinely equal scores (5: rank1+rank2, 3: rank2+rank1).
        let rankings = vec![vec![5, 3], vec![3, 5]];
        let result = rrf_fuse(&rankings, 60.0, 5);
        assert_eq!(result, vec![3, 5], "equal scores → ascending asset_id");
    }

    #[test]
    fn limit_truncates() {
        let rankings = vec![vec![1, 2, 3, 4, 5]];
        assert_eq!(
            rrf_fuse(&rankings, 60.0, 3),
            vec![1, 2, 3],
            "limit truncates"
        );
    }

    #[test]
    fn limit_zero_returns_empty() {
        assert!(
            rrf_fuse(&[vec![1, 2, 3]], 60.0, 0).is_empty(),
            "limit 0 → empty"
        );
    }

    #[test]
    fn empty_input_returns_empty() {
        assert!(rrf_fuse(&[], 60.0, 10).is_empty(), "no rankings → empty");
        assert!(
            rrf_fuse(&[vec![], vec![]], 60.0, 10).is_empty(),
            "all empty rankings → empty"
        );
    }

    #[test]
    fn duplicate_within_ranking_uses_best_rank() {
        // id 1 appears at rank 1 and rank 5 in the same list; only rank 1 counts.
        let rankings = vec![vec![1, 2, 3, 4, 1]];
        assert_eq!(
            rrf_fuse(&rankings, 60.0, 5),
            vec![1, 2, 3, 4],
            "duplicate within a ranking deduped to best rank"
        );
    }
}
