import { invoke } from "@tauri-apps/api/core";

export type GridItem = {
  asset_id: number;
  name: string;
  rel_path: string;
  media_type: string;
  size_bytes: number;
  status: string;
  has_preview: boolean;
  favorite: boolean;
  in_pack: boolean;
  package: string | null;
  triangles: number | null;
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
  pinned: boolean;
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
  mcp_disabled_tools: string[];
};

export const getSettings = () => invoke<Settings>("get_settings");
export const setSettings = (settings: Settings) => invoke<void>("set_settings", { settings });
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
  outbound: boolean;
  kind: string;
  state: string;
  declared_path: string | null;
  asset_id: number | null;
  name: string | null;
};

export const relatedAssets = (assetId: number) => invoke<RelatedAsset[]>("related_assets", { assetId });

/** The staging tray (I6) — in-memory ids, order-preserving. */
export const stage = (ids: number[]) => invoke<number>("stage", { ids });
export const unstage = (ids: number[]) => invoke<number>("unstage", { ids });
export const clearStaged = () => invoke<void>("clear_staged");
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

export const libraryStats = () => invoke<Stats>("library_stats");
export const duplicateGroups = () => invoke<DupGroup[]>("duplicate_groups");

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
  texture_asset_id: number | null;
  in_pack: boolean;
  favorite: boolean;
  trashed: boolean;
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
  { id: "modified", label: "Date" },
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
export type WriteResult = { changed: number; refused: number; failed: number };

export const setFavorite = (assetIds: number[], on: boolean) =>
  invoke<WriteResult>("set_favorite", { assetIds, on });

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
