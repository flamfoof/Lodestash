#!/usr/bin/env node
// check-styles.mjs — no design value outside tokens.css (spec §9.2 rule 3, plan P6 Task 6.2).
//
// Scans CSS declarations in web/src (<style> blocks of .svelte files, and .css files) and rejects
// literals that belong in docs/design/tokens.css: colours, lengths other than 0 / 1px / percentages /
// fr, numeric z-index, literal durations, literal shadows, opacities other than 0 and 1, numeric
// font sizes and weights. `var(--x, <fallback>)` fallbacks count too — a fallback hides drift.
//
// Escape hatch, for a reason a reviewer can judge: `/* style-lint-allow: <reason> */` on the line.
// Media/container query conditions are exempt (custom properties cannot be used there).
//
//   node scripts/design/check-styles.mjs [dir…]     (default: web/src)
// Exit 1 on any finding; each prints file:line.

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");

const RULES = [
  { re: /#[0-9a-fA-F]{3,8}\b|\b(rgba?|hsla?)\(/, why: "colour literal — use a --ls-* token" },
  {
    // Any px/rem/em length that is not 0 or 1px.
    re: /(?<![\w.-])(?!0(?:px|rem|em)?(?![\w.]))(?!1px(?![\w.]))\d*\.?\d+(px|rem|em)\b/,
    why: "length literal — use a --space-*/--fs-*/--r-*/--bw-* token or a layout variable",
  },
  { prop: /^z-index$/, re: /^-?\d+$/, why: "numeric z-index — use the --z-* ladder" },
  { prop: /^(transition|animation)(-duration|-delay)?$/, re: /(?<![\w-])\d*\.?\d+m?s\b/, why: "literal duration — use a --dur-* token" },
  { prop: /^box-shadow$/, re: /^(?!none$)(?!var\()/, why: "literal shadow — use a --glow-*/--ring-*/--shadow-* token" },
  { prop: /^opacity$/, re: /^(?!0$)(?!1$)(?!var\()/, why: "opacity literal — use an --op-* token" },
  { prop: /^font-weight$/, re: /^\d+$/, why: "numeric font-weight — use a --fw-* token" },
];

function* declarations(file) {
  const text = fs.readFileSync(file, "utf8");
  const lines = text.split(/\r?\n/);
  let inStyle = file.endsWith(".css");
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (file.endsWith(".svelte")) {
      if (/<style[\s>]/.test(line)) inStyle = true;
      if (/<\/style>/.test(line)) inStyle = false;
    }
    if (!inStyle) continue;
    if (/style-lint-allow:\s*[^*\s][^*]*\*\//.test(line)) continue;
    if (/^\s*@(media|container|supports|import)\b/.test(line)) continue;
    const m = line.match(/^\s*([a-z-]+)\s*:\s*(.+?)\s*;?\s*(\/\*.*\*\/)?\s*$/i);
    if (!m || m[1].startsWith("--")) continue; // custom properties are layout variables or tokens
    yield { line: i + 1, prop: m[1].toLowerCase(), value: m[2].replace(/;$/, "").trim() };
  }
}

export function checkFile(file) {
  const findings = [];
  for (const d of declarations(file)) {
    for (const rule of RULES) {
      if (rule.prop && !rule.prop.test(d.prop)) continue;
      // Ignore what is inside url(…) and the names inside var(--…) — only literal values count.
      const value = d.value.replace(/url\([^)]*\)/g, "url()").replace(/var\(--[\w-]+\s*\)/g, "var()");
      if (rule.re.test(value)) {
        // One finding per declaration: the most specific rule is listed last, so report that one.
        const specific = RULES.slice(RULES.indexOf(rule) + 1).find(
          (r) => r.prop && r.prop.test(d.prop) && r.re.test(value),
        );
        const why = (specific ?? rule).why;
        findings.push({ file, line: d.line, prop: d.prop, value: d.value, why });
        break;
      }
    }
  }
  return findings;
}

function walk(dir, out = []) {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) walk(p, out);
    else if (/\.(svelte|css)$/.test(e.name)) out.push(p);
  }
  return out;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const dirs = process.argv.slice(2);
  const roots = dirs.length ? dirs : [path.join(ROOT, "web", "src")];
  const findings = roots.flatMap((d) => walk(path.resolve(d))).flatMap(checkFile);
  for (const f of findings) console.error(`${path.relative(process.cwd(), f.file)}:${f.line}  ${f.prop}: ${f.value}  — ${f.why}`);
  console.log(`check-styles: ${findings.length} finding(s).`);
  process.exit(findings.length ? 1 : 0);
}
