// Pure presentation of asset data: every string and category the card and the inspector show is
// derived here from the model, so two places can never format the same fact two ways.

import type { Status, Tier } from "../../ui";
import type { AssetDetail, AssetLink, AssetStatus, AssetSummary, AssetTag, MediaType, ProjectRef, SizeThresholds, TraitDef } from "./model";

/** The three statuses a person sees (DATA-MODEL §8: discovered and indexed are Unidentified). */
export function userStatus(status: AssetStatus): Status {
  return status === "confirmed" ? "confirmed" : status === "identified" ? "identified" : "unidentified";
}

export type TypeTone = "model" | "sound" | "image" | "texture" | "material" | "sprite" | "font";

const TYPE: Record<MediaType, { tone: TypeTone; label: string }> = {
  model_3d: { tone: "model", label: "Model" },
  audio: { tone: "sound", label: "Sound" },
  image: { tone: "image", label: "Image" },
  texture: { tone: "texture", label: "Texture" },
  material: { tone: "material", label: "Material" },
  sprite: { tone: "sprite", label: "Sprite" },
  font: { tone: "font", label: "Font" },
};

export const typeTone = (media: MediaType): TypeTone => TYPE[media].tone;
export const typeLabel = (media: MediaType): string => TYPE[media].label;

const MB = 1024 * 1024;

/** "4.1 MB", "922 KB", "1.3 GB" — one decimal above a megabyte, whole kilobytes below. */
export function formatBytes(bytes: number): string {
  const mb = bytes / MB;
  if (mb >= 1024) return `${(mb / 1024).toFixed(1)} GB`;
  if (mb >= 1) return `${mb.toFixed(1)} MB`;
  return `${Math.round(mb * 1024)} KB`;
}

/** The size tier at the user's thresholds (UI-SPEC §11.4). */
export function sizeTier(bytes: number, t: SizeThresholds): Tier {
  const kb = bytes / 1024;
  return kb < t.m ? "S" : kb < t.l ? "M" : kb < t.xl ? "L" : "XL";
}

const compact = (n: number) => (n >= 1000 ? `${(n / 1000).toFixed(1)}k` : String(n));
const grouped = (n: number) => n.toLocaleString("en-US");
const kHz = (hz: number) => `${Number((hz / 1000).toFixed(1))} kHz`;
const pad2 = (n: number) => String(n).padStart(2, "0");

/** "1:24", "0:01"; under a second the tenths are the information: "0:00.8". */
function shortDuration(s: number): string {
  if (s < 1) return `0:00.${Math.round(s * 10)}`;
  return `${Math.floor(s / 60)}:${pad2(Math.floor(s % 60))}`;
}

/** As above, keeping the tenths for anything under a minute: "0:01.2". */
function longDuration(s: number): string {
  if (s >= 60) return shortDuration(s);
  const tenths = Math.round(s * 10);
  return `0:${pad2(Math.floor(tenths / 10))}.${tenths % 10}`;
}

const isPicture = (m: MediaType) => m === "image" || m === "texture" || m === "sprite";

/** The card's one-line stats: what distinguishes this asset among its neighbours. */
export function statLine(asset: AssetSummary): string {
  const { stats } = asset;
  const size = formatBytes(asset.size_bytes);
  if (asset.media_type === "model_3d" && stats.tris !== undefined) return `${compact(stats.tris)} tris · ${size}`;
  if (asset.media_type === "audio" && stats.duration_s !== undefined) {
    const rate = stats.sample_rate_hz === undefined ? "" : ` · ${kHz(stats.sample_rate_hz)}`;
    return `${shortDuration(stats.duration_s)}${rate}`;
  }
  if (isPicture(asset.media_type) && stats.width !== undefined && stats.height !== undefined) {
    const dims = stats.width === stats.height ? `${stats.width}²` : `${stats.width}×${stats.height}`;
    return `${dims} · ${size}`;
  }
  return size;
}

/** The stat block's rows, per modality (UI-SPEC §13.4). A metric that is not known has no row. */
export function statRows(asset: AssetSummary): [key: string, value: string][] {
  const { stats } = asset;
  const rows: [string, string][] = [["Format", asset.format.toUpperCase()]];
  if (asset.media_type === "audio") {
    if (stats.duration_s !== undefined) rows.push(["Duration", longDuration(stats.duration_s)]);
    if (stats.sample_rate_hz !== undefined) rows.push(["Sample rate", kHz(stats.sample_rate_hz)]);
    if (stats.channels !== undefined) rows.push(["Channels", stats.channels === 1 ? "mono" : stats.channels === 2 ? "stereo" : String(stats.channels)]);
  } else if (isPicture(asset.media_type)) {
    if (stats.width !== undefined && stats.height !== undefined) rows.push(["Resolution", `${stats.width} × ${stats.height}`]);
    if (stats.pixel_format) rows.push(["Channels", stats.pixel_format]);
    if (stats.color_space) rows.push(["Color space", stats.color_space]);
  } else {
    if (stats.tris !== undefined) rows.push(["Tris", grouped(stats.tris)]);
    if (stats.materials !== undefined) rows.push(["Materials", String(stats.materials)]);
    if (stats.bones !== undefined) rows.push(["Rig", stats.bones === 0 ? "none" : `${stats.bones} bones`]);
  }
  rows.push(["Size", formatBytes(asset.size_bytes)]);
  return rows;
}

export interface ChipView {
  name: string;
  source: "human" | "ai" | "imported";
  /** 0–1, machine tags only. */
  confidence?: number;
  /** A library trait's own colour and mark, when a manual tag is one of the user's traits. */
  color?: string;
  icon?: string;
}

/** How a tag is drawn: by where it came from (DESIGN-GUIDELINES §7 "Tag / trait chips"). */
export function chipOf(tag: AssetTag, traits: readonly TraitDef[] = []): ChipView {
  if (tag.source.startsWith("ai")) return { name: tag.value, source: "ai", confidence: tag.confidence ?? undefined };
  if (tag.source === "imported") return { name: tag.value, source: "imported" };
  const trait = traits.find((t) => t.name === tag.value);
  return { name: tag.value, source: "human", color: trait?.color, icon: trait?.icon };
}

export type LinkState = "in_pack" | "in_library" | "missing" | "ignored";

/** The four row states of LINKED FILES (UI-SPEC §13.6). */
export function linkState(link: AssetLink): LinkState {
  if (link.state === "ignored") return "ignored";
  if (link.state === "missing" || link.to_asset_id === null) return "missing";
  return link.same_pack ? "in_pack" : "in_library";
}

const VIA: Record<AssetLink["source"], string> = {
  fbx_binding: "FBX bindings",
  mtl_sidecar: ".mtl sidecar",
  unity_meta: ".meta GUID refs",
  unity_prefab: "prefab bindings",
  inferred: "name matches",
  manual: "your links",
};

/** The caption beside LINKED FILES: where the edges were read from. */
export function linksCaption(links: readonly AssetLink[]): string {
  const sources = [...new Set(links.map((l) => VIA[l.source]))];
  return sources.length ? `parsed from ${sources.join(" · ")}` : "";
}

export interface ImportedView {
  /** `active`: in the open project. `others`: only in other projects — history, not a warning. */
  tone: "active" | "others";
  label: string;
  /** The inspector's line, for the active project. */
  line?: string;
}

/** The "already imported" badge (DESIGN-GUIDELINES §7). Null when never imported: no badge. */
export function importedState(asset: Pick<AssetSummary, "imported_in">, project: ProjectRef | null): ImportedView | null {
  if (asset.imported_in.length === 0) return null;
  const here = project ? asset.imported_in.find((r) => r.project_id === project.id) : undefined;
  if (project && here) {
    return { tone: "active", label: `in ${project.name}`, line: `imported in ${project.name} → ${here.dest_path}` };
  }
  const n = asset.imported_in.length;
  return { tone: "others", label: `in ${n} other${n === 1 ? "" : "s"}` };
}

/** Traits from the library that fit this asset and are not on it yet: the quick-add chips. */
export function quickAddTraits(asset: AssetDetail, traits: readonly TraitDef[]): TraitDef[] {
  const tone = typeTone(asset.media_type);
  const scope = tone === "model" || tone === "sound" ? tone : isPicture(asset.media_type) ? "image" : null;
  return traits.filter((t) => (t.scope === "any" || t.scope === scope) && !asset.tags.some((x) => x.value === t.name));
}
