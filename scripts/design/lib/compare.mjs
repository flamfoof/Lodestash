// Compare an app element's measurement with the contract's (spec §10.4, ADR-0023).
//
// The property groups and tolerances are the ones documented at the top of anchors.mjs:
//   box   → w, h (±1 px)                   pos → x, y (±1 px)
//   type  → fontFamily, fontSize, fontWeight, letterSpacing, textTransform (exact)
//   color → color (exact)                  fill → background, backgroundImage (exact)
//   edge  → borderWidth, borderColor, radius (exact; 99 px and up is one "fully rounded")
//   space → padding, gap (±0.5 px)
// A difference is allowed only when docs/design/DEVIATIONS.md has a row naming that exact
// (state, anchor, property); `readDeviations` turns the table into those triples.

export const GROUPS = {
  box: ["w", "h"],
  pos: ["x", "y"],
  type: ["fontFamily", "fontSize", "fontWeight", "letterSpacing", "textTransform"],
  color: ["color"],
  fill: ["background", "backgroundImage"],
  edge: ["borderWidth", "borderColor", "radius"],
  space: ["padding", "gap"],
};

const TOLERANCE = { w: 1, h: 1, x: 1, y: 1, padding: 0.5, gap: 0.5 };
export const PROPERTIES = new Set(Object.values(GROUPS).flat());

const GEOMETRY = new Set([...GROUPS.box, ...GROUPS.pos]);

/**
 * The prototype writes "fully rounded" two ways, `99px` and `999px`. They draw the same and
 * tokens.css names both `--r-pill` (check-tokens.mjs folds every radius of 99 px and up into it),
 * so the gate reads them as one value too.
 */
const PILL = 99;
const canon = (prop, value) => (prop === "radius" && typeof value === "number" && value >= PILL ? "pill" : value);
const read = (m, prop) => canon(prop, GEOMETRY.has(prop) ? m.box?.[prop] : m.style?.[prop]);

function same(a, b, tol) {
  if (a === b) return true;
  if (a === null || b === null || a === undefined || b === undefined) return false;
  if (Array.isArray(a) || Array.isArray(b)) {
    if (!Array.isArray(a) || !Array.isArray(b) || a.length !== b.length) return false;
    return a.every((v, i) => same(v, b[i], tol));
  }
  if (typeof a === "number" && typeof b === "number") return Math.abs(a - b) <= tol + 1e-9;
  return false;
}

/**
 * @param expected  the contract's measurement `{ box, style }`
 * @param actual    the app's measurement (lib/measure.mjs `measureInPage`)
 * @param compareList  group names from anchors.mjs (`["box", "type", …]`)
 * @param options.ignore  property names exempted by DEVIATIONS rows for this state and anchor
 * @param options.skip    group names not checked in this mode (the gallery skips `pos`)
 * @returns Diff[] — `{ group, property, expected, actual }`, empty when they match
 */
export function compare(expected, actual, compareList, { ignore = [], skip = [] } = {}) {
  const exempt = new Set(ignore);
  const diffs = [];
  for (const group of compareList) {
    if (skip.includes(group)) continue;
    const props = GROUPS[group];
    if (!props) throw new Error(`unknown compare group "${group}"`);
    for (const property of props) {
      if (exempt.has(property)) continue;
      const e = read(expected, property);
      const a = read(actual, property);
      if (!same(e, a, TOLERANCE[property] ?? 0)) diffs.push({ group, property, expected: e, actual: a });
    }
  }
  return diffs;
}

/**
 * Parse the DEVIATIONS.md table. A row exempts the backticked anchor names in its Anchor cell, for
 * the backticked property names in its Properties cell, in the states its first cell names
 * (`all`, or backticked/comma-separated state names). A row with no backticked anchor exempts
 * nothing (it documents a difference the contract does not measure). A backticked word in the
 * Properties cell that is not a measured property is an error — a typo must not silently exempt
 * nothing.
 * @returns {{ states: string[] | "all", anchors: string[], properties: string[], line: number }[]}
 */
export function readDeviations(markdown) {
  const rows = [];
  const lines = markdown.split(/\r?\n/);
  let header = null;
  lines.forEach((line, i) => {
    if (!line.trim().startsWith("|")) return;
    const cells = line.trim().replace(/^\||\|$/g, "").split("|").map((c) => c.trim());
    if (!header) {
      header = cells.map((c) => c.toLowerCase());
      return;
    }
    if (cells.every((c) => /^:?-+:?$/.test(c))) return;
    const col = (name) => cells[header.findIndex((h) => h.startsWith(name))] ?? "";
    const ticks = (s) => [...s.matchAll(/`([^`]+)`/g)].map((m) => m[1].trim());
    const stateCell = col("screen");
    const anchors = ticks(col("anchor"));
    const properties = ticks(col("properties"));
    if (anchors.length === 0) return;
    for (const p of properties) {
      if (!PROPERTIES.has(p)) throw new Error(`DEVIATIONS.md:${i + 1}: "${p}" is not a measured property (${[...PROPERTIES].join(", ")})`);
    }
    if (properties.length === 0) throw new Error(`DEVIATIONS.md:${i + 1}: a row naming an anchor must name its properties in backticks`);
    const states = /^all\b/i.test(stateCell) ? "all" : ticks(stateCell).length ? ticks(stateCell) : stateCell.split(/[,\s]+/).filter(Boolean);
    rows.push({ states, anchors, properties, line: i + 1 });
  });
  return rows;
}

/** The properties DEVIATIONS exempts for one (state, anchor). */
export function exemptionsFor(rows, state, anchor) {
  const out = new Set();
  for (const r of rows) {
    if (r.states !== "all" && !r.states.includes(state)) continue;
    if (!r.anchors.includes(anchor)) continue;
    r.properties.forEach((p) => out.add(p));
  }
  return out;
}

/** One line per diff, for a table cell or a log. */
export function describe(diffs) {
  const fmt = (v) => (v === null ? "none" : Array.isArray(v) ? v.join("/") : String(v));
  return diffs.map((d) => `${d.property}: want ${fmt(d.expected)}, got ${fmt(d.actual)}`).join("; ");
}
