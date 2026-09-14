//! One atomic, attributed tag write (v1 plan S04, ADR-0017).
//!
//! Every tag mutation with an author — a person's bulk edit or rejection, a model's result, an
//! agent's batch — goes through [`Catalog::apply_tag_batch`]. The batch is validated whole before
//! anything is written, then applied in **one transaction** together with its FTS re-index and,
//! when asked, one history row holding the complete before/after state of every association it
//! touched. A failure anywhere leaves neither partial tags nor orphaned history.
//!
//! Undo and redo replay those snapshots, and only after checking that the associations still look
//! the way the operation left them: a newer write by another producer is reported as a conflict,
//! never silently overwritten.

use super::*;

/// The key M26 gave every AI association written before producers were recorded. Which model made
/// those rows is unrecoverable — the local drain stored its zero-shot proposals under the VLM's
/// model id — so they say exactly that, rather than carrying an invented producer.
pub const LEGACY_AI_SOURCE: &str = "ai:legacy";

/// The association source of one producer: `ai:<producer>`.
pub fn ai_source(producer_id: &str) -> String {
    format!("ai:{producer_id}")
}

/// Every AI association source: namespaced producers, the legacy key, and the bare `ai` that
/// pre-M26 rows (and older fixtures) carry.
pub fn is_ai_source(source: &str) -> bool {
    source == "ai" || source.starts_with("ai:")
}

/// Assets one batch may touch (ENGINE-API §8.3). A measured starting point, not a setting.
pub const MAX_BATCH_ASSETS: usize = 100;
/// Longest label, in Unicode scalar values, after normalisation.
pub const MAX_LABEL_CHARS: usize = 128;
/// Longest caption a batch may write.
pub const MAX_DESCRIPTION_CHARS: usize = 2048;
/// A model's assignments per asset, after deduplication…
pub const MAX_AI_ASSIGNMENTS: usize = 32;
/// …and per namespace.
pub const MAX_AI_PER_NAMESPACE: usize = 8;

/// Namespaces a model may author (IDENTIFICATION-AND-SEARCH §2). The audio zero-shot scores into
/// `subject` and `usage` too, so one set covers every current producer.
pub const AI_NAMESPACES: &[&str] = &["subject", "style", "material", "mood", "usage", "free"];
/// Namespaces a person may author: every facet except `technical`, which is measured from the file
/// and never typed.
pub const HUMAN_NAMESPACES: &[&str] = &[
    "subject", "style", "material", "color", "mood", "usage", "audio", "free",
];

/// How one item changes an asset's associations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagMode {
    /// Add these; every existing association stays as it is.
    Append,
    /// Replace this source's own set; every other source's associations survive.
    ReplaceSource,
    /// Remove exactly these associations of this source, and nothing else.
    RemoveScoped,
    /// A person rejecting a term: remove it from **every** source on the asset. Explicit, and only
    /// a person may do it — removing a manual tag never implies it.
    RejectAllSources,
}

/// Who a recorded operation belongs to. Retention is partitioned on it: the interactive tail (a
/// person's and an agent's approved writes) keeps its 200 newest rows however much the background
/// lane writes, and the background lane keeps its own 200.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryOrigin {
    User,
    Agent,
    Background,
}

impl HistoryOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            HistoryOrigin::User => "user",
            HistoryOrigin::Agent => "agent",
            HistoryOrigin::Background => "background",
        }
    }
}

/// One asset's part of a batch.
#[derive(Debug, Clone)]
pub struct TagBatchItem {
    pub asset_id: i64,
    /// `manual` | `imported` | `ai:<producer>`.
    pub source: String,
    /// The model that actually produced these tags — reported provenance, `None` when unknown.
    pub model_id: Option<String>,
    pub mode: TagMode,
    pub tags: Vec<TagAssignment>,
    /// `Some` replaces the caption, atomically with the tags; `None` leaves it alone.
    pub description: Option<String>,
    /// The [`Catalog::annotation_revision`] the writer last saw. Checked inside the transaction:
    /// anything changed since refuses the whole batch with `revision_conflict`, rather than
    /// writing over a change the writer never saw. `None` for writers that own their own state.
    pub expected_revision: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TagBatch {
    pub items: Vec<TagBatchItem>,
    /// Clear `tags_dirty` on every asset in the batch: the configured tagger finished them, with a
    /// result or with "nothing to say". A person's edit is not model work and leaves the queue be.
    pub completes_queue: bool,
    /// Record one undoable history row under this origin. `None` records nothing.
    pub history: Option<HistoryOrigin>,
    /// Record a durable receipt for this request in the same transaction (ENGINE-API §8.3), so a
    /// retry after a lost response finds the result instead of applying the batch twice.
    pub receipt: Option<TagReceiptKey>,
}

/// One review outcome for an existing AI association. An acceptance preserves the association and
/// records why it was checked; a rejection removes only that exact source association.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TagReviewAction {
    Accept,
    Reject,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TagReviewDecision {
    pub asset_id: i64,
    pub expected_revision: String,
    pub namespace: String,
    pub value: String,
    pub source: String,
    pub action: TagReviewAction,
    pub reason: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TagReviewBatch {
    pub reviewer: String,
    pub decisions: Vec<TagReviewDecision>,
    pub receipt: Option<TagReceiptKey>,
}

/// A request's identity: the id its caller chose and a digest of the validated body.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TagReceiptKey {
    pub request_id: String,
    pub digest: String,
}

/// What a committed request left behind: its body digest and its terminal result.
#[derive(Debug, Clone, PartialEq)]
pub struct TagReceipt {
    pub digest: String,
    pub result: serde_json::Value,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct TagBatchOutcome {
    /// The history row, when one was recorded (a batch that changed nothing records none).
    pub op_id: Option<i64>,
    pub changed_assets: Vec<i64>,
    pub unchanged_assets: Vec<i64>,
}

/// Why `source` cannot be written to, or `None` when it can.
pub fn source_problem(source: &str) -> Option<String> {
    if source == "manual" || source == "imported" {
        return None;
    }
    match source.strip_prefix("ai:") {
        Some("legacy") => Some(
            "ai:legacy is history, not a producer — nothing new is written under it".to_string(),
        ),
        Some(p)
            if !p.is_empty()
                && p.len() <= 128
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._@:+-".contains(c)) =>
        {
            None
        }
        _ => Some(format!(
            "source {source:?} is not manual, imported or ai:<producer>"
        )),
    }
}

/// Why one assignment cannot be written under `source`, or `None` when it can.
///
/// Shared with the model-output parsers (`lodestash_ai::tagger::validate`), which **drop** what this
/// **refuses** — so a drain never hands the boundary something it would reject, and there is one
/// set of rules rather than two that drift. A model's output is untrusted even from a local model.
pub fn assignment_problem(source: &str, t: &TagAssignment) -> Option<String> {
    let ai = is_ai_source(source);
    let ns = namespace_of(&t.namespace);
    let allowed = if ai { AI_NAMESPACES } else { HUMAN_NAMESPACES };
    if !allowed.contains(&ns.as_str()) {
        let who = if ai { "a model" } else { "a person" };
        return Some(format!("namespace {ns:?} is not one {who} may write"));
    }
    let Some((_, display)) = normalize_tag(&ns, &t.value) else {
        return Some("empty label".to_string());
    };
    if display.chars().count() > MAX_LABEL_CHARS {
        return Some(format!("label longer than {MAX_LABEL_CHARS} characters"));
    }
    if ai {
        let raw = t.value.trim();
        if raw.parse::<f64>().is_ok() || raw.starts_with('{') || raw.starts_with('[') {
            return Some(format!(
                "label {raw:?} is a number or an object — a score in the label slot, not a tag"
            ));
        }
        if let Some(c) = t.confidence {
            if !c.is_finite() || !(0.0..=1.0).contains(&c) {
                return Some(format!("confidence {c} is not a number in [0, 1]"));
            }
        }
    } else if t.confidence.is_some() {
        return Some("a person's tag carries no confidence".to_string());
    }
    None
}

fn namespace_of(raw: &str) -> String {
    let ns = raw.trim().to_lowercase();
    if ns.is_empty() {
        "free".to_string()
    } else {
        ns
    }
}

/// One normalised, deduplicated assignment, ready to write.
struct Wanted {
    norm: String,
    display: String,
    namespace: String,
    confidence: Option<f64>,
}

/// Validate and normalise one item's tags. Deduplication happens **before** the caps: nine
/// spellings of one label are one assignment, not nine against a limit of eight.
fn normalize_item(
    source: &str,
    tags: &[TagAssignment],
) -> std::result::Result<Vec<Wanted>, String> {
    let mut out: Vec<Wanted> = Vec::new();
    for t in tags {
        if let Some(p) = assignment_problem(source, t) {
            return Err(format!("{}:{} — {p}", t.namespace, t.value));
        }
        let namespace = namespace_of(&t.namespace);
        let (norm, display) = normalize_tag(&namespace, &t.value).expect("checked above");
        match out.iter_mut().find(|w| w.norm == norm) {
            // The same label twice keeps the model's more confident guess.
            Some(w) => {
                if t.confidence.unwrap_or(-1.0) > w.confidence.unwrap_or(-1.0) {
                    w.confidence = t.confidence;
                }
            }
            None => out.push(Wanted {
                norm,
                display,
                namespace,
                confidence: t.confidence,
            }),
        }
    }
    if is_ai_source(source) {
        if out.len() > MAX_AI_ASSIGNMENTS {
            return Err(format!(
                "{} assignments after deduplication; a model may write at most {MAX_AI_ASSIGNMENTS}",
                out.len()
            ));
        }
        for ns in AI_NAMESPACES {
            let n = out.iter().filter(|w| w.namespace == *ns).count();
            if n > MAX_AI_PER_NAMESPACE {
                return Err(format!(
                    "{n} values in {ns}; a model may write at most {MAX_AI_PER_NAMESPACE} per namespace"
                ));
            }
        }
    }
    Ok(out)
}

/// Validate one item's assignments exactly as [`Catalog::apply_tag_batch`] will — normalisation,
/// deduplication, caps, per-author policy — so an API can refuse a request before it is parked for
/// approval, with the same words the batch would use.
pub fn validate_assignments(
    source: &str,
    tags: &[TagAssignment],
) -> std::result::Result<(), String> {
    normalize_item(source, tags).map(|_| ())
}

fn invalid(why: impl std::fmt::Display) -> CoreError {
    CoreError::Catalog(format!("invalid tag batch: {why}"))
}

/// One association as history records it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct Row {
    source: String,
    tag_id: i64,
    confidence: Option<f64>,
    model_id: Option<String>,
    created_at: i64,
}

impl Row {
    fn key(&self) -> (&str, i64) {
        (&self.source, self.tag_id)
    }
}

/// Equal as associations: same rows, same scores, same models. When a row was created is history,
/// not state — a replaced-with-itself row is not a change.
fn same_rows(a: &[Row], b: &[Row]) -> bool {
    a.len() == b.len()
        && a.iter().all(|x| {
            b.iter().any(|y| {
                x.key() == y.key() && x.confidence == y.confidence && x.model_id == y.model_id
            })
        })
}

/// One side (before or after) of one asset's change.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Side {
    rows: Vec<Row>,
    /// Meaningful only when the change touched the caption (`AssetChange::caption`).
    description: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct AssetChange {
    asset_id: i64,
    /// The sources whose associations this change owns — replay touches these and no others.
    sources: Vec<String>,
    caption: bool,
    before: Side,
    after: Side,
}

/// The `tag_batch` history payload. Versioned: the inverse-only payloads before it could not
/// restore what they removed.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct BatchPayload {
    v: u32,
    changes: Vec<AssetChange>,
    #[serde(default)]
    reviewer: Option<String>,
    #[serde(default)]
    reviews: Vec<TagReviewDecision>,
}

const PAYLOAD_VERSION: u32 = 2;

/// The associations one asset holds under `sources`.
fn rows_for(conn: &Connection, asset_id: i64, sources: &[String]) -> Result<Vec<Row>> {
    if sources.is_empty() {
        return Ok(Vec::new());
    }
    let marks = vec!["?"; sources.len()].join(",");
    let sql = format!(
        "SELECT source, tag_id, confidence, model_id, created_at FROM asset_tag
          WHERE asset_id = ? AND source IN ({marks}) ORDER BY source, tag_id"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| CoreError::Catalog(format!("tag rows prep: {e}")))?;
    let mut args: Vec<&dyn rusqlite::ToSql> = vec![&asset_id];
    for s in sources {
        args.push(s);
    }
    let rows = stmt
        .query_map(args.as_slice(), |r| {
            Ok(Row {
                source: r.get(0)?,
                tag_id: r.get(1)?,
                confidence: r.get(2)?,
                model_id: r.get(3)?,
                created_at: r.get(4)?,
            })
        })
        .map_err(|e| CoreError::Catalog(format!("tag rows: {e}")))?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| CoreError::Catalog(format!("tag row: {e}")))
}

/// The sources that hold a label on an asset — what a rejection has to reach.
fn sources_holding(conn: &Connection, asset_id: i64, norm: &str) -> Result<Vec<String>> {
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT at.source FROM asset_tag at JOIN tag t ON t.id = at.tag_id
              WHERE at.asset_id = ?1 AND t.norm_name = ?2",
        )
        .map_err(|e| CoreError::Catalog(format!("tag sources prep: {e}")))?;
    let rows = stmt
        .query_map(params![asset_id, norm], |r| r.get::<_, String>(0))
        .map_err(|e| CoreError::Catalog(format!("tag sources: {e}")))?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| CoreError::Catalog(format!("tag source: {e}")))
}

/// The `tag` row for a label: looked up, and created only when `create` and it does not exist — so
/// a batch that changes nothing writes nothing, not even a vocabulary row.
fn tag_id_for(conn: &Connection, w: &Wanted, create: bool) -> Result<Option<i64>> {
    let found: Option<i64> = conn
        .query_row(
            "SELECT id FROM tag WHERE norm_name = ?1",
            params![w.norm],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| CoreError::Catalog(format!("tag lookup: {e}")))?;
    if found.is_some() || !create {
        return Ok(found);
    }
    conn.execute(
        "INSERT INTO tag (name, norm_name, namespace) VALUES (?1, ?2, ?3)",
        params![w.display, w.norm, w.namespace],
    )
    .map_err(|e| CoreError::Catalog(format!("tag insert: {e}")))?;
    Ok(Some(conn.last_insert_rowid()))
}

/// Turn `current` into `target` for one asset: delete what went, insert what came, update what
/// changed score or model. Rows outside `current`/`target` are never touched.
fn write_rows(conn: &Connection, asset_id: i64, current: &[Row], target: &[Row]) -> Result<()> {
    for r in current {
        if !target.iter().any(|t| t.key() == r.key()) {
            conn.execute(
                "DELETE FROM asset_tag WHERE asset_id = ?1 AND tag_id = ?2 AND source = ?3",
                params![asset_id, r.tag_id, r.source],
            )
            .map_err(|e| CoreError::Catalog(format!("tag delete: {e}")))?;
        }
    }
    for t in target {
        match current.iter().find(|r| r.key() == t.key()) {
            None => {
                conn.execute(
                    "INSERT INTO asset_tag (asset_id, tag_id, source, confidence, model_id, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![asset_id, t.tag_id, t.source, t.confidence, t.model_id, t.created_at],
                )
                .map_err(|e| CoreError::Catalog(format!("tag insert: {e}")))?;
            }
            Some(r) if r.confidence != t.confidence || r.model_id != t.model_id => {
                conn.execute(
                    "UPDATE asset_tag SET confidence = ?4, model_id = ?5
                      WHERE asset_id = ?1 AND tag_id = ?2 AND source = ?3",
                    params![asset_id, t.tag_id, t.source, t.confidence, t.model_id],
                )
                .map_err(|e| CoreError::Catalog(format!("tag update: {e}")))?;
            }
            Some(_) => {}
        }
    }
    Ok(())
}

pub(super) fn annotation_revision_in(conn: &Connection, asset_id: i64) -> Result<String> {
    let mut stmt = conn
        .prepare(
            "SELECT source, tag_id, confidence, model_id FROM asset_tag
              WHERE asset_id = ?1 ORDER BY source, tag_id",
        )
        .map_err(|e| CoreError::Catalog(format!("revision prep: {e}")))?;
    let mut hasher = blake3::Hasher::new();
    let rows = stmt
        .query_map(params![asset_id], |r| {
            Ok(format!(
                "{}\t{}\t{:?}\t{:?}\n",
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, Option<f64>>(2)?,
                r.get::<_, Option<String>>(3)?
            ))
        })
        .map_err(|e| CoreError::Catalog(format!("revision rows: {e}")))?;
    for row in rows {
        hasher.update(
            row.map_err(|e| CoreError::Catalog(format!("revision row: {e}")))?
                .as_bytes(),
        );
    }
    hasher.update(b"\ncaption:");
    if let Some(caption) = description_of(conn, asset_id)? {
        hasher.update(caption.as_bytes());
    }
    Ok(hasher.finalize().to_hex()[..32].to_string())
}

fn description_of(conn: &Connection, asset_id: i64) -> Result<Option<String>> {
    conn.query_row(
        "SELECT description FROM asset WHERE id = ?1",
        params![asset_id],
        |r| r.get(0),
    )
    .map_err(|e| CoreError::Catalog(format!("description read: {e}")))
}

/// Re-index once: a caption UPDATE fires the FTS trigger, which reads the view — and so the final
/// tags — itself; otherwise the tag change re-indexes explicitly. One refresh either way.
fn finish_asset(conn: &Connection, asset_id: i64, caption: Option<Option<&str>>) -> Result<()> {
    match caption {
        Some(text) => conn
            .execute(
                "UPDATE asset SET description = ?2, updated_at = ?3 WHERE id = ?1",
                params![asset_id, text, unix_now()],
            )
            .map(|_| ())
            .map_err(|e| CoreError::Catalog(format!("caption write: {e}"))),
        None => refresh_fts(conn, asset_id),
    }
}

impl Catalog {
    /// Apply one tag batch atomically (v1 plan S04). See the module header for the contract.
    pub fn apply_tag_batch(&mut self, batch: &TagBatch) -> Result<TagBatchOutcome> {
        // ---- the whole batch is validated before anything is written ----
        let mut assets: Vec<i64> = batch.items.iter().map(|i| i.asset_id).collect();
        assets.sort_unstable();
        assets.dedup();
        if assets.len() > MAX_BATCH_ASSETS {
            return Err(invalid(format!(
                "{} assets; a batch holds at most {MAX_BATCH_ASSETS}",
                assets.len()
            )));
        }
        let mut seen: HashSet<(i64, &str)> = HashSet::new();
        let mut captions: HashSet<i64> = HashSet::new();
        let mut wanted = Vec::with_capacity(batch.items.len());
        for it in &batch.items {
            if let Some(p) = source_problem(&it.source) {
                return Err(invalid(p));
            }
            if !seen.insert((it.asset_id, it.source.as_str())) {
                return Err(invalid(format!(
                    "asset {} appears twice under {}",
                    it.asset_id, it.source
                )));
            }
            if it.mode == TagMode::RejectAllSources && it.source != "manual" {
                return Err(invalid("only a person rejects a tag across every source"));
            }
            if let Some(d) = &it.description {
                let text = d.trim();
                if text.is_empty() {
                    return Err(invalid("an empty caption is not a caption"));
                }
                if text.chars().count() > MAX_DESCRIPTION_CHARS {
                    return Err(invalid(format!(
                        "caption longer than {MAX_DESCRIPTION_CHARS} characters"
                    )));
                }
                if !captions.insert(it.asset_id) {
                    return Err(invalid(format!(
                        "asset {} is given two captions in one batch",
                        it.asset_id
                    )));
                }
            }
            wanted.push(normalize_item(&it.source, &it.tags).map_err(invalid)?);
        }

        let tx = self
            .conn
            .transaction()
            .map_err(|e| CoreError::Catalog(format!("tag batch begin: {e}")))?;
        // Existence is checked inside the transaction, so nothing can delete an asset between the
        // check and the write. One missing id refuses the whole batch.
        for id in &assets {
            let exists: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM asset WHERE id = ?1)",
                    params![id],
                    |r| r.get(0),
                )
                .map_err(|e| CoreError::Catalog(format!("asset lookup: {e}")))?;
            if !exists {
                return Err(invalid(format!("asset {id} does not exist")));
            }
        }

        // Revisions are checked inside the transaction too: a revision read outside it could
        // change between the check and the write.
        for it in &batch.items {
            if let Some(expected) = &it.expected_revision {
                let current = annotation_revision_in(&tx, it.asset_id)?;
                if &current != expected {
                    return Err(CoreError::Catalog(format!(
                        "revision_conflict: asset {} changed since revision {expected}",
                        it.asset_id
                    )));
                }
            }
        }

        let now = unix_now();
        let mut outcome = TagBatchOutcome::default();
        let mut changes: Vec<AssetChange> = Vec::new();
        for &asset_id in &assets {
            let items: Vec<(usize, &TagBatchItem)> = batch
                .items
                .iter()
                .enumerate()
                .filter(|(_, it)| it.asset_id == asset_id)
                .collect();
            let mut sources: Vec<String> = Vec::new();
            for (idx, it) in &items {
                if it.mode == TagMode::RejectAllSources {
                    for w in &wanted[*idx] {
                        for s in sources_holding(&tx, asset_id, &w.norm)? {
                            if !sources.contains(&s) {
                                sources.push(s);
                            }
                        }
                    }
                } else if !sources.contains(&it.source) {
                    sources.push(it.source.clone());
                }
            }
            sources.sort();
            let before = rows_for(&tx, asset_id, &sources)?;
            let before_caption = description_of(&tx, asset_id)?;

            let mut target = before.clone();
            for (idx, it) in &items {
                let ws = &wanted[*idx];
                match it.mode {
                    TagMode::Append => {
                        for w in ws {
                            let id = tag_id_for(&tx, w, true)?.expect("created");
                            if !target
                                .iter()
                                .any(|r| r.source == it.source && r.tag_id == id)
                            {
                                target.push(Row {
                                    source: it.source.clone(),
                                    tag_id: id,
                                    confidence: w.confidence,
                                    model_id: it.model_id.clone(),
                                    created_at: now,
                                });
                            }
                        }
                    }
                    TagMode::ReplaceSource => {
                        let mut next = Vec::with_capacity(ws.len());
                        for w in ws {
                            let id = tag_id_for(&tx, w, true)?.expect("created");
                            let created_at = target
                                .iter()
                                .find(|r| r.source == it.source && r.tag_id == id)
                                .map_or(now, |r| r.created_at);
                            next.push(Row {
                                source: it.source.clone(),
                                tag_id: id,
                                confidence: w.confidence,
                                model_id: it.model_id.clone(),
                                created_at,
                            });
                        }
                        target.retain(|r| r.source != it.source);
                        target.extend(next);
                    }
                    TagMode::RemoveScoped => {
                        for w in ws {
                            if let Some(id) = tag_id_for(&tx, w, false)? {
                                target.retain(|r| !(r.source == it.source && r.tag_id == id));
                            }
                        }
                    }
                    TagMode::RejectAllSources => {
                        for w in ws {
                            if let Some(id) = tag_id_for(&tx, w, false)? {
                                target.retain(|r| r.tag_id != id);
                            }
                        }
                    }
                }
            }
            target.sort_by(|a, b| a.key().cmp(&b.key()));

            let new_caption = items
                .iter()
                .find_map(|(_, it)| it.description.as_deref().map(str::trim));
            let caption_changes = new_caption.is_some_and(|c| Some(c) != before_caption.as_deref());
            let rows_change = !same_rows(&before, &target);
            if !rows_change && !caption_changes {
                outcome.unchanged_assets.push(asset_id);
                continue;
            }
            write_rows(&tx, asset_id, &before, &target)?;
            finish_asset(&tx, asset_id, caption_changes.then_some(new_caption))?;
            changes.push(AssetChange {
                asset_id,
                sources,
                caption: caption_changes,
                before: Side {
                    rows: before,
                    description: before_caption,
                },
                after: Side {
                    rows: target,
                    description: new_caption.map(str::to_string),
                },
            });
            outcome.changed_assets.push(asset_id);
        }

        if batch.completes_queue {
            for id in &assets {
                tx.execute(
                    "UPDATE asset SET tags_dirty = 0 WHERE id = ?1 AND tags_dirty = 1",
                    params![id],
                )
                .map_err(|e| CoreError::Catalog(format!("tag queue clear: {e}")))?;
            }
        }
        if let (Some(origin), false) = (batch.history, changes.is_empty()) {
            let payload = serde_json::to_string(&BatchPayload {
                v: PAYLOAD_VERSION,
                changes,
                reviewer: None,
                reviews: Vec::new(),
            })?;
            outcome.op_id = Some(record_op_in(
                &tx,
                "tag_batch",
                &payload,
                None,
                origin.as_str(),
            )?);
        }
        if let Some(key) = &batch.receipt {
            let result = serde_json::json!({
                "operation_id": outcome.op_id,
                "changed_assets": outcome.changed_assets,
                "unchanged_assets": outcome.unchanged_assets,
                "asset_ids": assets,
            });
            tx.execute(
                "INSERT INTO tag_request (request_id, body_digest, op_id, result_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    key.request_id,
                    key.digest,
                    outcome.op_id,
                    result.to_string(),
                    now
                ],
            )
            .map_err(|e| CoreError::Catalog(format!("request receipt: {e}")))?;
        }
        tx.commit()
            .map_err(|e| CoreError::Catalog(format!("tag batch commit: {e}")))?;
        Ok(outcome)
    }

    /// Review existing AI associations atomically. The target row and every expected revision are
    /// checked in the writer transaction; acceptance is recorded even when it changes no rows.
    pub fn apply_tag_review(&mut self, batch: &TagReviewBatch) -> Result<TagBatchOutcome> {
        if batch.reviewer.trim().is_empty() {
            return Err(invalid("a review needs a reviewer"));
        }
        let mut asset_ids: Vec<i64> = batch.decisions.iter().map(|d| d.asset_id).collect();
        asset_ids.sort_unstable();
        asset_ids.dedup();
        if batch.decisions.is_empty() || asset_ids.len() > MAX_BATCH_ASSETS {
            return Err(invalid(format!(
                "{} assets; a review holds 1 to {MAX_BATCH_ASSETS}",
                asset_ids.len()
            )));
        }
        let mut seen = HashSet::new();
        let mut expected = std::collections::HashMap::new();
        let mut norms = Vec::with_capacity(batch.decisions.len());
        for decision in &batch.decisions {
            if !is_ai_source(&decision.source) {
                return Err(invalid(
                    "protected_assignment: only AI associations are reviewable",
                ));
            }
            if decision.expected_revision.trim().is_empty() {
                return Err(invalid("expected_revision is required"));
            }
            if decision.reason.trim().is_empty()
                || decision.reason.chars().count() > MAX_DESCRIPTION_CHARS
            {
                return Err(invalid(format!(
                    "a review reason is 1 to {MAX_DESCRIPTION_CHARS} characters"
                )));
            }
            let tag = TagAssignment {
                namespace: decision.namespace.clone(),
                value: decision.value.clone(),
                confidence: None,
            };
            if let Some(problem) = assignment_problem("ai:review", &tag) {
                return Err(invalid(problem));
            }
            let namespace = namespace_of(&decision.namespace);
            let (norm, _) = normalize_tag(&namespace, &decision.value).expect("checked above");
            if !seen.insert((decision.asset_id, decision.source.as_str(), norm.clone())) {
                return Err(invalid(format!(
                    "asset {} reviews {}:{} from {} twice",
                    decision.asset_id, decision.namespace, decision.value, decision.source
                )));
            }
            match expected.insert(decision.asset_id, decision.expected_revision.as_str()) {
                Some(previous) if previous != decision.expected_revision => {
                    return Err(invalid(format!(
                        "asset {} names two expected revisions",
                        decision.asset_id
                    )))
                }
                _ => {}
            }
            norms.push(norm);
        }

        let tx = self
            .conn
            .transaction()
            .map_err(|e| CoreError::Catalog(format!("tag review begin: {e}")))?;
        for asset_id in &asset_ids {
            let exists: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM asset WHERE id = ?1)",
                    params![asset_id],
                    |r| r.get(0),
                )
                .map_err(|e| CoreError::Catalog(format!("review asset lookup: {e}")))?;
            if !exists {
                return Err(invalid(format!("asset {asset_id} does not exist")));
            }
            let current = annotation_revision_in(&tx, *asset_id)?;
            if current != expected[asset_id] {
                return Err(CoreError::Catalog(format!(
                    "revision_conflict: asset {asset_id} changed since review"
                )));
            }
        }
        let mut target_ids = Vec::with_capacity(batch.decisions.len());
        for (decision, norm) in batch.decisions.iter().zip(&norms) {
            let tag_id: Option<i64> = tx
                .query_row(
                    "SELECT at.tag_id FROM asset_tag at JOIN tag t ON t.id = at.tag_id
                      WHERE at.asset_id = ?1 AND at.source = ?2 AND t.norm_name = ?3",
                    params![decision.asset_id, decision.source, norm],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| CoreError::Catalog(format!("review target lookup: {e}")))?;
            target_ids.push(tag_id.ok_or_else(|| {
                invalid(format!(
                    "review_target_missing: asset {} has no {}:{} from {}",
                    decision.asset_id, decision.namespace, decision.value, decision.source
                ))
            })?);
        }

        let mut outcome = TagBatchOutcome::default();
        let mut changes = Vec::new();
        for asset_id in &asset_ids {
            let indices: Vec<usize> = batch
                .decisions
                .iter()
                .enumerate()
                .filter_map(|(index, decision)| (decision.asset_id == *asset_id).then_some(index))
                .collect();
            let mut sources = indices
                .iter()
                .map(|&index| batch.decisions[index].source.clone())
                .collect::<Vec<_>>();
            sources.sort();
            sources.dedup();
            let before = rows_for(&tx, *asset_id, &sources)?;
            let mut target = before.clone();
            for index in &indices {
                if batch.decisions[*index].action == TagReviewAction::Reject {
                    let source = &batch.decisions[*index].source;
                    let tag_id = target_ids[*index];
                    target.retain(|row| !(row.source == *source && row.tag_id == tag_id));
                }
            }
            if same_rows(&before, &target) {
                outcome.unchanged_assets.push(*asset_id);
                continue;
            }
            write_rows(&tx, *asset_id, &before, &target)?;
            finish_asset(&tx, *asset_id, None)?;
            changes.push(AssetChange {
                asset_id: *asset_id,
                sources,
                caption: false,
                before: Side {
                    rows: before,
                    description: None,
                },
                after: Side {
                    rows: target,
                    description: None,
                },
            });
            outcome.changed_assets.push(*asset_id);
        }
        let payload = serde_json::to_string(&BatchPayload {
            v: PAYLOAD_VERSION,
            changes,
            reviewer: Some(batch.reviewer.clone()),
            reviews: batch.decisions.clone(),
        })?;
        outcome.op_id = Some(record_op_in(
            &tx,
            "tag_batch",
            &payload,
            None,
            HistoryOrigin::Agent.as_str(),
        )?);
        if let Some(key) = &batch.receipt {
            let result = serde_json::json!({
                "operation_id": outcome.op_id,
                "changed_assets": outcome.changed_assets,
                "unchanged_assets": outcome.unchanged_assets,
                "asset_ids": asset_ids,
                "reviewed_decisions": batch.decisions.len(),
            });
            tx.execute(
                "INSERT INTO tag_request (request_id, body_digest, op_id, result_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    key.request_id,
                    key.digest,
                    outcome.op_id,
                    result.to_string(),
                    unix_now()
                ],
            )
            .map_err(|e| CoreError::Catalog(format!("review receipt: {e}")))?;
        }
        tx.commit()
            .map_err(|e| CoreError::Catalog(format!("tag review commit: {e}")))?;
        Ok(outcome)
    }

    /// The opaque revision of one asset's annotations — every association (source, tag, score,
    /// model) and the caption. Reads report it; a batch item that names it is refused if anything
    /// moved since. A digest, so it is stable across processes and builds.
    pub fn annotation_revision(&self, asset_id: i64) -> Result<String> {
        annotation_revision_in(&self.conn, asset_id)
    }

    /// The receipt a committed request left, if any.
    pub fn tag_request_receipt(&self, request_id: &str) -> Result<Option<TagReceipt>> {
        let row: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT body_digest, result_json FROM tag_request WHERE request_id = ?1",
                params![request_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| CoreError::Catalog(format!("receipt read: {e}")))?;
        row.map(|(digest, result)| {
            Ok(TagReceipt {
                digest,
                result: serde_json::from_str(&result)?,
            })
        })
        .transpose()
    }

    /// Undo a `tag_batch` op: put back the recorded `before` state of every association it
    /// touched — only where they still look the way it left them — then mark it undone.
    pub(super) fn revert_tag_batch(&mut self, op_id: i64, payload: &str) -> Result<()> {
        self.replay_tag_batch(op_id, payload, true)
    }

    /// Redo an undone `tag_batch` op, under the same conflict rule, then consume it (ADR-0006).
    pub(super) fn redo_tag_batch(&mut self, op_id: i64, payload: &str) -> Result<()> {
        self.replay_tag_batch(op_id, payload, false)
    }

    fn replay_tag_batch(&mut self, op_id: i64, payload: &str, undo: bool) -> Result<()> {
        let p: BatchPayload = serde_json::from_str(payload)?;
        if p.v != PAYLOAD_VERSION {
            return Err(CoreError::Catalog(format!(
                "tag_batch op {op_id} has payload version {}, not {PAYLOAD_VERSION}",
                p.v
            )));
        }
        let verb = if undo { "undone" } else { "redone" };
        let tx = self
            .conn
            .transaction()
            .map_err(|e| CoreError::Catalog(format!("tag replay begin: {e}")))?;
        for ch in &p.changes {
            let (from, to) = if undo {
                (&ch.after, &ch.before)
            } else {
                (&ch.before, &ch.after)
            };
            let now_rows = rows_for(&tx, ch.asset_id, &ch.sources)?;
            let caption_moved = ch.caption && description_of(&tx, ch.asset_id)? != from.description;
            if !same_rows(&now_rows, &from.rows) || caption_moved {
                return Err(CoreError::Catalog(format!(
                    "conflict: asset {} has changed since this edit, so it was not {verb} — \
                     another writer's newer tags are left as they are",
                    ch.asset_id
                )));
            }
            write_rows(&tx, ch.asset_id, &now_rows, &to.rows)?;
            finish_asset(
                &tx,
                ch.asset_id,
                ch.caption.then_some(to.description.as_deref()),
            )?;
        }
        if undo {
            tx.execute(
                "UPDATE op_log SET undone_at = ?2 WHERE id = ?1",
                params![op_id, unix_now()],
            )
        } else {
            tx.execute("DELETE FROM op_log WHERE id = ?1", params![op_id])
        }
        .map_err(|e| CoreError::Catalog(format!("tag replay log: {e}")))?;
        tx.commit()
            .map_err(|e| CoreError::Catalog(format!("tag replay commit: {e}")))?;
        Ok(())
    }
}

/// One association with its full provenance — what read APIs report and the checks assert
/// (v1 plan S04; S05's readback builds on it).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct AssetTagRow {
    pub source: String,
    pub namespace: String,
    pub value: String,
    pub confidence: Option<f64>,
    pub model_id: Option<String>,
}

impl Catalog {
    /// Every association on one asset with who wrote it, ordered by source, then label.
    pub fn asset_tag_rows(&self, asset_id: i64) -> Result<Vec<AssetTagRow>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT at.source, COALESCE(t.namespace, 'free'), t.name, at.confidence, at.model_id
                   FROM asset_tag at JOIN tag t ON t.id = at.tag_id
                  WHERE at.asset_id = ?1 ORDER BY at.source, t.name",
            )
            .map_err(|e| CoreError::Catalog(format!("tag rows prep: {e}")))?;
        let rows = stmt
            .query_map(params![asset_id], |r| {
                Ok(AssetTagRow {
                    source: r.get(0)?,
                    namespace: r.get(1)?,
                    value: r.get(2)?,
                    confidence: r.get(3)?,
                    model_id: r.get(4)?,
                })
            })
            .map_err(|e| CoreError::Catalog(format!("tag rows: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("tag row: {e}")))
    }
}
