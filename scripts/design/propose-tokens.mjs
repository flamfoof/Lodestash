#!/usr/bin/env node
// propose-tokens.mjs — print a token declaration for every value the prototype draws that
// tokens.css does not name yet, using the naming rules of plan P6 Task 6.1. A helper for a
// re-import: review the output, paste what is right into tokens.css, then run check-tokens.mjs.
//
//   node scripts/design/propose-tokens.mjs

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { check, drawnValues, parseTokens } from "./check-tokens.mjs";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const css = fs.readFileSync(path.join(ROOT, "docs", "design", "tokens.css"), "utf8");
const vocab = JSON.parse(fs.readFileSync(path.join(ROOT, "docs", "design", "vocabulary.json"), "utf8"));

// The semantic name of each base colour. A base not listed here is named by its hex.
export const BASE_NAMES = {
  "#0e1117": "bg", "#161b26": "surface", "#1e2533": "surface-raised", "#232c3d": "overlay",
  "#2c3547": "border", "#3a465c": "border-strong", "#e6e9f0": "text", "#a7b0c0": "text-muted",
  "#6e7689": "text-dim", "#e7b765": "gold", "#ffd98a": "gold-bright", "#49d6c9": "arcane",
  "#7bf0e5": "arcane-bright", "#a98bff": "violet", "#49c9e0": "type-sound", "#6fb1ff": "type-image",
  "#5bd897": "success", "#f2b84b": "warning", "#f2666e": "danger", "#8a93a6": "status-unidentified",
  "#000000": "black", "#ffffff": "white", "#0a0c11": "backdrop",
  "#062028": "on-sound", "#08130c": "on-success", "#171207": "on-gold", "#0a0d13": "well",
  "#0b0e14": "sunken", "#12161f": "pane", "#141926": "viewer", "#20293a": "thumb",
  "#121722": "render-sky-top", "#222b3e": "render-sky-bottom", "#2f5b66": "wave-rest",
  "#4a5468": "wave-unidentified", "#39455c": "checker", "#5a6273": "text-faint",
  "#7e8aa0": "text-placeholder", "#8fe3b8": "unity-idle", "#b8e07a": "project-lime",
  "#ff9d6f": "project-coral", "#c9b6ff": "violet-bright", "#1a2130": "stage-center",
  "#0c1017": "stage-edge", "#3fbf7e": "success-deep", "#d9a54e": "gold-deep",
};
const GRADIENT_NAMES = {
  "linear-gradient(#49d6c90d, transparent)": "arcane-wash",
  "linear-gradient(#5bd897, #3fbf7e)": "success",
  "linear-gradient(#e7b765, #d9a54e)": "gold",
  "linear-gradient(90deg, #49d6c9, #7bf0e5)": "arcane-bar",
  "linear-gradient(90deg, #e7b765, #ffd98a)": "gold-bar",
  "linear-gradient(90deg, #e7b765, transparent)": "gold-fade",
  "linear-gradient(105deg, transparent, #ffd98a59, transparent)": "shimmer",
  "radial-gradient(120% 120% at 50% 20%, #1a2130, #0c1017)": "stage",
  "radial-gradient(1200px 600px at 80% -10%, #e7b7650f, transparent 60%), radial-gradient(900px 500px at -10% 110%, #49d6c90d, transparent 60%)": "app",
  "linear-gradient(45deg, #39455c 25%, transparent 25%), linear-gradient(-45deg, #39455c 25%, transparent 25%), linear-gradient(45deg, transparent 75%, #39455c 75%), linear-gradient(-45deg, transparent 75%, #39455c 75%)": "checker",
  "linear-gradient(#ffffff08 1px, transparent 1px), linear-gradient(90deg, #ffffff08 1px, transparent 1px)": "grid",
};

const pct = (aa) => Math.round((parseInt(aa, 16) / 255) * 100);
const colorName = (hex) => {
  const base = hex.slice(0, 7);
  const name = BASE_NAMES[base] ?? base.slice(1);
  return hex.length > 7 ? `--ls-${name}-a${pct(hex.slice(7))}` : `--ls-${name}`;
};
const num = (v) => String(parseFloat(v)).replace(".", "_");
const shadowName = (canon) => {
  const inset = canon.startsWith("inset ");
  const [x, y, blur, spread, color] = canon.replace("inset ", "").split(" ");
  const base = BASE_NAMES[color.slice(0, 7)] ?? color.slice(1, 7);
  const alpha = color.length > 7 ? `-a${pct(color.slice(7))}` : "";
  if (parseFloat(x) === 0 && parseFloat(y) === 0 && parseFloat(blur) === 0)
    return `--ring-${inset ? "inset-" : ""}${base}-${num(spread)}${alpha}`;
  if (parseFloat(x) === 0 && parseFloat(y) === 0) return `--glow-${base}-${num(blur)}${alpha}`;
  return `--shadow-${num(y)}-${num(blur)}${alpha}`;
};
const shadowCss = (canon) => {
  const inset = canon.startsWith("inset ");
  const [x, y, blur, spread, color] = canon.replace("inset ", "").split(" ");
  const len = (v) => (parseFloat(v) === 0 ? "0" : v);
  return `${inset ? "inset " : ""}${[x, y, blur, spread].map(len).join(" ")} ${color}`;
};

const NAMERS = {
  color: (c) => [colorName(c), c],
  fontSize: (v) => [`--fs-${num(v)}`, v],
  fontWeight: (v) => [`--fw-${v}`, v],
  fontFamily: (v) => [`--ff-${v.replace(/\s+/g, "-")}`, `"${v}"`],
  letterSpacing: (v) => [`--track-${num(v)}`, v],
  radius: (v) => (v === "pill" ? ["--r-pill", "999px"] : [`--r-${num(v)}`, v]),
  space: (v) => [`--space-${num(v)}`, v],
  borderWidth: (v) => [`--bw-${num(v)}`, v],
  shadow: (c) => [shadowName(c), shadowCss(c)],
  gradient: (g, i) => [`--grad-${GRADIENT_NAMES[g] ?? `g${i}`}`, g],
  duration: (v) => [`--dur-${parseInt(v, 10)}`, v],
  opacity: (v) => [`--op-${Math.round(Number(v) * 100)}`, v],
};

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const drawn = drawnValues(vocab);
  const { problems } = check(css, vocab);
  const missing = new Set(problems.filter((p) => p.startsWith("drawn, no token:")).map((p) => p.split("   (")[0].slice(17)));
  const taken = new Set(parseTokens(css).map((t) => t.name));
  for (const [fam, values] of Object.entries(drawn)) {
    const lines = [];
    let i = 0;
    for (const canon of values.keys()) {
      if (!missing.has(`${fam} ${canon}`)) continue;
      let [name, value] = NAMERS[fam](canon, i++);
      while (taken.has(name)) name += "-x";
      taken.add(name);
      lines.push(`  ${name}: ${value};`);
    }
    if (lines.length) console.log(`  /* ---- ${fam} ---- */\n${lines.sort().join("\n")}\n`);
  }
}
