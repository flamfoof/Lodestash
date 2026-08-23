//! Dependency- and tag-graph reads (DATA-MODEL §10).
//!
//! Split out of `catalog.rs` on 2026-08-22 — a **verbatim move of whole `impl` blocks**, no
//! behaviour change. That file had reached 10,312 lines behind eight `impl Catalog` blocks that
//! already marked these seams. Types, free helpers and constants deliberately stayed in `mod.rs`:
//! they are shared, and the first attempt at this split moved them with whichever block they
//! happened to sit under, which put `CollectionRow` at the bottom of the test module.
//!
//! Privacy still works — a child module can see its ancestors' private items, so `self.conn` and
//! the crate-private helpers resolve exactly as they did when this was one file.

use super::*;

impl Catalog {
    /// The Stats Tag Graph (UI-SPEC §8.2): every tag in use on live assets with its node
    /// count, every tag pair co-occurring on the same asset with its edge weight, plus
    /// the `tags · links · assets` header counts. Soft-deleted assets are excluded, so
    /// the graph matches the Library's identified set. The UI draws the tag nodes and
    /// co-occurrence edges itself and overlays the dependency arcs from
    /// [`Self::asset_edges`].
    pub fn tag_graph(&self) -> Result<TagGraph> {
        let mut nodes_stmt = self
            .conn
            .prepare(
                "SELECT t.id, COALESCE(t.namespace, 'free'), t.name, COUNT(DISTINCT at.asset_id) AS n
                   FROM tag t
                   JOIN asset_tag at ON at.tag_id = t.id
                   JOIN asset a      ON a.id = at.asset_id AND a.deleted_at IS NULL
                  GROUP BY t.id
                  ORDER BY n DESC, t.name",
            )
            .map_err(|e| CoreError::Catalog(format!("tag graph nodes prep: {e}")))?;
        let nodes = nodes_stmt
            .query_map([], |r| {
                Ok(TagGraphNode {
                    tag_id: r.get(0)?,
                    namespace: r.get(1)?,
                    value: r.get(2)?,
                    usage_count: r.get(3)?,
                })
            })
            .map_err(|e| CoreError::Catalog(format!("tag graph nodes query: {e}")))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("tag graph nodes row: {e}")))?;

        let mut edges_stmt = self
            .conn
            .prepare(
                "SELECT at1.tag_id, at2.tag_id, COUNT(DISTINCT at1.asset_id) AS w
                   FROM asset_tag at1
                   JOIN asset_tag at2 ON at2.asset_id = at1.asset_id AND at2.tag_id > at1.tag_id
                   JOIN asset a       ON a.id = at1.asset_id AND a.deleted_at IS NULL
                  GROUP BY at1.tag_id, at2.tag_id
                  ORDER BY w DESC, at1.tag_id, at2.tag_id",
            )
            .map_err(|e| CoreError::Catalog(format!("tag graph edges prep: {e}")))?;
        let edges = edges_stmt
            .query_map([], |r| {
                Ok(TagGraphEdge {
                    tag_a: r.get(0)?,
                    tag_b: r.get(1)?,
                    weight: r.get(2)?,
                })
            })
            .map_err(|e| CoreError::Catalog(format!("tag graph edges query: {e}")))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("tag graph edges row: {e}")))?;

        let asset_count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(DISTINCT at.asset_id) FROM asset_tag at
                   JOIN asset a ON a.id = at.asset_id AND a.deleted_at IS NULL",
                [],
                |r| r.get(0),
            )
            .map_err(|e| CoreError::Catalog(format!("tag graph asset count: {e}")))?;
        let link_count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM asset_relationship r
                   JOIN asset f ON f.id = r.from_asset_id AND f.deleted_at IS NULL
                   JOIN asset t ON t.id = r.to_asset_id   AND t.deleted_at IS NULL
                  WHERE r.to_asset_id IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .map_err(|e| CoreError::Catalog(format!("tag graph link count: {e}")))?;

        Ok(TagGraph {
            nodes,
            edges,
            asset_count,
            link_count,
        })
    }
}

impl Catalog {
    /// Every resolved asset→asset edge between live assets, ordered by kind then endpoint —
    /// the full edge set behind the LINKS filter (`has_dependencies` / `has_dependents`,
    /// DATA-MODEL §10) and the tag graph's dependency arcs (UI-SPEC §8.2). Unresolved
    /// (`to_asset_id NULL`) and missing-endpoint edges are excluded; the inspector's
    /// per-asset view is [`Self::related_assets`].
    pub fn asset_edges(&self) -> Result<Vec<AssetEdge>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT r.from_asset_id, r.to_asset_id, r.kind, r.source
                   FROM asset_relationship r
                   JOIN asset f ON f.id = r.from_asset_id AND f.deleted_at IS NULL
                   JOIN asset t ON t.id = r.to_asset_id   AND t.deleted_at IS NULL
                  ORDER BY r.kind, r.from_asset_id, r.to_asset_id",
            )
            .map_err(|e| CoreError::Catalog(format!("asset edges prep: {e}")))?;
        let rows = stmt
            .query_map([], |r| {
                Ok(AssetEdge {
                    from: r.get(0)?,
                    to: r.get(1)?,
                    kind: r.get(2)?,
                    source: r.get(3)?,
                })
            })
            .map_err(|e| CoreError::Catalog(format!("asset edges query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("asset edges row: {e}")))
    }
}
