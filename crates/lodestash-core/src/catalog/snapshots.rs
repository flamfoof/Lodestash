//! Resident embedding snapshots (v1 plan S09).
//!
//! A KNN query needs every vector of one model in memory. Loading them per query — which
//! `ServerState::similar` and `hybrid_search` both did — reads the whole embedding table and
//! rebuilds the index for every "more like this", and the desktop's own cache was dropped on each
//! library reload. So the vectors are loaded once per **generation** (M30) and shared for the life
//! of the process.
//!
//! Two rules make a resident copy safe:
//!
//! - **Staleness is a fact in the catalog, not an event.** The generation moves inside the
//!   embedding write's transaction, so a headless process sees a GUI process's write; no
//!   notification crosses process boundaries and none is needed.
//! - **Eligibility is not cached.** Trash, status, sources and tags change constantly and have
//!   nothing to do with the vectors; the caller passes the ids that are eligible *now*, and scoring
//!   happens over that set ([`crate::vector::BruteForceIndex::knn_eligible`]).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use super::*;
use crate::vector::BruteForceIndex;

/// One model's vectors as of one generation. Immutable: a new generation builds a new snapshot and
/// replaces the entry, so a query already holding this one keeps a consistent view.
pub struct EmbeddingSnapshot {
    pub model_id: String,
    pub generation: i64,
    pub index: BruteForceIndex,
}

impl EmbeddingSnapshot {
    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }
}

/// How many snapshots one process keeps. A visual and an audio model are the working set; a third
/// is a model being replaced. Beyond that the least recently used is dropped rather than holding
/// ~150 MiB per 50k x 768 model for a space nothing is querying.
const MAX_RESIDENT: usize = 3;

type Key = (String, String);

struct Resident {
    snapshot: Arc<EmbeddingSnapshot>,
    /// Monotonic tick of the last read, for the eviction above.
    used: u64,
}

fn cache() -> &'static Mutex<(HashMap<Key, Resident>, u64)> {
    static CACHE: OnceLock<Mutex<(HashMap<Key, Resident>, u64)>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new((HashMap::new(), 0)))
}

/// Drop every resident snapshot — for a test that needs a cold process. Nothing in the app calls it.
pub fn clear_resident_snapshots() {
    cache().lock().expect("snapshot cache").0.clear();
}

/// How many snapshots are resident right now (tests, and the memory report).
pub fn resident_snapshot_count() -> usize {
    cache().lock().expect("snapshot cache").0.len()
}

impl Catalog {
    /// This model's embedding generation (M30). `0` when the model has never been written, which is
    /// a generation like any other — a snapshot of nothing is still a correct answer.
    pub fn embedding_generation(&self, model_id: &str) -> Result<i64> {
        self.conn
            .query_row(
                "SELECT generation FROM embedding_generation WHERE model_id = ?1",
                params![model_id],
                |r| r.get(0),
            )
            .optional()
            .map(|g| g.unwrap_or(0))
            .map_err(|e| CoreError::Catalog(format!("embedding generation: {e}")))
    }

    /// The resident snapshot of `model_id`, built if this generation has not been loaded yet.
    ///
    /// The generation and the vectors are read in one transaction, so a write landing mid-load
    /// cannot produce a snapshot labelled with a generation it does not contain. Concurrent misses
    /// serialise on the cache and load once. An in-memory catalog shares nothing and always builds
    /// its own — two tests must never see each other's vectors.
    pub fn embedding_snapshot(&self, model_id: &str) -> Result<Arc<EmbeddingSnapshot>> {
        let Some(db) = self
            .db_path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
        else {
            return Ok(Arc::new(self.build_snapshot(model_id)?));
        };
        let key: Key = (db, model_id.to_string());
        let generation = self.embedding_generation(model_id)?;
        let mut guard = cache().lock().expect("snapshot cache");
        guard.1 += 1;
        let tick = guard.1;
        if let Some(resident) = guard.0.get_mut(&key) {
            if resident.snapshot.generation == generation {
                resident.used = tick;
                return Ok(resident.snapshot.clone());
            }
        }
        // Built while holding the cache: every waiter behind this one finds the fresh snapshot
        // rather than starting a second read of the same table.
        let snapshot = Arc::new(self.build_snapshot(model_id)?);
        guard.0.insert(
            key,
            Resident {
                snapshot: snapshot.clone(),
                used: tick,
            },
        );
        while guard.0.len() > MAX_RESIDENT {
            let Some(oldest) = guard
                .0
                .iter()
                .min_by_key(|(_, r)| r.used)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            guard.0.remove(&oldest);
        }
        Ok(snapshot)
    }

    /// Read the generation and the vectors under one read transaction, so the pair is consistent.
    fn build_snapshot(&self, model_id: &str) -> Result<EmbeddingSnapshot> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| CoreError::Catalog(format!("snapshot begin: {e}")))?;
        let generation: i64 = tx
            .query_row(
                "SELECT generation FROM embedding_generation WHERE model_id = ?1",
                params![model_id],
                |r| r.get(0),
            )
            .optional()
            .map(|g| g.unwrap_or(0))
            .map_err(|e| CoreError::Catalog(format!("snapshot generation: {e}")))?;
        let items = {
            let mut stmt = tx
                .prepare(
                    "SELECT e.asset_id, e.embedding
                       FROM asset_embedding e
                      WHERE e.model_id = ?1",
                )
                .map_err(|e| CoreError::Catalog(format!("snapshot prep: {e}")))?;
            let rows = stmt
                .query_map(params![model_id], |r| {
                    let id: i64 = r.get(0)?;
                    let blob: Vec<u8> = r.get(1)?;
                    Ok((id, decode_vector(&blob)))
                })
                .map_err(|e| CoreError::Catalog(format!("snapshot query: {e}")))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| CoreError::Catalog(format!("snapshot row: {e}")))?
        };
        tx.rollback()
            .map_err(|e| CoreError::Catalog(format!("snapshot end: {e}")))?;
        tracing::debug!(
            model = %model_id,
            generation,
            vectors = items.len(),
            "embedding snapshot built"
        );
        Ok(EmbeddingSnapshot {
            model_id: model_id.to_string(),
            generation,
            index: BruteForceIndex::new(items),
        })
    }

    /// The assets a vector search may return right now: live, and of `media_type_id` when one is
    /// given. Deliberately not part of the snapshot — trash and facets move on their own clock.
    pub fn eligible_for_search(
        &self,
        media_type_id: Option<i64>,
    ) -> Result<std::collections::HashSet<i64>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id FROM asset
                  WHERE deleted_at IS NULL AND (?1 IS NULL OR media_type_id = ?1)",
            )
            .map_err(|e| CoreError::Catalog(format!("eligible prep: {e}")))?;
        let rows = stmt
            .query_map(params![media_type_id], |r| r.get(0))
            .map_err(|e| CoreError::Catalog(format!("eligible query: {e}")))?;
        rows.collect::<std::result::Result<std::collections::HashSet<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("eligible row: {e}")))
    }
}
