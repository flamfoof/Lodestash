//! Per-source blacklist (DATA-MODEL §10) — what a scan is told to skip.
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
    /// The per-source blacklist (UI-SPEC §11.1 expanded panel): every `{kind, value}`
    /// exclusion the crawler will skip for this source (DATA-MODEL §10). Empty when the
    /// source has no blacklist yet.
    pub fn list_source_excludes(&self, source_id: i64) -> Result<Vec<SourceExclude>> {
        source_excludes_in(&self.conn, source_id)
    }

    /// The absolute root of one source — the key the crawler's config-side blacklist is
    /// stored under. `None` when the source id doesn't exist.
    pub fn source_root(&self, source_id: i64) -> Result<Option<String>> {
        source_root_in(&self.conn, source_id)
    }

    /// Add one `{kind, value}` exclusion to a source's blacklist (UI-SPEC §11.1 ⊘ folder /
    /// ⊘ asset add buttons). `kind` must be `folder` or `asset`; an exact duplicate is a
    /// no-op (the set is idempotent). Non-destructive — only cataloguing is affected.
    pub fn add_source_exclude(&self, source_id: i64, kind: &str, value: &str) -> Result<()> {
        if kind != "folder" && kind != "asset" {
            return Err(CoreError::Catalog(format!(
                "add_source_exclude: kind must be 'folder' or 'asset', got {kind:?}"
            )));
        }
        let value = value.trim();
        if value.is_empty() {
            return Err(CoreError::Catalog("add_source_exclude: empty value".into()));
        }
        let mut list = source_excludes_in(&self.conn, source_id)?;
        if list.iter().any(|e| e.kind == kind && e.value == value) {
            return Ok(()); // already excluded — idempotent
        }
        list.push(SourceExclude {
            kind: kind.to_string(),
            value: value.to_string(),
        });
        write_source_excludes(&self.conn, source_id, &list)
    }

    /// Remove one `{kind, value}` exclusion from a source's blacklist (UI-SPEC §11.1 chip ✕).
    /// Returns `true` if an entry was removed, `false` if it wasn't there.
    pub fn remove_source_exclude(&self, source_id: i64, kind: &str, value: &str) -> Result<bool> {
        let mut list = source_excludes_in(&self.conn, source_id)?;
        let before = list.len();
        list.retain(|e| !(e.kind == kind && e.value == value));
        if list.len() == before {
            return Ok(false);
        }
        write_source_excludes(&self.conn, source_id, &list)?;
        Ok(true)
    }
}
