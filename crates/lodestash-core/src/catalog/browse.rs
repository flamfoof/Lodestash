//! Scoped discovery for the agent planes (v1 plan S12, ENGINE-API §8.1).
//!
//! Search ranks; it is not a way to visit a library. An agent auditing "every model in this pack"
//! needs a walk that reaches each eligible asset once, that a crash or a closed connection can
//! resume, and that stays honest while the catalog keeps changing underneath it. So there are three
//! shapes here, all over one scope predicate:
//!
//! - **Ordered walk** — ascending id, keyset-paginated below an upper id captured on the first page.
//!   Assets created after the walk started wait for the next walk; eligibility (trash, status, tags)
//!   is evaluated per page, so an asset trashed mid-walk is not returned later.
//! - **Random sample** — one eligible-id query ordered by `random()`: sampling without replacement
//!   over the whole scope, not over the first page of a search.
//! - **Scoped search** — the same predicate applied *before* BM25's LIMIT and before vector scoring,
//!   so a scope holding ten models behind thousands of images still finds the ten.
//!
//! One predicate, [`AssetScope::clause`], so the three cannot disagree about what "in scope" means.

use rusqlite::types::Value as SqlValue;

use super::*;

/// Which of an asset's tag states a walk wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TagState {
    #[default]
    Any,
    Tagged,
    Untagged,
}

impl TagState {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "any" => Some(Self::Any),
            "tagged" => Some(Self::Tagged),
            "untagged" => Some(Self::Untagged),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::Tagged => "tagged",
            Self::Untagged => "untagged",
        }
    }
}

/// What a discovery request is restricted to. Every field narrows; the default is every live,
/// content-bearing asset.
///
/// Values are taken as validated — the API layer refuses bad enums, ids and tag syntax before a
/// scope exists, so a typo is an error rather than a silently empty or widened result.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AssetScope {
    pub source_id: Option<i64>,
    pub package_id: Option<i64>,
    /// `media_type.code`.
    pub media_type: Option<String>,
    /// `asset.status` code.
    pub status: Option<String>,
    pub tag_state: TagState,
    /// Canonical `namespace:value` pairs, all of which must be present (AND).
    pub tags: Vec<(String, String)>,
}

impl AssetScope {
    /// The `WHERE` predicate over `asset a`, and its parameters.
    ///
    /// Always excludes trashed assets and rows with no primary file — the ones no summary can
    /// describe and no preview can render.
    fn clause(&self) -> (String, Vec<SqlValue>) {
        let mut sql = String::from("a.deleted_at IS NULL AND a.primary_file_id IS NOT NULL");
        let mut params: Vec<SqlValue> = Vec::new();
        if let Some(id) = self.source_id {
            sql.push_str(" AND a.source_id = ?");
            params.push(id.into());
        }
        if let Some(id) = self.package_id {
            sql.push_str(" AND a.package_id = ?");
            params.push(id.into());
        }
        if let Some(code) = &self.media_type {
            sql.push_str(" AND a.media_type_id = (SELECT id FROM media_type WHERE code = ?)");
            params.push(code.clone().into());
        }
        if let Some(status) = &self.status {
            sql.push_str(" AND a.status = ?");
            params.push(status.clone().into());
        }
        match self.tag_state {
            TagState::Any => {}
            TagState::Tagged => {
                sql.push_str(" AND EXISTS (SELECT 1 FROM asset_tag x WHERE x.asset_id = a.id)")
            }
            TagState::Untagged => {
                sql.push_str(" AND NOT EXISTS (SELECT 1 FROM asset_tag x WHERE x.asset_id = a.id)")
            }
        }
        for (namespace, value) in &self.tags {
            // `norm_name` carries the namespace (see `normalize_tag`); a pair that does not
            // normalize can match nothing, which is what the empty string does.
            let norm = normalize_tag(namespace, value)
                .map(|(n, _)| n)
                .unwrap_or_default();
            sql.push_str(
                " AND EXISTS (SELECT 1 FROM asset_tag x JOIN tag t ON t.id = x.tag_id
                               WHERE x.asset_id = a.id AND t.norm_name = ?)",
            );
            params.push(norm.into());
        }
        (sql, params)
    }

    /// A canonical rendering of the scope, for a cursor to prove it is continuing the same walk.
    /// Tags are normalized and sorted, so `A,B` and `B,A` are one walk.
    pub fn signature(&self) -> String {
        let mut tags: Vec<String> = self
            .tags
            .iter()
            .filter_map(|(ns, v)| normalize_tag(ns, v).map(|(n, _)| n))
            .collect();
        tags.sort();
        tags.dedup();
        format!(
            "source={:?};package={:?};media={:?};status={:?};tag_state={};tags={}",
            self.source_id,
            self.package_id,
            self.media_type,
            self.status,
            self.tag_state.as_str(),
            tags.join(",")
        )
    }
}

/// One library in [`Catalog::libraries_page`] (ENGINE-API §8.1 `list_libraries`). No root path:
/// discovery does not need to know where a library is on disk.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct LibraryRow {
    pub id: i64,
    pub name: String,
    pub kind: String,
    /// Live, content-bearing assets — the population a walk of this library visits.
    pub asset_count: i64,
}

impl Catalog {
    fn scoped_ids_query(&self, sql: &str, params: Vec<SqlValue>, what: &str) -> Result<Vec<i64>> {
        let mut stmt = self
            .conn
            .prepare(sql)
            .map_err(|e| CoreError::Catalog(format!("{what} prep: {e}")))?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(params), |r| r.get::<_, i64>(0))
            .map_err(|e| CoreError::Catalog(format!("{what} query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("{what} row: {e}")))
    }

    /// The highest asset id — the upper bound a new ordered walk captures.
    pub fn max_asset_id(&self) -> Result<i64> {
        self.conn
            .query_row("SELECT COALESCE(MAX(id), 0) FROM asset", [], |r| r.get(0))
            .map_err(|e| CoreError::Catalog(format!("max asset id: {e}")))
    }

    /// One page of an ordered walk: in-scope ids in `(after, upper]`, ascending, at most `limit`.
    pub fn browse_asset_ids(
        &self,
        scope: &AssetScope,
        after: i64,
        upper: i64,
        limit: usize,
    ) -> Result<Vec<i64>> {
        let (clause, mut params) = scope.clause();
        params.push(after.into());
        params.push(upper.into());
        params.push((limit as i64).into());
        self.scoped_ids_query(
            &format!(
                "SELECT a.id FROM asset a WHERE {clause} AND a.id > ? AND a.id <= ?
                  ORDER BY a.id LIMIT ?"
            ),
            params,
            "browse",
        )
    }

    /// Up to `limit` in-scope ids drawn without replacement from the whole scope.
    ///
    /// ponytail: `ORDER BY random()` scans every eligible id — fine for an occasional audit sample
    /// over a library; profile before replacing it with a maintained sampling index.
    pub fn sample_asset_ids(&self, scope: &AssetScope, limit: usize) -> Result<Vec<i64>> {
        let (clause, mut params) = scope.clause();
        params.push((limit as i64).into());
        self.scoped_ids_query(
            &format!("SELECT a.id FROM asset a WHERE {clause} ORDER BY random() LIMIT ?"),
            params,
            "sample",
        )
    }

    /// Every in-scope id — the eligible set a scoped vector search scores over.
    pub fn scoped_asset_ids(&self, scope: &AssetScope) -> Result<std::collections::HashSet<i64>> {
        let (clause, params) = scope.clause();
        Ok(self
            .scoped_ids_query(
                &format!("SELECT a.id FROM asset a WHERE {clause}"),
                params,
                "scoped ids",
            )?
            .into_iter()
            .collect())
    }

    /// The most recently catalogued in-scope assets — a scoped search with no query.
    pub fn recent_scoped_asset_ids(&self, scope: &AssetScope, limit: usize) -> Result<Vec<i64>> {
        let (clause, mut params) = scope.clause();
        params.push((limit as i64).into());
        self.scoped_ids_query(
            &format!(
                "SELECT a.id FROM asset a WHERE {clause}
                  ORDER BY a.updated_at DESC, a.id DESC LIMIT ?"
            ),
            params,
            "recent scoped",
        )
    }

    /// BM25 over `query`, restricted to `scope` **before** the LIMIT. `None` when the query has
    /// nothing to match on (the caller falls back to recent assets).
    pub fn search_assets_scoped(
        &self,
        query: &str,
        scope: &AssetScope,
        limit: usize,
    ) -> Result<Option<Vec<i64>>> {
        let Some(match_expr) = fts_match_query(query) else {
            return Ok(None);
        };
        let (clause, scope_params) = scope.clause();
        let mut params: Vec<SqlValue> = vec![match_expr.into()];
        params.extend(scope_params);
        params.push((limit as i64).into());
        self.scoped_ids_query(
            &format!(
                "SELECT a.id FROM asset_fts f JOIN asset a ON a.id = f.rowid
                  WHERE asset_fts MATCH ? AND {clause}
                  ORDER BY bm25(asset_fts) LIMIT ?"
            ),
            params,
            "scoped search",
        )
        .map(Some)
    }

    /// [`Self::hybrid_search`] over a scope: BM25 and every vector space rank only in-scope assets,
    /// so neither retriever spends its depth on candidates the scope excludes.
    pub fn hybrid_search_scoped(
        &self,
        query: &str,
        spaces: &[(&str, &[f32])],
        scope: &AssetScope,
        limit: usize,
    ) -> Result<Vec<i64>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let depth = limit.saturating_mul(5).max(50);
        let mut rankings = Vec::with_capacity(spaces.len() + 1);
        rankings.push(
            self.search_assets_scoped(query, scope, depth)?
                .unwrap_or_default(),
        );
        let eligible = self.scoped_asset_ids(scope)?;
        for (model_id, query_vec) in spaces {
            let snapshot = self.embedding_snapshot(model_id)?;
            rankings.push(
                snapshot
                    .index
                    .knn_eligible(query_vec, depth, &eligible)
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect(),
            );
        }
        Ok(rrf_fuse(&rankings, 60.0, limit))
    }

    /// One page of libraries in ascending id after `after`.
    pub fn libraries_page(&self, after: i64, limit: usize) -> Result<Vec<LibraryRow>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT s.id, s.name, s.kind,
                        (SELECT COUNT(*) FROM asset a
                          WHERE a.source_id = s.id AND a.deleted_at IS NULL
                            AND a.primary_file_id IS NOT NULL)
                   FROM import_source s WHERE s.id > ?1 ORDER BY s.id LIMIT ?2",
            )
            .map_err(|e| CoreError::Catalog(format!("libraries prep: {e}")))?;
        let rows = stmt
            .query_map(params![after, limit as i64], |r| {
                Ok(LibraryRow {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    kind: r.get(2)?,
                    asset_count: r.get(3)?,
                })
            })
            .map_err(|e| CoreError::Catalog(format!("libraries query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("libraries row: {e}")))
    }

    /// Whether a library with this id exists.
    pub fn source_exists(&self, source_id: i64) -> Result<bool> {
        self.conn
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM import_source WHERE id = ?1)",
                params![source_id],
                |r| r.get(0),
            )
            .map_err(|e| CoreError::Catalog(format!("source exists: {e}")))
    }

    /// The library a package belongs to, or `None` for no such package.
    pub fn package_source_id(&self, package_id: i64) -> Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT source_id FROM package WHERE id = ?1",
                params![package_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| CoreError::Catalog(format!("package source: {e}")))
    }

    /// An asset's scalar description (caption), if it has one.
    pub fn asset_description(&self, asset_id: i64) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT description FROM asset WHERE id = ?1",
                params![asset_id],
                |r| r.get(0),
            )
            .optional()
            .map(Option::flatten)
            .map_err(|e| CoreError::Catalog(format!("description: {e}")))
    }

    /// An opaque fingerprint of everything an analysis of this asset depends on (ENGINE-API §8.1):
    /// the primary file's identity — size, mtime, content hash, pack entry, extracted path — plus the
    /// annotation revision (every tag association and the description) and the review state.
    ///
    /// Read in one transaction, so a write landing between the parts cannot produce a revision that
    /// never described the asset. It changes when the evidence or the annotations change, even
    /// though the asset id does not.
    pub fn analysis_revision(&self, asset_id: i64) -> Result<Option<String>> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| CoreError::Catalog(format!("analysis revision begin: {e}")))?;
        let identity: Option<String> = tx
            .query_row(
                "SELECT printf('%s|%s|%s|%s|%s|%s|%s', af.size_bytes, af.mtime, a.content_hash,
                               af.package_entry_id, af.workspace_rel, a.status, a.deleted_at)
                   FROM asset a JOIN asset_file af ON af.id = a.primary_file_id
                  WHERE a.id = ?1",
                params![asset_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| CoreError::Catalog(format!("analysis revision identity: {e}")))?;
        let Some(identity) = identity else {
            return Ok(None);
        };
        let annotations = tagbatch::annotation_revision_in(&tx, asset_id)?;
        drop(tx);
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"analysis-v1\n");
        hasher.update(identity.as_bytes());
        hasher.update(b"\n");
        hasher.update(annotations.as_bytes());
        Ok(Some(format!("a1-{}", &hasher.finalize().to_hex()[..32])))
    }
}
