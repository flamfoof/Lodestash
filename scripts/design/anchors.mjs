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

const LIBRARY = '[data-screen-label="Library"]';
const INSPECTOR = '[data-screen-label="Inspector"]';

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
  "library.hint": { proto: { text: "Space quick-looks · ⤢ full size · ⟡ stage" }, compare: ["type", "color"] },
  // Scoped: the Library nav row draws the same ⊞.
  "library.view": { proto: { text: "⊞", within: LIBRARY, up: 1 }, compare: ["box", "pos", "fill", "edge", "space"] },
  "library.view.on": { proto: { text: "⊞", within: LIBRARY }, compare: ["box", "color", "fill", "edge", "space"] },
  "library.grid": { proto: { text: "Beaver_LowPoly", up: 5 }, compare: ["pos", "space"] },

  "card": { proto: { text: "Beaver_LowPoly", up: 4 }, compare: ["box", "fill", "edge"] },
  "card.thumb": { proto: { text: "Beaver_LowPoly", up: 4, child: 0 }, compare: ["box", "fill"] },
  "card.body": { proto: { text: "Beaver_LowPoly", up: 3 }, compare: ["box", "space"] },
  "card.type": { proto: { text: "Beaver_LowPoly", up: 2, child: 0 }, compare: ["box", "fill", "edge"] },
  "card.name": { proto: { text: "Beaver_LowPoly", up: 1 }, compare: ["type", "color"] },
  "card.meta": { proto: { text: "Forest Pack", up: 2 }, compare: ["box", "type", "color"] },
  "card.pick": { proto: { css: `${LIBRARY} [title="Select for bulk actions"]` }, compare: ["box", "pos", "fill", "edge"] },
  "card.quicklook": { proto: { css: `${LIBRARY} [title="Full-size preview"]` }, compare: ["box", "pos", "type", "color", "fill", "edge", "space"] },
  "card.imported": { proto: { text: "✓ in MyRPG", up: 1 }, compare: ["box", "type", "color", "fill", "edge", "space"] },
  "card.chip.lead": { proto: { text: "animal", up: 1 }, compare: ["box", "type", "color", "fill", "edge", "space"] },
  "card.chip": { proto: { text: "lowpoly", up: 1 }, compare: ["box", "type", "color", "fill", "edge", "space"] },
  "card.chip.more": { proto: { text: "+1", up: 1 }, compare: ["box", "type", "color", "fill", "edge", "space"] },
  "card.size": { proto: { text: "4.1 MB", up: 1 }, compare: ["box", "type", "color", "fill", "edge", "space"] },
  "card.stage": { proto: { text: "+ stage", up: 1 }, compare: ["box", "type", "color", "fill", "edge", "space"] },
  "card.favorite": { proto: { text: "☆", up: 1 }, compare: ["box", "type", "color", "fill", "edge"] },
  "card.status": { proto: { text: "◆", up: 1 }, compare: ["box", "type", "color", "fill", "edge"] },
};

// The inspector, with the prototype's first asset selected (Beaver_LowPoly: confirmed, imported,
// four traits, five linked files). `pos` only where it proves a rhythm: the panel, the name, the
// first stat row, the first link row and the action row — each is where it is only if everything
// above it has its height. The three library anchors are the ones the inspector's column moves.
const inInspector = (text, up = 0) => ({ text, within: INSPECTOR, ...(up ? { up } : {}) });
const inspector = {
  "library.toolbar": library["library.toolbar"],
  "library.grid": library["library.grid"],
  "card": library["card"],

  "inspector": { proto: { css: INSPECTOR }, compare: ["box", "pos", "fill"] },
  "inspector.preview": { proto: { css: INSPECTOR, child: 0 }, compare: ["box", "fill"] },
  "inspector.fullsize": { proto: inInspector("⤢ full size"), compare: ["box", "pos", "type", "color", "fill", "edge", "space"] },
  "inspector.close": { proto: inInspector("✕"), compare: ["box", "pos", "color", "fill", "edge"] },
  "inspector.body": { proto: { css: INSPECTOR, child: 1 }, compare: ["pos", "space"] },
  "inspector.name": { proto: inInspector("Beaver_LowPoly", 1), compare: ["pos", "type", "color"] },
  "inspector.status": { proto: inInspector("Confirmed", 2), compare: ["box", "type", "color", "space"] },
  "inspector.rule": { proto: { ...inInspector("Confirmed", 3), child: 2 }, compare: ["box", "fill", "edge"] },
  "inspector.stat": { proto: inInspector("Format", 2), compare: ["box", "pos", "type", "space"] },
  "inspector.stat.key": { proto: inInspector("Format", 1), compare: ["type", "color"] },
  "inspector.stat.value": { proto: inInspector("FBX", 1), compare: ["type", "color"] },
  "inspector.heading": { proto: inInspector("TRAITS"), compare: ["type", "color"] },
  "inspector.chip.lead": { proto: inInspector("animal", 1), compare: ["box", "type", "color", "fill", "edge", "space"] },
  "inspector.chip": { proto: inInspector("lowpoly", 1), compare: ["box", "type", "color", "fill", "edge", "space"] },
  "inspector.trait.add": { proto: inInspector("hero-prop", 1), compare: ["box", "type", "color", "edge", "space"] },
  "inspector.link": { proto: inInspector("M_Beaver_Body", 2), compare: ["box", "pos", "fill", "edge", "space"] },
  "inspector.link.name": { proto: inInspector("M_Beaver_Body", 1), compare: ["type", "color"] },
  "inspector.link.state": { proto: inInspector("✓ in pack", 1), compare: ["type", "color"] },
  "inspector.collection": { proto: inInspector("Dungeon tileset picks", 1), compare: ["box", "type", "color", "edge", "space"] },
  "inspector.provenance": { proto: inInspector("Source ·", 1), compare: ["box", "type", "color", "edge", "space"] },
  "inspector.stage": { proto: inInspector("⟡ Stage", 1), compare: ["box", "pos", "type", "color", "edge", "space"] },
  "inspector.import": { proto: inInspector("✓ Imported", 1), compare: ["box", "type", "color", "fill", "edge", "space"] },
  "inspector.reidentify": { proto: inInspector("✦ Re-identify"), compare: ["box", "pos", "type", "color", "edge", "space"] },
};

/** The shell's anchors (top bar and sidebar) — present in every state; the gate's shell specimen. */
export const FRAME_ANCHORS = Object.keys(frame);

/** Anchors per state. A state not listed here still contributes to the vocabulary. */
export const ANCHORS = {
  library: { ...frame, ...library },
  "library+inspector": inspector,
};
