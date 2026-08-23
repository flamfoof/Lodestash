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
        self.conn
            .execute(
                "INSERT INTO op_log (op_kind, payload_json, forward_json, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![op_kind, payload_json, forward_json, unix_now()],
            )
            .map_err(|e| CoreError::Catalog(format!("record_op: {e}")))?;
        // New work invalidates any undone ops.
        self.conn
            .execute("DELETE FROM op_log WHERE undone_at IS NOT NULL", [])
            .map_err(|e| CoreError::Catalog(format!("record_op redo clear: {e}")))?;
        // The log is a recent-actions tail, not a history — cap it so a 10k-asset bulk write
        // cannot grow the file forever. The oldest rows go first.
        let _ = self.conn.execute(
            "DELETE FROM op_log WHERE id NOT IN (
                 SELECT id FROM op_log ORDER BY id DESC LIMIT 200)",
            [],
        );
        Ok(self.conn.last_insert_rowid())
    }

    /// The newest undo-log rows, newest first, capped at `limit`. Undone ops are hidden — they
    /// are the redo stack, not the undo button's target.
    pub fn recent_ops(&self, limit: usize) -> Result<Vec<OpLogRow>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, op_kind, payload_json, created_at FROM op_log
                 WHERE undone_at IS NULL
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
                 WHERE undone_at IS NOT NULL
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

    /// Record a `set_tags` op with the asset's **current** per-source tags snapshotted
    /// into the payload — call this *before* [`Self::set_tags`] mutates, so
    /// [`Self::revert_op`] can restore the exact prior set.
    pub fn record_set_tags_op(
        &self,
        asset_id: i64,
        source: &str,
        tags: &[TagAssignment],
    ) -> Result<i64> {
        let prior = {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT COALESCE(t.namespace, 'free'), t.name, at.confidence
                       FROM asset_tag at JOIN tag t ON t.id = at.tag_id
                      WHERE at.asset_id = ?1 AND at.source = ?2
                      ORDER BY t.name",
                )
                .map_err(|e| CoreError::Catalog(format!("set_tags snapshot prep: {e}")))?;
            let rows = stmt
                .query_map(params![asset_id, source], |r| {
                    Ok(TagAssignment {
                        namespace: r.get(0)?,
                        value: r.get(1)?,
                        confidence: r.get(2)?,
                    })
                })
                .map_err(|e| CoreError::Catalog(format!("set_tags snapshot query: {e}")))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| CoreError::Catalog(format!("set_tags snapshot row: {e}")))?
        };
        let payload = serde_json::json!({
            "asset_id": asset_id,
            "source": source,
            "tags": prior.into_iter().map(|t| serde_json::json!({
                "namespace": t.namespace,
                "value": t.value,
                "confidence": t.confidence,
            })).collect::<Vec<_>>(),
        })
        .to_string();
        let forward = serde_json::json!({
            "asset_id": asset_id,
            "source": source,
            "tags": tags.iter().map(|t| serde_json::json!({
                "namespace": t.namespace,
                "value": t.value,
                "confidence": t.confidence,
            })).collect::<Vec<_>>(),
        })
        .to_string();
        self.record_op("set_tags", &payload, Some(&forward))
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

    /// Record one bulk-tag op for the assets that actually changed (a no-op add on an
    /// already-tagged asset is not recorded — its revert would wrongly remove a pre-existing
    /// tag). One row per batch, so undoing a 5-asset bulk edit reverts all five.
    pub fn record_bulk_tag_op(
        &self,
        asset_ids: &[i64],
        namespace: &str,
        value: &str,
        on: bool,
    ) -> Result<i64> {
        let payload = serde_json::json!({
            "asset_ids": asset_ids,
            "namespace": namespace,
            "value": value,
            "on": on,
        })
        .to_string();
        self.record_op("bulk_tag", &payload, None)
    }

    /// Undo the operation recorded as `op_id`, then remove the op from the log (a
    /// reverted op is gone — re-undo would double-apply). Dispatches on `op_kind`:
    /// - `set_tags` → restores the snapshotted per-source tag set (reuses
    ///   [`Self::set_tags`]);
    /// - `set_trashed` → re-applies [`Self::set_trashed`] with the opposite direction (an
    ///   untrash is undone by trashing again and vice versa);
    /// - `create_collection` → deletes the created collection;
    /// - `bulk_tag` → applies the exact inverse per asset (add ⇄ remove of the same tag).
    pub fn revert_op(&mut self, op_id: i64) -> Result<()> {
        let (kind, _) = self.op_payload(op_id)?;
        match kind.as_str() {
            "set_tags" => self.revert_set_tags(op_id),
            "set_trashed" => self.revert_set_trashed(op_id),
            "create_collection" => self.revert_create_collection(op_id),
            "bulk_tag" => self.revert_bulk_tag(op_id),
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
            "set_tags" => self.redo_set_tags(op_id, &forward),
            "set_trashed" => self.redo_set_trashed(op_id, &forward),
            "create_collection" => self.redo_create_collection(op_id, &forward),
            "bulk_tag" => self.redo_bulk_tag(op_id, &forward),
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

    /// Redo a `set_tags` op: write the recorded forward tag set back, then consume the row.
    pub fn redo_set_tags(&mut self, op_id: i64, payload: &str) -> Result<()> {
        let v: serde_json::Value = serde_json::from_str(payload)
            .map_err(|e| CoreError::Catalog(format!("redo_set_tags payload: {e}")))?;
        let asset_id = v["asset_id"]
            .as_i64()
            .ok_or_else(|| CoreError::Catalog("redo_set_tags: missing asset_id".into()))?;
        let source = v["source"]
            .as_str()
            .ok_or_else(|| CoreError::Catalog("redo_set_tags: missing source".into()))?;
        let mut tags = Vec::new();
        for t in v["tags"].as_array().unwrap_or(&Vec::new()) {
            tags.push(TagAssignment {
                namespace: t["namespace"].as_str().unwrap_or("free").to_string(),
                value: t["value"]
                    .as_str()
                    .ok_or_else(|| CoreError::Catalog("redo_set_tags: tag without value".into()))?
                    .to_string(),
                confidence: t["confidence"].as_f64(),
            });
        }
        self.set_tags(asset_id, source, None, &tags)?;
        self.delete_ops(&[op_id])?;
        Ok(())
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

    /// Redo a `bulk_tag` op: re-apply the recorded change, then consume the row.
    pub fn redo_bulk_tag(&mut self, op_id: i64, payload: &str) -> Result<()> {
        let p: serde_json::Value = serde_json::from_str(payload)
            .map_err(|e| CoreError::Catalog(format!("redo_bulk_tag payload: {e}")))?;
        let ids: Vec<i64> = p["asset_ids"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_i64()).collect())
            .unwrap_or_default();
        let namespace = p["namespace"].as_str().unwrap_or("free");
        let value = p["value"].as_str().unwrap_or_default();
        let on = p["on"].as_bool().unwrap_or(true);
        for id in &ids {
            if on {
                self.add_asset_tag(*id, "manual", namespace, value)?;
            } else {
                self.remove_asset_tag(*id, namespace, value)?;
            }
        }
        self.delete_ops(&[op_id])?;
        Ok(())
    }

    /// Undo a `bulk_tag` op: apply the exact inverse per recorded asset (an add is undone by
    /// removing that tag, a removal by re-adding it), then drop the op. Only the assets that
    /// actually changed are in the payload, so a no-op never gets its pre-existing tag yanked.
    pub fn revert_bulk_tag(&mut self, op_id: i64) -> Result<()> {
        let (kind, payload) = self.op_payload(op_id)?;
        debug_assert_eq!(kind, "bulk_tag");
        let p: serde_json::Value = serde_json::from_str(&payload)
            .map_err(|e| CoreError::Catalog(format!("revert_bulk_tag payload: {e}")))?;
        let ids: Vec<i64> = p["asset_ids"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_i64()).collect())
            .unwrap_or_default();
        let namespace = p["namespace"].as_str().unwrap_or("free");
        let value = p["value"].as_str().unwrap_or_default();
        let on = p["on"].as_bool().unwrap_or(true);
        if ids.is_empty() {
            return Err(CoreError::Catalog("revert_bulk_tag: empty payload".into()));
        }
        for id in &ids {
            if on {
                self.remove_asset_tag(*id, namespace, value)?;
            } else {
                self.add_asset_tag(*id, "manual", namespace, value)?;
            }
        }
        self.mark_undone(op_id)?;
        Ok(())
    }

    /// Undo a `set_tags` op: write the snapshotted prior tag set back for the recorded
    /// asset + source, then drop the op.
    pub fn revert_set_tags(&mut self, op_id: i64) -> Result<()> {
        let (kind, payload) = self.op_payload(op_id)?;
        if kind != "set_tags" {
            return Err(CoreError::Catalog(format!(
                "revert_set_tags: op {op_id} is {kind:?}, not set_tags"
            )));
        }
        let v: serde_json::Value = serde_json::from_str(&payload)?;
        let asset_id = v["asset_id"]
            .as_i64()
            .ok_or_else(|| CoreError::Catalog("set_tags op payload: missing asset_id".into()))?;
        let source = v["source"]
            .as_str()
            .ok_or_else(|| CoreError::Catalog("set_tags op payload: missing source".into()))?;
        let mut tags = Vec::new();
        for t in v["tags"]
            .as_array()
            .ok_or_else(|| CoreError::Catalog("set_tags op payload: missing tags".into()))?
        {
            tags.push(TagAssignment {
                namespace: t["namespace"].as_str().unwrap_or("free").to_string(),
                value: t["value"]
                    .as_str()
                    .ok_or_else(|| {
                        CoreError::Catalog("set_tags op payload: tag without value".into())
                    })?
                    .to_string(),
                confidence: t["confidence"].as_f64(),
            });
        }
        self.set_tags(asset_id, source, None, &tags)?;
        self.mark_undone(op_id)?;
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
