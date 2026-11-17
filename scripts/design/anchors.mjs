// Named anchors: the elements whose geometry, type and colour the app must reproduce.
//
// `proto` locates the element in the prototype (see lib/measure.mjs for the locator forms); the
// app marks the same element with `data-anchor="<name>"`. `compare` lists what the gate checks:
//   box   → w and h (±1 px)          pos → x and y (±1 px)
//   type  → fontFamily, fontSize, fontWeight, letterSpacing, textTransform (exact)
//   color → color (exact)            fill → background (+ backgroundImage) (exact)
//   edge  → borderWidth, borderColor, radius (exact)
//   space → padding, gap (±0.5 px)
// Choose what makes the anchor *recognisable*: a container's box and fill, a label's type and
// colour. Over-specifying (positions of everything) turns every layout fix into a contract edit.
//
// Add anchors per screen as the screen is rebuilt (plan P7); each state lists only its own.

const frame = {
  "topbar": { proto: { text: "LODESTASH", up: 2 }, compare: ["box", "pos", "fill", "edge", "space"] },
  "topbar.wordmark": { proto: { text: "LODESTASH" }, compare: ["type", "color"] },
  "topbar.mode": { proto: { text: "⟡ Tags", up: 1 }, compare: ["box", "fill", "edge", "space"] },
  "topbar.mode.on": { proto: { text: "⟡ Tags" }, compare: ["box", "type", "color", "fill", "edge", "space"] },
  "topbar.mode.off": { proto: { text: "✦ AI" }, compare: ["type", "color", "fill", "space"] },
  "topbar.search": { proto: { placeholder: "filter by tag, name, or pack…", up: 1 }, compare: ["box", "pos", "fill", "edge", "space"] },
  "topbar.category.on": { proto: { text: "◆ All", up: 1 }, compare: ["box", "type", "color", "fill", "edge", "space"] },
  "topbar.category": { proto: { text: "⬡ Models", up: 1 }, compare: ["type", "color", "fill", "edge", "space"] },
  "topbar.meter": { proto: { text: "Identified", up: 2 }, compare: ["box"] },
  "topbar.meter.label": { proto: { text: "Identified" }, compare: ["type", "color"] },
  "topbar.project": { proto: { text: "MyRPG · Unity 6", up: 2 }, compare: ["box", "type", "fill", "edge", "space"] },
  "topbar.hardware": { proto: { text: "✦ local · RTX 4070" }, compare: ["type", "color", "fill", "edge", "space"] },

  "sidebar": { proto: { text: "Collections", up: 3 }, compare: ["box", "pos", "fill", "edge", "space"] },
  "sidebar.nav.on": { proto: { text: "Library", up: 2 }, compare: ["box", "type", "color", "fill", "edge", "space"] },
  "sidebar.nav": { proto: { text: "Collections", up: 2 }, compare: ["box", "type", "color", "fill", "space"] },
  "sidebar.nav.badge": { proto: { text: "5", up: 1 }, compare: ["box", "type", "color", "fill", "edge", "space"] },
  "sidebar.heading": { proto: { text: "STATUS" }, compare: ["type", "color"] },
  "sidebar.facet": { proto: { text: "Unidentified", up: 2 }, compare: ["box", "type", "color", "fill", "space"] },
  "sidebar.addSource": { proto: { text: "Add source…", up: 1 }, compare: ["box", "type", "color", "edge", "space"] },
};

const library = {
  "library.banner": { proto: { text: "Open Identify mode →", up: 1 }, compare: ["box", "fill", "space"] },
  "library.banner.action": { proto: { text: "Open Identify mode →" }, compare: ["type", "color"] },
  "library.toolbar": { proto: { text: "19 assets · all libraries", up: 2 }, compare: ["box", "space"] },
  "library.toolbar.count": { proto: { text: "19 assets · all libraries" }, compare: ["type", "color"] },
  "library.sort": { proto: { css: '[data-screen-label="Library"] select' }, compare: ["box", "type", "color", "fill", "edge", "space"] },
  "library.favorites": { proto: { text: "★ Favorites" }, compare: ["box", "type", "color", "edge", "space"] },
  "library.filters": { proto: { text: "⚑ Filters" }, compare: ["box", "type", "color", "edge", "space"] },
  "library.density": { proto: { text: "▦", up: 2 }, compare: ["box", "fill", "edge", "space"] },
  "library.density.on": { proto: { text: "▦", up: 1 }, compare: ["box", "color", "fill", "edge", "space"] },
  "library.grid": { proto: { text: "Beaver_LowPoly", up: 5 }, compare: ["pos", "space"] },

  "card": { proto: { text: "Beaver_LowPoly", up: 4 }, compare: ["box", "fill", "edge"] },
  "card.thumb": { proto: { text: "Beaver_LowPoly", up: 4, child: 0 }, compare: ["box", "fill"] },
  "card.body": { proto: { text: "Beaver_LowPoly", up: 3 }, compare: ["box", "space"] },
  "card.name": { proto: { text: "Beaver_LowPoly", up: 1 }, compare: ["type", "color"] },
  "card.meta": { proto: { text: "Forest Pack", up: 2 }, compare: ["box", "type", "color"] },
  "card.chip.lead": { proto: { text: "animal", up: 1 }, compare: ["box", "type", "color", "fill", "edge", "space"] },
  "card.chip": { proto: { text: "lowpoly", up: 1 }, compare: ["box", "type", "color", "fill", "edge", "space"] },
  "card.chip.more": { proto: { text: "+1", up: 1 }, compare: ["box", "type", "color", "fill", "edge", "space"] },
  "card.size": { proto: { text: "4.1 MB", up: 1 }, compare: ["box", "type", "color", "fill", "edge", "space"] },
  "card.stage": { proto: { text: "+ stage", up: 1 }, compare: ["box", "type", "color", "fill", "edge", "space"] },
  "card.favorite": { proto: { text: "☆", up: 1 }, compare: ["box", "type", "color", "fill", "edge"] },
  "card.status": { proto: { text: "◆", up: 1 }, compare: ["box", "type", "color", "fill", "edge"] },
};

/** The shell's anchors (top bar and sidebar) — present in every state; the gate's shell specimen. */
export const FRAME_ANCHORS = Object.keys(frame);

/** Anchors per state. A state not listed here still contributes to the vocabulary. */
export const ANCHORS = {
  library: { ...frame, ...library },
};
