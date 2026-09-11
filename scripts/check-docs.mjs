#!/usr/bin/env node
// check-docs.mjs — drift detection for docs/ (CLAUDE.md rule 8).
//
// The docs are the source of truth, and they cite the code heavily (`writer.rs:602`,
// `docs/KANBAN-SVELTE.md`, `tokens.css:98`). A citation that no longer resolves is drift:
// either the code moved (update the doc) or the doc is describing something gone (fix the doc).
//
// Checks:
//   1. Every `path:line` citation resolves (file exists, line <= file length).
//   2. Every `crates/...`, `docs/...`, `scripts/...` path mention resolves (best-effort).
//   3. Every `docs/*.md` cross-link resolves.
//   4. Warning: markdown files with no 4-digit year near the top (no "last reviewed" marker).
//
// Exit 0 = clean (warnings allowed), 1 = dead references found. Run from the repo root:
//   node scripts/check-docs.mjs

import { readdirSync, readFileSync, statSync, existsSync, readdirSync as ls } from "node:fs";
import { join, dirname, resolve, relative, extname } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const DOCS = join(ROOT, "docs");

function walk(dir, out = []) {
  for (const e of ls(dir)) {
    const p = join(dir, e);
    if (statSync(p).isDirectory()) {
      if (e === "node_modules" || e === ".git") continue;
      walk(p, out);
    } else if (e.endsWith(".md")) {
      out.push(p);
    }
  }
  return out;
}

const mdFiles = walk(DOCS).sort();
// The repo-root README + CLAUDE.md also cross-link docs/ — scan them too, or a dead link
// there ships silently.
for (const root of ["README.md", "CLAUDE.md"]) {
  if (existsSync(join(ROOT, root))) mdFiles.push(join(ROOT, root));
}
const lineCache = new Map();
function linesOf(p) {
  if (!lineCache.has(p)) {
    lineCache.set(p, readFileSync(p, "utf8").split("\n"));
  }
  return lineCache.get(p);
}

let problems = 0;
let warnings = 0;
const seen = new Set();
const flag = (kind, file, msg) => {
  const key = `${kind}:${file}:${msg}`;
  if (seen.has(key)) return;
  seen.add(key);
  console.log(`${kind === "error" ? "DEAD" : "warn"} ${relative(ROOT, file)}: ${msg}`);
  if (kind === "error") problems += 1;
  else warnings += 1;
};

const citRe = /([A-Za-z0-9_./-]+\.(?:rs|svelte|ts|mjs|js|css|py|toml|md|html)):(\d+)\b/g;
const bareRe = /(?:`)?((?:crates|docs|scripts|src)\/[A-Za-z0-9_./-]+\.(?:rs|svelte|ts|mjs|json|js|css|py|toml|md|html)\b)/g;

// Resolve a cited path to a real file. The docs use short names a lot:
//   `writer.rs`        → crates/lodestash-core/src/ | .../lodestash-app/src/ | ui/src/lib/ | ui/src/
//   `TopBar.svelte`    → crates/lodestash-desktop/ui/src/lib/TopBar.svelte
//   `lodestash-app/...`→ crates/lodestash-app/... (src/ inserted when the direct form misses)
const SEARCH_ROOTS = [
  (rel) => resolve(ROOT, rel),
  (rel) => resolve(ROOT, "crates", rel),
  (rel) => resolve(ROOT, "crates", rel.replace(/^([a-z-]+)\//, "$1/src/")),
  (rel) => resolve(ROOT, "docs", "design", rel),
  (rel) => resolve(ROOT, "docs", rel),
  (rel) => resolve(ROOT, "crates", "lodestash-core", "src", rel),
  (rel) => resolve(ROOT, "crates", "lodestash-app", "src", rel),
  (rel) => resolve(ROOT, "crates", "lodestash-desktop", "ui", "src", "lib", rel),
  (rel) => resolve(ROOT, "crates", "lodestash-desktop", "ui", "src", "styles", rel),
  (rel) => resolve(ROOT, "crates", "lodestash-desktop", "ui", "src", rel),
  (rel) => resolve(ROOT, "crates", "lodestash-desktop", "src", rel),
  (rel) => resolve(ROOT, "crates", "lodestash-ingest", "src", rel),
  (rel) => resolve(ROOT, "crates", "lodestash-ai", "src", rel),
];
function resolveCite(rel) {
  const candidates = [
    ...SEARCH_ROOTS.map((f) => f(rel)),
    // one directory up for files that live under a subdir (e.g. `docs/design/icons/icons.svg`)
    resolve(ROOT, rel.replace(/^\.\.\//, "")),
  ];
  return candidates.find((p) => existsSync(p)) ?? null;
}

for (const file of mdFiles) {
  const text = readFileSync(file, "utf8");

  // 4. Last-reviewed marker: a 4-digit year in the first 40 lines.
  const head = text.split("\n").slice(0, 40).join("\n");
  if (!/\b20\d\d\b/.test(head)) {
    flag("warn", file, "no year/date in the header block — add a last-reviewed date");
  }

  // 1. path:line citations
  for (const m of text.matchAll(citRe)) {
    const [raw, rel, lineStr] = m;
    // An explicit historical marker ("egui-era `app.rs:812`", "deleted at B7") means the
    // citation is a deliberate record of deleted code — not drift. Honour it.
    const ctx = text.slice(Math.max(0, m.index - 24), m.index);
    if (/egui-era|deleted at B7|retired at B7/.test(ctx)) continue;
    const target = resolveCite(rel);
    if (!target) {
      flag("error", file, `citation target missing: ${raw}`);
      continue;
    }
    const n = Number(lineStr);
    if (n > linesOf(target).length) {
      flag("error", file, `line ${n} out of range in ${rel} (has ${linesOf(target).length})`);
    }
  }

  // 2. bare path mentions
  for (const m of text.matchAll(bareRe)) {
    const rel = m[1].replace(/`/g, "");
    if (rel.startsWith("src/")) continue; // src/ is the caveman repo's layout, not ours
    if (!existsSync(resolve(ROOT, rel))) {
      flag("error", file, `path mention missing: ${rel}`);
    }
  }

  // 3. docs cross-links
  for (const m of text.matchAll(/docs\/([A-Za-z0-9_./-]+\.md)/g)) {
    const target = join(DOCS, m[1]);
    if (!existsSync(target)) {
      flag("error", file, `cross-link missing: docs/${m[1]}`);
    }
  }
}

console.log(`\n${mdFiles.length} docs scanned — ${problems} dead reference(s), ${warnings} warning(s).`);
process.exit(problems > 0 ? 1 : 0);
