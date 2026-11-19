// The prototype states the contract covers. Each state starts from a fresh page load and runs its
// steps. `root` scopes the vocabulary walk; `screen` is the data-screen-label that must be visible
// once the steps ran (the extractor fails loudly if it is not — the canvas changed).
//
// The app gate (spec §10.4) drives the real app into the same named states by its own steps.

import { clickCss, clickText } from "./lib/prototype.mjs";

const openLibrary = (page) => clickText(page, "Open library →");
const nav = (label) => async (page) => {
  await openLibrary(page);
  await clickText(page, label);
};

const LIBRARY = '[data-screen-label="Library"]';
/** A card's ⤢, by the card's place in the prototype's order (0 Beaver_LowPoly … 18 Rock_Mossy_L). */
const quickLook = (index) => async (page) => {
  await openLibrary(page);
  await clickCss(page, `${LIBRARY} [title="Full-size preview"]`, index);
};

export const STATES = {
  onboarding: { screen: "First-run onboarding", steps: async () => {} },
  library: { screen: "Library", steps: openLibrary },
  // AI search mode recolours the search well and mode toggle (arcane rings); nothing else draws them.
  "library+ai": {
    screen: "Library",
    steps: async (page) => {
      await openLibrary(page);
      await clickText(page, "✦ AI");
    },
  },
  "library+inspector": {
    screen: "Inspector",
    steps: async (page) => {
      await openLibrary(page);
      await clickText(page, "Beaver_LowPoly");
    },
  },
  // ⚑ open with one value chosen in each group — rock ∧ ≤ 1k ∧ M leaves the two mossy rocks — so
  // the on and the off form of every chip is drawn and the button carries its count.
  "library+filters": {
    screen: "Library",
    steps: async (page) => {
      await openLibrary(page);
      await clickText(page, "⚑ Filters");
      await clickText(page, "rock"); // the popover's chip comes before any card's in the document
      await clickText(page, "≤ 1k");
      await clickText(page, "M"); // the size chip's letter; nothing else is a bare "M"
    },
  },
  // The first three cards picked: the bulk bar, and a pick box that is on.
  "library+picks": {
    screen: "Library",
    steps: async (page) => {
      await openLibrary(page);
      for (const i of [0, 1, 2]) await clickCss(page, `${LIBRARY} [title="Select for bulk actions"]`, i);
    },
  },
  "library+list": {
    screen: "Library",
    steps: async (page) => {
      await openLibrary(page);
      await clickText(page, "☰");
    },
  },
  // Quick Look per media type: a model (Beaver_LowPoly), a sound (rock_pickup_01), an image
  // (bark_oak_albedo). The ⤢ also selects the asset, so the inspector is open behind the scrim.
  quicklook: { screen: "Quick Look", steps: quickLook(0) },
  "quicklook+sound": { screen: "Quick Look", steps: quickLook(7) },
  "quicklook+image": { screen: "Quick Look", steps: quickLook(12) },
  // Two models and a sound staged. Each "+ stage" raises a 3.2 s toast: wait them out, so what
  // is measured does not depend on timing.
  staging: {
    screen: "Staging tray",
    steps: async (page) => {
      await openLibrary(page);
      await clickText(page, "+ stage", 0); // Beaver_LowPoly
      await clickText(page, "+ stage", 0); // Rock_Mossy_03 — the first card now reads "✓ staged"
      await clickText(page, "+ stage", 5); // rock_pickup_01
      await page.waitForTimeout(3400);
    },
  },
  favorites: { screen: "Favorites", steps: nav("Favorites") },
  identify: { screen: "Identify mode", steps: nav("Identify") },
  collections: { screen: "Collections", steps: nav("Collections") },
  packages: { screen: "Packages", steps: nav("Packages") },
  unity: { screen: "Unity Assets", steps: nav("Unity Assets") },
  traits: { screen: "Trait library", steps: nav("Traits") },
  stats: { screen: "Stats", steps: nav("Stats") },
  duplicates: { screen: "Duplicate review", steps: nav("Duplicates") },
  trash: { screen: "Trash", steps: nav("Trash") },
  settings: { screen: "Settings", steps: nav("Settings") },
};
