// The prototype states the contract covers. Each state starts from a fresh page load and runs its
// steps. `root` scopes the vocabulary walk; `screen` is the data-screen-label that must be visible
// once the steps ran (the extractor fails loudly if it is not — the canvas changed).
//
// The app gate (spec §10.4) drives the real app into the same named states by its own steps.

import { clickText } from "./lib/prototype.mjs";

const openLibrary = (page) => clickText(page, "Open library →");
const nav = (label) => async (page) => {
  await openLibrary(page);
  await clickText(page, label);
};

export const STATES = {
  onboarding: { screen: "First-run onboarding", steps: async () => {} },
  library: { screen: "Library", steps: openLibrary },
  "library+inspector": {
    screen: "Inspector",
    steps: async (page) => {
      await openLibrary(page);
      await clickText(page, "Beaver_LowPoly");
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
