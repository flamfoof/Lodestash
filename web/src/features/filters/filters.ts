// The ⚑ content filter (UI-SPEC §2 item 3, FEATURES §1.4, DESIGN-GUIDELINES §7): three
// multi-select groups — trait, triangles, file size — that compose on top of the search box, the
// type tabs and the sidebar facets.
//
// Everything here is pure. `passes` is the predicate the docs ask for; the specimen pages and the
// tests filter with it, and it states exactly what the engine's library query must answer once
// the filter travels with the request (plan P6b). The filter set itself is a value the screen is
// handed and reports changes to — never state kept here.

import type { Tier } from "../../ui";
import type { AssetSummary, SizeThresholds } from "../asset/model";
import { formatBytes, sizeTier } from "../asset/present";

export type TriBucket = "low" | "mid" | "high" | "ultra";

/** The triangle buckets, in order. Upper bounds are inclusive: 1 000 tris is `≤ 1k`. */
export const TRI_BUCKETS: readonly { id: TriBucket; label: string; max: number }[] = [
  { id: "low", label: "≤ 1k", max: 1000 },
  { id: "mid", label: "1k–3k", max: 3000 },
  { id: "high", label: "3k–6k", max: 6000 },
  { id: "ultra", label: "6k +", max: Infinity },
];

export const SIZE_TIERS: readonly Tier[] = ["S", "M", "L", "XL"];

export interface LibraryFilters {
  /** Tag values; an asset passes with any one of them. */
  readonly traits: readonly string[];
  readonly tris: readonly TriBucket[];
  readonly sizes: readonly Tier[];
}

export type FilterGroup = keyof LibraryFilters;

export const NO_FILTERS: LibraryFilters = Object.freeze({ traits: [], tris: [], sizes: [] });

/** Put `value` in `group`, or take it out. Returns a new set; the one given is not changed. */
export function toggleFilter<G extends FilterGroup>(filters: LibraryFilters, group: G, value: LibraryFilters[G][number]): LibraryFilters {
  const current = filters[group] as readonly unknown[];
  const next = current.includes(value) ? current.filter((v) => v !== value) : [...current, value];
  return { ...filters, [group]: next };
}

/** How many values are chosen — the count on the ⚑ button. */
export const filterCount = (filters: LibraryFilters): number => filters.traits.length + filters.tris.length + filters.sizes.length;

/** The bucket a triangle count falls in; null when the count is not known (DATA-MODEL §14: unknown is not zero). */
export function triBucket(tris: number | undefined): TriBucket | null {
  if (tris === undefined || !(tris > 0)) return null;
  return TRI_BUCKETS.find((b) => tris <= b.max)!.id;
}

/**
 * Does `asset` pass the ⚑ filters? Within a group any chosen value is enough (OR); across groups
 * every active group must be satisfied (AND); a group with nothing chosen does not constrain.
 * TRIANGLES is about models: with it active, anything without a triangle count — a sound, an
 * image, a model not measured yet — does not pass.
 */
export function passes(asset: AssetSummary, filters: LibraryFilters, thresholds: SizeThresholds): boolean {
  if (filters.traits.length > 0 && !asset.tags.some((tag) => filters.traits.includes(tag.value))) return false;
  if (filters.sizes.length > 0 && !filters.sizes.includes(sizeTier(asset.size_bytes, thresholds))) return false;
  if (filters.tris.length > 0) {
    const bucket = asset.media_type === "model_3d" ? triBucket(asset.stats.tris) : null;
    if (bucket === null || !filters.tris.includes(bucket)) return false;
  }
  return true;
}

export interface TraitCount {
  name: string;
  /** Assets carrying it. */
  count: number;
}

/**
 * The tags on `assets`, most frequent first; equally frequent ones keep the order they were first
 * seen in. The specimen's stand-in for the engine's trait facet — the popover is handed the list.
 */
export function traitCounts(assets: readonly AssetSummary[]): TraitCount[] {
  const counts = new Map<string, number>();
  for (const asset of assets) for (const tag of asset.tags) counts.set(tag.value, (counts.get(tag.value) ?? 0) + 1);
  return [...counts].map(([name, count]) => ({ name, count })).sort((a, b) => b.count - a.count);
}

/**
 * Each size tier's live range at the user's thresholds (UI-SPEC §11.4): "0–1.0 MB",
 * "1.0 MB–4.0 MB", … XL has no upper bound, so it says so.
 */
export function sizeRanges(thresholds: SizeThresholds): Record<Tier, string> {
  const at = (kb: number) => formatBytes(kb * 1024);
  return {
    S: `0–${at(thresholds.m)}`,
    M: `${at(thresholds.m)}–${at(thresholds.l)}`,
    L: `${at(thresholds.l)}–${at(thresholds.xl)}`,
    XL: `${at(thresholds.xl)}+`,
  };
}
