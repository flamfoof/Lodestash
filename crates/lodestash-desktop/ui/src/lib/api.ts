import { invoke } from "@tauri-apps/api/core";

export type GridItem = {
  asset_id: number;
  name: string;
  rel_path: string;
  media_type: string;
  size_bytes: number;
  status: string;
  rating: number | null;
  has_preview: boolean;
  triangles: number | null;
};

export type GridPage = { items: GridItem[]; total: number; page: number; pages: number };

export type SourceSummary = { id: number; name: string; root_path: string; asset_count: number };

export type LibraryStatus = {
  open: boolean;
  catalog_path: string | null;
  sources: SourceSummary[];
  total_assets: number;
  with_preview: number;
};

export type GridQuery = { tab: string; search: string; page: number; page_size: number };

export const libraryStatus = () => invoke<LibraryStatus>("library_status");
export const gridPage = (query: GridQuery) => invoke<GridPage>("grid_page", { query });
export const reloadLibrary = () => invoke<LibraryStatus>("reload_library");

/**
 * Base for the custom scheme.
 *
 * WebView2 does not intercept a bare `ls://` URL — Tauri registers custom schemes there as
 * `http://<scheme>.localhost/`, and only that form reaches the Rust handler. Everywhere else the
 * real scheme works. Getting this wrong fails silently as a broken image, so it is computed once
 * here rather than spelled out at each call site.
 */
const LS_BASE = navigator.userAgent.includes("Windows")
  ? "http://ls.localhost"
  : "ls:/";

/** Preview bytes come over the custom scheme, never through IPC — a page of cards is JSON only. */
export const previewUrl = (assetId: number) => `${LS_BASE}/preview/${assetId}.svg`;
export const fileUrl = (assetId: number) => `${LS_BASE}/file/${assetId}`;

/** `media_type.code` -> the CSS custom property naming its colour (DESIGN-GUIDELINES §2). */
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
