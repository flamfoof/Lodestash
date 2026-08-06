/**
 * The screen enum, verbatim from FEATURES.md §0.1:
 *   onboarding | library | favorites | identify | collections | packages | unity
 *   | traits | stats | dups | trash | settings
 *
 * Twelve screens; `onboarding` is an overlay on the library body, not a nav row, so the
 * rail renders eleven destinations.
 */
export const ROUTES = [
  "onboarding",
  "library",
  "favorites",
  "identify",
  "collections",
  "packages",
  "unity",
  "traits",
  "stats",
  "dups",
  "trash",
  "settings",
] as const;

export type Route = (typeof ROUTES)[number];

export type NavItem = {
  route: Route;
  label: string;
  /** Symbol id in /icons.svg. */
  icon: string;
  /** Which count, if any, badges this row. */
  badge?: "unidentified" | "dups" | "trashed";
};

/** Sidebar order per FEATURES §0.3 — Favorites sits second, directly under Library. */
export const NAV: NavItem[] = [
  { route: "library", label: "Library", icon: "ls-nav-library" },
  { route: "favorites", label: "Favorites", icon: "ls-nav-favorites" },
  { route: "identify", label: "Identify", icon: "ls-nav-identify", badge: "unidentified" },
  { route: "collections", label: "Collections", icon: "ls-nav-collections" },
  { route: "packages", label: "Packages", icon: "ls-nav-packages" },
  { route: "unity", label: "Unity Assets", icon: "ls-unitypackage" },
  { route: "traits", label: "Traits", icon: "ls-nav-traits" },
  { route: "stats", label: "Stats", icon: "ls-nav-stats" },
  { route: "dups", label: "Duplicates", icon: "ls-nav-duplicates", badge: "dups" },
  { route: "trash", label: "Trash", icon: "ls-nav-trash", badge: "trashed" },
  { route: "settings", label: "Settings", icon: "ls-nav-settings" },
];

/**
 * Title + owning milestone for each screen. Stubs are table-driven rather than twelve
 * near-identical .svelte files; a route graduates to its own component when its milestone
 * gives it real content, and this table keeps only the title.
 */
export const SCREENS: Record<Route, { title: string; milestone: string }> = {
  onboarding: { title: "Welcome", milestone: "B6" },
  library: { title: "Library", milestone: "B3" },
  favorites: { title: "Favorites", milestone: "B3" },
  identify: { title: "Identify", milestone: "B6" },
  collections: { title: "Collections", milestone: "B6" },
  packages: { title: "Packages", milestone: "B6" },
  unity: { title: "Unity Assets", milestone: "B6" },
  traits: { title: "Traits", milestone: "B6" },
  stats: { title: "Stats", milestone: "B6" },
  dups: { title: "Duplicates", milestone: "B6" },
  trash: { title: "Trash", milestone: "B6" },
  settings: { title: "Settings", milestone: "B5" },
};

/** Category tabs (FEATURES §0.2) — a presentation grouping over the 7-value media_type enum. */
export const TABS = [
  { id: "all", label: "All", icon: "ls-type-all", token: "--ls-gold", media: [] },
  { id: "models", label: "Models", icon: "ls-type-model", token: "--ls-type-model", media: ["model_3d"] },
  { id: "materials", label: "Materials", icon: "ls-type-material", token: "--ls-type-material", media: ["material"] },
  { id: "sounds", label: "Sounds", icon: "ls-type-sound", token: "--ls-type-sound", media: ["audio"] },
  { id: "images", label: "Images", icon: "ls-type-image", token: "--ls-type-image", media: ["image", "texture", "sprite", "font"] },
] as const;

export type TabId = (typeof TABS)[number]["id"];
