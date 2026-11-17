//! Trait taxonomies, categories and aliases (UI-SPEC §9.2).
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
    /// Every taxonomy for the Mapping tab's switcher (UI-SPEC §9.2), active first then
    /// name — with the `v<version>` readout and the `● active` marker built in.
    pub fn list_taxonomies(&self) -> Result<Vec<TaxonomyRow>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, name, version, is_active FROM taxonomy
                  ORDER BY is_active DESC, name COLLATE NOCASE",
            )
            .map_err(|e| CoreError::Catalog(format!("list_taxonomies prep: {e}")))?;
        let rows = stmt
            .query_map([], |r| {
                Ok(TaxonomyRow {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    version: r.get(2)?,
                    is_active: r.get::<_, i64>(3)? != 0,
                })
            })
            .map_err(|e| CoreError::Catalog(format!("list_taxonomies query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("list_taxonomies row: {e}")))
    }

    /// Make `taxonomy_id` the single active taxonomy (UI-SPEC §9.2 `● active` marker);
    /// every other taxonomy is deactivated. Errors when the id doesn't exist.
    pub fn set_active_taxonomy(&mut self, taxonomy_id: i64) -> Result<()> {
        let tx = self
            .conn
            .transaction()
            .map_err(|e| CoreError::Catalog(format!("activate begin: {e}")))?;
        tx.execute("UPDATE taxonomy SET is_active = 0", [])
            .map_err(|e| CoreError::Catalog(format!("taxonomy deactivate: {e}")))?;
        let n = tx
            .execute(
                "UPDATE taxonomy SET is_active = 1 WHERE id = ?1",
                params![taxonomy_id],
            )
            .map_err(|e| CoreError::Catalog(format!("taxonomy activate: {e}")))?;
        if n == 0 {
            return Err(CoreError::NotFound(format!(
                "no such taxonomy {taxonomy_id}"
            )));
        }
        tx.commit()
            .map_err(|e| CoreError::Catalog(format!("taxonomy activate commit: {e}")))?;
        Ok(())
    }

    /// Delete a taxonomy and everything under it (UI-SPEC §9.2): its categories (deepest
    /// first, so the `category.parent_id` FK never trips) and — via FK cascade — their
    /// aliases, mapping rules and `asset_category` assignments. Returns the number of
    /// rows removed (categories + the taxonomy row).
    pub fn delete_taxonomy(&mut self, taxonomy_id: i64) -> Result<usize> {
        let ids = {
            let mut stmt = self
                .conn
                .prepare(
                    "WITH RECURSIVE subtree(id, depth) AS (
                         SELECT c.id, 0 FROM category c WHERE c.taxonomy_id = ?1
                         UNION ALL
                         SELECT c.id, s.depth + 1 FROM category c JOIN subtree s ON c.parent_id = s.id
                     )
                     SELECT id FROM subtree ORDER BY depth DESC",
                )
                .map_err(|e| CoreError::Catalog(format!("taxonomy subtree prep: {e}")))?;
            let rows = stmt
                .query_map(params![taxonomy_id], |r| r.get::<_, i64>(0))
                .map_err(|e| CoreError::Catalog(format!("taxonomy subtree query: {e}")))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| CoreError::Catalog(format!("taxonomy subtree row: {e}")))?
        };
        let tx = self
            .conn
            .transaction()
            .map_err(|e| CoreError::Catalog(format!("delete taxonomy begin: {e}")))?;
        let mut removed = 0usize;
        for id in ids {
            tx.execute("DELETE FROM category WHERE id = ?1", params![id])
                .map_err(|e| CoreError::Catalog(format!("delete category {id}: {e}")))?;
            removed += 1;
        }
        let n = tx
            .execute("DELETE FROM taxonomy WHERE id = ?1", params![taxonomy_id])
            .map_err(|e| CoreError::Catalog(format!("delete taxonomy: {e}")))?;
        if n == 0 {
            return Err(CoreError::NotFound(format!(
                "no such taxonomy {taxonomy_id}"
            )));
        }
        removed += 1;
        tx.commit()
            .map_err(|e| CoreError::Catalog(format!("delete taxonomy commit: {e}")))?;
        Ok(removed)
    }

    /// Rename a category (UI-SPEC §9.2 row rename): sets `pref_label` and rewrites the
    /// materialized `path` — and every descendant's path — in one transaction, keeping
    /// the `UNIQUE(taxonomy_id, path)` invariant. A same-label rename is a no-op; an
    /// empty label is rejected.
    pub fn rename_category(&mut self, category_id: i64, new_label: &str) -> Result<()> {
        let label = new_label.trim();
        if label.is_empty() {
            return Err(CoreError::Catalog("rename_category: empty label".into()));
        }
        let tx = self
            .conn
            .transaction()
            .map_err(|e| CoreError::Catalog(format!("rename begin: {e}")))?;
        let (parent_id, old_path): (Option<i64>, String) = tx
            .query_row(
                "SELECT parent_id, path FROM category WHERE id = ?1",
                params![category_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| CoreError::Catalog(format!("rename lookup: {e}")))?
            .ok_or_else(|| CoreError::NotFound(format!("no such category {category_id}")))?;
        let parent_path: String = match parent_id {
            Some(pid) => tx
                .query_row(
                    "SELECT path FROM category WHERE id = ?1",
                    params![pid],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| CoreError::Catalog(format!("rename parent path: {e}")))?
                .unwrap_or_default(),
            None => String::new(),
        };
        let new_path = if parent_path.is_empty() {
            label.to_string()
        } else {
            format!("{parent_path}/{label}")
        };
        if new_path == old_path {
            return Ok(()); // same label — nothing to change
        }
        tx.execute(
            "UPDATE category SET pref_label = ?2, path = ?3 WHERE id = ?1",
            params![category_id, label, new_path],
        )
        .map_err(|e| CoreError::Catalog(format!("rename category: {e}")))?;
        // Rewrite every descendant's path prefix in place (exact-prefix match, so a label
        // containing LIKE metacharacters is safe).
        tx.execute(
            "UPDATE category SET path = ?2 || substr(path, length(?3) + 1)
              WHERE id IN (
                    WITH RECURSIVE subtree(id) AS (
                        SELECT ?1
                        UNION ALL
                        SELECT c.id FROM category c JOIN subtree s ON c.parent_id = s.id
                    )
                    SELECT id FROM subtree WHERE id <> ?1)
                AND length(path) > length(?3)
                AND substr(path, 1, length(?3)) = ?3",
            params![category_id, new_path, old_path],
        )
        .map_err(|e| CoreError::Catalog(format!("rename descendants: {e}")))?;
        tx.commit()
            .map_err(|e| CoreError::Catalog(format!("rename commit: {e}")))?;
        Ok(())
    }

    /// Delete a category and its whole subtree (UI-SPEC §9.2 row delete — "assignments
    /// cascade"). Aliases, rules and `asset_category` rows cascade via FK; children are
    /// removed deepest-first so the `category.parent_id` FK never blocks. Returns the
    /// number of category rows removed.
    pub fn delete_category(&mut self, category_id: i64) -> Result<usize> {
        let ids = self.category_subtree(category_id)?;
        if ids.is_empty() {
            return Err(CoreError::NotFound(format!(
                "no such category {category_id}"
            )));
        }
        let tx = self
            .conn
            .transaction()
            .map_err(|e| CoreError::Catalog(format!("delete category begin: {e}")))?;
        let mut removed = 0usize;
        for id in ids {
            tx.execute("DELETE FROM category WHERE id = ?1", params![id])
                .map_err(|e| CoreError::Catalog(format!("delete category {id}: {e}")))?;
            removed += 1;
        }
        tx.commit()
            .map_err(|e| CoreError::Catalog(format!("delete category commit: {e}")))?;
        Ok(removed)
    }

    /// A category's aliases (UI-SPEC §9.2 ALIASES chips), alphabetical.
    pub fn list_category_aliases(&self, category_id: i64) -> Result<Vec<CategoryAliasRow>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, alias FROM category_alias WHERE category_id = ?1 ORDER BY alias COLLATE NOCASE",
            )
            .map_err(|e| CoreError::Catalog(format!("aliases prep: {e}")))?;
        let rows = stmt
            .query_map(params![category_id], |r| {
                Ok(CategoryAliasRow {
                    id: r.get(0)?,
                    alias: r.get(1)?,
                })
            })
            .map_err(|e| CoreError::Catalog(format!("aliases query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("aliases row: {e}")))
    }

    /// Add an alias to a category (UI-SPEC §9.2 ALIASES add field). Normalized
    /// case-insensitively (`norm_alias`), so `Barrel` and `barrel` are the same alias —
    /// adding an existing one is a no-op that returns its row id. An empty alias is
    /// rejected.
    pub fn add_category_alias(&self, category_id: i64, alias: &str) -> Result<i64> {
        let alias = alias.trim();
        if alias.is_empty() {
            return Err(CoreError::Catalog("add_category_alias: empty alias".into()));
        }
        let norm = alias.to_lowercase();
        self.conn
            .execute(
                "INSERT OR IGNORE INTO category_alias (category_id, alias, norm_alias)
                 VALUES (?1, ?2, ?3)",
                params![category_id, alias, norm],
            )
            .map_err(|e| CoreError::Catalog(format!("alias insert: {e}")))?;
        self.conn
            .query_row(
                "SELECT id FROM category_alias WHERE category_id = ?1 AND norm_alias = ?2",
                params![category_id, norm],
                |r| r.get(0),
            )
            .map_err(|e| CoreError::Catalog(format!("alias lookup: {e}")))
    }

    /// Remove an alias chip (UI-SPEC §9.2 ALIASES ✕).
    pub fn remove_category_alias(&self, alias_id: i64) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM category_alias WHERE id = ?1",
                params![alias_id],
            )
            .map(|_| ())
            .map_err(|e| CoreError::Catalog(format!("alias remove: {e}")))
    }

    /// Flip a mapping rule's enable toggle (UI-SPEC §9.2 RULES enable toggle). A disabled
    /// rule is skipped by [`Self::apply_mapping`] (`enabled = 0`).
    pub fn set_rule_enabled(&self, rule_id: i64, enabled: bool) -> Result<()> {
        self.conn
            .execute(
                "UPDATE mapping_rule SET enabled = ?2 WHERE id = ?1",
                params![rule_id, enabled as i64],
            )
            .map(|_| ())
            .map_err(|e| CoreError::Catalog(format!("rule enable: {e}")))
    }

    /// The ids of a category's subtree (itself included), deepest first — the order a
    /// delete must remove them so `category.parent_id` never dangles.
    fn category_subtree(&self, root_id: i64) -> Result<Vec<i64>> {
        let mut stmt = self
            .conn
            .prepare(
                "WITH RECURSIVE subtree(id, depth) AS (
                     SELECT c.id, 0 FROM category c WHERE c.id = ?1
                     UNION ALL
                     SELECT c.id, s.depth + 1 FROM category c JOIN subtree s ON c.parent_id = s.id
                 )
                 SELECT id FROM subtree ORDER BY depth DESC",
            )
            .map_err(|e| CoreError::Catalog(format!("subtree prep: {e}")))?;
        let rows = stmt
            .query_map(params![root_id], |r| r.get::<_, i64>(0))
            .map_err(|e| CoreError::Catalog(format!("subtree query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("subtree row: {e}")))
    }
}
