//! Prefab evidence (v1 plan S06, ADR-0018): what each `.prefab` declares, kept per file with the
//! revision it was read at, and the `unity_prefab` mesh → material edges derived from it.
//!
//! A prefab is not an asset (`prefab` is no `media_type`), so its evidence cannot hang off an
//! asset row. It is recorded per walked **scope** — a source root, or the package workspace — so a
//! walk proves deletions only where it actually looked: a drive that is gone, or a directory the
//! walk could not open, keeps what it said before.

use super::*;

/// The scope M28 files the pre-S06 prefab edges under. They came from whole-file pairings with no
/// record of which prefab said so; kept as evidence here, nothing is deleted before a reharvest has
/// replaced it. Never a real path — every other scope is an absolute directory.
pub const LEGACY_PREFAB_SCOPE: &str = "legacy";

/// One renderer slot a prefab binds (`lodestash_ingest::deps::prefab_evidence`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefabBindingRow {
    pub game_object: i64,
    pub renderer: i64,
    pub slot: i64,
    pub mesh_guid: String,
    pub material_guid: String,
}

/// One prefab as read at one revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefabFileScan {
    pub path: String,
    pub size: i64,
    pub mtime: i64,
    pub bindings: Vec<PrefabBindingRow>,
}

impl Catalog {
    /// The `(size, mtime)` each prefab recorded under `scope` was read at — a walk re-parses only
    /// the files whose revision moved.
    pub fn prefab_revisions(&self, scope: &str) -> Result<HashMap<String, (i64, i64)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, size, mtime FROM prefab_file WHERE scope = ?1")
            .map_err(|e| CoreError::Catalog(format!("prefab revisions prep: {e}")))?;
        let rows = stmt
            .query_map(params![scope], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))
            .map_err(|e| CoreError::Catalog(format!("prefab revisions query: {e}")))?;
        rows.collect::<std::result::Result<HashMap<_, _>, _>>()
            .map_err(|e| CoreError::Catalog(format!("prefab revisions row: {e}")))
    }

    /// Record one walk of `scope`: each `parsed` file replaces what it declared before, and each
    /// `removed` path — a prefab a *completed* walk no longer found — goes with its bindings.
    /// The caller passes `removed` only for a walk that saw the whole tree. Returns files touched.
    pub fn record_prefab_walk(
        &mut self,
        scope: &str,
        parsed: &[PrefabFileScan],
        removed: &[String],
    ) -> Result<usize> {
        let tx = self
            .conn
            .transaction()
            .map_err(|e| CoreError::Catalog(format!("prefab walk begin: {e}")))?;
        let mut touched = 0usize;
        for path in removed {
            touched += tx
                .execute(
                    "DELETE FROM prefab_file WHERE scope = ?1 AND path = ?2",
                    params![scope, path],
                )
                .map_err(|e| CoreError::Catalog(format!("prefab forget: {e}")))?;
        }
        for file in parsed {
            tx.execute(
                "INSERT INTO prefab_file (scope, path, size, mtime) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(scope, path) DO UPDATE SET size = excluded.size, mtime = excluded.mtime",
                params![scope, file.path, file.size, file.mtime],
            )
            .map_err(|e| CoreError::Catalog(format!("prefab file upsert: {e}")))?;
            let id: i64 = tx
                .query_row(
                    "SELECT id FROM prefab_file WHERE scope = ?1 AND path = ?2",
                    params![scope, file.path],
                    |r| r.get(0),
                )
                .map_err(|e| CoreError::Catalog(format!("prefab file id: {e}")))?;
            tx.execute(
                "DELETE FROM prefab_binding WHERE prefab_id = ?1",
                params![id],
            )
            .map_err(|e| CoreError::Catalog(format!("prefab bindings clear: {e}")))?;
            for b in &file.bindings {
                tx.execute(
                    "INSERT OR IGNORE INTO prefab_binding
                         (prefab_id, game_object, renderer, slot, mesh_guid, material_guid)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        id,
                        b.game_object,
                        b.renderer,
                        b.slot,
                        b.mesh_guid,
                        b.material_guid
                    ],
                )
                .map_err(|e| CoreError::Catalog(format!("prefab binding insert: {e}")))?;
            }
            touched += 1;
        }
        tx.commit()
            .map_err(|e| CoreError::Catalog(format!("prefab walk commit: {e}")))?;
        Ok(touched)
    }

    /// Forget the evidence of every scope not in `scopes` — a source removed from the library, and
    /// M28's [`LEGACY_PREFAB_SCOPE`]. Only after a walk that covered every scope there is and
    /// completed in each: anything less has not replaced what it would forget.
    pub fn retain_prefab_scopes(&mut self, scopes: &[String]) -> Result<usize> {
        let recorded: Vec<String> = {
            let mut stmt = self
                .conn
                .prepare("SELECT DISTINCT scope FROM prefab_file")
                .map_err(|e| CoreError::Catalog(format!("prefab scopes prep: {e}")))?;
            let rows = stmt
                .query_map([], |r| r.get(0))
                .map_err(|e| CoreError::Catalog(format!("prefab scopes query: {e}")))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| CoreError::Catalog(format!("prefab scopes row: {e}")))?
        };
        let mut forgotten = 0usize;
        for scope in recorded.iter().filter(|s| !scopes.contains(s)) {
            forgotten += self
                .conn
                .execute("DELETE FROM prefab_file WHERE scope = ?1", params![scope])
                .map_err(|e| CoreError::Catalog(format!("prefab scope forget: {e}")))?;
        }
        Ok(forgotten)
    }

    /// Whether M28's pre-S06 edges still wait for a complete reharvest to replace them.
    pub fn prefab_reharvest_pending(&self) -> Result<bool> {
        self.conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM prefab_file WHERE scope = ?1)",
                params![LEGACY_PREFAB_SCOPE],
                |r| r.get(0),
            )
            .map_err(|e| CoreError::Catalog(format!("prefab reharvest probe: {e}")))
    }

    /// Retire M28's legacy pairings for the meshes the source at `root` holds, once a walk of that
    /// source has completed: its prefabs were just re-read, so what they declare has replaced the
    /// whole-file guess (ADR-0018). A legacy pairing whose mesh is no longer catalogued at all backs
    /// nothing and goes too, and the legacy scope itself goes once it is empty. Returns how many
    /// pairings were retired.
    ///
    /// ponytail: a legacy pairing is taken to belong to its mesh's source. A prefab in one source
    /// that painted a mesh from another loses that edge while its own source is unplugged, and gets
    /// it back from that source's first complete walk.
    pub fn retire_legacy_prefab_evidence(&mut self, root: &str) -> Result<usize> {
        let tx = self
            .conn
            .transaction()
            .map_err(|e| CoreError::Catalog(format!("legacy retire begin: {e}")))?;
        let retired = tx
            .execute(
                "DELETE FROM prefab_binding
                  WHERE prefab_id IN (SELECT id FROM prefab_file WHERE scope = ?1)
                    AND (mesh_guid IN (SELECT a.unity_guid FROM asset a
                                         JOIN import_source s ON s.id = a.source_id
                                        WHERE s.root_path = ?2 AND a.unity_guid IS NOT NULL)
                         OR NOT EXISTS (SELECT 1 FROM asset a
                                         WHERE a.unity_guid = prefab_binding.mesh_guid))",
                params![LEGACY_PREFAB_SCOPE, root],
            )
            .map_err(|e| CoreError::Catalog(format!("legacy retire: {e}")))?;
        tx.execute(
            "DELETE FROM prefab_file
              WHERE scope = ?1
                AND NOT EXISTS (SELECT 1 FROM prefab_binding WHERE prefab_id = prefab_file.id)",
            params![LEGACY_PREFAB_SCOPE],
        )
        .map_err(|e| CoreError::Catalog(format!("legacy scope drop: {e}")))?;
        tx.commit()
            .map_err(|e| CoreError::Catalog(format!("legacy retire commit: {e}")))?;
        Ok(retired)
    }

    /// Derive the `unity_prefab` mesh → material edges from the recorded evidence (DATA-MODEL §10).
    /// Returns how many edges were added or removed.
    ///
    /// There is one edge per distinct (catalogued mesh, material GUID) a binding names. An
    /// unresolved material is kept `missing` for [`Self::resolve_pending_relationships`] to
    /// late-bind, and a mesh catalogued after its prefab was read gets its edge on the next call. An
    /// edge no evidence backs any more is removed — unless someone muted it: an ignored row is never
    /// deleted and never duplicated, exactly as `record_dependencies` treats its own. A material the
    /// model file embeds names the model's own GUID; that is not a dependency on another file, and
    /// no self-edge is kept.
    pub fn reconcile_prefab_edges(&mut self) -> Result<usize> {
        let tx = self
            .conn
            .transaction()
            .map_err(|e| CoreError::Catalog(format!("prefab edges begin: {e}")))?;
        let removed = tx
            .execute(
                "DELETE FROM asset_relationship
                  WHERE source = 'unity_prefab' AND state <> 'ignored'
                    AND NOT EXISTS (
                        SELECT 1 FROM prefab_binding b
                          JOIN asset m ON m.unity_guid = b.mesh_guid AND m.deleted_at IS NULL
                         WHERE m.id = asset_relationship.from_asset_id
                           AND b.material_guid = asset_relationship.declared_path
                           AND b.material_guid <> b.mesh_guid)",
                [],
            )
            .map_err(|e| CoreError::Catalog(format!("prefab edges prune: {e}")))?;
        let added = tx
            .execute(
                "INSERT INTO asset_relationship
                     (from_asset_id, to_asset_id, kind, source, confidence, declared_path, state)
                 SELECT DISTINCT m.id,
                        (SELECT id FROM asset
                          WHERE unity_guid = b.material_guid AND deleted_at IS NULL),
                        'uses_material', 'unity_prefab', 0.9, b.material_guid,
                        CASE WHEN EXISTS (SELECT 1 FROM asset
                                           WHERE unity_guid = b.material_guid
                                             AND deleted_at IS NULL)
                             THEN 'resolved' ELSE 'missing' END
                   FROM prefab_binding b
                   JOIN asset m ON m.unity_guid = b.mesh_guid AND m.deleted_at IS NULL
                  WHERE b.material_guid <> b.mesh_guid
                    AND NOT EXISTS (SELECT 1 FROM asset_relationship r
                                     WHERE r.from_asset_id = m.id
                                       AND r.declared_path = b.material_guid
                                       AND r.source = 'unity_prefab')
                 ON CONFLICT(from_asset_id, to_asset_id, kind) DO NOTHING",
                [],
            )
            .map_err(|e| CoreError::Catalog(format!("prefab edges derive: {e}")))?;
        tx.commit()
            .map_err(|e| CoreError::Catalog(format!("prefab edges commit: {e}")))?;
        Ok(removed + added)
    }
}
