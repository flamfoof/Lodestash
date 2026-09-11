import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type GridItem = {
  asset_id: number;
  /**
   * Which `import_source` owns the row. The frontend looks its availability up in `sources`
   * (`sources.svelte.ts`) rather than the backend stamping a second copy of `state` onto every
   * card — the plate, the inert actions and Quick Look's panel are one fact, read three ways.
   */
  source_id: number;
  name: string;
  rel_path: string;
  media_type: string;
  size_bytes: number;
  status: string;
  has_preview: boolean;
  favorite: boolean;
  in_pack: boolean;
  package: string | null;
  /**
   * Already imported into the **active** project (DESIGN-GUIDELINES §7 "Already imported badge").
   * Filled by one bulk read per reload, so switching projects re-paints every badge in the grid.
   */
  in_project: boolean;
  /** Imported here, but the source changed on disk since — the one actionable state of the four. */
  import_stale: boolean;
  /** How many *other* projects hold it. History, not a warning. */
  in_other_projects: number;
  triangles: number | null;
  /** Clip length for audio — the sound card's `pos / dur` readout before playback starts. */
  duration_ms: number | null;
  /** Tag values, drawn as chips under the meta line (FEATURES §1.1). */
  traits: string[];
  /** `S` / `M` / `L` / `XL` from the user's own thresholds — never recompute it here. */
  size_tier: string;
};

export type GridPage = { items: GridItem[]; total: number; page: number; pages: number };

export type SourceSummary = {
  id: number;
  name: string;
  root_path: string;
  asset_count: number;
  /** Whether the source is being watched (UI-SPEC §11.1) — the Settings pause/resume toggle. */
  watch_enabled: boolean;
  /**
   * `available` | `paused` | `unavailable` (M18, UI-SPEC §11.1). Unavailable means the root is not
   * on disk right now — every row it owns is still listed and searchable, only the files are out of
   * reach. Derived by the health check, never authored.
   */
  state: string;
  /** Unix seconds the root last resolved — "unavailable since …". */
  last_seen_at: number | null;
  /** `folder`, or a cache kind — onboarding (§1) tags rows "auto-detected" vs "folder". */
  kind: string;
  /** Unix seconds of the last completed crawl; `null` = never re-scanned since indexing. */
  last_scanned_at: number | null;
};

export type LibraryStatus = {
  open: boolean;
  catalog_path: string | null;
  sources: SourceSummary[];
  total_assets: number;
  in_pack: number;
  favorites: number;
  with_preview: number;
  trashed: number;
  unidentified: number;
  identified: number;
  confirmed: number;
  duplicate_groups: number;
  /** Type-tab counts (UI-SPEC §0.3) — what each category tab's number reads, and which hide. */
  tab_counts: TabCount[];
};

export type TabCount = { id: string; count: number };

/**
 * The ⚑ Filters popover's selection (UI-SPEC §2.3), sent as one object because it deserializes
 * into `lodestash_app::types::ContentFilter` — the same filter the egui build applies, with the
 * same `passes` predicate. Chips OR within a group and AND across groups.
 *
 * The ids are opaque here on purpose: they come from `filter_facets`, which derives them from the
 * Rust enums' own serde names. Spelling one out in TypeScript is how a chip comes to select
 * nothing.
 */
export type ContentFilter = {
  traits: string[];
  tri_buckets: string[];
  sizes: string[];
};

export const emptyFilter = (): ContentFilter => ({ traits: [], tri_buckets: [], sizes: [] });

export const filterCount = (f: ContentFilter): number =>
  f.traits.length + f.tri_buckets.length + f.sizes.length;

/** One selectable chip: `id` goes back in the filter, `label` is what it reads as. */
export type FacetOption = { id: string; label: string };
export type Facets = {
  traits: { name: string; count: number }[];
  tri_buckets: FacetOption[];
  sizes: FacetOption[];
};

export type GridQuery = {
  tab: string;
  search: string;
  /** ⟨Tags⟩/⟨AI⟩ — "tags" = FTS ranks, "ai" = RRF-fused FTS + query embeddings. */
  search_mode: "tags" | "ai";
  /** Restrict to exactly these asset ids — the Identify screen's "just appraised" session set. */
  ids: number[];
  status: string;
  sort_by: string;
  sort_asc: boolean;
  favorites_only: boolean;
  /** Show the Trash instead of the library — not a facet you combine with the others. */
  trashed_only: boolean;
  filter: ContentFilter;
  /** Restrict to one pack — what Packages' Browse → sets. */
  package?: string | null;
  /** Restrict to one source library — the sidebar's LIBRARIES facet. Not the same as `package`. */
  source?: string | null;
  /** Restrict to one collection's members — what Collections' Browse → sets (UI-SPEC §6). */
  collection?: number | null;
  /**
   * Restrict to assets sharing an `asset_relationship` edge with this one — the ⚑ Filters LINKS
   * section (FEATURES §1.4). The selected asset's id; the grid shows only its linked neighbours.
   */
  links?: number | null;
  page: number;
  page_size: number;
};

/**
 * One level of the Folders view. `source: null` is the top level, where the tiles are the sources.
 * `dir` is source-root-relative and `/`-separated; for an in-pack asset its first segment is the
 * pack name, which is what keeps a source's dozens of packs from merging into one `Assets` folder.
 */
export type FolderQuery = {
  tab: string;
  search: string;
  status: string;
  filter: ContentFilter;
  source: string | null;
  dir: string;
  /** Pack-rooted browse (PACK-DIRECTORY-NAV): `dir` is relative to the pack, only its assets show. */
  package?: string | null;
};

export type FolderTile = { name: string; direct: number; recursive: number };
export type FolderPage = { tiles: FolderTile[]; items: GridItem[]; third: string[]; total: number };

export const folderPage = (query: FolderQuery) => invoke<FolderPage>("folder_page", { query });

/** One folder in the pack browser's left tree (PACK-DIRECTORY-NAV §5). */
export type PackDirNode = {
  path: string;
  name: string;
  depth: number;
  direct: number;
  recursive: number;
  has_children: boolean;
};

export const packDirTree = (packageName: string) =>
  invoke<PackDirNode[]>("pack_dir_tree", { package: packageName });

/** UI-SPEC §8.1 overview. Every number is derived from the rows the shell already holds. */
export type TypeStat = {
  media_type: string;
  count: number;
  bytes: number;
  triangles: number;
  duration_ms: number;
};
export type PackStat = { name: string; count: number; bytes: number };
export type Stats = {
  total_assets: number;
  sources: number;
  packs: number;
  bytes_total: number;
  triangles_total: number;
  models: number;
  unidentified: number;
  by_type: TypeStat[];
  by_pack: PackStat[];
  heaviest: GridItem[];
  duplicate_groups: number;
};

/** One exact-duplicate group (UI-SPEC §5). A review queue — nothing resolves itself. */
/**
 * One row of a two-column comparison (UI-SPEC §13 / §5). Lives here rather than in the component
 * because both the inspector (`AssetDetail`) and Duplicates (`GridItem`) build these from different
 * shapes — and a Svelte component's instance script cannot export a type.
 */
export type CompareRow = { label: string; values: [string, string] };

export type DupGroup = {
  hash: string;
  size_bytes: number;
  items: GridItem[];
  keeper: number;
  cross_pack: boolean;
};

/** One pack on the Packages screen (UI-SPEC §4). `count` is indexed rows, not the manifest claim. */
export type PackageRow = {
  id: number;
  name: string;
  publisher: string;
  category: string | null;
  count: number;
  unidentified: number;
  bytes: number;
  triangles: number;
  by_type: TypeStat[];
  pinned: boolean;
  /** M23 — its extracted bytes are worth keeping. Separate from `pinned` (Add to Library). */
  keep_cached: boolean;
  /** Where its `.unitypackage` lives — what Retarget… replaces (UI-SPEC §15). */
  package_path: string;
  /** Its `.unitypackage` still resolves on disk. `false` ⇒ the extracted copy is the only one. */
  archive_present: boolean;
  extracted: boolean;
};

export const packages = () => invoke<PackageRow[]>("packages");

/** One row of the trait vocabulary (UI-SPEC §9). Authoring is wired through the writer ops below. */
export type VocabRow = {
  /** The `tag` row's id — what the authoring commands address. */
  id: number;
  namespace: string;
  value: string;
  /** False for a plain tag — the row the promote strip shows. */
  is_trait: boolean;
  scope: string | null;
  usage_note: string | null;
  usage_count: number;
  manual_count: number;
  ai_count: number;
  teach: boolean;
  /** The trait's chip style — a CSS token or hex colour, and an icon id. */
  color: string | null;
  icon: string | null;
};

export const vocabulary = () => invoke<VocabRow[]>("vocabulary");

/** Create or promote a trait — an existing tag keeps its assignments and gains trait status. */
export const upsertTrait = (args: {
  namespace: string;
  value: string;
  scope?: string | null;
  /** camelCase on purpose — Tauri v2 maps it to the command's snake_case `usage_note` param. */
  usageNote?: string | null;
  teach?: boolean;
}) => invoke<{ changed: number }>("upsert_trait", args);

export const unpromoteTrait = (tagId: number) => invoke("unpromote_trait", { tagId });
export const setTraitTeach = (tagId: number, teach: boolean) => invoke("set_trait_teach", { tagId, teach });
export const setTraitStyle = (tagId: number, color: string | null, icon?: string | null) =>
  invoke("set_trait_style", { tagId, color, icon });

/**
 * The Settings screen's snapshot (UI-SPEC §11) — size tiers, preview quality, the first-run
 * flag, watch/auto-identify, MCP allow-list and storage caps. Whole-object read/write.
 */
export type Settings = {
  m_kb: number;
  l_kb: number;
  xl_kb: number;
  cap_kb: number;
  preview_quality: "low" | "medium" | "high";
  skip_onboarding: boolean;
  watch_sources: boolean;
  auto_identify: boolean;
  cache_extracted: boolean;
  preview_cache_cap_gb: number;
  auto_purge_preview_cache: boolean;
  confirm_before_trash: boolean;
  mcp_disabled_tools: string[];
  tagger_model: string | null;
  tagger_vision_model: string | null;
  api_tagging_enabled: boolean;
  api_base_url: string;
  api_model: string | null;
  api_key: string;
  /** Hard per-run cap on cloud-tagged assets (AI-PIPELINE §5.1). 0 = uncapped. */
  api_max_per_run: number;
};

export const getSettings = () => invoke<Settings>("get_settings");
export const setSettings = (settings: Settings) => invoke<void>("set_settings", { settings });

/**
 * One CLOUD-card endpoint preset (AI-PIPELINE §5). `model_hint` is guidance, never a committed
 * value — providers rename models often, so only `base_url` is prefilled. `free` drives the
 * budget confirm: a paid endpoint gets one before a run, a free/local one does not.
 */
export type ApiPreset = { label: string; base_url: string; model_hint: string; free: boolean };
export const apiPresets = () => invoke<ApiPreset[]>("api_presets");

/**
 * Test the saved cloud config against a generated sample image (AI-PIPELINE §5.1). Resolves with
 * the model's **raw** reply, rejects with the raw error — a wrong base URL, a non-vision model id
 * or a dead key otherwise produce nothing from a run that takes hours, with no signal at all.
 */
export const testApiTagger = () => invoke<string>("test_api_tagger");

/**
 * What a paid cloud Identify run would cost, or `null` when no confirm is needed (AI-PIPELINE
 * §5.1 budget guard). `null` covers both a local pass and a free/loopback endpoint.
 *
 * The free/paid call is made in Rust, where `is_free_endpoint` already owns it and treats an
 * unknown remote as **paid** — deciding it here would give the confirm a different opinion from
 * the drain it is guarding.
 */
export type CloudRunEstimate = {
  /** Assets on the tagging queue right now. */
  queued: number;
  /** What the run would actually bill: `queued`, bounded by the per-run cap. One call per asset. */
  calls: number;
  /** The cap bit — there is more queued than this run will take. */
  capped: boolean;
  endpoint: string;
  model: string | null;
};
export const cloudRunEstimate = () => invoke<CloudRunEstimate | null>("cloud_run_estimate");
export const setSourceWatch = (id: number, watch: boolean) => invoke("set_source_watch", { id, watch });

/** One row of the Collections screen (UI-SPEC §6). */
export type CollectionRow = { id: number; name: string; asset_count: number };

export const collections = () => invoke<CollectionRow[]>("collections");
export const createCollection = (name: string) => invoke<{ changed: number }>("create_collection", { name });
export const deleteCollection = (id: number) => invoke<{ changed: number }>("delete_collection", { id });
export const collectionMembers = (collectionId: number) =>
  invoke<number[]>("collection_members", { collectionId });
export const setPackagePinned = (id: number, pinned: boolean) =>
  invoke<{ changed: number }>("set_package_pinned", { id, pinned });

/** One related asset (DATA-MODEL §10): a dependency edge endpoint from the inspector's view. */
export type RelatedAsset = {
  /** `asset_relationship.id` — what ⊘ Ignore acts on (UI-SPEC §13.6). */
  edge_id: number;
  outbound: boolean;
  kind: string;
  /** `resolved` | `missing` | `ignored`. */
  state: string;
  declared_path: string | null;
  asset_id: number | null;
  name: string | null;
  media_code: string | null;
  /**
   * §13.6's **✓ in pack** vs **⤷ in library**: is the other end in the *same* package?
   * `null` when the edge resolves to nothing. A cross-pack dependency is what ENGINE-API §5.5
   * flags — Unity reassigns GUIDs per import, so that reference will not survive the trip.
   */
  same_pack: boolean | null;
};

export const relatedAssets = (assetId: number) => invoke<RelatedAsset[]>("related_assets", { assetId });

/**
 * "More like this" (UI-SPEC §13.8) — nearest neighbours in the asset's own embedding space,
 * best first. Empty when the asset has no embedding yet.
 */
export const similarAssets = (assetId: number, limit = 3) =>
  invoke<GridItem[]>("similar_assets", { assetId, limit });

/**
 * Storage (UI-SPEC §11.4). Two stores, two policies: previews are disposable and capped,
 * extracted packs are per-pack and chosen.
 */
export type ReclaimEntry = { path: string; bytes: number; files: number };
export type ReclaimPlan = {
  entries: ReclaimEntry[];
  bytes: number;
  files: number;
  /** Folders left alone because a live asset still resolves through them. */
  kept_dirs: number;
};

/** What a reclaim *would* remove. Always a dry run — the button reports before it deletes. */
export const workspaceUsage = () => invoke<ReclaimPlan>("workspace_usage");

/** Do it. Re-plans server-side rather than trusting a plan this screen was holding. */
export const reclaimWorkspace = () =>
  invoke<{ removed: number; freed: number; log: string | null }>("reclaim_workspace");

/** A pack holding extracted bytes. `archive_present: false` ⇒ this copy is the only one. */
export type WorkspacePackRow = {
  id: number;
  name: string;
  package_path: string;
  keep_cached: boolean;
  files: number;
  archive_present: boolean;
};
export const workspacePacks = () => invoke<WorkspacePackRow[]>("workspace_packs");

/**
 * Keep a pack's extracted bytes (UI-SPEC §15). Not `pinned`, which means "Add to Library" (M7).
 * A pack that is not kept stays fully searchable and previewable — its assets just load out of
 * the archive instead of off disk.
 */
export const setPackKeepCached = (packageId: number, keep: boolean) =>
  invoke<WriteResult>("set_pack_keep_cached", { packageId, keep });

/** How much of a pack a candidate archive accounts for, by GUID. Evidence, never a verdict. */
export const retargetPreview = (packageId: number, path: string) =>
  invoke<{ matched: number; total: number }>("retarget_preview", { packageId, path });

export const retargetPackage = (packageId: number, path: string) =>
  invoke<WriteResult>("retarget_package", { packageId, path });

/** ⊘ Ignore / ↺ restore one dependency edge (UI-SPEC §13.6). Muted, not deleted — reversible. */
export const setLinkIgnored = (edgeId: number, ignored: boolean) =>
  invoke<WriteResult>("set_link_ignored", { edgeId, ignored });

/** The staging tray (I6) — in-memory ids, order-preserving. */
/**
 * The staging tray (UI-SPEC §16.2). These are **catalog writes** through the single writer, not
 * in-memory shell state — the tray moved into `collection(is_staging=1)` on 2026-08-27 so the
 * app and ENGINE-API §6 stop answering "what is staged" differently (ADR-0010). Picks survive a
 * restart, and follow the active project.
 */
export const stage = (ids: number[]) => invoke<WriteResult>("stage", { ids });
export const unstage = (ids: number[]) => invoke<WriteResult>("unstage", { ids });
export const clearStaged = () => invoke<WriteResult>("clear_staged");
export const staged = () => invoke<number[]>("staged");

/** The Mapping tab (UI-SPEC §9.2). */
export type MappingRuleRow = {
  id: number;
  category_id: number;
  category_path: string;
  match_type: string;
  pattern: string;
  priority: number;
  enabled: boolean;
};

export const listRules = (taxonomyId: number) => invoke<MappingRuleRow[]>("list_rules", { taxonomyId });
export const applyMapping = (taxonomyId: number) => invoke<{ changed: number }>("apply_mapping", { taxonomyId });
/** ✦ Identify all — one bounded appraise pass on the persist thread (B11). */
export const identifyAll = () => invoke<WriteResult>("identify_all");
/** ✦ Identify one asset (B11b) — flags it dirty and appraises it in one bounded pass. */
/** Appraise an explicit set in ONE bounded pass — what "Appraise this source" runs (UI-SPEC §3). */
export const identifyIds = (assetIds: number[]) =>
  invoke<WriteResult>("identify_ids", { assetIds });

export const identifyAsset = (assetId: number) =>
  invoke<WriteResult>("identify_asset", { assetId });
/** True while the persist thread is inside an appraise pass (B11d). */
export const pumpActive = () => invoke<boolean>("pump_active");

export type PumpProgress = { active: boolean; done: number; total: number; failed: number };
export const pumpProgress = () => invoke<PumpProgress>("pump_progress");

export type EngineBridge = {
  live: boolean;
  port: number | null;
  token_set: boolean;
  allowed: number | null;
};
export const engineBridge = () => invoke<EngineBridge>("engine_bridge");

/**
 * One pack in an import whose render pipeline disagrees with the target project's
 * (ENGINE-API §5.4). Advisory — the import proceeds either way.
 */
export type PipelineAdvisory = {
  package_id: number;
  package: string;
  package_pipeline: string;
  project_pipeline: string;
  asset_ids: number[];
};

/** `⇢ Add to project` — queue an engine import into the open Unity project (§5.2). */
export const engineImport = (assetIds: number[]) =>
  invoke<
    | {
        kind: "AwaitingApproval";
        detail: { job: Record<string, unknown>; pipeline_mismatch: PipelineAdvisory[] };
      }
    | { kind: "AlreadyImported"; detail: { ids: number[]; pipeline_mismatch: PipelineAdvisory[] } }
  >("engine_import", { assetIds });

/** How ENGINE-API §5.4's advisory reads to a person: which pack, which pipeline, what they'll see. */
export function pipelineWarning(a: PipelineAdvisory[]): string | null {
  if (a.length === 0) return null;
  const name = (p: string) => ({ built_in: "Built-in", urp: "URP", hdrp: "HDRP" })[p] ?? p;
  const head = a[0];
  const rest = a.length > 1 ? ` (+${a.length - 1} more)` : "";
  return `${head.package} targets ${name(head.package_pipeline)}; your project runs ${name(
    head.project_pipeline,
  )} — materials may appear pink${rest}`;
}
export const engineApprove = (jobId: string) => invoke<boolean>("engine_approve", { jobId });
export const engineDeny = (jobId: string) => invoke<boolean>("engine_deny", { jobId });
/** Jobs parked on the human approval gate (ENGINE-API §4) — the tray's Approve/Deny section. */
export const engineApprovals = () => invoke<Record<string, unknown>[]>("engine_approvals");
/** Per-pack Extract (B2b slice) — unpack into the workspace + reconcile the payloads. */
export const extractPackage = (packageId: number) =>
  invoke<WriteResult>("extract_package", { packageId });

// ── Job tray (B2b) ─────────────────────────────────────────────────────────────────────────

export interface JobSnapshot {
  id: number;
  label: string;
  kind: string;
  /** `(done, total)` bytes for extract; `null` = indeterminate (spinner). */
  progress: [number, number] | null;
  cancellable: boolean;
}
export interface RecentJob {
  id: number;
  label: string;
  kind: string;
  ok: boolean;
  cancelled: boolean;
  changed: number;
  at_unix: number;
}
export interface JobsView {
  active: JobSnapshot[];
  recent: RecentJob[];
}

export const jobs = () => invoke<JobsView>("jobs");
/** Ask one active job to stop (extract workers cancel between reads). */
export const cancelJob = (jobId: number) => invoke<boolean>("cancel_job", { jobId });

// ── Connect wizard app half (ENGINE-API §5) ─────────────────────────────────────────────────

export interface ProjectRow {
  id: number;
  name: string;
  project_path: string;
  unity_version: string | null;
  pipeline: string | null;
  is_active: boolean;
  last_seen_at: number | null;
}

export const projects = () => invoke<ProjectRow[]>("projects");
/** Switch which registered Unity editor is active. */
export const setActiveProject = (projectId: number) =>
  invoke<WriteResult>("set_active_project", { projectId });

// ── Recent exports & batch undo (UI-SPEC §16.1) ──────────────────────────────────────────────

/** One persisted import batch — every `import_record` sharing a `batch_id`, undoable as a unit. */
export type ImportBatch = {
  batch_id: string;
  project_id: number;
  project_name: string;
  /** Destination folder the batch's files were written into (e.g. `Assets/Wildlife`). */
  dest: string;
  file_count: number;
  imported_at: number;
};

/** The last `limit` import batches, newest first. */
export const recentExports = (limit: number) =>
  invoke<ImportBatch[]>("recent_exports", { limit });

/** Undo a batch: delete its copied files + `import_record`s (catalog assets untouched). */
export const undoImportBatch = (batchId: string) =>
  invoke<WriteResult>("undo_import_batch", { batchId });

/**
 * One **folder** export run — the other half of FEATURES §7.1's Recent exports.
 *
 * Separate from `ImportBatch` because the acts are separate (UI-SPEC §16.2 keeps *Import* and
 * *Export* apart) and because `import_record` cannot hold one: its `project_id` is `NOT NULL` and
 * its `dest_path` is project-relative, while a folder export goes to an absolute path of the
 * user's choosing.
 */
export type FolderExport = {
  batch_id: string;
  dest_path: string;
  /** Files written — what undo deletes, not how many assets were picked. */
  files: number;
  exported_at: number;
};
export const recentFolderExports = (limit: number) =>
  invoke<FolderExport[]>("recent_folder_exports", { limit });
/** Delete the files one folder export wrote, then forget it. Originals are never in scope. */
export const undoFolderExport = (batchId: string) =>
  invoke<WriteResult>("undo_export_batch", { batchId });

// ── Pump events (B2b) ────────────────────────────────────────────────────────────────────────

/**
 * Subscribe to writer-side change events (`ls:changed`). `kind` is "catalog" (re-read status
 * + the current page) or "jobs" (refetch the tray). Returns an unsubscribe function.
 */
/**
 * What `ls:changed` carries (ADR-0016): the domains that changed since the last publication, who
 * changed the catalog in that window, and the per-domain generations a refresh reconciles against.
 * At most one event per window (250 ms), however many commits it covers.
 */
export type ChangedEvent = {
  catalog: boolean;
  jobs: boolean;
  origins: ("user" | "agent" | "background")[];
  catalog_gen: number;
  jobs_gen: number;
};

export function onChanged(kind: "catalog" | "jobs", cb: (e: ChangedEvent) => void): () => void {
  let unlisten: (() => void) | null = null;
  let disposed = false;
  listen<ChangedEvent>("ls:changed", (e) => {
    if (e.payload[kind]) cb(e.payload);
  }).then((u) => {
    // An effect torn down before `listen` resolved must still unsubscribe, or it leaks a listener.
    if (disposed) u();
    else unlisten = u;
  }).catch((e) => {
    // A missing `core:event:allow-listen` capability rejects here — and did, silently, from the
    // day these events were added until 2026-09-10, so the shell never heard one. Say so.
    console.error(`ls:changed: cannot subscribe to ${kind} changes`, e);
  });
  return () => {
    disposed = true;
    unlisten?.();
  };
}
export const addCategory = (
  taxonomyId: number,
  parentId: number | null,
  prefLabel: string,
  path: string,
) => invoke<{ changed: number }>("add_category", { taxonomyId, parentId, prefLabel, path });
export const addRule = (args: {
  taxonomyId: number;
  categoryId: number;
  matchType: string;
  pattern: string;
  priority: number;
}) => invoke<WriteResult>("add_rule", args);
export const deleteRule = (ruleId: number) => invoke<WriteResult>("delete_rule", { ruleId });

/** One node of the Stats Tag Graph (UI-SPEC §8.2): a tag with its usage count. */
export type TagGraphNode = {
  tag_id: number;
  namespace: string;
  value: string;
  usage_count: number;
};

/** One co-occurrence edge of the Stats Tag Graph (UI-SPEC §8.2): two tags on the same asset. */
export type TagGraphEdge = {
  tag_a: number;
  tag_b: number;
  /** The number of assets bearing both tags. */
  weight: number;
};

/** The full Stats Tag Graph payload (UI-SPEC §8.2), including the `tags · links · assets` counts. */
export type TagGraph = {
  nodes: TagGraphNode[];
  edges: TagGraphEdge[];
  asset_count: number;
  link_count: number;
};

export const tagGraph = () => invoke<TagGraph>("tag_graph");

/** One resolved asset→asset edge (DATA-MODEL §10) — behind the LINKS filter and the graph arcs. */
export type AssetEdge = {
  from: number;
  to: number;
  kind: string;
  source: string;
};

export const assetEdges = () => invoke<AssetEdge[]>("asset_edges");

/** One per-source blacklist entry (UI-SPEC §11.1): `kind` is `folder` or `asset`. */
export type SourceExclude = {
  kind: string;
  value: string;
};

export const sourceExcludes = (sourceId: number) =>
  invoke<SourceExclude[]>("source_excludes", { sourceId });
export const addSourceExclude = (sourceId: number, kind: string, value: string) =>
  invoke<WriteResult>("add_source_exclude", { sourceId, kind, value });
export const removeSourceExclude = (sourceId: number, kind: string, value: string) =>
  invoke<WriteResult>("remove_source_exclude", { sourceId, kind, value });

/** One row of the Mapping tab's taxonomy switcher (UI-SPEC §9.2). */
export type TaxonomyRow = {
  id: number;
  name: string;
  version: number;
  is_active: boolean;
};

export const taxonomies = () => invoke<TaxonomyRow[]>("taxonomies");
export const setActiveTaxonomy = (id: number) => invoke<WriteResult>("set_active_taxonomy", { id });
export const deleteTaxonomy = (id: number) => invoke<WriteResult>("delete_taxonomy", { id });
export const renameCategory = (id: number, label: string) =>
  invoke<WriteResult>("rename_category", { id, label });
export const deleteCategory = (id: number) => invoke<WriteResult>("delete_category", { id });

/** One undo-log row (UI-SPEC §2 Ctrl/Cmd-Z / L5) — the payload is opaque JSON to the frontend. */
export type OpLogRow = {
  id: number;
  op_kind: string;
  payload_json: string;
  created_at: number;
};

export const recentOps = (limit: number) => invoke<OpLogRow[]>("recent_ops", { limit });
export const redoOps = (limit: number) => invoke<OpLogRow[]>("redo_ops", { limit });
export const redo = (opId: number) => invoke<WriteResult>("redo", { opId });
export const undo = (opId: number) => invoke<WriteResult>("undo", { opId });

export const collectionAddAsset = (collectionId: number, assetId: number) =>
  invoke<WriteResult>("collection_add_asset", { collectionId, assetId });
export const collectionRemoveAsset = (collectionId: number, assetId: number) =>
  invoke<WriteResult>("collection_remove_asset", { collectionId, assetId });

/** Bulk-bar tag edit (KANBAN B9d): add/remove one tag on many assets; each is undoable. */
export const bulkTag = (assetIds: number[], namespace: string, value: string, on: boolean) =>
  invoke<WriteResult>("bulk_tag", { assetIds, namespace, value, on });

/**
 * Index a folder and add it as a library source — what drag-and-drop drops into.
 *
 * Synchronous: it does not resolve until the folder is crawled and written, which on a real
 * source is minutes. Show a spinner, not a progress bar — real progress needs the job tray.
 */
export const addSource = (path: string) => invoke<WriteResult>("add_source", { path });
export const syncUnityCache = () => invoke<WriteResult>("sync_unity_cache");

/** One Asset Store cache root found on disk and not yet a source (UI-SPEC §1). */
export type CacheCandidate = { path: string; packs: number };
/**
 * Detect the Unity cache **without importing it**. Onboarding has to show the cache before the
 * user agrees to index it — `syncUnityCache` is the version that crawls, and calling that to find
 * out whether there was anything to find would index the machine to answer a question.
 */
export const unityCacheCandidates = () => invoke<CacheCandidate[]>("unity_cache_candidates");
/** Native folder chooser for the sidebar's "+ Add source…" (B2b). `null` = cancelled. */
export const addSourceDialog = () =>
  invoke<WriteResult | null>("add_source_dialog");

// ── Import-packages dialog (FEATURES §5.2) ────────────────────────────────────────────────

/** One `.unitypackage` the import dialog lists — see `discover_package_folder`. */
export type DiscoveredPackageRow = {
  group: string;
  publisher: string;
  name: string;
  abs_path: string;
  size_bytes: number;
  version: string | null;
  assets: number;
  category: string | null;
  duplicate_of: string | null;
};

/** Native folder chooser for the import dialog. `null` = cancelled. */
export const pickFolder = (title?: string) => invoke<string | null>("pick_folder", { title });

/** Native `.unitypackage` chooser — Retarget… (UI-SPEC §15). `null` = cancelled. */
export const pickPackageFile = (title?: string) =>
  invoke<string | null>("pick_package_file", { title });

/** Scan a folder for `.unitypackage` files (read-only) — the dialog's list. */
export const discoverPackageFolder = (path: string) =>
  invoke<DiscoveredPackageRow[]>("discover_package_folder", { path });

/** The ticked packs sent back for import (locators are filled server-side). */
export type ImportSelection = {
  abs_path: string;
  name: string;
  publisher: string | null;
  category: string | null;
};

/** Import the ticked packs as libraries — one `package` row apiece, pinned + minted in place. */
export const importPackages = (root: string, packs: ImportSelection[]) =>
  invoke<WriteResult>("import_packages", { root, packs });

export const libraryStats = () => invoke<Stats>("library_stats");
export const duplicateGroups = () => invoke<DupGroup[]>("duplicate_groups");
export const nearDuplicateGroups = () => invoke<DupGroup[]>("near_duplicate_groups");

export type AssetDetail = {
  asset_id: number;
  name: string;
  rel_path: string;
  abs_path: string;
  media_type: string;
  size_bytes: number;
  status: string;
  has_preview: boolean;
  triangles: number | null;
  width_px: number | null;
  height_px: number | null;
  duration_ms: number | null;
  source: string | null;
  /** The pack it sits in, for §13.9's `Source · <library · pack>` row. `null` for a loose file. */
  package: string | null;
  /**
   * For an in-pack asset, where it lives *inside* the archive and which `.unitypackage` holds
   * it (UI-SPEC §13.9). Both `null` for a file on disk, which has `abs_path` instead.
   */
  pack_entry_path: string | null;
  pack_archive_path: string | null;
  /**
   * The licence that applies — the asset's own override falling back to its pack's
   * (DATA-MODEL §9). `null` when nothing states one, and §13.9 then omits the row entirely.
   */
  license: string | null;
  license_url: string | null;
  /** Its source's id — the key into `sources` for the "isn't reachable" panel (UI-SPEC §11.1). */
  source_id: number;
  texture_asset_id: number | null;
  in_pack: boolean;
  favorite: boolean;
  trashed: boolean;
  /** This asset's associations with who wrote each one (v1 plan S05). */
  tags: TagRow[];
};

/** One association and its provenance: `source` is `manual`, `imported` or `ai:<producer>`. */
export type TagRow = TagAssignment & { source: string; model_id: string | null };

export type TagAssignment = {
  namespace: string;
  value: string;
  confidence: number | null;
};

/**
 * Sort keys the backend understands — the ids are `config::SortBy`'s serde names.
 *
 * Relevance is BM25 order from the FTS index and only means anything while a query is active;
 * with an empty search box every asset is unranked and it degrades to Name.
 */
export const SORTS = [
  { id: "relevance", label: "Relevance" },
  { id: "name", label: "Name" },
  { id: "type", label: "Type" },
  { id: "size", label: "Size" },
  { id: "modified", label: "Date modified" },
  /** When Lodestash first indexed it — insert order, not the file's mtime. */
  { id: "added", label: "Date added" },
  /** Lifecycle order (DATA-MODEL §8), Unidentified first — the work sorts to the top. */
  { id: "status", label: "Status" },
] as const;

/** STATUS facet (UI-SPEC §0.2). `discovered`/`indexed` both read as Unidentified. */
export const STATUSES = [
  { id: "all", label: "All" },
  { id: "unidentified", label: "Unidentified" },
  { id: "identified", label: "Identified" },
  { id: "confirmed", label: "Confirmed" },
] as const;

/** Grid density stops (UI-SPEC §20.10) — card min-widths 150 / 205 / 288px. */
export const DENSITIES = [
  { id: "compact", label: "Compact", icon: "ls-grid-compact", px: 150 },
  { id: "cozy", label: "Cozy", icon: "ls-grid-cozy", px: 205 },
  { id: "large", label: "Large", icon: "ls-grid-large", px: 288 },
] as const;

/** What a write actually did. `refused` + `failed` are the parts worth telling the user about. */
export type WriteResult = {
  changed: number;
  refused: number;
  failed: number;
  /** Asset ids the appraise pass just promoted — the Identify screen's session set. */
  promoted: number[];
};

export const setFavorite = (assetIds: number[], on: boolean) =>
  invoke<WriteResult>("set_favorite", { assetIds, on });

/**
 * Mark assets **confirmed** — "I checked these tags" (DATA-MODEL §8, AI-PIPELINE §6). The one
 * status a person sets: `promote_statuses` never writes or clears it, and un-confirming recomputes
 * the status the ingest work actually earned. No agent-plane equivalent, deliberately — an MCP
 * client must not be able to sign off as the user.
 */
export const setConfirmed = (assetIds: number[], on: boolean) =>
  invoke<WriteResult>("set_confirmed", { assetIds, on });

/** Soft delete / restore. The file on disk is never touched. */
export const setTrashed = (assetIds: number[], trashed: boolean) =>
  invoke<WriteResult>("set_trashed", { assetIds, trashed });

/**
 * Hide an asset for good without deleting it: the row is soft-deleted **and** the file is added
 * to its source's blacklist, so re-indexing that folder will not bring it back.
 */
export const removeFromLibrary = (assetIds: number[]) =>
  invoke<WriteResult>("remove_from_library", { assetIds });

/** **Irreversible.** Confirm before calling — the command does not ask. */
export const deleteFromDisk = (assetIds: number[]) =>
  invoke<WriteResult>("delete_from_disk", { assetIds });

/** The chips the popover draws. Changes only when the catalog does — fetch per load, not per keystroke. */
export const filterFacets = () => invoke<Facets>("filter_facets");

export const libraryStatus = () => invoke<LibraryStatus>("library_status");
export const gridPage = (query: GridQuery) => invoke<GridPage>("grid_page", { query });
export const reloadLibrary = () => invoke<LibraryStatus>("reload_library");
export const assetDetail = (assetId: number) =>
  invoke<AssetDetail | null>("asset_detail", { assetId });

/**
 * The Svelte shell's persisted view prefs (FEATURES §10) — browse view × row style, density,
 * sort. Hydrated once at startup; the whole object is written back on any change.
 */
export type WebPrefs = {
  browse: "flat" | "folders";
  row_style: "grid" | "list";
  density: string;
  sort_by: string;
  sort_asc: boolean;
};

export const getWebPrefs = () => invoke<WebPrefs>("get_web_prefs");
export const setWebPrefs = (prefs: WebPrefs) => invoke<void>("set_web_prefs", { prefs });

/**
 * Base for the custom scheme.
 *
 * WebView2 does not intercept a bare `lodestash://` URL — Tauri registers custom schemes there as
 * `http://<scheme>.localhost/`, and only that form reaches the Rust handler. Everywhere else the
 * real scheme works. Getting this wrong fails silently as a broken image, so it is computed once
 * here rather than spelled out at each call site.
 */
const LODESTASH_BASE = navigator.userAgent.includes("Windows")
  ? "http://lodestash.localhost"
  : "lodestash:/";

/** Preview bytes come over the custom scheme, never through IPC — a page of cards is JSON only. */
export const previewUrl = (assetId: number) => `${LODESTASH_BASE}/preview/${assetId}.svg`;
export const fileUrl = (assetId: number) => `${LODESTASH_BASE}/file/${assetId}`;
/**
 * Raster thumbnail, generated on first request and cached on disk.
 *
 * Preferred over the type icon for everything without a vector preview — which is most of the
 * library. In-pack assets answer 503 and fall back to the icon.
 */
/** Bumped when the Settings preview-quality changes — thumb URLs carry it as a cache-buster
 *  (`?q=`). The backend reads the actual quality from config; the query exists so the webview
 *  re-fetches instead of serving a stale-size PNG from its own cache. */
export let thumbKey = "1";
export const previewCacheUsage = () => invoke<[number, number]>("preview_cache_usage");
export const clearPreviewCache = () => invoke<number>("clear_preview_cache");

/**
 * Tell the appraisal queues where the user is looking (UI-SPEC §3): this page first, then the rest
 * of this filter, then everything else newest-first.
 *
 * A hint, not a write — it never queues behind a running appraise pass, and nothing is persisted.
 */
export const setAppraiseFocus = (query: GridQuery) => invoke<void>("set_appraise_focus", { query });

/**
 * What an **Export to folder** actually did (UI-SPEC §16.2).
 *
 * The two skip counts stay apart because they have different fixes: `in_pack` needs the pack
 * extracted first, `unreachable` needs the drive back. One merged number would say nothing.
 */
export type ExportReport = {
  written: number;
  in_pack: number;
  unreachable: number;
  dependencies_added: number;
  dest_path: string;
};

/**
 * Copy assets to a folder the user picked — the level-1 way out of the library, no editor and no
 * plugin (UI-SPEC §16.3). Originals are never moved, and the destination is refused if it is
 * inside a library source root.
 */
export const exportAssets = (assetIds: number[], destPath: string, withDependencies = true) =>
  invoke<ExportReport>("export_assets", { assetIds, destPath, withDependencies });

/** Re-derive source availability from disk (UI-SPEC §11.1). One stat per root, on the writer. */
export const refreshSourceHealth = () => invoke<WriteResult>("refresh_source_health");

/** What a relink would reach — shown **before** it commits (UI-SPEC §11.1). */
export type RelinkCount = { matched: number; total: number };
export const relinkPreview = (sourceId: number, newRoot: string) =>
  invoke<RelinkCount>("relink_preview", { sourceId, newRoot });
/** Point a source at a new root. A rename, not a re-scan: every row keeps its identity. */
export const relinkSource = (sourceId: number, newRoot: string) =>
  invoke<WriteResult>("relink_source", { sourceId, newRoot });

/** AI model install status (Settings MODELS): installed vs not + size. */
export const modelsStatus = () =>
  invoke<{ id: string; label: string; installed: boolean; size_mb: number }[]>("models_status");
export const modelDownload = (model: string) => invoke<boolean>("model_download", { model });
export const modelRemove = () => invoke<void>("model_remove");

export const setThumbKey = (k: string) => {
  thumbKey = k;
};
export const thumbUrl = (assetId: number) => `${LODESTASH_BASE}/thumb/${assetId}.png?q=${thumbKey}`;

/** Decimated geometry as LSM1, for the WebGL viewer. */
export const meshUrl = (assetId: number) => `${LODESTASH_BASE}/mesh/${assetId}`;

/**
 * `media_type.code` -> the CSS custom property naming its colour (DESIGN-GUIDELINES §2).
 *
 * Deliberately not shared with `lodestash_app::types::Category::token`: that maps the five
 * *tabs*, this maps the seven *media types*, and the tokens themselves live in
 * `docs/design/tokens.css`, which the webview reads directly. Sending a CSS variable name per
 * card over IPC to avoid a switch statement would be the more coupled design, not the less.
 */
export function typeToken(mediaType: string): string {
  switch (mediaType) {
    case "model_3d":
      return "--ls-type-model";
    case "material":
      return "--ls-type-material";
    case "audio":
      return "--ls-type-sound";
    case "texture":
      return "--ls-type-texture";
    case "sprite":
      return "--ls-type-sprite";
    case "font":
      return "--ls-type-font";
    default:
      return "--ls-type-image";
  }
}

/** Sprite id for the type mark. Colour alone never carries meaning (DESIGN-GUIDELINES §9). */
export function typeIcon(mediaType: string): string {
  switch (mediaType) {
    case "model_3d":
      return "ls-type-model";
    case "material":
      return "ls-type-material";
    case "audio":
      return "ls-type-sound";
    case "texture":
      return "ls-type-texture";
    case "sprite":
      return "ls-type-sprite";
    case "font":
      return "ls-type-font";
    default:
      return "ls-type-image";
  }
}

/**
 * `mm:ss` from milliseconds.
 *
 * Truncates, matching `lodestash_app::audio::fmt_time` — which is the reference, not a
 * coincidence: both frontends show the same asset's duration and a 9.7 s clip reading `0:09` in
 * one and `0:10` in the other is a bug report nobody can reproduce. Formatting stays in the view
 * layer (an IPC round-trip to format two numbers is not a deduplication), so the rule lives in two
 * places on purpose and this comment is the link between them.
 */
export function fmtDuration(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/** The user-facing status vocabulary is two words, not four (DATA-MODEL §8). */
export function statusLabel(status: string): string {
  switch (status) {
    case "identified":
      return "Identified";
    case "confirmed":
      return "Confirmed";
    default:
      return "Unidentified";
  }
}

export function fmtSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = bytes / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v < 10 ? v.toFixed(1) : Math.round(v)} ${units[i]}`;
}
