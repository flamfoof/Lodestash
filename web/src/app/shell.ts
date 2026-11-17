// The shell's data: what the top bar and the rail show. Static props for now (plan P6 Task 6.5);
// the resource cache feeds them from /v1 in P6b.

import type { IconBox, IconName, IconSize, Status } from "../ui";

/** The eleven nav destinations, in FEATURES.md §0.3 order. Onboarding is an overlay, not a row. */
export type NavId =
  | "library"
  | "favorites"
  | "identify"
  | "collections"
  | "packages"
  | "unity"
  | "traits"
  | "stats"
  | "duplicates"
  | "trash"
  | "settings";

export const NAV: readonly { id: NavId; label: string; icon: IconName }[] = [
  { id: "library", label: "Library", icon: "nav-library" },
  { id: "favorites", label: "Favorites", icon: "nav-favorites" },
  { id: "identify", label: "Identify", icon: "nav-identify" },
  { id: "collections", label: "Collections", icon: "nav-collections" },
  { id: "packages", label: "Packages", icon: "nav-packages" },
  // FEATURES §0.3: Unity Assets carries the real Unity package mark, `ls-unitypackage`.
  { id: "unity", label: "Unity Assets", icon: "unitypackage" },
  { id: "traits", label: "Traits", icon: "nav-traits" },
  { id: "stats", label: "Stats", icon: "nav-stats" },
  { id: "duplicates", label: "Duplicates", icon: "nav-duplicates" },
  { id: "trash", label: "Trash", icon: "nav-trash" },
  { id: "settings", label: "Settings", icon: "nav-settings" },
];

/** Rows that carry a count badge (FEATURES §0.3); Favorites deliberately carries none. */
export type BadgeCounts = Partial<Record<"identify" | "duplicates" | "trash", number>>;

/** The category tabs: a presentation grouping over the 7-value media_type enum (FEATURES §0.2). */
export type Category = "all" | "model" | "material" | "sound" | "image";

export const CATEGORIES: readonly {
  value: Category;
  label: string;
  icon: IconName;
  iconBox: IconBox;
  iconSize?: IconSize;
  tone: "gold" | "model" | "material" | "sound" | "image";
}[] = [
  // Each icon keeps the advance of the glyph the prototype draws (◆ ⬡ ◇ ♪ ▦) — DEVIATIONS row 1.
  { value: "all", label: "All", icon: "type-all", iconBox: "13", tone: "gold" },
  { value: "model", label: "Models", icon: "type-model", iconBox: "9", tone: "model" },
  { value: "material", label: "Materials", icon: "type-material", iconBox: "12", iconSize: "11", tone: "material" },
  { value: "sound", label: "Sounds", icon: "type-sound", iconBox: "6", iconSize: "10", tone: "sound" },
  { value: "image", label: "Images", icon: "type-image", iconBox: "10", tone: "image" },
];

export type SearchMode = "tags" | "ai";

export interface Project {
  name: string;
  engine: string;
  /** A CSS colour (the project's own, from data). */
  color: string;
}

export type StatusFacet = "all" | Status;

export interface ShellData {
  route: NavId;
  badges: BadgeCounts;
  mode: SearchMode;
  query: string;
  category: Category;
  health: { identified: number; total: number };
  project: Project | null;
  /** The detected inference device: "RTX 4070", or "CPU" — never hard-coded (FEATURES §0.2). */
  device: string;
  status: { value: StatusFacet; counts: Record<StatusFacet, number> };
  libraries: { value: string; options: { id: string; label: string; count: number }[] };
}

export interface ShellEvents {
  onnavigate?: (id: NavId) => void;
  onmode?: (mode: SearchMode) => void;
  onquery?: (query: string) => void;
  oncategory?: (category: Category) => void;
  onstatus?: (value: StatusFacet) => void;
  onlibrary?: (id: string) => void;
  onaddsource?: () => void;
  onproject?: () => void;
}
