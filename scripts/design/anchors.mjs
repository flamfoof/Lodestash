// Named anchors: the elements whose geometry, type and colour the app must reproduce.
//
// `proto` locates the element in the prototype (see lib/measure.mjs for the locator forms); the
// app marks the same element with `data-anchor="<name>"`. `compare` lists what the gate checks:
//   box   → w and h (±1 px)          pos → x and y (±1 px)
//   type  → fontFamily, fontSize, fontWeight, letterSpacing, textTransform (exact)
//   color → color (exact)            fill → background (+ backgroundImage) (exact)
//   edge  → borderWidth, borderColor, radius (exact; 99 px and up is one "fully rounded")
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

// ---- the Library's overlays (plan P7b) ----
//
// Interpolated text sits in a <span> of its own in the prototype, so a chip or a button is one hop
// up from its text; static text ("+ Add tag") is the element's own.

const QUICKLOOK = '[data-screen-label="Quick Look"]';
const TRAY = '[data-screen-label="Staging tray"]';
const inLibrary = (text, up = 0) => ({ text, within: LIBRARY, ...(up ? { up } : {}) });
const inQuickLook = (text, up = 0) => ({ text, within: QUICKLOOK, ...(up ? { up } : {}) });
const inTray = (text, up = 0) => ({ text, within: TRAY, ...(up ? { up } : {}) });
const control = ["box", "type", "color", "fill", "edge", "space"];

// ⚑ Filters open, rock ∧ ≤ 1k ∧ M chosen. The popover hangs from the button's right edge; the
// first chip of each group proves the rhythm of the groups above it. The search field's type is
// compared too: the prototype never styled it (Arial), which is a DEVIATIONS row, not a blind spot.
const filters = {
  "library.toolbar.count": { proto: inLibrary("2 assets · all libraries"), compare: ["type", "color"] },
  "library.filters": { proto: inLibrary("⚑ Filters"), compare: ["pos", ...control] },
  "library.filters.count": { proto: { ...inLibrary("⚑ Filters"), child: 0 }, compare: control },
  "filters": { proto: inLibrary("FILTER LIBRARY", 2), compare: ["box", "pos", "fill", "edge", "space"] },
  "filters.title": { proto: inLibrary("FILTER LIBRARY"), compare: ["pos", "type", "color"] },
  "filters.clear": { proto: inLibrary("Clear all"), compare: ["pos", "type", "color"] },
  "filters.heading": { proto: inLibrary("TRAIT"), compare: ["pos", "type", "color"] },
  "filters.heading.note": { proto: inLibrary("· models only"), compare: ["type", "color"] },
  "filters.search": { proto: { placeholder: "search traits…" }, compare: ["pos", ...control] },
  "filters.trait.on": { proto: inLibrary("rock", 1), compare: ["pos", ...control] },
  "filters.trait": { proto: inLibrary("moss", 1), compare: ["pos", ...control] },
  "filters.tris.on": { proto: inLibrary("≤ 1k", 1), compare: ["pos", ...control] },
  "filters.tris": { proto: inLibrary("1k–3k", 1), compare: control },
  "filters.size": { proto: inLibrary("S", 2), compare: ["pos", ...control] },
  "filters.size.on": { proto: inLibrary("M", 2), compare: control },
  "filters.size.tier": { proto: inLibrary("S", 1), compare: ["type", "color"] },
  "filters.size.range": { proto: inLibrary("0–1.0 MB", 1), compare: ["type", "color"] },
};

// Three cards picked: the bulk bar between the toolbar and the grid, and a pick box that is on.
// The bar's hairline is its bottom edge, which the ruler does not read — its height carries it.
const picks = {
  "bulk": { proto: inLibrary("3 selected", 2), compare: ["box", "pos", "fill", "space"] },
  "bulk.count": { proto: inLibrary("3 selected", 1), compare: ["pos", "type", "color"] },
  "bulk.tag": { proto: { placeholder: "tag name…" }, compare: ["pos", ...control] },
  "bulk.add": { proto: inLibrary("+ Add tag"), compare: ["pos", ...control] },
  "bulk.remove": { proto: inLibrary("− Remove tag"), compare: control },
  "bulk.stage": { proto: inLibrary("⟡ Stage all"), compare: ["pos", ...control] },
  "bulk.clear": { proto: inLibrary("✕ Clear"), compare: ["box", "pos", "type", "color"] },
  "card.pick.on": { proto: library["card.pick"].proto, compare: ["box", "pos", "fill", "edge"] },
  "library.grid": library["library.grid"],
};

// The ☰ stop: rows in a hairline frame. The first row's parts carry positions — the columns are
// fixed widths, so each x proves the ones before it; the second row proves the zebra and the pitch.
const firstRow = (extra) => ({ ...inLibrary("Beaver_LowPoly", 2), ...extra });
const list = {
  "library.toolbar": library["library.toolbar"],
  "library.view": library["library.view"],
  "library.view.on": { proto: inLibrary("☰"), compare: ["box", "pos", "color", "fill", "edge", "space"] },
  "list": { proto: inLibrary("Beaver_LowPoly", 3), compare: ["box", "pos", "edge"] },
  "row": { proto: firstRow(), compare: ["box", "pos", "fill", "space"] },
  "row.alt": { proto: inLibrary("Rock_Mossy_03", 2), compare: ["box", "pos", "fill"] },
  "row.thumb": { proto: firstRow({ child: 0 }), compare: ["box", "pos", "fill", "edge"] },
  "row.type": { proto: firstRow({ child: 1 }), compare: ["box", "pos", "fill", "edge"] },
  "row.name": { proto: inLibrary("Beaver_LowPoly", 1), compare: ["box", "pos", "type", "color"] },
  "row.name.unknown": { proto: inLibrary("SM_Asset_0117", 1), compare: ["type", "color"] },
  "row.pack": { proto: inLibrary("Forest Pack", 1), compare: ["box", "pos", "type", "color"] },
  "row.size": { proto: inLibrary("4.1 MB", 1), compare: ["box", "pos", "type", "color"] },
  "row.stats": { proto: inLibrary("1.2k tris · 4.1 MB", 1), compare: ["box", "pos", "type", "color"] },
  "row.status": { proto: inLibrary("Confirmed", 1), compare: ["box", "pos", "type", "color"] },
  "row.favorite": { proto: inLibrary("☆", 1), compare: ["pos", ...control] },
  "row.quicklook": { proto: { css: `${LIBRARY} [title="Full-size preview"]` }, compare: ["pos", ...control] },
  "row.stage": { proto: inLibrary("+ stage", 1), compare: ["pos", ...control] },
};

// Quick Look. The panel is centred in the window whatever is behind it, so every position here is
// the dialog's own layout. `peek` is what all three states share; each adds what its media type draws.
const peek = (name, typeLabel, lead) => ({
  "quicklook": { proto: { css: QUICKLOOK, child: 0 }, compare: ["box", "pos", "fill", "edge"] },
  "quicklook.preview": { proto: { css: `${QUICKLOOK} > div > div` }, compare: ["box", "pos", "fill"] },
  "quicklook.body": { proto: { css: `${QUICKLOOK} > div > div`, nth: 1 }, compare: ["box", "pos", "space"] },
  "quicklook.name": { proto: inQuickLook(name, 1), compare: ["pos", "type", "color"] },
  "quicklook.status": { proto: inQuickLook("Confirmed", 2), compare: ["box", "pos", "type", "color", "space"] },
  "quicklook.status.type": { proto: inQuickLook(typeLabel, 1), compare: ["type", "color"] },
  "quicklook.stat": { proto: inQuickLook("Format", 2), compare: ["box", "pos", "type", "space"] },
  "quicklook.chip.lead": { proto: inQuickLook(lead, 1), compare: ["pos", ...control] },
  "quicklook.stage": { proto: inQuickLook("⟡ Stage", 1), compare: ["box", "pos", "type", "color", "edge", "space"] },
  "quicklook.import": { proto: inQuickLook("✓ Imported", 1), compare: ["pos", ...control] },
});
const quicklook = {
  ...peek("Beaver_LowPoly", "Model — low-res pre-render", "animal"),
  "quicklook.scrim": { proto: { css: QUICKLOOK }, compare: ["box", "pos", "fill"] },
  "quicklook.caption": { proto: inQuickLook("low-res pre-render · orbit viewport in app"), compare: ["pos", ...control] },
  "quicklook.type": { proto: { ...inQuickLook("Beaver_LowPoly", 2), child: 0 }, compare: ["box", "fill", "edge"] },
  "quicklook.close": { proto: inQuickLook("✕"), compare: ["box", "pos", "color", "fill", "edge"] },
  "quicklook.rule": { proto: { ...inQuickLook("Confirmed", 3), child: 2 }, compare: ["box", "pos", "fill", "edge"] },
  "quicklook.stat.key": { proto: inQuickLook("Format", 1), compare: ["type", "color"] },
  "quicklook.stat.value": { proto: inQuickLook("FBX", 1), compare: ["type", "color"] },
  "quicklook.chip": { proto: inQuickLook("lowpoly", 1), compare: control },
  "quicklook.prev": { proto: inQuickLook("← prev"), compare: ["pos", "type", "color"] },
  "quicklook.hint": { proto: inQuickLook("Esc closes · ← → step"), compare: ["pos", "type", "color"] },
  "quicklook.next": { proto: inQuickLook("next →"), compare: ["pos", "type", "color"] },
};
// A sound: the 30-bar waveform over the transport. The scrubber is a native range input the
// prototype never styled — only where it sits is compared.
const bar = (extra) => ({ css: `${QUICKLOOK} > div > div > div > div > div`, ...extra });
const quicklookSound = {
  ...peek("rock_pickup_01", "Sound", "pickup"),
  "quicklook.wave": { proto: { css: `${QUICKLOOK} > div > div > div > div` }, compare: ["box", "pos", "space"] },
  "quicklook.wave.bar": { proto: bar(), compare: ["box", "pos", "fill", "edge"] },
  "quicklook.wave.bar.last": { proto: bar({ nth: 29 }), compare: ["box", "pos", "fill"] },
  "quicklook.transport": { proto: inQuickLook("▶", 2), compare: ["box", "pos", "space"] },
  "quicklook.play": { proto: inQuickLook("▶", 1), compare: ["box", "pos", "color", "fill", "edge"] },
  "quicklook.scrub": { proto: { css: `${QUICKLOOK} input[type="range"]` }, compare: ["box", "pos"] },
  "quicklook.time": { proto: inQuickLook("0:00.0 / 0:01.2", 1), compare: ["box", "pos", "type", "color"] },
};
const quicklookImage = peek("bark_oak_albedo", "Image — full preview", "albedo");

// The staging tray, bottom right, with two models and a sound in it. A sound has no picture: its
// tile holds the type mark instead.
const tile = (extra) => ({ css: `${TRAY} > div:nth-child(2) > div`, ...extra });
const staging = {
  "staging": { proto: { css: TRAY }, compare: ["box", "pos", "fill", "edge", "space"] },
  "staging.title": { proto: inTray("⟡ Staging"), compare: ["pos", "type", "color"] },
  "staging.project": { proto: inTray("MyRPG", 2), compare: ["pos", "type", "color", "space"] },
  "staging.project.dot": { proto: { ...inTray("MyRPG", 2), child: 0 }, compare: ["box", "fill", "edge"] },
  "staging.count": { proto: inTray("3 assets", 1), compare: ["pos", "type", "color"] },
  "staging.clear": { proto: inTray("clear"), compare: ["box", "pos", "type", "color"] },
  "staging.tiles": { proto: { css: `${TRAY} > div`, nth: 1 }, compare: ["box", "pos", "space"] },
  "staging.tile": { proto: tile(), compare: ["box", "pos", "fill", "edge"] },
  "staging.tile.sound": { proto: tile({ nth: 2 }), compare: ["box", "pos", "fill", "edge"] },
  "staging.tile.mark": { proto: tile({ nth: 2, child: 0 }), compare: ["box", "fill", "edge"] },
  "staging.deps": { proto: inTray("3 picks → 11 files incl. materials & textures", 1), compare: ["box", "pos", "type", "color"] },
  "staging.dest.label": { proto: inTray("→ /"), compare: ["pos", "type", "color"] },
  "staging.dest": { proto: { css: `${TRAY} input` }, compare: ["pos", ...control] },
  "staging.import": { proto: inTray("Export to →"), compare: ["pos", ...control] },
  "staging.caption": { proto: inTray("picks + linked files only — never the whole pack"), compare: ["box", "pos", "type", "color"] },
  "card.stage.on": { proto: inLibrary("✓ staged", 1), compare: control },
};

/** The shell's anchors (top bar and sidebar) — present in every state; the gate's shell specimen. */
export const FRAME_ANCHORS = Object.keys(frame);

/** Anchors per state. A state not listed here still contributes to the vocabulary. */
export const ANCHORS = {
  library: { ...frame, ...library },
  "library+inspector": inspector,
  "library+filters": filters,
  "library+picks": picks,
  "library+list": list,
  quicklook,
  "quicklook+sound": quicklookSound,
  "quicklook+image": quicklookImage,
  staging,
};
