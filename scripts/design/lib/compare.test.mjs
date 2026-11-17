// node --test scripts/design/lib/compare.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { compare, exemptionsFor, readDeviations } from "./compare.mjs";

const contract = {
  box: { x: 100, y: 50, w: 120, h: 30 },
  style: {
    fontFamily: "Inter", fontSize: 12, fontWeight: 400, letterSpacing: "normal", textTransform: "none",
    color: "#a7b0c0", background: "#232c3d", backgroundImage: null,
    borderWidth: 1, borderColor: "#3a465c", radius: 6, padding: [4, 8, 4, 8], gap: null,
  },
};
const app = (patch = {}, box = {}) => ({ box: { ...contract.box, ...box }, style: { ...contract.style, ...patch } });
const ALL = ["box", "pos", "type", "color", "fill", "edge", "space"];
const props = (diffs) => diffs.map((d) => d.property);

test("identical measurements have no diff", () => {
  assert.deepEqual(compare(contract, app(), ALL), []);
});

test("geometry is ±1 px: 1 passes, 1.5 fails", () => {
  assert.deepEqual(compare(contract, app({}, { w: 121, h: 29, x: 101 }), ALL), []);
  assert.deepEqual(props(compare(contract, app({}, { w: 121.5 }), ALL)), ["w"]);
  assert.deepEqual(props(compare(contract, app({}, { y: 48.5 }), ALL)), ["y"]);
});

test("colour, type and edges are exact", () => {
  assert.deepEqual(props(compare(contract, app({ color: "#a7b0c1" }), ALL)), ["color"]);
  assert.deepEqual(props(compare(contract, app({ fontSize: 12.5, fontFamily: "Arial" }), ALL)), ["fontFamily", "fontSize"]);
  assert.deepEqual(props(compare(contract, app({ radius: 999 }), ALL)), ["radius"]);
  assert.deepEqual(props(compare(contract, app({ backgroundImage: "linear-gradient(red, blue)" }), ALL)), ["backgroundImage"]);
});

test("spacing is ±0.5 px, and no gap is not a zero gap", () => {
  assert.deepEqual(compare(contract, app({ padding: [4.5, 8, 3.5, 8] }), ALL), []);
  assert.deepEqual(props(compare(contract, app({ padding: [5, 8, 4, 8] }), ALL)), ["padding"]);
  assert.deepEqual(props(compare(contract, app({ gap: [0, 0] }), ALL)), ["gap"]);
});

test("only the listed groups are checked, and skipped groups are not", () => {
  const moved = app({ color: "#ffffff" }, { x: 0 });
  assert.deepEqual(props(compare(contract, moved, ["box"])), []);
  assert.deepEqual(props(compare(contract, moved, ["pos", "color"], { skip: ["pos"] })), ["color"]);
  assert.throws(() => compare(contract, moved, ["shape"]), /unknown compare group/);
});

const TABLE = `
| Screen / state | Anchor | Properties | Prototype | App | Reason | Decided |
|---|---|---|---|---|---|---|
| all | nav and action glyphs (no anchor) | glyph shape only | Unicode | SVG | ADR-0012 | 2026-11-16 |
| \`library\` | \`library.sort\` | \`fontFamily\`, \`w\` | Arial | Inter | never styled | 2026-11-17 |
`;

test("a DEVIATIONS row exempts only its own properties, anchor and state", () => {
  const rows = readDeviations(TABLE);
  assert.equal(rows.length, 1, "a row with no backticked anchor exempts nothing");
  const ignore = exemptionsFor(rows, "library", "library.sort");
  assert.deepEqual([...ignore].sort(), ["fontFamily", "w"]);

  const sort = app({ fontFamily: "Inter", fontSize: 13 }, { w: 140 });
  assert.deepEqual(props(compare(contract, sort, ALL, { ignore })), ["fontSize"]);
  // Another anchor, or another state, gets no exemption from that row.
  assert.deepEqual([...exemptionsFor(rows, "library", "library.filters")], []);
  assert.deepEqual([...exemptionsFor(rows, "favorites", "library.sort")], []);
});

test("an `all` row applies to every state; a typo in Properties is an error", () => {
  const all = readDeviations("| Screen | Anchor | Properties |\n|---|---|---|\n| all | `card.size` | `color` |\n");
  assert.deepEqual([...exemptionsFor(all, "trash", "card.size")], ["color"]);
  assert.throws(() => readDeviations("| Screen | Anchor | Properties |\n|---|---|---|\n| all | `card.size` | `colour` |\n"), /not a measured property/);
});
