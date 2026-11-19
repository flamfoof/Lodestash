//! Identity follows content (DATA-MODEL §6.1, ADR-0025): the matching one scan of one source runs
//! before its crawl is recorded.
//!
//! An asset is its `asset_id` — tags, favourite, collections, import records, embeddings and undo
//! history are all keyed by it. This module decides, for one scan, which row each file on disk
//! *is*: rows whose path still holds their bytes stay, rows whose bytes turned up elsewhere follow
//! them, and a row whose file is gone takes over an untouched duplicate of its bytes. Where the
//! evidence does not single out one answer, nothing is matched.

use super::*;

/// The folder a superseded row is recorded under ([`Catalog::reconcile_moves`], step 5). A
/// dot-directory, which the crawler never walks — so no file can turn up "at" such a path.
pub const SUPERSEDED_DIR: &str = ".lodestash-superseded";

/// What [`Catalog::reconcile_moves`] did with one scan.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reconciled {
    /// Crawl paths whose record this pass applied in full. The caller drops them from what it
    /// hands `record_crawl`; they are on disk, so they still count as present.
    pub consumed: Vec<String>,
    /// Rows that followed their bytes to an open path.
    pub moved: usize,
    /// Untouched duplicates a row with no file left took the place of (and that were deleted).
    pub absorbed: usize,
    /// Rows whose path another row took and whose own bytes are nowhere.
    pub superseded: usize,
}

/// One `asset_file` row of the source, as the matching sees it.
struct Row {
    file_id: i64,
    asset_id: i64,
    path: String,
    hash: Option<String>,
    /// Loose (not a package's) and not in the Trash.
    loose_and_live: bool,
    size: Option<i64>,
    mtime: Option<i64>,
}

/// What this scan saw at a row's path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seen {
    /// The path is not in the crawl.
    Gone,
    /// The path still holds the row's recorded bytes: the record's hash equals the row's, or the
    /// file passed the `(size, mtime)` gate and was not read.
    Same,
    /// The file was read and holds something else.
    Overwritten,
    /// The file changed and could not be read. Nothing is known, so nothing is done.
    Unread,
}

impl Catalog {
    /// Decide which row each file of one full-source crawl **is**, and apply every answer in one
    /// transaction (DATA-MODEL §6.1). Call it **before** `record_crawl`, and drop
    /// [`Reconciled::consumed`] from the records handed on.
    ///
    /// 1. Rows whose path still holds their recorded hash are settled. A *movable* row — loose,
    ///    live, hashed, alone on its path — that is not settled is displaced: vanished, or
    ///    overwritten in place.
    /// 2. Crawl paths with a hash that are free, or held by a displaced row, are open.
    /// 3. Displaced rows and open paths pair one to one by hash ([`pair_by_evidence`]).
    /// 4. An overwritten row leaves a path that still exists only when another row's match lands
    ///    on it; otherwise it stays, an in-place edit, for `record_crawl` to update.
    /// 5. An overwritten row with no match whose path was taken is *superseded*: flagged missing
    ///    under [`SUPERSEDED_DIR`]. A vanished row with no match is left for `reconcile_missing`.
    /// 6. A vanished or superseded row that carries user data takes over a settled row with the
    ///    same hash that carries none ([`untouched`]): the duplicate is deleted.
    ///
    /// Trashed rows, package rows and rows with no recorded hash hold their paths throughout, and
    /// a path is entered only when it is free or its holder leaves in the same transaction — so
    /// nothing here can put two rows on one path.
    pub fn reconcile_moves(
        &mut self,
        source_id: i64,
        records: &[CrawlRecord],
    ) -> Result<Reconciled> {
        let rows = self.rows_for_matching(source_id)?;
        let by_path: HashMap<&str, &CrawlRecord> =
            records.iter().map(|r| (r.rel_path.as_str(), r)).collect();
        let mut holders: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, row) in rows.iter().enumerate() {
            holders.entry(row.path.as_str()).or_default().push(i);
        }
        let seen: Vec<Seen> = rows
            .iter()
            .map(|row| look(row, by_path.get(row.path.as_str()).copied()))
            .collect();
        // Two rows on one path is damage from before this rule; neither is moved.
        let movable = |i: usize| {
            rows[i].loose_and_live
                && rows[i].hash.is_some()
                && holders[rows[i].path.as_str()].len() == 1
        };
        let displaced: Vec<usize> = (0..rows.len())
            .filter(|&i| movable(i) && matches!(seen[i], Seen::Gone | Seen::Overwritten))
            .collect();
        if displaced.is_empty() {
            return Ok(Reconciled::default());
        }
        let is_displaced: HashSet<usize> = displaced.iter().copied().collect();

        // Step 2: the open paths.
        let open: Vec<&CrawlRecord> = records
            .iter()
            .filter(|r| r.content_hash.is_some())
            .filter(|r| match holders.get(r.rel_path.as_str()) {
                None => true,
                Some(held_by) => held_by.len() == 1 && is_displaced.contains(&held_by[0]),
            })
            .collect();

        // Step 3: pair by content.
        let hash_of = |i: usize| rows[i].hash.as_deref().unwrap_or_default();
        let mut displaced_by_hash: HashMap<&str, Vec<usize>> = HashMap::new();
        for &i in &displaced {
            displaced_by_hash.entry(hash_of(i)).or_default().push(i);
        }
        let mut open_by_hash: HashMap<&str, Vec<usize>> = HashMap::new();
        for (p, rec) in open.iter().enumerate() {
            let hash = rec.content_hash.as_deref().unwrap_or_default();
            open_by_hash.entry(hash).or_default().push(p);
        }
        let mut moves: Vec<(usize, usize)> = Vec::new();
        for (hash, row_ids) in &displaced_by_hash {
            let Some(places) = open_by_hash.get(hash) else {
                continue;
            };
            let row_paths: Vec<&str> = row_ids.iter().map(|&i| rows[i].path.as_str()).collect();
            let place_paths: Vec<&str> =
                places.iter().map(|&p| open[p].rel_path.as_str()).collect();
            for (r, p) in pair_by_evidence(&row_paths, &place_paths) {
                moves.push((row_ids[r], places[p]));
            }
        }

        // Step 4: a path that still exists is given up only to another identity. Dropping one
        // match can strand the one that was going to take its row's path, so repeat until stable;
        // a cycle supports itself and survives whole.
        loop {
            let arrivals: HashSet<&str> = moves
                .iter()
                .map(|&(_, p)| open[p].rel_path.as_str())
                .collect();
            let before = moves.len();
            moves.retain(|&(r, _)| {
                seen[r] == Seen::Gone || arrivals.contains(rows[r].path.as_str())
            });
            if moves.len() == before {
                break;
            }
        }
        let arrivals: HashSet<&str> = moves
            .iter()
            .map(|&(_, p)| open[p].rel_path.as_str())
            .collect();
        let leaving: HashSet<usize> = moves.iter().map(|&(r, _)| r).collect();
        let taken: HashSet<usize> = moves.iter().map(|&(_, p)| p).collect();

        // Step 5: an overwritten row that did not move, and whose path another row is taking.
        let evicted: HashSet<usize> = displaced
            .iter()
            .copied()
            .filter(|i| !leaving.contains(i) && arrivals.contains(rows[*i].path.as_str()))
            .collect();

        // Step 6: rows with no file left, and the untouched duplicates of their bytes.
        let spare_hashes: HashSet<&str> = open
            .iter()
            .enumerate()
            .filter(|(p, _)| !taken.contains(p))
            .filter_map(|(_, rec)| rec.content_hash.as_deref())
            .collect();
        let mut homeless_by_hash: HashMap<&str, Vec<usize>> = HashMap::new();
        for &i in &displaced {
            let no_file = !leaving.contains(&i) && (seen[i] == Seen::Gone || evicted.contains(&i));
            if no_file && !spare_hashes.contains(hash_of(i)) {
                homeless_by_hash.entry(hash_of(i)).or_default().push(i);
            }
        }
        // (the row taking over, the duplicate it replaces, the record at the duplicate's path)
        let mut absorbs: Vec<(usize, usize, &CrawlRecord)> = Vec::new();
        if !homeless_by_hash.is_empty() {
            let mut duplicates_by_hash: HashMap<&str, Vec<usize>> = HashMap::new();
            for (i, saw) in seen.iter().enumerate() {
                let settled = *saw == Seen::Same && movable(i);
                if settled && homeless_by_hash.contains_key(hash_of(i)) {
                    duplicates_by_hash.entry(hash_of(i)).or_default().push(i);
                }
            }
            if !duplicates_by_hash.is_empty() {
                // Read once, and only when there is a take-over to decide.
                let history = match people_touched(&self.conn) {
                    Ok(ids) => Some(ids),
                    Err(e) => {
                        tracing::warn!(%e, "moves: the history could not be read; no duplicate is taken over");
                        None
                    }
                };
                if let Some(history) = history {
                    let conn = &self.conn;
                    let no_user_data = |i: usize| -> Result<bool> {
                        Ok(!history.contains(&rows[i].asset_id)
                            && untouched(conn, rows[i].asset_id)?)
                    };
                    for (hash, homeless) in &homeless_by_hash {
                        let Some(duplicates) = duplicates_by_hash.get(hash) else {
                            continue;
                        };
                        let mut carriers = Vec::new();
                        for &i in homeless {
                            if !no_user_data(i)? {
                                carriers.push(i);
                            }
                        }
                        let mut empties = Vec::new();
                        for &i in duplicates {
                            if no_user_data(i)? {
                                empties.push(i);
                            }
                        }
                        let from: Vec<&str> =
                            carriers.iter().map(|&i| rows[i].path.as_str()).collect();
                        let to: Vec<&str> =
                            empties.iter().map(|&i| rows[i].path.as_str()).collect();
                        for (c, e) in pair_by_evidence(&from, &to) {
                            // `Seen::Same` means the duplicate's path is in the crawl.
                            if let Some(rec) = by_path.get(to[e]) {
                                absorbs.push((carriers[c], empties[e], rec));
                            }
                        }
                    }
                }
            }
        }
        let absorbing: HashSet<usize> = absorbs.iter().map(|&(v, _, _)| v).collect();
        let mut parked: Vec<usize> = evicted.difference(&absorbing).copied().collect();
        parked.sort_unstable();
        if moves.is_empty() && absorbs.is_empty() {
            return Ok(Reconciled::default());
        }

        let now = unix_now();
        let tx = self
            .conn
            .transaction()
            .map_err(|e| CoreError::Catalog(format!("moves begin: {e}")))?;
        let mut out = Reconciled::default();
        for &(r, p) in &moves {
            repoint(&tx, &rows[r], open[p], now)?;
            out.consumed.push(open[p].rel_path.clone());
            out.moved += 1;
        }
        for &(carrier, duplicate, rec) in &absorbs {
            let (carrier, duplicate) = (&rows[carrier], &rows[duplicate]);
            // A resident vector snapshot is keyed by its model's generation: bump it for every
            // model the duplicate had a vector in, or the snapshot keeps a row that is gone.
            tx.execute(
                "UPDATE embedding_generation SET generation = generation + 1, updated_at = ?2
                  WHERE model_id IN (SELECT model_id FROM asset_embedding WHERE asset_id = ?1)",
                params![duplicate.asset_id, now],
            )
            .map_err(|e| CoreError::Catalog(format!("moves generation: {e}")))?;
            tx.execute(
                "DELETE FROM asset WHERE id = ?1",
                params![duplicate.asset_id],
            )
            .map_err(|e| CoreError::Catalog(format!("moves duplicate delete: {e}")))?;
            repoint(&tx, carrier, rec, now)?;
            out.consumed.push(rec.rel_path.clone());
            out.absorbed += 1;
        }
        for &i in &parked {
            let row = &rows[i];
            let file_name = row.path.rsplit('/').next().unwrap_or(&row.path);
            tx.execute(
                "UPDATE asset_file SET rel_path = ?2, missing = 1 WHERE id = ?1",
                params![
                    row.file_id,
                    format!("{SUPERSEDED_DIR}/{}/{file_name}", row.asset_id)
                ],
            )
            .map_err(|e| CoreError::Catalog(format!("moves supersede: {e}")))?;
            out.superseded += 1;
        }
        tx.commit()
            .map_err(|e| CoreError::Catalog(format!("moves commit: {e}")))?;
        Ok(out)
    }

    /// Every `asset_file` row of the source — package rows too: `record_crawl` looks a path up
    /// across all of them, so each one holds its path.
    fn rows_for_matching(&self, source_id: i64) -> Result<Vec<Row>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT af.id, a.id, af.rel_path, a.content_hash,
                        a.package_id IS NULL AND a.deleted_at IS NULL,
                        af.size_bytes, af.mtime
                   FROM asset_file af JOIN asset a ON a.id = af.asset_id
                  WHERE a.source_id = ?1",
            )
            .map_err(|e| CoreError::Catalog(format!("moves prep: {e}")))?;
        let rows = stmt
            .query_map(params![source_id], |r| {
                Ok(Row {
                    file_id: r.get(0)?,
                    asset_id: r.get(1)?,
                    path: r.get(2)?,
                    hash: r.get(3)?,
                    loose_and_live: r.get(4)?,
                    size: r.get(5)?,
                    mtime: r.get(6)?,
                })
            })
            .map_err(|e| CoreError::Catalog(format!("moves query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("moves row: {e}")))
    }
}

/// What the scan saw at `row`'s path.
fn look(row: &Row, rec: Option<&CrawlRecord>) -> Seen {
    let Some(rec) = rec else {
        return Seen::Gone;
    };
    match (rec.content_hash.as_deref(), row.hash.as_deref()) {
        (Some(read), Some(recorded)) if read == recorded => Seen::Same,
        (Some(_), _) => Seen::Overwritten,
        // Not read. The crawl skips a file only at the `(size, mtime)` the catalog has for it.
        (None, _) if row.size == Some(rec.size_bytes) && row.mtime == Some(rec.mtime) => Seen::Same,
        (None, _) => Seen::Unread,
    }
}

/// Point `row` at the file `rec` describes: the same asset, at a new path. The hash is not
/// touched — the row is here because the path holds its bytes.
fn repoint(tx: &Connection, row: &Row, rec: &CrawlRecord, now: i64) -> Result<()> {
    tx.execute(
        "UPDATE asset_file SET rel_path = ?2, ext = ?3, size_bytes = ?4, mtime = ?5, missing = 0
          WHERE id = ?1",
        params![
            row.file_id,
            rec.rel_path,
            rec.rel_path.rsplit('.').next().map(str::to_lowercase),
            rec.size_bytes,
            rec.mtime,
        ],
    )
    .map_err(|e| CoreError::Catalog(format!("moves file update: {e}")))?;
    // The name is the file stem, so it follows; the FTS trigger re-indexes on it.
    tx.execute(
        "UPDATE asset SET name = ?2, updated_at = ?3 WHERE id = ?1",
        params![row.asset_id, rec.name, now],
    )
    .map_err(|e| CoreError::Catalog(format!("moves asset update: {e}")))?;
    Ok(())
}

/// Pair rows with places of **one content hash**, one to one, by evidence alone
/// (DATA-MODEL §6.1 "The pairing rule"). Returns index pairs into the two slices.
///
/// - One row and one place pair.
/// - Otherwise a row and a place pair when they share a trailing run of path components that no
///   other unpaired row and no other unpaired place shares — the longest runs first, down to the
///   file name alone. A folder moved rigidly keeps its byte-identical files apart this way.
/// - Then, if exactly one row and one place are left, they pair.
///
/// Whatever remains is unmatched. Nothing depends on the order of either slice.
pub(super) fn pair_by_evidence(rows: &[&str], places: &[&str]) -> Vec<(usize, usize)> {
    if rows.len() == 1 && places.len() == 1 {
        return vec![(0, 0)];
    }
    let suffix = |path: &str, k: usize| -> Option<String> {
        let parts: Vec<&str> = path.split('/').collect();
        (parts.len() >= k).then(|| parts[parts.len() - k..].join("/"))
    };
    let depth = |path: &&str| path.split('/').count();
    let deepest = rows.iter().chain(places).map(depth).max().unwrap_or(0);
    let mut free_rows: Vec<usize> = (0..rows.len()).collect();
    let mut free_places: Vec<usize> = (0..places.len()).collect();
    let mut pairs = Vec::new();
    for k in (1..=deepest).rev() {
        let mut sharing: HashMap<String, (Vec<usize>, Vec<usize>)> = HashMap::new();
        for &r in &free_rows {
            if let Some(s) = suffix(rows[r], k) {
                sharing.entry(s).or_default().0.push(r);
            }
        }
        for &p in &free_places {
            if let Some(s) = suffix(places[p], k) {
                sharing.entry(s).or_default().1.push(p);
            }
        }
        let (mut paired_rows, mut paired_places) = (HashSet::new(), HashSet::new());
        for (rs, ps) in sharing.into_values() {
            if let ([r], [p]) = (rs.as_slice(), ps.as_slice()) {
                pairs.push((*r, *p));
                paired_rows.insert(*r);
                paired_places.insert(*p);
            }
        }
        free_rows.retain(|r| !paired_rows.contains(r));
        free_places.retain(|p| !paired_places.contains(p));
        if free_rows.is_empty() || free_places.is_empty() {
            break;
        }
    }
    if let ([r], [p]) = (free_rows.as_slice(), free_places.as_slice()) {
        pairs.push((*r, *p));
    }
    pairs.sort_unstable();
    pairs
}

/// Has nothing a person — or an agent they approved — did to this asset left a mark **in its
/// rows**? (DATA-MODEL §6.1 "No user data"; [`people_touched`] is the other half.)
///
/// Live, not a favourite, unrated, not confirmed; no description; no licence override; no tag
/// from a source other than `ai` / `ai:*`; in no collection or staging tray; no import record and
/// no export record; no manually assigned or pinned category; no ignored link. What the machine
/// made — AI tags, embeddings, previews, metrics, harvested edges, rule-assigned categories — does
/// not count.
fn untouched(conn: &Connection, asset_id: i64) -> Result<bool> {
    conn.query_row(
        "SELECT a.deleted_at IS NULL AND a.favorite = 0 AND a.rating IS NULL
                AND a.status <> 'confirmed'
                AND TRIM(COALESCE(a.description, '')) = ''
                AND a.license IS NULL AND a.attribution IS NULL AND a.license_url IS NULL
                AND NOT EXISTS (SELECT 1 FROM asset_tag t
                                 WHERE t.asset_id = a.id
                                   AND t.source <> 'ai' AND t.source NOT LIKE 'ai:%')
                AND NOT EXISTS (SELECT 1 FROM collection_asset c WHERE c.asset_id = a.id)
                AND NOT EXISTS (SELECT 1 FROM import_record i WHERE i.asset_id = a.id)
                AND NOT EXISTS (SELECT 1 FROM export_record e WHERE e.asset_id = a.id)
                AND NOT EXISTS (SELECT 1 FROM asset_category ac
                                 WHERE ac.asset_id = a.id
                                   AND (ac.assigned_by <> 'rule' OR ac.is_pinned <> 0))
                AND NOT EXISTS (SELECT 1 FROM asset_relationship r
                                 WHERE r.from_asset_id = a.id AND r.state = 'ignored')
           FROM asset a WHERE a.id = ?1",
        params![asset_id],
        |r| r.get(0),
    )
    .map_err(|e| CoreError::Catalog(format!("moves user data: {e}")))
}

/// Every asset a person's or an approved agent's action names in the history that is kept: the
/// interactive `op_log` rows (undo and redo payloads) and the `tag_request` receipts. An action
/// can leave nothing in the asset's own rows — a rejected tag, a trash that was undone — and
/// still be something done to it.
///
/// An `Err` when a row cannot be read as JSON: the caller then takes over nothing, rather than
/// treat history it could not read as history that is not there.
fn people_touched(conn: &Connection) -> Result<HashSet<i64>> {
    let mut ids = HashSet::new();
    for sql in [
        "SELECT payload_json FROM op_log WHERE origin <> 'background'",
        "SELECT forward_json FROM op_log WHERE origin <> 'background' AND forward_json IS NOT NULL",
        "SELECT result_json FROM tag_request",
    ] {
        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| CoreError::Catalog(format!("moves history prep: {e}")))?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| CoreError::Catalog(format!("moves history: {e}")))?;
        for json in rows {
            let json = json.map_err(|e| CoreError::Catalog(format!("moves history row: {e}")))?;
            collect_asset_ids(&serde_json::from_str(&json)?, &mut ids);
        }
    }
    Ok(ids)
}

/// Every asset id a history payload names: a number under `asset_id`, or the numbers of an array
/// under `asset_ids` / `…_assets`, at any depth.
fn collect_asset_ids(value: &serde_json::Value, out: &mut HashSet<i64>) {
    use serde_json::Value;
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                match child {
                    Value::Number(n) if key == "asset_id" => out.extend(n.as_i64()),
                    Value::Array(items) if key == "asset_ids" || key.ends_with("_assets") => {
                        out.extend(items.iter().filter_map(Value::as_i64));
                    }
                    _ => {}
                }
                collect_asset_ids(child, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|v| collect_asset_ids(v, out)),
        _ => {}
    }
}
