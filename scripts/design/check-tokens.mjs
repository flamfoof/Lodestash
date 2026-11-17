#!/usr/bin/env node
// check-tokens.mjs — tokens.css must name every value the prototype draws, and nothing else
// (plan P6 Task 6.1, ADR-0023 §2).
//
//   1. Every value in docs/design/vocabulary.json has a token with exactly that value, in the
//      token family for its kind (colours → --ls-*, sizes → --fs-*, …).
//   2. Every token in those families is drawn by the prototype, or sits inside an
//      `/* app-only: <reason> */ … /* end app-only */` block, or a
//      `/* legacy: … */ … /* end legacy */` block (names the old ui/ still imports until P5).
//
//   node scripts/design/check-tokens.mjs [--tokens <file>] [--vocabulary <file>]
// Exit 0 = clean, 1 = a drawn value without a token or a token nobody draws.

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const arg = (name, dflt) => {
  const i = process.argv.indexOf(name);
  return i > 0 ? process.argv[i + 1] : dflt;
};
const tokensFile = arg("--tokens", path.join(ROOT, "docs", "design", "tokens.css"));
const vocabFile = arg("--vocabulary", path.join(ROOT, "docs", "design", "vocabulary.json"));

// ---------- normalisation: both sides reduce to the same canonical string ----------

const h2 = (n) => Math.round(n).toString(16).padStart(2, "0");
/** Any CSS colour literal we use → lowercase #rrggbb or #rrggbbaa (null for transparent). */
export function canonColor(c) {
  c = c.trim().toLowerCase();
  let m;
  if ((m = c.match(/^#([0-9a-f]{3})$/))) return "#" + [...m[1]].map((x) => x + x).join("");
  if ((m = c.match(/^#([0-9a-f]{6})$/))) return c;
  if ((m = c.match(/^#([0-9a-f]{6})([0-9a-f]{2})$/))) return m[2] === "ff" ? `#${m[1]}` : c;
  if ((m = c.match(/^rgba?\(([^)]+)\)$/))) {
    const p = m[1].split(/[\s,/]+/).filter(Boolean).map(Number);
    const a = p.length > 3 ? p[3] : 1;
    if (a === 0) return null;
    return `#${h2(p[0])}${h2(p[1])}${h2(p[2])}${a < 1 ? h2(a * 255) : ""}`;
  }
  if (c === "transparent") return null;
  return undefined; // not a colour
}
const COLOR_RE = /#[0-9a-fA-F]{3,8}\b|rgba?\([^)]*\)/g;
const canonLen = (v) => {
  const n = parseFloat(v);
  return Number.isFinite(n) ? `${Math.round(n * 100) / 100}px` : null;
};
const canonDur = (v) => {
  v = v.trim();
  if (v.endsWith("ms")) return `${parseFloat(v)}ms`;
  if (v.endsWith("s")) return `${Math.round(parseFloat(v) * 1000)}ms`;
  return null;
};
/** box-shadow → "inset? x y blur spread #color", lengths in px. */
export function canonShadow(s) {
  const colors = s.match(COLOR_RE) ?? [];
  const color = colors.length ? canonColor(colors[0]) : "#000000";
  const rest = s.replace(COLOR_RE, " ").replace(/\binset\b/, " ");
  const lens = (rest.match(/-?[\d.]+(px)?/g) ?? []).map((x) => `${parseFloat(x)}px`);
  while (lens.length < 4) lens.push("0px");
  return `${/\binset\b/.test(s) ? "inset " : ""}${lens.slice(0, 4).join(" ")} ${color}`;
}
/** linear-gradient → colours canonicalised, default direction dropped, whitespace collapsed. */
export function canonGradient(g) {
  return g
    .toLowerCase()
    .replace(COLOR_RE, (c) => canonColor(c) ?? "transparent")
    .replace(/\(\s*(180deg|to bottom)\s*,\s*/g, "(")
    .replace(/\s+/g, " ")
    .replace(/\(\s+/g, "(")
    .replace(/\s*,\s*/g, ", ")
    // A `background` shorthand ends with its colour layer (or `none` once computed); that layer
    // is a colour token, not part of the gradient.
    .replace(/, (none|#[0-9a-f]{6,8}|transparent)$/, "")
    .trim();
}
const firstFamily = (f) => f.split(",")[0].replace(/["']/g, "").trim().toLowerCase();

// ---------- token families ----------

const FAMILIES = {
  color: { prefixes: ["--ls-"], canon: (v) => canonColor(v) },
  fontSize: { prefixes: ["--fs-"], canon: canonLen },
  fontWeight: { prefixes: ["--fw-"], canon: (v) => String(parseInt(v, 10)) },
  fontFamily: { prefixes: ["--ff-"], canon: firstFamily },
  letterSpacing: { prefixes: ["--track-"], canon: canonLen },
  radius: { prefixes: ["--r-"], canon: (v) => (parseFloat(v) >= 99 ? "pill" : canonLen(v)) },
  // `--space-<px>`: the legacy `--sp-1…7` are ladder indices (`--sp-4` is 16px), so the numeric
  // scale cannot reuse that prefix while the old UI still imports them.
  space: { prefixes: ["--space-", "--sp-"], canon: canonLen },
  borderWidth: { prefixes: ["--bw-"], canon: canonLen },
  shadow: { prefixes: ["--glow-", "--ring-", "--shadow-"], canon: canonShadow },
  gradient: { prefixes: ["--grad-"], canon: canonGradient },
  duration: { prefixes: ["--dur-"], canon: canonDur },
  opacity: { prefixes: ["--op-"], canon: (v) => String(Number(v)) },
};
const familyOf = (name) =>
  Object.entries(FAMILIES).find(([, f]) => f.prefixes.some((p) => name.startsWith(p)))?.[0];

// ---------- parse tokens.css ----------

export function parseTokens(css) {
  const tokens = [];
  const blocks = [];
  const mark = /\/\*\s*(app-only|legacy)\b[^*]*\*\/|\/\*\s*end (app-only|legacy)\s*\*\//g;
  let open = null;
  for (const m of css.matchAll(mark)) {
    if (m[1]) open = { kind: m[1], start: m.index };
    else if (open) {
      blocks.push({ ...open, end: m.index });
      open = null;
    }
  }
  // Only :root declarations define the design's values; density overrides etc. are app-side.
  for (const root of css.matchAll(/:root\s*\{([\s\S]*?)\n\}/g)) {
    const base = root.index + root[0].indexOf("{") + 1;
    for (const d of root[1].matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)) {
      const at = base + d.index;
      const exempt = blocks.find((b) => at > b.start && at < b.end)?.kind ?? null;
      tokens.push({ name: d[1], value: d[2].trim(), exempt });
    }
  }
  return tokens;
}

// ---------- what the prototype draws ----------

export function drawnValues(vocabulary) {
  const out = Object.fromEntries(Object.keys(FAMILIES).map((k) => [k, new Map()]));
  const add = (fam, canon, raw) => {
    if (canon === null || canon === undefined) return;
    if (!out[fam].has(canon)) out[fam].set(canon, raw);
  };
  // Rendered values first, then values declared in the source for states never rendered.
  collect(vocabulary, add);
  if (vocabulary.declared) collect({ ...vocabulary.declared, hover: {} }, (f, c, raw) => add(f, c, `declared ${raw}`));
  return out;
}

function collect(vocab, add) {
  for (const v of Object.keys(vocab.color ?? {})) add("color", canonColor(v), `color ${v}`);
  for (const v of Object.keys(vocab.background ?? {})) add("color", canonColor(v), `background ${v}`);
  for (const v of Object.keys(vocab.border ?? {})) {
    const [w, c] = v.split(" ");
    add("color", canonColor(c), `border ${v}`);
    if (parseFloat(w) > 1) add("borderWidth", canonLen(w), `border ${v}`);
  }
  for (const v of Object.keys(vocab.gradient ?? {})) add("gradient", canonGradient(v), `gradient ${v}`);
  for (const v of Object.keys(vocab.fontSize ?? {})) add("fontSize", canonLen(v), `font-size ${v}`);
  for (const v of Object.keys(vocab.fontWeight ?? {})) add("fontWeight", String(parseInt(v, 10)), `font-weight ${v}`);
  for (const v of Object.keys(vocab.fontFamily ?? {})) add("fontFamily", firstFamily(v), `font-family ${v}`);
  for (const v of Object.keys(vocab.letterSpacing ?? {})) add("letterSpacing", canonLen(v), `letter-spacing ${v}`);
  for (const v of Object.keys(vocab.radius ?? {})) add("radius", parseFloat(v) >= 99 ? "pill" : canonLen(v), `radius ${v}`);
  for (const v of Object.keys(vocab.gap ?? {})) add("space", canonLen(v), `gap ${v}`);
  for (const v of Object.keys(vocab.padding ?? {}))
    for (const part of v.split(" ")) if (parseFloat(part) > 0) add("space", canonLen(part), `padding ${v}`);
  for (const v of Object.keys(vocab.shadow ?? {})) add("shadow", canonShadow(v), `box-shadow ${v}`);
  for (const v of Object.keys(vocab.duration ?? {})) add("duration", canonDur(v), `duration ${v}`);
  for (const v of Object.keys(vocab.opacity ?? {})) add("opacity", String(Number(v)), `opacity ${v}`);
  for (const decl of Object.keys(vocab.hover ?? {})) {
    if (decl.includes("{{")) continue; // a template hole, resolved at runtime from data
    const [prop, ...rest] = decl.split(":");
    const val = rest.join(":").trim();
    if (prop === "box-shadow") add("shadow", canonShadow(val), `hover ${decl}`);
    else if (/color|background/.test(prop)) {
      for (const c of val.match(COLOR_RE) ?? []) add("color", canonColor(c), `hover ${decl}`);
    }
  }
}

// ---------- check ----------

export function check(css, vocab) {
  const tokens = parseTokens(css);
  const drawn = drawnValues(vocab);
  const problems = [];
  const have = Object.fromEntries(Object.keys(FAMILIES).map((k) => [k, new Map()]));
  for (const t of tokens) {
    const fam = familyOf(t.name);
    if (!fam) continue;
    // Legacy names die at P5; a drawn value they happen to carry still needs a lasting token.
    if (t.exempt === "legacy") continue;
    if (t.value.includes("var(")) continue; // an alias of another token
    const c = FAMILIES[fam].canon(t.value);
    if (c === null || c === undefined) continue;
    if (!have[fam].has(c)) have[fam].set(c, []);
    have[fam].get(c).push(t);
  }
  for (const [fam, values] of Object.entries(drawn)) {
    for (const [canon, raw] of values) {
      if (!have[fam].has(canon)) problems.push(`drawn, no token: ${fam} ${canon}   (${raw})`);
    }
  }
  for (const [fam, values] of Object.entries(have)) {
    for (const [canon, toks] of values) {
      if (drawn[fam].has(canon)) continue;
      for (const t of toks) if (!t.exempt) problems.push(`token nobody draws: ${t.name}: ${t.value}`);
    }
  }
  return { problems, tokenCount: tokens.length };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const { problems, tokenCount } = check(
    fs.readFileSync(tokensFile, "utf8"),
    JSON.parse(fs.readFileSync(vocabFile, "utf8")),
  );
  for (const p of problems) console.error(p);
  console.log(`${tokenCount} tokens checked — ${problems.length} problem(s).`);
  process.exit(problems.length ? 1 : 0);
}
