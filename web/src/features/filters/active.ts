// The active-filter bar's model (UI-SPEC §2 item 7): every constraint on the result as one entry,
// in the order the filters compose. The filters live in several places — the search box, the type
// tabs, two sidebar facet groups, the ⚑ popover, the ★ toggle — and this is the one list of them.
//
// Pure: it describes what is active. Removing an entry is reported to whoever owns that axis, so
// there is exactly one source of truth per axis and the bar holds none of them.

import type { Status, Tier } from "../../ui";
import { STATUS_LABEL } from "../../ui";
import type { SizeThresholds } from "../asset/model";
import { SIZE_TIERS, sizeRanges, TRI_BUCKETS, type LibraryFilters } from "./filters";

/** The axes a result can be constrained on, in composition order. */
export type FilterAxis = "query" | "type" | "status" | "library" | "pack" | "collection" | "trait" | "tris" | "size" | "favorite";

/** The axes the shell owns: removing one of these is the shell's to carry out. */
export type ShellAxis = "query" | "type" | "status" | "library" | "pack" | "collection";

/** A category tab other than "All" (FEATURES §0.2). */
export type CategoryId = "model" | "material" | "sound" | "image";

/**
 * What the shell contributes: the search box, the type tabs, the sidebar facets, and the pack or
 * collection a link narrowed the library to. An absent field does not constrain.
 */
export interface Constraints {
  query?: string;
  category?: { id: CategoryId; label: string };
  status?: Status;
  library?: { id: string | number; name: string };
  pack?: { id: number; name: string };
  collection?: { id: number; name: string };
}

export interface ActiveFilter {
  /** Unique within the list. */
  key: string;
  axis: FilterAxis;
  /** The value within its axis: a category, a status, a trait, a bucket, a tier. */
  value?: string;
  label: string;
  /** A mono axis prefix, where the label alone would not say which axis it is on. */
  prefix?: string;
}

/** Every active constraint: `query · type · status · library · pack · collection · ⚑ · ★`. */
export function activeFilters(constraints: Constraints, filters: LibraryFilters, favoritesOnly: boolean, thresholds: SizeThresholds): ActiveFilter[] {
  const out: ActiveFilter[] = [];
  const query = constraints.query?.trim();
  if (query) out.push({ key: "query", axis: "query", label: query });
  if (constraints.category) out.push({ key: "type", axis: "type", value: constraints.category.id, label: constraints.category.label });
  if (constraints.status) out.push({ key: "status", axis: "status", value: constraints.status, label: STATUS_LABEL[constraints.status] });
  if (constraints.library) out.push({ key: "library", axis: "library", label: constraints.library.name, prefix: "lib" });
  if (constraints.pack) out.push({ key: "pack", axis: "pack", label: constraints.pack.name, prefix: "pack" });
  if (constraints.collection) out.push({ key: "collection", axis: "collection", label: constraints.collection.name, prefix: "coll" });

  for (const trait of filters.traits) out.push({ key: `trait:${trait}`, axis: "trait", value: trait, label: trait });
  // The ⚑ groups are listed in the popover's order, not the order they were clicked in.
  for (const bucket of TRI_BUCKETS) {
    if (filters.tris.includes(bucket.id)) out.push({ key: `tris:${bucket.id}`, axis: "tris", value: bucket.id, label: `${bucket.label} tris` });
  }
  const ranges = sizeRanges(thresholds);
  for (const tier of SIZE_TIERS) {
    // The live range rides along, so the letter is never a mystery.
    if (filters.sizes.includes(tier)) out.push({ key: `size:${tier}`, axis: "size", value: tier, label: `${tier} ${ranges[tier as Tier]}` });
  }
  if (favoritesOnly) out.push({ key: "favorite", axis: "favorite", label: "Favorites" });
  return out;
}

export const isShellAxis = (axis: FilterAxis): axis is ShellAxis =>
  axis === "query" || axis === "type" || axis === "status" || axis === "library" || axis === "pack" || axis === "collection";
