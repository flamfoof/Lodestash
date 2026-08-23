//! Collections (UI-SPEC §6) — named sets of assets, including the per-project staging trays.
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
    /// Every non-staging collection, name-ordered, with its member count (UI-SPEC §6).
    pub fn list_collections(&self) -> Result<Vec<CollectionRow>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT c.id, c.name, COUNT(ca.asset_id) AS n
                 FROM collection c
                 LEFT JOIN collection_asset ca ON ca.collection_id = c.id
                 WHERE c.is_staging = 0
                 GROUP BY c.id
                 ORDER BY c.name COLLATE NOCASE",
            )
            .map_err(|e| CoreError::Catalog(format!("list_collections: {e}")))?;
        let rows = stmt
            .query_map([], |r| {
                Ok(CollectionRow {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    asset_count: r.get(2)?,
                })
            })
            .map_err(|e| CoreError::Catalog(format!("list_collections: {e}")))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("list_collections: {e}")))
    }

    /// Create a named collection; returns its id (UI-SPEC §6 inline create).
    pub fn create_collection(&self, name: &str) -> Result<i64> {
        self.conn
            .execute(
                "INSERT INTO collection (uuid, name) VALUES (lower(hex(randomblob(16))), ?1)",
                params![name],
            )
            .map_err(|e| CoreError::Catalog(format!("create_collection: {e}")))?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Delete a collection; its membership rows cascade. The staging tray is protected — it
    /// is the shortlist, not a loadout.
    pub fn delete_collection(&self, id: i64) -> Result<()> {
        let n = self
            .conn
            .execute(
                "DELETE FROM collection WHERE id = ?1 AND is_staging = 0",
                params![id],
            )
            .map_err(|e| CoreError::Catalog(format!("delete_collection: {e}")))?;
        if n == 0 {
            return Err(CoreError::Catalog("no such collection".into()));
        }
        Ok(())
    }

    /// The member ids of one collection — what a collection-filtered grid matches on.
    pub fn collection_members(&self, id: i64) -> Result<Vec<i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT asset_id FROM collection_asset WHERE collection_id = ?1")
            .map_err(|e| CoreError::Catalog(format!("collection_members: {e}")))?;
        let rows = stmt
            .query_map(params![id], |r| r.get::<_, i64>(0))
            .map_err(|e| CoreError::Catalog(format!("collection_members: {e}")))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("collection_members: {e}")))
    }
}

impl Catalog {
    /// Add an asset to a collection (UI-SPEC §6 — membership is toggled from the asset
    /// inspector, §13.5). Idempotent: adding an already-present member is a no-op.
    pub fn collection_add_asset(&self, collection_id: i64, asset_id: i64) -> Result<()> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO collection_asset (collection_id, asset_id, added_at)
                 VALUES (?1, ?2, ?3)",
                params![collection_id, asset_id, unix_now()],
            )
            .map_err(|e| CoreError::Catalog(format!("collection_add_asset: {e}")))?;
        Ok(())
    }

    /// Remove an asset from a collection (UI-SPEC §6 membership toggle). Removing an
    /// absent member is a no-op.
    pub fn collection_remove_asset(&self, collection_id: i64, asset_id: i64) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM collection_asset WHERE collection_id = ?1 AND asset_id = ?2",
                params![collection_id, asset_id],
            )
            .map_err(|e| CoreError::Catalog(format!("collection_remove_asset: {e}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod collection_tests {
    use super::*;

    #[test]
    fn collections_list_create_delete_round_trip_and_skip_staging() {
        let c = Catalog::open_in_memory().expect("open");
        let id = c.create_collection("Terrain").expect("create");
        let rows = c.list_collections().expect("list");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Terrain");
        assert_eq!(rows[0].asset_count, 0);

        c.delete_collection(id).expect("delete");
        assert!(c.list_collections().expect("list").is_empty());

        // The staging tray is never listable or deletable as a user collection.
        assert!(c.delete_collection(0).is_err(), "no such collection");
    }
}
