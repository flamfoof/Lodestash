//! The undo/redo op log (DATA-MODEL §13).
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
    /// Append an operation to the undo log. `op_kind` names the mutation (`set_tags`,
    /// `set_trashed`, `create_collection`, …); `payload_json` carries whatever its revert
    /// needs (see the `revert_*` family and the `record_*_op` helpers). `forward_json` is the
    /// redo payload — `None` means the payload IS the forward too (the change-shaped ops).
    /// Recording a new op clears the redo stack: after new work, a redo would no longer land
    /// on the state it described. Returns the new op's id.
    pub fn record_op(
        &self,
        op_kind: &str,
        payload_json: &str,
        forward_json: Option<&str>,
    ) -> Result<i64> {
        record_op_in(&self.conn, op_kind, payload_json, forward_json, "user")
    }

    /// The newest undo-log rows, newest first, capped at `limit`. Undone ops are hidden — they
    /// are the redo stack, not the undo button's target.
    pub fn recent_ops(&self, limit: usize) -> Result<Vec<OpLogRow>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, op_kind, payload_json, created_at FROM op_log
                 WHERE undone_at IS NULL AND origin != 'background'
                 ORDER BY id DESC LIMIT ?1",
            )
            .map_err(|e| CoreError::Catalog(format!("recent_ops prep: {e}")))?;
        let rows = stmt
            .query_map(params![limit as i64], |r| {
                Ok(OpLogRow {
                    id: r.get(0)?,
                    op_kind: r.get(1)?,
                    payload_json: r.get(2)?,
                    created_at: r.get(3)?,
                })
            })
            .map_err(|e| CoreError::Catalog(format!("recent_ops query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("recent_ops row: {e}")))
    }

    /// The newest undone ops (the redo stack), newest first, capped at `limit`.
    pub fn redo_candidates(&self, limit: usize) -> Result<Vec<OpLogRow>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, op_kind, payload_json, created_at FROM op_log
                 WHERE undone_at IS NOT NULL AND origin != 'background'
                 ORDER BY id DESC LIMIT ?1",
            )
            .map_err(|e| CoreError::Catalog(format!("redo_candidates prep: {e}")))?;
        let rows = stmt
            .query_map(params![limit as i64], |r| {
                Ok(OpLogRow {
                    id: r.get(0)?,
                    op_kind: r.get(1)?,
                    payload_json: r.get(2)?,
                    created_at: r.get(3)?,
                })
            })
            .map_err(|e| CoreError::Catalog(format!("redo_candidates query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("redo_candidates row: {e}")))
    }

    /// Delete undo-log rows by id — after a revert, or when the user clears history.
    /// Returns the number removed.
    pub fn delete_ops(&self, ids: &[i64]) -> Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        // Ids are i64s we produced — safe to inline (rusqlite has no array binds).
        let id_list = ids
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!("DELETE FROM op_log WHERE id IN ({id_list})");
        self.conn
            .execute(&sql, [])
            .map_err(|e| CoreError::Catalog(format!("delete_ops: {e}")))
    }

    /// Record a trash/untrash op (the direction passed to [`Self::set_trashed`]) so the
    /// toggle can be reverted. Call before mutating.
    pub fn record_trash_op(&self, asset_ids: &[i64], trashed: bool) -> Result<i64> {
        let payload = serde_json::json!({
            "asset_ids": asset_ids,
            "trashed": trashed,
        })
        .to_string();
        self.record_op("set_trashed", &payload, None)
    }

    /// Record a collection-create op (id + name) so the created collection can be deleted
    /// on undo. Call after [`Self::create_collection`] returns its id.
    pub fn record_collection_create_op(&self, collection_id: i64, name: &str) -> Result<i64> {
        let payload = serde_json::json!({
            "collection_id": collection_id,
            "name": name,
        })
        .to_string();
        self.record_op("create_collection", &payload, None)
    }

    /// Undo the operation recorded as `op_id`, then remove the op from the log (a
    /// reverted op is gone — re-undo would double-apply). Dispatches on `op_kind`:
    /// - `tag_batch` → puts back the recorded before-state of every association it touched, after
    ///   checking nothing has changed them since (a conflict is refused, not overwritten);
    /// - `set_trashed` → re-applies [`Self::set_trashed`] with the opposite direction (an
    ///   untrash is undone by trashing again and vice versa);
    /// - `create_collection` → deletes the created collection;
    /// - `set_tags` / `bulk_tag` (recorded before M26) → refused, with the reason: they carry only an
    ///   inverse that could remove or re-attribute another source's tags.
    pub fn revert_op(&mut self, op_id: i64) -> Result<()> {
        let (kind, payload) = self.op_payload(op_id)?;
        match kind.as_str() {
            "tag_batch" => self.revert_tag_batch(op_id, &payload),
            "set_trashed" => self.revert_set_trashed(op_id),
            "create_collection" => self.revert_create_collection(op_id),
            "set_tags" | "bulk_tag" => Err(legacy_tag_replay(op_id)),
            other => Err(CoreError::Catalog(format!(
                "revert_op: no revert registered for op kind {other:?}"
            ))),
        }
    }

    /// Keep a reverted op for redo (UI-SPEC §2 Ctrl/Cmd-Z / Shift-Z): mark it undone instead of
    /// deleting, so [`Self::redo_op`] can apply the forward payload later. New work clears
    /// undone ops (see [`Self::record_op`]).
    fn mark_undone(&self, op_id: i64) -> Result<()> {
        self.conn
            .execute(
                "UPDATE op_log SET undone_at = ?2 WHERE id = ?1",
                params![op_id, unix_now()],
            )
            .map_err(|e| CoreError::Catalog(format!("mark_undone: {e}")))?;
        Ok(())
    }

    /// Redo one undone op (Shift-Z): apply its forward payload — `forward_json` when the op
    /// recorded one, else the payload itself (the change-shaped ops) — then consume the row.
    pub fn redo_op(&mut self, op_id: i64) -> Result<()> {
        let undone: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM op_log WHERE id = ?1 AND undone_at IS NOT NULL",
                params![op_id],
                |r| r.get(0),
            )
            .map_err(|e| CoreError::Catalog(format!("redo_op lookup: {e}")))?;
        if undone == 0 {
            return Err(CoreError::Catalog(format!(
                "redo_op: op {op_id} is not undone — nothing to redo"
            )));
        }
        let (kind, payload) = self.op_payload(op_id)?;
        let forward = self.op_forward(op_id)?.unwrap_or(payload);
        match kind.as_str() {
            "tag_batch" => self.redo_tag_batch(op_id, &forward),
            "set_trashed" => self.redo_set_trashed(op_id, &forward),
            "create_collection" => self.redo_create_collection(op_id, &forward),
            "set_tags" | "bulk_tag" => Err(legacy_tag_replay(op_id)),
            other => Err(CoreError::Catalog(format!(
                "redo_op: no redo registered for op kind {other:?}"
            ))),
        }
    }

    fn op_forward(&self, op_id: i64) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT forward_json FROM op_log WHERE id = ?1",
                params![op_id],
                |r| r.get(0),
            )
            .map_err(|e| CoreError::Catalog(format!("op_forward: {e}")))
    }

    /// Redo a `set_trashed` op: re-apply the recorded direction, then consume the row.
    pub fn redo_set_trashed(&self, op_id: i64, payload: &str) -> Result<()> {
        let v: serde_json::Value = serde_json::from_str(payload)
            .map_err(|e| CoreError::Catalog(format!("redo_set_trashed payload: {e}")))?;
        let trashed = v["trashed"]
            .as_bool()
            .ok_or_else(|| CoreError::Catalog("redo_set_trashed: missing trashed".into()))?;
        let ids: Vec<i64> = v["asset_ids"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .filter_map(|x| x.as_i64())
            .collect();
        self.set_trashed(&ids, trashed)?;
        self.delete_ops(&[op_id])?;
        Ok(())
    }

    /// Redo a `create_collection` op: re-create it (a fresh id is fine), then consume the row.
    pub fn redo_create_collection(&self, op_id: i64, payload: &str) -> Result<()> {
        let v: serde_json::Value = serde_json::from_str(payload)
            .map_err(|e| CoreError::Catalog(format!("redo_create_collection payload: {e}")))?;
        let name = v["name"]
            .as_str()
            .ok_or_else(|| CoreError::Catalog("redo_create_collection: missing name".into()))?;
        self.create_collection(name)?;
        self.delete_ops(&[op_id])?;
        Ok(())
    }

    /// Undo a `set_trashed` op: re-run [`Self::set_trashed`] with the opposite direction
    /// (an untrash is undone by trashing again and vice versa), then drop the op.
    pub fn revert_set_trashed(&self, op_id: i64) -> Result<()> {
        let (kind, payload) = self.op_payload(op_id)?;
        if kind != "set_trashed" {
            return Err(CoreError::Catalog(format!(
                "revert_set_trashed: op {op_id} is {kind:?}, not set_trashed"
            )));
        }
        let v: serde_json::Value = serde_json::from_str(&payload)?;
        let trashed = v["trashed"]
            .as_bool()
            .ok_or_else(|| CoreError::Catalog("set_trashed op payload: missing trashed".into()))?;
        let ids: Vec<i64> = v["asset_ids"]
            .as_array()
            .ok_or_else(|| CoreError::Catalog("set_trashed op payload: missing asset_ids".into()))?
            .iter()
            .filter_map(|x| x.as_i64())
            .collect();
        self.set_trashed(&ids, !trashed)?;
        self.mark_undone(op_id)?;
        Ok(())
    }

    /// Undo a `create_collection` op: delete the collection the op created (reuses
    /// [`Self::delete_collection`]), then mark it undone so Shift-Z can redo.
    pub fn revert_create_collection(&self, op_id: i64) -> Result<()> {
        let (kind, payload) = self.op_payload(op_id)?;
        if kind != "create_collection" {
            return Err(CoreError::Catalog(format!(
                "revert_create_collection: op {op_id} is {kind:?}, not create_collection"
            )));
        }
        let v: serde_json::Value = serde_json::from_str(&payload)?;
        let collection_id = v["collection_id"].as_i64().ok_or_else(|| {
            CoreError::Catalog("create_collection op payload: missing collection_id".into())
        })?;
        self.delete_collection(collection_id)?;
        self.mark_undone(op_id)?;
        Ok(())
    }

    /// Read one op's `(op_kind, payload_json)`.
    fn op_payload(&self, op_id: i64) -> Result<(String, String)> {
        self.conn
            .query_row(
                "SELECT op_kind, payload_json FROM op_log WHERE id = ?1",
                params![op_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| CoreError::Catalog(format!("op_payload: {e}")))?
            .ok_or_else(|| CoreError::Catalog(format!("no such op {op_id}")))
    }
}

/// The refusal a pre-M26 tag op gets instead of a replay. Those rows recorded only an inverse —
/// "remove this tag", "re-add it as manual" — which could delete another source's copy of the tag
/// or re-attribute a model's row to the person. Refusing, and saying so, is the honest answer.
fn legacy_tag_replay(op_id: i64) -> CoreError {
    CoreError::Catalog(format!(
        "op {op_id} was recorded before per-source tag history (M26); replaying it could remove \
         or re-attribute tags another source added, so its undo is disabled — edit the tags directly"
    ))
}
