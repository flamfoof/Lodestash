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
};

export type GridPage = { items: GridItem[]; total: number; page: number; pages: number };

export type SourceSummary = { id: number; name: string; root_path: string; asset_count: number };

export type LibraryStatus = {
  open: boolean;
  catalog_path: string | null;
  sources: SourceSummary[];
  total_assets: number;
  in_pack: number;
  favorites: number;
  with_preview: number;
};

export type GridQuery = {
  tab: string;
  search: string;
  status: string;
  sort_by: string;
  sort_asc: boolean;
  favorites_only: boolean;
  page: number;
  page_size: number;
};

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

/** Grid density stops (DESIGN-GUIDELINES §4) — card min-width in px. */
export const DENSITIES = [
  { id: "compact", label: "Compact", icon: "ls-grid-compact", px: 118 },
  { id: "cozy", label: "Cozy", icon: "ls-grid-cozy", px: 148 },
  { id: "large", label: "Large", icon: "ls-grid-large", px: 200 },
] as const;

export const libraryStatus = () => invoke<LibraryStatus>("library_status");
export const gridPage = (query: GridQuery) => invoke<GridPage>("grid_page", { query });
export const reloadLibrary = () => invoke<LibraryStatus>("reload_library");
export const assetDetail = (assetId: number) =>
  invoke<AssetDetail | null>("asset_detail", { assetId });

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
export const thumbUrl = (assetId: number) => `${LODESTASH_BASE}/thumb/${assetId}.png`;

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
