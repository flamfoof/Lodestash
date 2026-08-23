#!/usr/bin/env node
// check-board.mjs — drift detection for the kanban boards (CLAUDE.md rule 8, companion to
// check-docs.mjs).
//
// A checkbox is a claim about the code, and claims rot in two directions:
//
//   1. A **ticked** card cites something that no longer exists — the work shipped, then the thing
//      it named was renamed or replaced, and the card still points at the old name.
//   2. An **open** card says something is missing that has since been built. This is the nastier
//      one: building a feature does not touch the card asserting its absence, so the card sits
//      there forever telling the next reader to write code that already exists. Five cards were
//      found in this state in a single session (2026-08-22) — three of them sent work down a path
//      that turned out to be already finished.
//
// Neither check can prove a card is *done*. What they catch is a card citing code that is not
// there, and a card claiming absence while the thing it names is present — both of which are worth
// a human look, and neither of which any amount of careful reading will reliably notice.
//
// Exit 0 = clean, 1 = a ticked card cites something missing. Open-card flags are warnings: "the
// identifier exists" is consistent with "DDL-only" or "zero callers", so they need judgement.
//
//   node scripts/check-board.mjs

import { readdirSync, readFileSync, existsSync, statSync } from "node:fs";
import { join, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const BOARDS = ["docs/KANBAN.md", "docs/KANBAN-SVELTE.md"].filter((b) => existsSync(join(ROOT, b)));
const SKIP = new Set([".git", "node_modules", "target", "dist", ".reasonix"]);
const CODE = /\.(rs|ts|svelte|mjs|css)$/;

/** Every source file's text, concatenated, plus the set of file names. One read, many lookups. */
function loadSources(dir, acc = { text: [], names: new Set() }) {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    if (SKIP.has(e.name)) continue;
    const p = join(dir, e.name);
    if (e.isDirectory()) loadSources(p, acc);
    else {
      acc.names.add(e.name);
      if (CODE.test(e.name)) {
        try {
          acc.text.push(readFileSync(p, "utf8"));
        } catch {
          /* unreadable file is not the board's problem */
        }
      }
    }
  }
  return acc;
}

const src = loadSources(ROOT);
const SRC = src.text.join("\n");

/** Cards, with their wrapped continuation lines folded in.
 *
 * Line endings are normalised first. The repo is CRLF, and `\r` is a line terminator — so a
 * per-line `/^- \[([ x])\] (.*)$/` matches **nothing** against raw CRLF text: `.` stops before the
 * `\r` and `$` never reaches the end. It reports zero cards and zero problems, which reads exactly
 * like a clean board. That silent-zero is the failure this whole script exists to catch, so it is
 * worth the one line and this paragraph. */
function parseCards(raw) {
  const text = raw.replace(/\r\n/g, "\n");
  const cards = [];
  let cur = null;
  for (const line of text.split("\n")) {
    const m = /^- \[([ x])\] (.*)$/.exec(line);
    if (m) {
      if (cur) cards.push(cur);
      cur = { done: m[1] === "x", text: m[2] };
    } else if (cur && line.startsWith("      ")) {
      cur.text += " " + line.trim();
    } else if (cur && !line.trim()) {
      cards.push(cur);
      cur = null;
    }
  }
  if (cur) cards.push(cur);
  return cards;
}

const TESTID = /data-testid="([a-zA-Z0-9_-]+)"/g;
const FILE = /`([A-Za-z0-9_-]+\.(?:svelte|rs|ts|mjs|css))`/g;
const IDENT = /`([A-Za-z_][A-Za-z0-9_]*(?:::|\.)?[A-Za-z0-9_]*)`/g;
/** `some_fn()` — the trailing parens keep it out of IDENT, and cards write functions that way. */
const FN = /`([A-Za-z_][A-Za-z0-9_:]{3,})\(\)?`/g;
/** Phrases that assert a thing is *not* built. */
const ABSENCE =
  /absent|does ?n[o']t exist|no implementation|never (?:written|wired|called|set|feeds|reads)|zero callers|not wired|not honou?red|still lacks|is static|DDL-only|nothing (?:feeds|surfaces)/i;

/** Code-shaped enough to be worth checking: has an underscore, `::`, or a `.` join. */
const codeShaped = (t) => t.includes("::") || t.includes(".") || (t.includes("_") && t === t.toLowerCase());
const leafOf = (t) => t.split(/::|\./).pop();

let errors = 0;
let warnings = 0;
let claims = 0;

for (const board of BOARDS) {
  const raw = readFileSync(join(ROOT, board), "utf8");
  // A board can declare itself a record of work that shipped in code since deleted. Its dead
  // citations are then history, not drift: reported, but never a failure. Without this the
  // pre-cutover board fails the check forever over the egui crate, and a check that always fails
  // is a check nobody reads.
  const historical = raw.includes("<!-- check-board: historical -->");
  const cards = parseCards(raw);
  for (const card of cards) {
    const label = card.text.slice(0, 90).replace(/\s+/g, " ");
    const cites = [];

    for (const [, id] of card.text.matchAll(TESTID)) cites.push({ kind: "testid", name: id, hit: SRC.includes(`data-testid="${id}`) });
    for (const [, f] of card.text.matchAll(FILE)) cites.push({ kind: "file", name: f, hit: src.names.has(f) });
    for (const [, tok] of card.text.matchAll(IDENT)) {
      if (!codeShaped(tok)) continue;
      const leaf = leafOf(tok);
      if (!leaf || leaf.length < 5) continue;
      cites.push({ kind: "ident", name: tok, hit: SRC.includes(leaf) });
    }
    for (const [, fn] of card.text.matchAll(FN)) {
      const leaf = leafOf(fn);
      if (!leaf || leaf.length < 5) continue;
      cites.push({ kind: "fn", name: fn, hit: SRC.includes(leaf) });
    }
    if (!cites.length) continue;
    claims += cites.length;

    if (card.done) {
      for (const c of cites.filter((c) => !c.hit)) {
        if (historical) warnings++;
        else errors++;
        const level = historical ? "note " : "error";
        console.log(`  ${level}  ${board}: ticked card cites a missing ${c.kind} \`${c.name}\`\n         ${label}…`);
      }
    } else if (ABSENCE.test(card.text)) {
      const present = cites.filter((c) => c.hit).map((c) => c.name);
      if (present.length) {
        warnings++;
        console.log(
          `  warn   ${board}: open card claims absence but these exist: ${present.join(", ")}\n         ${label}…\n` +
            `         (consistent with "DDL-only" / "zero callers" — check whether it is still true)`,
        );
      }
    }
  }
}

console.log(
  `\n${BOARDS.length} board(s), ${claims} cited symbol(s) checked — ${errors} stale citation(s), ${warnings} absence claim(s) to re-check.`,
);
process.exit(errors > 0 ? 1 : 0);
