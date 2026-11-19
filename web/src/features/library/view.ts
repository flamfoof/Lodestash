// The Library's view state: how the result is ordered and drawn. It is the view's, not the
// catalog's — it persists with the app's preferences (DESIGN-GUIDELINES §7 "View / sort toolbar")
// and is handed to the screen as a prop; the screen emits the change it was asked for.

/** Grid size: the discrete stops of a zoom slider. Card min-widths 150 / 205 / 288 px. */
export type GridSize = "compact" | "cozy" | "large";

/** How the result is laid out. Folders (DESIGN-GUIDELINES "Browse views") joins with its view. */
export type Layout = "grid" | "list";

/**
 * The sort the toolbar offers. These are the prototype's five; direction is part of the choice
 * ("largest first" / "smallest first"), as the prototype has it. DESIGN-GUIDELINES §7's further
 * keys (type, date modified, date added, status) join when the engine's list operation takes a
 * sort — today none does (ENGINE-API §8.1 `list_assets` is id-ordered or random).
 */
export type SortKey = "default" | "name" | "sizeDesc" | "sizeAsc" | "tris";

export const SORT_OPTIONS: readonly { value: SortKey; label: string }[] = [
  { value: "default", label: "sort · default" },
  { value: "name", label: "sort · name A–Z" },
  { value: "sizeDesc", label: "sort · largest first" },
  { value: "sizeAsc", label: "sort · smallest first" },
  { value: "tris", label: "sort · tri count" },
];

export interface LibraryView {
  sort: SortKey;
  size: GridSize;
  layout: Layout;
  /** The ★ Favorites toggle: a one-click view switch, not a facet (FEATURES §1.3). */
  favoritesOnly: boolean;
}

export const DEFAULT_VIEW: LibraryView = { sort: "default", size: "cozy", layout: "grid", favoritesOnly: false };
