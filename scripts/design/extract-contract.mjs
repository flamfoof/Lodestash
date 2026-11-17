#!/usr/bin/env node
// extract-contract.mjs — measure the design prototype (spec §10.2–§10.3, ADR-0023).
//
// Renders docs/design/Lodestash Prototype.dc.html at 1440×900, DPR 1, with the app's bundled
// fonts, steps it into every state in states.mjs, and writes:
//   docs/design/contract.json    per state: each anchor's measured box and style
//   docs/design/vocabulary.json  every colour, type size, radius, spacing … the prototype draws,
//                                with how often (the input to tokens.css)
// Exit 1 if a state's screen does not appear, an anchor is not found, or the page throws: the
// prototype changed and the contract must be looked at, not silently regenerated.
//
//   node scripts/design/extract-contract.mjs [--check]
// --check compares against the committed files instead of writing them (CI).

import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import { openPrototype, DESIGN_DIR, PROTOTYPE, VIEWPORT } from "./lib/prototype.mjs";
import { measureInPage, vocabularyInPage } from "./lib/measure.mjs";
import { STATES } from "./states.mjs";
import { ANCHORS } from "./anchors.mjs";

const check = process.argv.includes("--check");
const sha = (file) => crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex").slice(0, 16);
const protoFile = path.join(DESIGN_DIR, PROTOTYPE);

const contract = {
  generatedBy: "scripts/design/extract-contract.mjs",
  prototype: { file: PROTOTYPE, sha256: sha(protoFile), runtime: sha(path.join(DESIGN_DIR, "support.js")) },
  viewport: { ...VIEWPORT, deviceScaleFactor: 1 },
  states: {},
};
const vocabulary = {};
const failures = [];

for (const [name, state] of Object.entries(STATES)) {
  const { page, close, errors } = await openPrototype();
  try {
    await state.steps(page);
    const shown = await page.evaluate(
      (label) => [...document.querySelectorAll(`[data-screen-label="${label}"]`)].some((e) => e.getBoundingClientRect().width > 0),
      state.screen,
    );
    if (!shown) {
      failures.push(`${name}: screen "${state.screen}" not visible after its steps`);
      continue;
    }
    const vocab = await page.evaluate(vocabularyInPage, '[data-screen-label="Lodestash"]');
    for (const [cat, values] of Object.entries(vocab)) {
      vocabulary[cat] ??= {};
      for (const [v, n] of Object.entries(values)) vocabulary[cat][v] = (vocabulary[cat][v] ?? 0) + n;
    }
    const anchors = {};
    for (const [anchor, def] of Object.entries(ANCHORS[name] ?? {})) {
      const m = await page.evaluate(measureInPage, def.proto);
      if (!m.ok) failures.push(`${name}/${anchor}: ${m.reason}`);
      else anchors[anchor] = { tag: m.tag, box: m.box, style: m.style };
    }
    contract.states[name] = { screen: state.screen, anchors };
    for (const e of errors) failures.push(`${name}: ${e}`);
  } catch (e) {
    failures.push(`${name}: ${e.message}`);
  } finally {
    await close();
  }
}

// Hover styles never appear in a static render: the prototype declares them as `style-hover`
// attributes. Read them from the source so hover colours and glows get tokens too.
{
  const src = fs.readFileSync(protoFile, "utf8");
  const hover = {};
  for (const m of src.matchAll(/style-hover="([^"]*)"/g)) {
    for (const decl of m[1].split(";")) {
      const d = decl.trim().replace(/\s+/g, " ");
      if (d) hover[d] = (hover[d] ?? 0) + 1;
    }
  }
  vocabulary.hover = hover;
}

// Stable output: sort vocabulary values by use, most first.
const sortedVocab = Object.fromEntries(
  Object.entries(vocabulary).map(([cat, values]) => [
    cat,
    Object.fromEntries(Object.entries(values).sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))),
  ]),
);
const outputs = {
  [path.join(DESIGN_DIR, "contract.json")]: JSON.stringify(contract, null, 2) + "\n",
  [path.join(DESIGN_DIR, "vocabulary.json")]:
    JSON.stringify({ prototype: contract.prototype, ...sortedVocab }, null, 2) + "\n",
};

if (failures.length) {
  console.error(`extract-contract: ${failures.length} failure(s)`);
  for (const f of failures) console.error(`  ${f}`);
  process.exit(1);
}

let stale = 0;
for (const [file, text] of Object.entries(outputs)) {
  const rel = path.relative(process.cwd(), file);
  if (check) {
    const current = fs.existsSync(file) ? fs.readFileSync(file, "utf8").replace(/\r\n/g, "\n") : "";
    if (current !== text) {
      console.error(`stale: ${rel} — run node scripts/design/extract-contract.mjs`);
      stale++;
    }
  } else {
    fs.writeFileSync(file, text);
    console.log(`wrote ${rel}`);
  }
}
const anchorCount = Object.values(contract.states).reduce((n, s) => n + Object.keys(s.anchors).length, 0);
console.log(`${Object.keys(contract.states).length} states, ${anchorCount} anchors measured.`);
process.exit(stale ? 1 : 0);
