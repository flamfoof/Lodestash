// The prototype's vocabulary as *declared* in its source, not only as rendered.
//
// A render walks the states the extractor visits; menus, dialogs and hover states it never opens
// would otherwise have no tokens. Every inline `style="…"` declaration in the markup, and every
// colour literal in the component script, is read here and counted in the same categories the
// render uses (lib/measure.mjs → vocabularyInPage). Template holes (`{{ … }}`) are skipped: they are
// filled from data the script computes, and the script's own colour literals cover those.

const COLOR_RE = /#[0-9a-fA-F]{3,8}\b|rgba?\([^)]*\)/g;

/** Split on `sep` at paren depth 0 (box-shadow lists, gradient arguments). */
function splitTop(s, sep) {
  const out = [];
  let depth = 0;
  let cur = "";
  for (const ch of s) {
    if (ch === "(") depth++;
    if (ch === ")") depth--;
    if (ch === sep && depth === 0) {
      out.push(cur);
      cur = "";
    } else cur += ch;
  }
  if (cur.trim()) out.push(cur);
  return out.map((x) => x.trim()).filter(Boolean);
}

const lenTokens = (v) => (v.match(/-?\d*\.?\d+px/g) ?? []).map((x) => `${parseFloat(x)}px`);

export function sourceVocabulary(src) {
  const out = {
    color: {}, background: {}, gradient: {}, border: {}, fontFamily: {}, fontSize: {},
    fontWeight: {}, letterSpacing: {}, radius: {}, padding: {}, gap: {}, shadow: {}, opacity: {},
    duration: {},
  };
  const bump = (cat, v) => {
    if (v === null || v === undefined || v === "") return;
    out[cat][v] = (out[cat][v] ?? 0) + 1;
  };
  const markupEnd = src.indexOf('<script type="text/x-dc"');
  const markup = markupEnd > 0 ? src.slice(0, markupEnd) : src;
  const script = markupEnd > 0 ? src.slice(markupEnd) : "";

  for (const m of markup.matchAll(/\sstyle="([^"]*)"/g)) {
    for (const decl of splitTop(m[1], ";")) {
      const i = decl.indexOf(":");
      if (i < 0) continue;
      const prop = decl.slice(0, i).trim().toLowerCase();
      const val = decl.slice(i + 1).trim();
      if (val.includes("{{")) continue;
      if (prop === "color") (val.match(COLOR_RE) ?? []).forEach((c) => bump("color", c));
      else if (prop === "background" || prop === "background-color" || prop === "background-image") {
        if (/gradient\(/.test(val)) bump("gradient", val);
        else (val.match(COLOR_RE) ?? []).forEach((c) => bump("background", c));
      } else if (/^border(-(top|right|bottom|left))?$/.test(prop) || prop === "outline") {
        const w = lenTokens(val)[0] ?? "1px";
        (val.match(COLOR_RE) ?? []).forEach((c) => bump("border", `${w} ${c}`));
      } else if (/^border(-(top|right|bottom|left))?-color$/.test(prop))
        (val.match(COLOR_RE) ?? []).forEach((c) => bump("border", `1px ${c}`));
      else if (prop === "border-radius") lenTokens(val).forEach((r) => bump("radius", r));
      else if (prop === "font-size") bump("fontSize", lenTokens(val)[0]);
      else if (prop === "font-weight") bump("fontWeight", val);
      else if (prop === "font-family") bump("fontFamily", val.split(",")[0].replace(/["']/g, "").trim());
      else if (prop === "letter-spacing") bump("letterSpacing", lenTokens(val)[0] ?? val.replace(/^\./, "0."));
      else if (prop === "padding" || prop.startsWith("padding-")) bump("padding", lenTokens(val).join(" "));
      else if (prop === "gap" || prop === "row-gap" || prop === "column-gap") lenTokens(val).forEach((g) => bump("gap", g));
      else if (prop === "box-shadow") splitTop(val, ",").forEach((s) => s !== "none" && bump("shadow", s));
      else if (prop === "opacity") bump("opacity", String(Number(val)));
      else if (prop === "transition" || prop === "animation")
        (val.match(/(?<![\w-])\d*\.?\d+m?s\b/g) ?? []).forEach((d) => bump("duration", d));
      else if (prop === "fill" || prop === "stroke") (val.match(COLOR_RE) ?? []).forEach((c) => bump("color", c));
    }
  }
  // Colours the script computes into template holes (status colours, bar colours, chip tints).
  for (const m of script.matchAll(/['"`](#[0-9a-fA-F]{3,8})['"`]|(rgba?\([^)]*\))/g)) bump("color", m[1] ?? m[2]);
  // Hex literals embedded in script strings (e.g. 'border:1px solid #2C3547').
  for (const m of script.matchAll(/['"`][^'"`\n]*?(#[0-9a-fA-F]{6})\b/g)) bump("color", m[1]);
  return out;
}
