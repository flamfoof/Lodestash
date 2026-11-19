// The asset view model: what the Library screen and the Inspector are handed.
//
// Field names are ENGINE-API.md §3's (`AssetSummary`, `AssetDetail`) and §8.1's additions, so the
// data wiring (plan P6b) is a mapping of names that already agree. `api/generated.ts` replaces
// these declarations when it exists. Every field the UI needs that §3 does not carry yet sits
// under a "beyond §3" comment — that list is the API gap, readable in one place.

/** DATA-MODEL §2 — the seven-value enum; the top-bar tabs are a grouping over it. */
export type MediaType = "model_3d" | "texture" | "audio" | "image" | "sprite" | "material" | "font";

/** DATA-MODEL §8. `discovered` and `indexed` are both "Unidentified" to the user. */
export type AssetStatus = "discovered" | "indexed" | "identified" | "confirmed";

export interface PackageRef {
  id: number;
  name: string;
  publisher?: string | null;
}

/** A tag as `get_asset` ships it (ENGINE-API §8.1): the object, not §3's string shorthand. */
export interface AssetTag {
  namespace: string | null;
  value: string;
  /** 0–1; null for a tag a person or an importer wrote. */
  confidence: number | null;
  /** `manual` | `imported` | `ai:<producer>` (DATA-MODEL §2 `asset_tag.source`). */
  source: string;
  model_id?: string | null;
}

/** Modality-specific metrics (§3 `AssetDetail.stats`). Absent means "not known yet". */
export interface AssetStats {
  tris?: number;
  verts?: number;
  materials?: number;
  bones?: number;
  duration_s?: number;
  sample_rate_hz?: number;
  /** Audio channel count (1 mono, 2 stereo). */
  channels?: number;
  width?: number;
  height?: number;
  /** Image channel layout: `RGB`, `RGBA`. */
  pixel_format?: string;
  color_space?: string;
}

export interface ImportRecord {
  project_id: number;
  dest_path: string;
}

export interface AssetSummary {
  id: number;
  uuid: string;
  name: string;
  media_type: MediaType;
  format: string;
  status: AssetStatus;
  package: PackageRef | null;
  tags: AssetTag[];
  /** Null when there is nothing to draw (a sound has a waveform, not a thumbnail). */
  preview_url: string | null;
  file_url: string;
  // §8.1 summary additions
  source_id: number;
  package_id: number | null;

  // ---- beyond §3: the card draws these, so the summary must carry them ----
  /** The size tier badge and the stat line. */
  size_bytes: number;
  /** The stat line ("1.2k tris · 4.1 MB"); §3 has `stats` on the detail only. */
  stats: AssetStats;
  /** DATA-MODEL §12 — the ☆/★ tile and the Favorites filter. */
  favorite: boolean;
  /** In the active staging tray (ENGINE-API §6) — "+ stage" / "✓ staged". */
  staged: boolean;
  /** §3 has this on the detail; the card's "✓ in <project>" badge needs it per card. */
  imported_in: ImportRecord[];
}

/** One `asset_relationship` edge from this asset (DATA-MODEL §10), as the inspector lists it. */
export interface AssetLink {
  id: number;
  kind: "uses_texture" | "uses_material" | "uses_mesh" | "uses_audio" | "uses_script" | "variant_of" | "derived_from";
  /** Where the edge was read from: the caption under LINKED FILES. */
  source: "unity_meta" | "unity_prefab" | "fbx_binding" | "mtl_sidecar" | "inferred" | "manual";
  state: "resolved" | "missing" | "ignored";
  /** Resolved inside this asset's pack (travels with it) rather than across packs. */
  same_pack: boolean;
  to_asset_id: number | null;
  /** The linked asset's name, or the declared name when unresolved. */
  name: string;
  declared_path: string | null;
  /** 0 for a direct dependency, 1 for a dependency of the row above (a material's texture). */
  depth: 0 | 1;
}

export interface License {
  name: string;
  attribution?: string | null;
  url?: string | null;
}

export interface AssetDetail extends AssetSummary {
  description: string | null;
  categories: string[];
  /** The effective licence: the asset's own, else its pack's (DATA-MODEL §9). Null = none stated. */
  license: License | null;
  dependencies: { count: number; asset_ids: number[] };

  // ---- beyond §3 ----
  /** The Library Source this asset came from (DATA-MODEL §10): "Source · Synty · Forest Pack". */
  library: { id: number; name: string };
  /** The dependency edges with their state — §3's `dependencies` is ids only. */
  links: AssetLink[];
  /** Ids of the collections this asset is in (§3 `collections` is unspecified). */
  collection_ids: number[];
}

/** `list_libraries` (ENGINE-API §8.1). */
export interface Library {
  id: number;
  name: string;
  kind: string;
  asset_count: number;
}

export interface Collection {
  id: number;
  name: string;
  asset_ids: number[];
}

/** A trait from the user's trait library (UI-SPEC §9.1): the quick-add chips, a manual chip's colour. */
export interface TraitDef {
  id: number;
  name: string;
  scope: "any" | "model" | "sound" | "image";
  usage: string;
  /** A CSS colour from data. */
  color: string;
  icon: string;
}

/** The active import target (ENGINE-API §5). */
export interface ProjectRef {
  id: number;
  name: string;
  engine: string;
  /** Destination folder inside the project. */
  dest: string;
}

/** Settings → file-size labels (UI-SPEC §11.4): the KB boundaries of M, L and XL. */
export interface SizeThresholds {
  m: number;
  l: number;
  xl: number;
}
