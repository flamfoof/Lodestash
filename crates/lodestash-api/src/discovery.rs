//! Discovery and selection for the agent planes (ENGINE-API §8.1, v1 plan S12).
//!
//! Every operation here is written once and called by both planes: REST parses its query string or
//! body into these requests, MCP parses its tool arguments into the same ones, and both render the
//! same [`AgentError`] when a request is refused. Validation is strict on purpose — a misspelled
//! media type, a negative id or a limit of 5,000 is an `invalid_argument`, never a silently widened
//! or silently empty result an agent would then report as "the library has none".

use lodestash_core::catalog::{AssetScope, AssetSummaryRow, Catalog, TagState};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::ServerState;

/// Default and maximum page sizes for walks and library listings.
pub const DEFAULT_PAGE: usize = 25;
pub const MAX_PAGE: usize = 200;
/// Most ids one `get_assets` call may name.
pub const MAX_READ: usize = 100;
/// Archive pulls and renders allowed at once (ENGINE-API §8.2), and how long a caller waits for one.
pub const HEAVY_SLOTS: usize = 2;
pub const HEAVY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);
/// At most one 4 MiB PNG is presently delivered; multi-view evidence shares this same aggregate cap.
pub const MAX_EVIDENCE_IMAGE_BYTES: u64 = 4 * 1024 * 1024;

const STATUSES: &[&str] = &["discovered", "indexed", "identified", "confirmed"];

/// A refusal with a stable code and HTTP status (ENGINE-API §8.4), rendered identically by both
/// planes: REST as the error envelope, MCP as an `isError` tool result carrying the same object.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
}

impl AgentError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self {
            status: 400,
            code: "invalid_argument",
            message: message.into(),
        }
    }

    pub fn cursor(message: impl Into<String>) -> Self {
        Self {
            status: 400,
            code: "invalid_cursor",
            message: message.into(),
        }
    }

    fn with(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    /// Asset bytes are not released to agents (ADR-0020).
    pub fn bytes_not_released() -> Self {
        Self::with(
            403,
            "bytes_not_released",
            "this agent may read metadata and links; serving asset bytes needs `mcp_bytes_enabled` \
             in Settings",
        )
    }

    pub fn asset_not_found(id: i64) -> Self {
        Self::with(404, "asset_not_found", format!("no live asset has id {id}"))
    }

    pub fn not_local(message: impl Into<String>) -> Self {
        Self::with(503, "not_local", message)
    }

    pub fn archive_missing(message: impl Into<String>) -> Self {
        Self::with(503, "archive_missing", message)
    }

    pub fn source_changed(message: impl Into<String>) -> Self {
        Self::with(409, "source_changed", message)
    }

    pub fn asset_too_large(message: impl Into<String>) -> Self {
        Self::with(413, "asset_too_large", message)
    }

    pub fn busy() -> Self {
        Self::with(
            429,
            "busy",
            format!("{HEAVY_SLOTS} archive pulls or renders are already running — retry shortly"),
        )
    }

    pub fn deadline_exceeded() -> Self {
        Self::with(
            504,
            "deadline_exceeded",
            "the work did not finish in time; it continues, and a retry may find it done",
        )
    }

    pub fn resource_too_large(message: impl Into<String>) -> Self {
        Self::with(413, "resource_too_large", message)
    }

    fn internal(e: lodestash_core::error::CoreError) -> Self {
        Self {
            status: 500,
            code: "internal",
            message: e.to_string(),
        }
    }

    /// `{"error": {code, message, status}}` — the MCP tool-error body.
    pub fn to_json(&self) -> Value {
        json!({ "error": { "code": self.code, "message": self.message, "status": self.status } })
    }
}

impl From<lodestash_core::error::CoreError> for AgentError {
    fn from(e: lodestash_core::error::CoreError) -> Self {
        Self::internal(e)
    }
}

/// `list_libraries`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LibrariesRequest {
    pub limit: Option<usize>,
    pub cursor: Option<String>,
}

/// `list_assets`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ListAssetsRequest {
    pub source_id: Option<i64>,
    pub package_id: Option<i64>,
    pub media_type: Option<String>,
    pub status: Option<String>,
    pub tag_state: Option<String>,
    pub selection: Option<String>,
    pub limit: Option<usize>,
    pub cursor: Option<String>,
}

/// `get_assets`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadAssetsRequest {
    pub asset_ids: Vec<i64>,
    pub source_id: Option<i64>,
}

/// `get_asset_evidence`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct EvidenceRequest {
    pub kind: Option<String>,
    pub max_dimension: Option<usize>,
    pub views: Option<usize>,
}

/// `get_vocabulary`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct VocabularyRequest {
    pub media_type: Option<String>,
    pub taxonomy_id: Option<i64>,
    pub prefix: Option<String>,
    pub limit: Option<usize>,
    pub cursor: Option<String>,
}

/// The optional scope `search_assets` and `semantic_search` accept on top of their query.
#[derive(Debug, Clone, Default)]
pub struct SearchScope {
    pub source_id: Option<i64>,
    pub package_id: Option<i64>,
    pub media_type: Option<String>,
    /// `namespace:value`, AND-ed.
    pub tags: Vec<String>,
}

fn positive(name: &str, id: Option<i64>) -> Result<Option<i64>, AgentError> {
    match id {
        Some(v) if v <= 0 => Err(AgentError::invalid(format!(
            "{name} must be a positive id, not {v}"
        ))),
        other => Ok(other),
    }
}

fn page_limit(limit: Option<usize>) -> Result<usize, AgentError> {
    match limit {
        None => Ok(DEFAULT_PAGE),
        Some(n) if (1..=MAX_PAGE).contains(&n) => Ok(n),
        Some(n) => Err(AgentError::invalid(format!(
            "limit must be between 1 and {MAX_PAGE}, not {n}"
        ))),
    }
}

fn digest16(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex()[..16].to_string()
}

/// Validate every scope field against the catalog and build the scope.
fn build_scope(
    cat: &Catalog,
    source_id: Option<i64>,
    package_id: Option<i64>,
    media_type: Option<&str>,
    status: Option<&str>,
    tag_state: Option<&str>,
    tags: &[String],
) -> Result<AssetScope, AgentError> {
    let source_id = positive("source_id", source_id)?;
    let package_id = positive("package_id", package_id)?;
    if let Some(source) = source_id {
        if !cat.source_exists(source)? {
            return Err(AgentError::invalid(format!("no library has id {source}")));
        }
    }
    if let Some(package) = package_id {
        match (cat.package_source_id(package)?, source_id) {
            (None, _) => {
                return Err(AgentError::invalid(format!("no package has id {package}")));
            }
            (Some(owner), Some(source)) if owner != source => {
                return Err(AgentError::invalid(format!(
                    "package {package} is not in library {source}"
                )));
            }
            _ => {}
        }
    }
    if let Some(code) = media_type {
        if cat.media_type_id(code)?.is_none() {
            return Err(AgentError::invalid(format!(
                "unknown media_type {code:?} — one of model_3d, texture, audio, image, sprite, \
                 material, font"
            )));
        }
    }
    if let Some(s) = status {
        if !STATUSES.contains(&s) {
            return Err(AgentError::invalid(format!(
                "unknown status {s:?} — one of {}",
                STATUSES.join(", ")
            )));
        }
    }
    let tag_state = match tag_state {
        None => TagState::Any,
        Some(s) => TagState::parse(s).ok_or_else(|| {
            AgentError::invalid(format!("unknown tag_state {s:?} — any, tagged or untagged"))
        })?,
    };
    let mut pairs = Vec::with_capacity(tags.len());
    for tag in tags {
        let (namespace, value) = tag
            .split_once(':')
            .ok_or_else(|| AgentError::invalid(format!("tag {tag:?} is not namespace:value")))?;
        if lodestash_core::catalog::normalize_tag(namespace, value).is_none()
            || namespace.trim().is_empty()
        {
            return Err(AgentError::invalid(format!(
                "tag {tag:?} is not namespace:value"
            )));
        }
        pairs.push((namespace.to_string(), value.to_string()));
    }
    Ok(AssetScope {
        source_id,
        package_id,
        media_type: media_type.map(str::to_string),
        status: status.map(str::to_string),
        tag_state,
        tags: pairs,
    })
}

impl ServerState {
    /// The scope a search request names, validated (ENGINE-API §8.1).
    pub fn search_scope(&self, s: &SearchScope) -> Result<AssetScope, AgentError> {
        let cat = self.catalog.lock().unwrap();
        build_scope(
            &cat,
            s.source_id,
            s.package_id,
            s.media_type.as_deref(),
            None,
            None,
            &s.tags,
        )
    }

    /// An asset summary as both planes return it: the ENGINE-API §3 shape plus the §8.1 additions
    /// — which library and pack it is in, a resource reference for the file, and whether the file
    /// can actually be had right now.
    pub fn summary(&self, r: &AssetSummaryRow) -> Value {
        let mut v = crate::routes::summary_json(r);
        v["source_id"] = json!(r.source_id);
        v["package_id"] = json!(r.package_id);
        v["file_resource_uri"] = json!(format!("asset://{}/file", r.id));
        v["file_availability"] = json!(self.file_availability(r));
        v
    }

    /// `available`, `in_pack`, `missing` or `denied` (ENGINE-API §8.1). Policy first: with bytes
    /// not released the file is `denied` wherever it is, which is the answer that decides what the
    /// agent can do next.
    fn file_availability(&self, r: &AssetSummaryRow) -> &'static str {
        if !self.byte_release() {
            return "denied";
        }
        if r.package_id.is_some() && r.workspace_rel.is_none() {
            return "in_pack";
        }
        match self.source_path(r) {
            Some(p) if p.is_file() => "available",
            _ => "missing",
        }
    }

    /// `list_libraries` — libraries in id order, paged.
    pub fn list_libraries(&self, req: &LibrariesRequest) -> Result<Value, AgentError> {
        let limit = page_limit(req.limit)?;
        let cat = self.catalog.lock().unwrap();
        let catalog_id = cat.catalog_id()?;
        let after = match &req.cursor {
            None => 0,
            Some(c) => {
                let parts: Vec<&str> = c.split('.').collect();
                match parts.as_slice() {
                    ["l1", last, cat_id] if *cat_id == catalog_id => last
                        .parse::<i64>()
                        .ok()
                        .filter(|v| *v >= 0)
                        .ok_or_else(|| AgentError::cursor("the cursor is malformed"))?,
                    ["l1", _, _] => {
                        return Err(AgentError::cursor("the cursor belongs to another catalog"))
                    }
                    _ => return Err(AgentError::cursor("the cursor is malformed")),
                }
            }
        };
        let mut rows = cat.libraries_page(after, limit + 1)?;
        let next = (rows.len() > limit).then(|| {
            rows.truncate(limit);
            format!("l1.{}.{catalog_id}", rows.last().map_or(after, |r| r.id))
        });
        Ok(json!({ "libraries": rows, "next_cursor": next }))
    }

    /// `list_assets` — an ordered keyset walk of one library, or a random sample of it.
    ///
    /// The ordered cursor carries its version, the last id returned, the upper id the walk
    /// captured, the catalog's identity and a digest of the filters, so a cursor from another
    /// catalog, another scope or another selection is refused rather than quietly walking something
    /// else.
    pub fn list_assets(&self, req: &ListAssetsRequest) -> Result<Value, AgentError> {
        let source_id = req
            .source_id
            .ok_or_else(|| AgentError::invalid("source_id is required — see list_libraries"))?;
        let limit = page_limit(req.limit)?;
        let random = match req.selection.as_deref() {
            None | Some("ordered") => false,
            Some("random") => true,
            Some(other) => {
                return Err(AgentError::invalid(format!(
                    "unknown selection {other:?} — ordered or random"
                )))
            }
        };
        let cat = self.catalog.lock().unwrap();
        let scope = build_scope(
            &cat,
            Some(source_id),
            req.package_id,
            req.media_type.as_deref(),
            req.status.as_deref(),
            req.tag_state.as_deref(),
            &[],
        )?;
        let catalog_id = cat.catalog_id()?;
        let signature = digest16(&scope.signature());

        if random {
            if req.cursor.is_some() {
                return Err(AgentError::cursor(
                    "a random selection has no pages — each request is a fresh sample",
                ));
            }
            let ids = cat.sample_asset_ids(&scope, limit)?;
            let rows = cat.asset_summaries(&ids)?;
            drop(cat);
            return Ok(json!({
                "assets": rows.iter().map(|r| self.summary(r)).collect::<Vec<_>>(),
                "next_cursor": null,
                "selection": "random",
                "upper_id": null,
            }));
        }

        let (after, upper) = match &req.cursor {
            None => (0, cat.max_asset_id()?),
            Some(c) => {
                let parts: Vec<&str> = c.split('.').collect();
                let ["w1", last, upper, cat_id, sig] = parts.as_slice() else {
                    return Err(AgentError::cursor("the cursor is malformed"));
                };
                if *cat_id != catalog_id {
                    return Err(AgentError::cursor("the cursor belongs to another catalog"));
                }
                if *sig != signature {
                    return Err(AgentError::cursor(
                        "the cursor was issued for different filters — start a new walk",
                    ));
                }
                match (last.parse::<i64>(), upper.parse::<i64>()) {
                    (Ok(l), Ok(u)) if l >= 0 && u >= l => (l, u),
                    _ => return Err(AgentError::cursor("the cursor is malformed")),
                }
            }
        };
        let mut ids = cat.browse_asset_ids(&scope, after, upper, limit + 1)?;
        let next = (ids.len() > limit).then(|| {
            ids.truncate(limit);
            format!(
                "w1.{}.{upper}.{catalog_id}.{signature}",
                ids.last().copied().unwrap_or(after)
            )
        });
        let rows = cat.asset_summaries(&ids)?;
        drop(cat);
        Ok(json!({
            "assets": rows.iter().map(|r| self.summary(r)).collect::<Vec<_>>(),
            "next_cursor": next,
            "selection": "ordered",
            "upper_id": upper,
        }))
    }

    /// Where an asset's bytes can be read right now (ENGINE-API §8.2) — the one resolver behind the
    /// REST file route and MCP `asset://{id}/file` reads, so no delivery path has a policy of its own.
    ///
    /// Byte release is checked first. A file on disk is served where it is; an in-pack asset is
    /// pulled — verified, capped, into a scratch file — by the resolver the app installs, without
    /// unpacking the package or changing the catalog. Anything else is `not_local`.
    pub fn resolve_file(&self, id: i64) -> Result<std::path::PathBuf, AgentError> {
        if !self.byte_release() {
            return Err(AgentError::bytes_not_released());
        }
        let row = self
            .asset(id)?
            .ok_or_else(|| AgentError::asset_not_found(id))?;
        if let Some(path) = self.source_path(&row).filter(|p| p.is_file()) {
            return Ok(path);
        }
        let entry = self.catalog.lock().unwrap().in_pack_entry(id)?;
        let pull = self.payload.lock().unwrap().clone();
        match (entry, pull) {
            (Some(entry), Some(pull)) => self.heavy(move || pull(&entry)),
            (Some(_), None) => Err(AgentError::not_local(
                "the asset is inside a package and this server cannot pull payloads",
            )),
            (None, _) => Err(AgentError::not_local(
                "the asset's file is not on this machine — its drive may be disconnected",
            )),
        }
    }

    /// Install the in-pack payload resolver ([`crate::PayloadFn`]).
    pub fn set_payload_resolver(&self, f: crate::PayloadFn) {
        *self.payload.lock().unwrap() = Some(std::sync::Arc::new(f));
    }

    /// Run archive or render work under the §8.2 bounds: at most [`HEAVY_SLOTS`] at once — a
    /// request beyond that is `busy` immediately rather than queued behind work of unknown length —
    /// and an answer within the deadline, or `deadline_exceeded`.
    ///
    /// A job that overruns is not abandoned: it keeps its slot until it finishes, so the bound is
    /// honest about what is really running, and whatever it publishes (a scratch file, a render) is
    /// there for the retry.
    pub(crate) fn heavy<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> Result<T, AgentError> + Send + 'static,
    ) -> Result<T, AgentError> {
        use std::sync::atomic::Ordering;
        let slots = self.heavy_in_flight.clone();
        if slots
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                (n < HEAVY_SLOTS).then_some(n + 1)
            })
            .is_err()
        {
            return Err(AgentError::busy());
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = work();
            slots.fetch_sub(1, Ordering::SeqCst);
            let _ = tx.send(result);
        });
        let deadline = *self.heavy_deadline.lock().unwrap();
        rx.recv_timeout(deadline)
            .unwrap_or_else(|_| Err(AgentError::deadline_exceeded()))
    }

    #[cfg(test)]
    pub(crate) fn set_heavy_deadline(&self, d: std::time::Duration) {
        *self.heavy_deadline.lock().unwrap() = d;
    }

    /// `get_assets` — full detail for 1–100 named assets, in the order asked, with the ones that
    /// cannot be returned listed separately.
    ///
    /// An asset outside `source_id` is reported as `asset_not_found`, the same as one that does not
    /// exist: a scoped read must not become a way to learn what another library holds.
    pub fn read_assets(&self, req: &ReadAssetsRequest) -> Result<Value, AgentError> {
        if req.asset_ids.is_empty() || req.asset_ids.len() > MAX_READ {
            return Err(AgentError::invalid(format!(
                "asset_ids must name 1 to {MAX_READ} assets, not {}",
                req.asset_ids.len()
            )));
        }
        let mut seen = std::collections::HashSet::new();
        for &id in &req.asset_ids {
            positive("asset_ids", Some(id))?;
            if !seen.insert(id) {
                return Err(AgentError::invalid(format!("asset {id} is named twice")));
            }
        }
        let source_id = positive("source_id", req.source_id)?;
        let mut assets = Vec::new();
        let mut unavailable = Vec::new();
        for &id in &req.asset_ids {
            let in_scope = match (self.asset(id)?, source_id) {
                (Some(row), Some(source)) => row.source_id == source,
                (Some(_), None) => true,
                (None, _) => false,
            };
            match in_scope.then(|| self.asset_detail_json(id)).transpose()? {
                Some(Some(detail)) => assets.push(detail),
                _ => unavailable.push(json!({ "id": id, "code": "asset_not_found" })),
            }
        }
        Ok(json!({ "assets": assets, "unavailable": unavailable }))
    }

    /// `get_vocabulary` — a bounded, revisioned view of applicable canonical traits and one
    /// taxonomy. Raw tags remain on `list_tags`; this is the review vocabulary only.
    pub fn get_vocabulary(&self, req: &VocabularyRequest) -> Result<Value, AgentError> {
        let limit = page_limit(req.limit)?;
        let prefix = req.prefix.as_deref().unwrap_or("").to_lowercase();
        let cat = self.catalog.lock().unwrap();
        if let Some(media_type) = req.media_type.as_deref() {
            if cat.media_type_id(media_type)?.is_none() {
                return Err(AgentError::invalid(format!(
                    "unknown media_type {media_type:?} — one of model_3d, texture, audio, image, sprite, material, font"
                )));
            }
        }
        let taxonomy_id = match positive("taxonomy_id", req.taxonomy_id)? {
            Some(id) => {
                if cat.taxonomy_meta(id)?.is_none() {
                    return Err(AgentError::invalid(format!("no taxonomy has id {id}")));
                }
                Some(id)
            }
            None => cat.active_taxonomy()?,
        };
        let traits = cat
            .list_vocabulary()?
            .into_iter()
            .filter(|trait_row| trait_row.is_trait)
            .filter(|trait_row| {
                req.media_type.as_deref().map_or(true, |media_type| {
                    trait_row
                        .scope
                        .as_deref()
                        .map_or(true, |scope| scope == media_type)
                })
            })
            .filter(|trait_row| {
                prefix.is_empty()
                    || trait_row.namespace.to_lowercase().starts_with(&prefix)
                    || trait_row.value.to_lowercase().starts_with(&prefix)
            })
            .map(|trait_row| {
                json!({
                    "id": trait_row.id,
                    "namespace": trait_row.namespace,
                    "value": trait_row.value,
                    "scope": trait_row.scope,
                    "usage_note": trait_row.usage_note,
                    "teach": trait_row.teach,
                })
            })
            .collect::<Vec<_>>();
        let (categories, aliases, rules) = match taxonomy_id {
            None => (Vec::new(), Vec::new(), Vec::new()),
            Some(taxonomy_id) => {
                let rows = cat.list_categories(taxonomy_id)?;
                let ids_by_path = rows
                    .iter()
                    .map(|(id, _, path)| (path.as_str(), *id))
                    .collect::<std::collections::HashMap<_, _>>();
                let categories = rows
                    .iter()
                    .map(|(id, label, path)| {
                        let parent_id = path
                            .rsplit_once('/')
                            .and_then(|(parent, _)| ids_by_path.get(parent).copied());
                        json!({
                            "id": id,
                            "taxonomy_id": taxonomy_id,
                            "parent_id": parent_id,
                            "path": path,
                            "label": label,
                        })
                    })
                    .collect::<Vec<_>>();
                let mut aliases = Vec::new();
                for (category_id, _, _) in &rows {
                    for alias in cat.list_category_aliases(*category_id)? {
                        aliases.push(json!({
                            "id": alias.id,
                            "taxonomy_id": taxonomy_id,
                            "category_id": category_id,
                            "alias": alias.alias,
                        }));
                    }
                }
                let rules = cat
                    .list_rules(taxonomy_id)?
                    .into_iter()
                    .map(
                        |(
                            id,
                            category_id,
                            category_path,
                            match_type,
                            pattern,
                            priority,
                            enabled,
                        )| {
                            json!({
                                "id": id,
                                "taxonomy_id": taxonomy_id,
                                "category_id": category_id,
                                "category_path": category_path,
                                "match_type": match_type,
                                "pattern": pattern,
                                "priority": priority,
                                "enabled": enabled,
                            })
                        },
                    )
                    .collect::<Vec<_>>();
                (categories, aliases, rules)
            }
        };
        let policy = self.vocabulary_policy();
        let revision = cat.vocabulary_revision(&policy)?;
        drop(cat);
        let signature = digest16(&format!("{:?}|{:?}|{prefix}", req.media_type, taxonomy_id));
        let mut entries = Vec::new();
        entries.extend(
            traits
                .into_iter()
                .map(|row| (0_u8, row["id"].as_i64().unwrap(), "traits", row)),
        );
        entries.extend(
            categories
                .into_iter()
                .map(|row| (1_u8, row["id"].as_i64().unwrap(), "categories", row)),
        );
        entries.extend(
            aliases
                .into_iter()
                .map(|row| (2_u8, row["id"].as_i64().unwrap(), "aliases", row)),
        );
        entries.extend(
            rules
                .into_iter()
                .map(|row| (3_u8, row["id"].as_i64().unwrap(), "rules", row)),
        );
        let start = match req.cursor.as_deref() {
            None => 0,
            Some(cursor) => {
                let parts = cursor.split('.').collect::<Vec<_>>();
                let ["v1", kind, id, cursor_revision, cursor_signature] = parts.as_slice() else {
                    return Err(AgentError::cursor("the cursor is malformed"));
                };
                if *cursor_revision != revision || *cursor_signature != signature {
                    return Err(AgentError::cursor(
                        "the vocabulary changed or the cursor has different filters",
                    ));
                }
                let key = (kind.parse::<u8>().ok(), id.parse::<i64>().ok());
                entries
                    .iter()
                    .position(|(entry_kind, entry_id, _, _)| {
                        Some(*entry_kind) == key.0 && Some(*entry_id) == key.1
                    })
                    .map(|index| index + 1)
                    .ok_or_else(|| {
                        AgentError::cursor("the cursor no longer names a vocabulary entry")
                    })?
            }
        };
        let end = (start + limit).min(entries.len());
        let mut page_traits = Vec::new();
        let mut page_categories = Vec::new();
        let mut page_aliases = Vec::new();
        let mut page_rules = Vec::new();
        for (_, _, collection, row) in &entries[start..end] {
            match *collection {
                "traits" => page_traits.push(row.clone()),
                "categories" => page_categories.push(row.clone()),
                "aliases" => page_aliases.push(row.clone()),
                "rules" => page_rules.push(row.clone()),
                _ => unreachable!(),
            }
        }
        let next_cursor = (end > 0 && end < entries.len()).then(|| {
            let (kind, id, _, _) = &entries[end - 1];
            format!("v1.{kind}.{id}.{revision}.{signature}")
        });
        Ok(json!({
            "revision": revision,
            "policy": policy,
            "traits": page_traits,
            "categories": page_categories,
            "aliases": page_aliases,
            "rules": page_rules,
            "next_cursor": next_cursor,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lodestash_core::catalog::CrawlRecord;

    fn fixture(images: usize, models: usize) -> (tempfile::TempDir, ServerState, i64, i64) {
        fixture_with_preview(images, models, Box::new(|_| None))
    }

    fn fixture_with_preview(
        images: usize,
        models: usize,
        preview: crate::PreviewFn,
    ) -> (tempfile::TempDir, ServerState, i64, i64) {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("catalog.db");
        let (src, other) = {
            let mut c = Catalog::open(&db).unwrap();
            c.seed_media_types().unwrap();
            let src = c.upsert_import_source("Lib", "C:/lib", "folder").unwrap();
            let rec = |name: String, media: &str| CrawlRecord {
                workspace_rel: None,
                content_hash: None,
                rel_path: name.clone(),
                name,
                media_type: media.into(),
                size_bytes: 1,
                mtime: 1,
            };
            let mut records: Vec<CrawlRecord> = (0..images)
                .map(|i| rec(format!("wall {i:04}.png"), "image"))
                .collect();
            records.extend((0..models).map(|i| rec(format!("wall pillar {i:02}.fbx"), "model_3d")));
            c.record_crawl(src, None, &records).unwrap();
            let other = c
                .upsert_import_source("Other", "C:/other", "folder")
                .unwrap();
            c.record_crawl(other, None, &[rec("arch.fbx".into(), "model_3d")])
                .unwrap();
            (src, other)
        };
        let state = ServerState::new(&db, dir.path().to_path_buf(), preview).unwrap();
        (dir, state, src, other)
    }

    fn ids(page: &Value) -> Vec<i64> {
        page["assets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["id"].as_i64().unwrap())
            .collect()
    }

    #[test]
    fn vocabulary_returns_applicable_traits_and_a_revision() {
        let (dir, state, _, _) = fixture(0, 0);
        Catalog::open(&dir.path().join("catalog.db"))
            .unwrap()
            .upsert_trait(
                "style",
                "low poly",
                Some("model_3d"),
                Some("Use for stylized meshes"),
                false,
            )
            .unwrap();

        let vocabulary = state
            .get_vocabulary(&VocabularyRequest {
                media_type: Some("model_3d".into()),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(vocabulary["policy"], "extend_with_approval");
        assert!(vocabulary["revision"]
            .as_str()
            .is_some_and(|revision| revision.starts_with("v2-")));
        assert_eq!(vocabulary["traits"][0]["namespace"], "style");
        assert_eq!(vocabulary["traits"][0]["value"], "low poly");
        assert_eq!(vocabulary["traits"][0]["scope"], "model_3d");
    }

    #[test]
    fn evidence_reports_denied_without_byte_release() {
        let (_dir, state, src, _) = fixture(0, 1);
        let asset_id = ids(&state
            .list_assets(&ListAssetsRequest {
                source_id: Some(src),
                media_type: Some("model_3d".into()),
                ..Default::default()
            })
            .unwrap())[0];

        let evidence = state
            .get_asset_evidence(asset_id, &EvidenceRequest::default())
            .unwrap();

        assert_eq!(evidence["asset_id"], asset_id);
        assert_eq!(evidence["status"], "denied");
        assert!(evidence["representations"].as_array().unwrap().is_empty());
        assert_eq!(
            evidence["file_resource_uri"],
            format!("asset://{asset_id}/file")
        );
        assert!(evidence["analysis_revision"]
            .as_str()
            .is_some_and(|revision| revision.starts_with("a1-")));
    }

    #[test]
    fn evidence_advertises_a_cached_model_preview_after_byte_release() {
        let preview_dir = tempfile::tempdir().unwrap();
        let preview = preview_dir.path().join("model.png");
        image::RgbaImage::from_pixel(64, 32, image::Rgba([30, 60, 90, 255]))
            .save(&preview)
            .unwrap();
        let preview_for_renderer = preview.clone();
        let (_dir, state, src, _) =
            fixture_with_preview(0, 1, Box::new(move |_| Some(preview_for_renderer.clone())));
        let asset_id = ids(&state
            .list_assets(&ListAssetsRequest {
                source_id: Some(src),
                media_type: Some("model_3d".into()),
                ..Default::default()
            })
            .unwrap())[0];
        state.set_byte_release(true);

        let evidence = state
            .get_asset_evidence(asset_id, &EvidenceRequest::default())
            .unwrap();

        assert_eq!(evidence["status"], "ready");
        assert_eq!(
            evidence["representations"],
            json!([{
                "kind": "model_views",
                "mime_type": "image/png",
                "url": format!("/v1/assets/{asset_id}/preview?kind=model_views&view=0&max_dimension=512&views=1"),
                "width": 64,
                "height": 32,
            }])
        );
    }

    #[test]
    fn evidence_does_not_advertise_a_preview_over_the_inline_image_cap() {
        let preview_dir = tempfile::tempdir().unwrap();
        let preview = preview_dir.path().join("large.png");
        let mut bytes = vec![0; MAX_EVIDENCE_IMAGE_BYTES as usize + 1];
        bytes[..24].copy_from_slice(&[
            0x89, b'P', b'N', b'G', 13, 10, 26, 10, 0, 0, 0, 13, b'I', b'H', b'D', b'R', 0, 0, 0,
            64, 0, 0, 0, 32,
        ]);
        std::fs::write(&preview, bytes).unwrap();
        let preview_for_renderer = preview.clone();
        let (_dir, state, src, _) =
            fixture_with_preview(0, 1, Box::new(move |_| Some(preview_for_renderer.clone())));
        let asset_id = ids(&state
            .list_assets(&ListAssetsRequest {
                source_id: Some(src),
                media_type: Some("model_3d".into()),
                ..Default::default()
            })
            .unwrap())[0];
        state.set_byte_release(true);

        let evidence = state
            .get_asset_evidence(asset_id, &EvidenceRequest::default())
            .unwrap();

        assert_eq!(evidence["status"], "unavailable");
        assert!(evidence["representations"].as_array().unwrap().is_empty());
        assert_eq!(evidence["limitations"][0]["code"], "evidence_too_large");
    }

    /// A walk through the planes' own request type: pages chain by cursor to every asset once, and
    /// a cursor is refused when it is not this walk's — another scope, another catalog, a random
    /// selection, or garbage.
    #[test]
    fn a_walk_pages_by_cursor_and_refuses_anyone_elses() {
        let (_dir, state, src, _) = fixture(60, 0);
        let mut req = ListAssetsRequest {
            source_id: Some(src),
            limit: Some(25),
            ..Default::default()
        };
        let mut all = Vec::new();
        let mut pages = 0;
        loop {
            let page = state.list_assets(&req).unwrap();
            pages += 1;
            all.extend(ids(&page));
            match page["next_cursor"].as_str() {
                Some(c) => req.cursor = Some(c.to_string()),
                None => break,
            }
        }
        assert_eq!((pages, all.len()), (3, 60));
        let mut unique = all.clone();
        unique.dedup();
        assert_eq!(unique.len(), 60, "an asset came back twice");

        let cursor = state
            .list_assets(&ListAssetsRequest {
                source_id: Some(src),
                limit: Some(25),
                ..Default::default()
            })
            .unwrap()["next_cursor"]
            .as_str()
            .unwrap()
            .to_string();
        let catalog_id = state.catalog_id().unwrap();
        let base = || ListAssetsRequest {
            source_id: Some(src),
            cursor: Some(cursor.clone()),
            ..Default::default()
        };
        assert!(
            state.list_assets(&base()).is_ok(),
            "the cursor continues its own walk"
        );
        let code = |r: ListAssetsRequest| state.list_assets(&r).unwrap_err().code;
        let mut r = base();
        r.media_type = Some("image".into());
        assert_eq!(code(r), "invalid_cursor", "another scope");
        let mut r = base();
        r.selection = Some("random".into());
        assert_eq!(code(r), "invalid_cursor", "a random selection");
        let mut r = base();
        r.cursor = Some("w1.x.y.z.q".into());
        assert_eq!(code(r), "invalid_cursor", "garbage");
        let mut r = base();
        r.cursor = Some(cursor.replace(&catalog_id, "00000000000000000000000000000000"));
        assert_eq!(code(r), "invalid_cursor", "another catalog");
    }

    /// Bad input is refused by name, never widened or emptied: enums, limits, ids, a package from
    /// another library, a missing library.
    #[test]
    fn a_bad_request_is_invalid_not_empty() {
        let (_dir, state, src, other) = fixture(3, 1);
        let package_of_other = {
            let db = state.workspace_dir.join("catalog.db");
            let c = rusqlite::Connection::open(db).unwrap();
            c.execute(
                "INSERT INTO package (uuid, source_id, package_path, name) VALUES ('p', ?1, 'x', 'P')",
                [other],
            )
            .unwrap();
            c.last_insert_rowid()
        };
        let code = |r: ListAssetsRequest| state.list_assets(&r).unwrap_err().code;
        let ok = || ListAssetsRequest {
            source_id: Some(src),
            ..Default::default()
        };
        assert_eq!(
            code(ListAssetsRequest::default()),
            "invalid_argument",
            "no source"
        );
        for bad in [
            ListAssetsRequest {
                media_type: Some("mesh".into()),
                ..ok()
            },
            ListAssetsRequest {
                status: Some("done".into()),
                ..ok()
            },
            ListAssetsRequest {
                tag_state: Some("some".into()),
                ..ok()
            },
            ListAssetsRequest {
                selection: Some("first".into()),
                ..ok()
            },
            ListAssetsRequest {
                limit: Some(0),
                ..ok()
            },
            ListAssetsRequest {
                limit: Some(MAX_PAGE + 1),
                ..ok()
            },
            ListAssetsRequest {
                source_id: Some(-1),
                ..ok()
            },
            ListAssetsRequest {
                source_id: Some(9999),
                ..ok()
            },
            ListAssetsRequest {
                package_id: Some(package_of_other),
                ..ok()
            },
        ] {
            assert_eq!(code(bad.clone()), "invalid_argument", "{bad:?}");
        }
        let models = state
            .list_assets(&ListAssetsRequest {
                media_type: Some("model_3d".into()),
                ..ok()
            })
            .unwrap();
        assert_eq!(
            ids(&models).len(),
            1,
            "the other library's model is not in this one"
        );
        assert_eq!(models["assets"][0]["source_id"], src);
        assert_eq!(models["assets"][0]["file_availability"], "denied");

        let scope = |tags: Vec<&str>| SearchScope {
            tags: tags.into_iter().map(str::to_string).collect(),
            ..Default::default()
        };
        assert_eq!(
            state.search_scope(&scope(vec!["wall"])).unwrap_err().code,
            "invalid_argument"
        );
        assert_eq!(
            state.search_scope(&scope(vec![":wall"])).unwrap_err().code,
            "invalid_argument"
        );
        assert!(state.search_scope(&scope(vec!["subject:wall"])).is_ok());
    }

    /// Both search operations take the scope before they rank: two models behind sixty images that
    /// match the same word are found with a limit of five (v1 plan S12). The media filter used to
    /// run after the LIMIT, which answered this with nothing.
    #[test]
    fn search_ranks_inside_its_scope() {
        let (_dir, state, src, _) = fixture(60, 2);
        let scope = state
            .search_scope(&SearchScope {
                source_id: Some(src),
                media_type: Some("model_3d".into()),
                ..Default::default()
            })
            .unwrap();
        let found = state.search("wall", 5, &scope).unwrap();
        assert_eq!(
            found.len(),
            2,
            "{:?}",
            found.iter().map(|r| &r.name).collect::<Vec<_>>()
        );
        assert!(found.iter().all(|r| r.media_type == "model_3d"));
        state.set_query_embedder(Box::new(|_| vec![("none".to_string(), vec![1.0])]));
        let semantic = state.semantic_search("wall", 5, &scope).unwrap().unwrap();
        assert_eq!(
            semantic.len(),
            2,
            "the hybrid path must scope before ranking too"
        );
    }

    /// Heavy work is bounded (v1 plan S12b): two pulls at once, a third is `busy` straight away,
    /// and a caller waiting past the deadline gets `deadline_exceeded` while the pull runs on — its
    /// slot held until it really ends, and its result there for the retry.
    #[test]
    fn archive_pulls_are_bounded_and_answer_within_a_deadline() {
        use std::sync::{Arc, Barrier};
        let (_dir, state, _, _) = fixture(1, 0);
        let state = Arc::new(state);
        state.set_heavy_deadline(std::time::Duration::from_millis(300));
        let release = Arc::new(Barrier::new(3));
        let (a, b) = (release.clone(), release.clone());
        let jobs: Vec<_> = [a, b]
            .into_iter()
            .map(|gate| {
                let state = state.clone();
                std::thread::spawn(move || {
                    state.heavy(move || {
                        gate.wait();
                        Ok(1)
                    })
                })
            })
            .collect();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while state
            .heavy_in_flight
            .load(std::sync::atomic::Ordering::SeqCst)
            < 2
        {
            assert!(
                std::time::Instant::now() < deadline,
                "the two pulls never started"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(state.heavy(|| Ok(3)).unwrap_err().code, "busy");
        for job in jobs {
            assert_eq!(job.join().unwrap().unwrap_err().code, "deadline_exceeded");
        }
        assert_eq!(
            state
                .heavy_in_flight
                .load(std::sync::atomic::Ordering::SeqCst),
            2,
            "an overrun gave its slot back while still running"
        );
        release.wait();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while state
            .heavy_in_flight
            .load(std::sync::atomic::Ordering::SeqCst)
            > 0
        {
            assert!(
                std::time::Instant::now() < deadline,
                "the pulls never finished"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(state.heavy(|| Ok(4)).unwrap(), 4);
    }

    /// `get_assets` answers in the order asked, lists what it cannot return — including an asset
    /// outside the named library, reported like one that does not exist — and refuses a malformed
    /// id list outright.
    #[test]
    fn a_read_keeps_order_and_names_what_it_cannot_return() {
        let (_dir, state, src, other) = fixture(3, 0);
        let mine = ids(&state
            .list_assets(&ListAssetsRequest {
                source_id: Some(src),
                ..Default::default()
            })
            .unwrap());
        let theirs = ids(&state
            .list_assets(&ListAssetsRequest {
                source_id: Some(other),
                ..Default::default()
            })
            .unwrap())[0];
        let read = state
            .read_assets(&ReadAssetsRequest {
                asset_ids: vec![mine[2], 424242, mine[0], theirs],
                source_id: Some(src),
            })
            .unwrap();
        assert_eq!(ids(&read), vec![mine[2], mine[0]]);
        assert_eq!(
            read["unavailable"],
            json!([{"id": 424242, "code": "asset_not_found"}, {"id": theirs, "code": "asset_not_found"}])
        );
        assert!(read["assets"][0]["analysis_revision"]
            .as_str()
            .is_some_and(|r| r.starts_with("a1-")));
        for bad in [vec![], vec![mine[0], mine[0]], vec![0], (1..=101).collect()] {
            let err = state
                .read_assets(&ReadAssetsRequest {
                    asset_ids: bad.clone(),
                    source_id: None,
                })
                .unwrap_err();
            assert_eq!(err.code, "invalid_argument", "{bad:?}");
        }
    }
}
