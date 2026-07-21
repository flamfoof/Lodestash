//! Embed worker (AI-PIPELINE §3, DATA-MODEL §3 "index sync"): drains assets whose
//! embedding is stale, encodes their text with the active [`Embedder`], and upserts the
//! vectors — clearing `embedding_dirty` in the same step (`upsert_embedding`).
//!
//! Modality is `text` here (the query-side + a name/description fallback embedding); image
//! (SigLIP) and audio (CLAP) encoders drain the same queue behind the same trait later.

use lodestash_core::catalog::Catalog;

use crate::embedder::Embedder;

/// Embed up to `limit` dirty assets, storing each vector under `embedder.model_id()`.
/// Returns how many were embedded (0 when the queue is empty). Idempotent: a re-run picks
/// up only assets still dirty, since `upsert_embedding` clears the flag.
pub fn embed_dirty(
    catalog: &mut Catalog,
    embedder: &dyn Embedder,
    limit: usize,
) -> Result<usize, String> {
    let jobs = catalog
        .assets_needing_embedding(limit)
        .map_err(|e| e.to_string())?;
    let mut done = 0usize;
    for job in jobs {
        let vector = embedder.embed_text(&job.text);
        catalog
            .upsert_embedding(
                job.asset_id,
                embedder.model_id(),
                "text",
                Some(job.media_type_id),
                Some(job.source_id),
                &vector,
            )
            .map_err(|e| e.to_string())?;
        done += 1;
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use lodestash_core::catalog::{Catalog, CrawlRecord};

    use crate::embedder::{Embedder, HashEmbedder};

    fn rec(name: &str) -> CrawlRecord {
        CrawlRecord {
            rel_path: format!("{name}.png"),
            name: name.to_string(),
            media_type: "image".into(),
            size_bytes: 1,
            mtime: 1,
            workspace_rel: None,
        }
    }

    #[test]
    fn embed_dirty_stores_vectors_and_clears_the_flag() {
        let mut c = Catalog::open_in_memory().expect("open");
        c.seed_media_types().expect("seed");
        let sid = c
            .upsert_import_source("Lib", "C:/lib", "folder")
            .expect("src");
        c.record_crawl(sid, None, &[rec("rock"), rec("tree")])
            .expect("crawl");
        let embedder = HashEmbedder::new(64);

        // First pass embeds both dirty assets.
        let n = super::embed_dirty(&mut c, &embedder, 100).expect("embed");
        assert_eq!(n, 2, "both new assets were dirty");

        // Vectors are now retrievable for KNN under the model id, with the right dim.
        let cands = c.embeddings_for_knn("hash-stub-v1", None).expect("knn");
        assert_eq!(cands.len(), 2);
        assert_eq!(cands[0].1.len(), 64);

        // The flag was cleared, so a second pass finds nothing.
        assert_eq!(
            super::embed_dirty(&mut c, &embedder, 100).expect("embed2"),
            0,
            "nothing left dirty"
        );

        // Identical text embeds identically → self-retrieval works through hybrid_search's
        // vector arm (query_vec = the same asset's embedding).
        let rock_vec = cands
            .iter()
            .find(|(_, v)| v == &embedder.embed_text("rock"))
            .map(|(id, _)| *id);
        assert!(rock_vec.is_some(), "the stub is deterministic across calls");
    }
}
