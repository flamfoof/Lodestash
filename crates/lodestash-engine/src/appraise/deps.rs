//! The dependency stage: each asset's GUID and declared references, the prefab evidence that
//! pairs meshes with materials, and the naming heuristic for raw exports (DATA-MODEL §10).
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use lodestash_core::catalog::Catalog;

use super::plan::{past, Scope};

/// Harvest Unity's own bookkeeping for the rows marked `deps_dirty` (DATA-MODEL §10) within
/// `scope`: each asset's GUID from its `.meta` sidecar, and — for materials — the texture GUIDs
/// it declares, which become `asset_relationship` edges. Same forward-progress rule as the other
/// stages: an asset with no `.meta` still clears its flag.
///
/// Order matters within a pass but not across them: an edge whose target isn't catalogued yet is
/// stored `missing` and late-bound by `resolve_pending_relationships`, so a texture indexed after
/// its material still connects.
///
/// **The harvest is per asset; the graph is the library's.** Prefab evidence, the naming
/// heuristic and late binding read every source, so they run once the *library's* queue has run
/// dry — not after every timed slice of it, which would walk every source tree once a slice, and
/// not in a scoped pass, which is confined to the assets it was given. A pass that harvests
/// without reconciling raises `graph_due`, and the next library-wide pass that finds the queue
/// empty reconciles and lowers it. That reconcile is not timed: it happens once per harvest, and
/// half a graph is worse than a late one.
pub fn drain_dependencies(
    cat: &mut Catalog,
    scope: &Scope,
    workspace_dir: Option<&std::path::Path>,
    deadline: Option<Instant>,
    graph_due: &AtomicBool,
) {
    const BATCH: usize = 256;
    let mut total = 0usize;
    let mut edges = 0usize;
    let mut drained = false;
    'queue: loop {
        let jobs = match cat.assets_needing_dep_scan_within(BATCH, scope) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!(%e, "dependency queue query failed; retries next scan");
                break;
            }
        };
        if jobs.is_empty() {
            drained = true;
            break;
        }
        let n = jobs.len();
        for job in jobs {
            if past(deadline) {
                break 'queue;
            }
            let path = match (&job.disk_path, &job.workspace_rel) {
                (Some(p), _) => Some(std::path::PathBuf::from(p)),
                (None, Some(rel)) => workspace_dir.map(|w| w.join(rel)),
                _ => None,
            };
            let guid = path.as_deref().and_then(lodestash_ingest::deps::guid_of);
            // Materials are the only catalogued type that declares GUIDs today: meshes embed
            // their materials (`externalObjects` is empty on a stock FBX import), and prefabs
            // aren't a `media_type`.
            // Each texture binding keeps the property it fills (M29): that is what tells a preview
            // the albedo from the normal map declared before it.
            let refs: Vec<(Option<String>, String)> =
                match (job.media_code.as_str(), path.as_deref()) {
                    ("material", Some(p)) => std::fs::read_to_string(p)
                        .map(|yaml| {
                            lodestash_ingest::deps::material_texture_bindings(&yaml)
                                .into_iter()
                                .map(|(property, g)| (Some(property).filter(|p| !p.is_empty()), g))
                                .collect()
                        })
                        .unwrap_or_default(),
                    _ => Vec::new(),
                };
            edges += refs.len();
            if let Err(e) = cat.record_dependencies_with_roles(
                job.asset_id,
                guid.as_deref(),
                "uses_texture",
                &refs,
            ) {
                tracing::warn!(%e, "record_dependencies failed; stopping dependency drain");
                break 'queue;
            }
            total += 1;
        }
        if n < BATCH {
            drained = true;
            break;
        }
    }
    if total > 0 {
        graph_due.store(true, Ordering::Relaxed);
        tracing::info!(scanned = total, edges, "dependency graph harvested");
        // Out of time on the very last row is still a queue run dry.
        drained = drained
            || cat
                .assets_needing_dep_scan_within(1, scope)
                .is_ok_and(|q| q.is_empty());
    }
    if !scope.is_all() || !drained {
        return;
    }
    // M28 left the pre-S06 prefab edges as `legacy` evidence until a walk that completes everywhere
    // replaces it. That reharvest is tried once per process: a source whose drive stays unplugged
    // keeps it pending, and re-walking the library on every idle tick for that would cost more than
    // waiting for the next change.
    static REHARVEST_TRIED: AtomicBool = AtomicBool::new(false);
    let reharvest =
        !REHARVEST_TRIED.load(Ordering::Relaxed) && cat.prefab_reharvest_pending().unwrap_or(false);
    let due = graph_due.load(Ordering::Relaxed);
    if !(due || reharvest) {
        return;
    }
    REHARVEST_TRIED.store(true, Ordering::Relaxed);
    let (scopes, whole_library) = library_prefab_scopes(cat, workspace_dir);
    refresh_prefab_evidence(cat, &scopes, whole_library);
    if due {
        drain_inferred_texture_edges(cat);
    }
    match cat.resolve_pending_relationships() {
        Ok(late) if late > 0 => tracing::info!(late, "dependency edges late-bound"),
        Ok(_) => {}
        Err(e) => tracing::warn!(%e, "resolving pending dependency edges failed"),
    }
    graph_due.store(false, Ordering::Relaxed);
}

/// One tree of `.prefab` files the evidence pass walks — a source root, or the package workspace.
pub struct PrefabScope {
    pub root: std::path::PathBuf,
    /// For the workspace, a directory that does not exist yet means nothing was extracted: proof
    /// of no prefabs. A source root that does not resolve is a drive that is gone: proof of
    /// nothing.
    pub absent_is_empty: bool,
}

/// Every scope the library's prefab evidence comes from, and whether that is all of them — it is
/// not when there is no workspace directory to walk, or the source list could not be read.
pub fn library_prefab_scopes(
    cat: &Catalog,
    workspace_dir: Option<&std::path::Path>,
) -> (Vec<PrefabScope>, bool) {
    let roots = match cat.source_roots() {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(%e, "prefab pass: source roots unavailable");
            return (Vec::new(), false);
        }
    };
    let mut scopes: Vec<PrefabScope> = roots
        .into_iter()
        .map(|root| PrefabScope {
            root: root.into(),
            absent_is_empty: false,
        })
        .collect();
    if let Some(ws) = workspace_dir {
        scopes.push(PrefabScope {
            root: ws.to_path_buf(),
            absent_is_empty: true,
        });
    }
    (scopes, workspace_dir.is_some())
}

/// Re-read the prefab evidence under `scopes`, then re-derive the mesh → material edges from it
/// (DATA-MODEL §10 step 1b, ADR-0018). Prefabs are not assets — not a `media_type` — so they cannot
/// ride the `deps_dirty` queue; they are read as evidence about assets that are catalogued. Returns
/// how many edges changed.
///
/// Only a prefab whose size or mtime moved is parsed. A scope whose walk completed also forgets the
/// prefabs it no longer found; one that did not — a source whose drive is gone, a directory the walk
/// could not open — keeps everything it said before. `whole_library` says `scopes` is every scope
/// there is: only then, and only when every walk completed, is the evidence of scopes no longer
/// walked forgotten (a removed source; M28's `legacy` scope).
pub fn refresh_prefab_evidence(
    cat: &mut Catalog,
    scopes: &[PrefabScope],
    whole_library: bool,
) -> usize {
    use lodestash_core::catalog::{PrefabBindingRow, PrefabFileScan};
    let mut all_complete = true;
    let mut nested = 0usize;
    for scope in scopes {
        let key = scope.root.to_string_lossy().into_owned();
        let (files, complete) = if scope.absent_is_empty && !scope.root.exists() {
            (Vec::new(), true)
        } else {
            lodestash_ingest::deps::prefab_files_under(&scope.root)
        };
        let known = match cat.prefab_revisions(&key) {
            Ok(k) => k,
            Err(e) => {
                tracing::warn!(%e, scope = %key, "prefab revisions unreadable; scope skipped");
                all_complete = false;
                continue;
            }
        };
        all_complete &= complete;
        let mut seen = std::collections::HashSet::new();
        let mut parsed = Vec::new();
        for file in files {
            let path = file.path.to_string_lossy().into_owned();
            let revision = (file.size as i64, file.mtime_ns);
            if known.get(&path) != Some(&revision) {
                // Unreadable right now: what it said before stands, and the next walk retries it.
                if let Ok(bytes) = std::fs::read(&file.path) {
                    let evidence =
                        lodestash_ingest::deps::prefab_evidence(&String::from_utf8_lossy(&bytes));
                    nested += evidence.nested_instances;
                    parsed.push(PrefabFileScan {
                        path: path.clone(),
                        size: revision.0,
                        mtime: revision.1,
                        bindings: evidence
                            .bindings
                            .into_iter()
                            .map(|b| PrefabBindingRow {
                                game_object: b.game_object,
                                renderer: b.renderer,
                                slot: i64::from(b.slot),
                                mesh_guid: b.mesh,
                                material_guid: b.material,
                            })
                            .collect(),
                    });
                }
            }
            seen.insert(path);
        }
        let removed: Vec<String> = if complete {
            known.into_keys().filter(|p| !seen.contains(p)).collect()
        } else {
            Vec::new()
        };
        match cat.record_prefab_walk(&key, &parsed, &removed) {
            // This scope's evidence is current, so M28's legacy pairings for the meshes it holds
            // have been replaced (ADR-0018).
            Ok(_) if complete => {
                if let Err(e) = cat.retire_legacy_prefab_evidence(&key) {
                    tracing::warn!(%e, scope = %key, "retiring legacy prefab evidence failed");
                }
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(%e, scope = %key, "recording prefab evidence failed");
                all_complete = false;
            }
        }
    }
    if whole_library && all_complete {
        let keys: Vec<String> = scopes
            .iter()
            .map(|s| s.root.to_string_lossy().into_owned())
            .collect();
        if let Err(e) = cat.retain_prefab_scopes(&keys) {
            tracing::warn!(%e, "forgetting retired prefab scopes failed");
        }
    }
    if nested > 0 {
        tracing::info!(
            nested,
            "nested prefab instances left unresolved: their overrides are not bindings"
        );
    }
    match cat.reconcile_prefab_edges() {
        Ok(changed) => {
            if changed > 0 {
                tracing::info!(changed, "prefab edges reconciled");
            }
            changed
        }
        Err(e) => {
            tracing::warn!(%e, "reconciling prefab edges failed");
            0
        }
    }
}

/// The §10 step-2 naming heuristic: connect a raw-export model (no `.meta`, so no GUID and no edge)
/// to sibling textures in the same source folder whose file stem name-matches the model's — e.g.
/// `Human3.fbx` → `Human3_Albedo.png`. The matching (`deps::texture_matches_model`) runs here in the
/// ingest-aware layer; the catalog only supplies the rows and takes the resolved edges. Applies only
/// to edge-less models, so it never overrides the deterministic GUID/prefab graph.
pub fn drain_inferred_texture_edges(cat: &mut Catalog) {
    use std::path::{Path, PathBuf};
    let models = match cat.edgeless_models() {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(%e, "naming heuristic: edge-less models query failed");
            return;
        }
    };
    if models.is_empty() {
        return;
    }
    let textures = match cat.inference_textures() {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(%e, "naming heuristic: textures query failed");
            return;
        }
    };
    // Index textures by (source, parent folder) → (stem, id, rel_path).
    type DirIndex = std::collections::HashMap<(i64, PathBuf), Vec<(String, i64, String)>>;
    let mut by_dir: DirIndex = DirIndex::new();
    for (id, src, rel) in &textures {
        let p = Path::new(rel);
        let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
        if let Some(stem) = p.file_stem().map(|s| s.to_string_lossy().into_owned()) {
            by_dir
                .entry((*src, dir))
                .or_default()
                .push((stem, *id, rel.clone()));
        }
    }
    let mut edges: Vec<(i64, i64, String)> = Vec::new();
    for (mid, src, rel) in &models {
        let p = Path::new(rel);
        let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
        let Some(mstem) = p.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
            continue;
        };
        if let Some(sibs) = by_dir.get(&(*src, dir)) {
            for (tstem, tid, trel) in sibs {
                if lodestash_ingest::deps::texture_matches_model(&mstem, tstem) {
                    edges.push((*mid, *tid, trel.clone()));
                }
            }
        }
    }
    if edges.is_empty() {
        return;
    }
    let borrowed: Vec<(i64, i64, &str)> =
        edges.iter().map(|(f, t, d)| (*f, *t, d.as_str())).collect();
    match cat.insert_inferred_texture_edges(&borrowed) {
        Ok(n) if n > 0 => tracing::info!(inferred = n, "naming-heuristic texture edges added"),
        Ok(_) => {}
        Err(e) => tracing::warn!(%e, "inserting inferred texture edges failed"),
    }
}
