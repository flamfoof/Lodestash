// In-page measurement shared by the contract extractor (prototype) and the design gate (app).
//
// Both sides must measure the same way, or a "difference" is just two rulers. Everything here
// runs inside the page via `page.evaluate`, so it is written as self-contained functions with no
// closures over module state.

/**
 * Resolve an anchor locator inside the page and measure the element.
 *
 * Locator forms (prototype side — the app side always uses `{ anchor: "name" }`):
 *   { anchor: "sidebar.nav" }              → [data-anchor="sidebar.nav"], nth match
 *   { css: "selector" }                    → document.querySelectorAll(selector)[nth]
 *   { text: "Library" }                    → the nth visible element whose *own* text (text nodes
 *                                            directly inside it, trimmed and whitespace-collapsed)
 *                                            equals the string exactly
 *   { placeholder: "filter by…" }          → the nth input/textarea with that placeholder
 * Modifiers applied in order: `within` (css scope), `nth` (default 0), `up` (ancestor hops),
 * `child` (index into the resulting element's element children).
 *
 * Returns { ok: true, box, style } or { ok: false, reason }.
 */
export function measureInPage(spec) {
  const visible = (el) => {
    const r = el.getBoundingClientRect();
    if (r.width === 0 && r.height === 0) return false;
    const cs = getComputedStyle(el);
    return cs.visibility !== "hidden" && cs.display !== "none";
  };
  const ownText = (el) =>
    [...el.childNodes]
      .filter((n) => n.nodeType === 3)
      .map((n) => n.textContent)
      .join(" ")
      .replace(/\s+/g, " ")
      .trim();
  const scope = spec.within ? document.querySelector(spec.within) : document.body;
  if (!scope) return { ok: false, reason: `scope not found: ${spec.within}` };

  let candidates;
  if (spec.anchor) candidates = [...scope.querySelectorAll(`[data-anchor="${spec.anchor}"]`)];
  else if (spec.css) candidates = [...scope.querySelectorAll(spec.css)];
  else if (spec.placeholder)
    candidates = [...scope.querySelectorAll("input,textarea")].filter(
      (e) => e.getAttribute("placeholder") === spec.placeholder,
    );
  else if (spec.text !== undefined)
    candidates = [...scope.querySelectorAll("*")].filter((e) => ownText(e) === spec.text);
  else return { ok: false, reason: "locator has no anchor/css/text/placeholder" };

  candidates = candidates.filter(visible);
  const nth = spec.nth ?? 0;
  let el = candidates[nth];
  if (!el) return { ok: false, reason: `no visible match #${nth} (found ${candidates.length})` };
  for (let i = 0; i < (spec.up ?? 0); i++) {
    el = el.parentElement;
    if (!el) return { ok: false, reason: `ran out of ancestors at hop ${i + 1}` };
  }
  if (spec.child !== undefined) {
    el = el.children[spec.child];
    if (!el) return { ok: false, reason: `no child #${spec.child}` };
  }

  const hex = (c) => {
    const m = c.match(/rgba?\(([^)]+)\)/);
    if (!m) return c;
    const p = m[1].split(/[\s,/]+/).filter(Boolean).map(Number);
    const [r, g, b] = p;
    const a = p.length > 3 ? p[3] : 1;
    const h = (n) => Math.round(n).toString(16).padStart(2, "0");
    if (a === 0) return "transparent";
    return `#${h(r)}${h(g)}${h(b)}${a < 1 ? h(a * 255) : ""}`;
  };
  const px = (v) => (v === "normal" ? v : Math.round(parseFloat(v) * 10) / 10);
  const r = el.getBoundingClientRect();
  const cs = getComputedStyle(el);
  const round = (n) => Math.round(n * 2) / 2;
  return {
    ok: true,
    tag: el.tagName.toLowerCase(),
    box: { x: round(r.x), y: round(r.y), w: round(r.width), h: round(r.height) },
    style: {
      fontFamily: cs.fontFamily.split(",")[0].replace(/["']/g, "").trim(),
      fontSize: px(cs.fontSize),
      fontWeight: Number(cs.fontWeight),
      lineHeight: px(cs.lineHeight),
      letterSpacing: px(cs.letterSpacing),
      textTransform: cs.textTransform,
      color: hex(cs.color),
      background: hex(cs.backgroundColor),
      backgroundImage: cs.backgroundImage === "none" ? null : cs.backgroundImage,
      borderWidth: px(cs.borderTopWidth),
      borderColor: parseFloat(cs.borderTopWidth) > 0 ? hex(cs.borderTopColor) : null,
      radius: px(cs.borderTopLeftRadius),
      padding: [cs.paddingTop, cs.paddingRight, cs.paddingBottom, cs.paddingLeft].map(px),
      gap: cs.rowGap === "normal" && cs.columnGap === "normal" ? null : [px(cs.rowGap), px(cs.columnGap)],
      shadow: cs.boxShadow === "none" ? null : cs.boxShadow,
      opacity: Number(cs.opacity),
    },
  };
}

/**
 * Walk every visible element under `rootSelector` and count the design vocabulary actually
 * drawn: colours, type, radii, spacing, shadows, motion. Returns { category: { value: count } }.
 */
export function vocabularyInPage(rootSelector) {
  const root = document.querySelector(rootSelector) ?? document.body;
  const out = {
    color: {}, background: {}, gradient: {}, border: {}, fontFamily: {}, fontSize: {}, fontWeight: {},
    letterSpacing: {}, radius: {}, padding: {}, gap: {}, shadow: {}, opacity: {}, duration: {},
  };
  const bump = (cat, v) => {
    if (v === null || v === undefined || v === "") return;
    out[cat][v] = (out[cat][v] ?? 0) + 1;
  };
  const hex = (c) => {
    const m = c.match(/rgba?\(([^)]+)\)/);
    if (!m) return c;
    const p = m[1].split(/[\s,/]+/).filter(Boolean).map(Number);
    const a = p.length > 3 ? p[3] : 1;
    if (a === 0) return null;
    const h = (n) => Math.round(n).toString(16).padStart(2, "0");
    return `#${h(p[0])}${h(p[1])}${h(p[2])}${a < 1 ? h(a * 255) : ""}`;
  };
  for (const el of [root, ...root.querySelectorAll("*")]) {
    const r = el.getBoundingClientRect();
    if (r.width === 0 && r.height === 0) continue;
    const cs = getComputedStyle(el);
    if (cs.visibility === "hidden" || cs.display === "none") continue;
    const hasText = [...el.childNodes].some((n) => n.nodeType === 3 && n.textContent.trim());
    if (hasText) {
      bump("color", hex(cs.color));
      bump("fontFamily", cs.fontFamily.split(",")[0].replace(/["']/g, "").trim());
      bump("fontSize", cs.fontSize);
      bump("fontWeight", cs.fontWeight);
      if (cs.letterSpacing !== "normal") bump("letterSpacing", cs.letterSpacing);
    }
    bump("background", hex(cs.backgroundColor));
    if (cs.backgroundImage.includes("gradient")) bump("gradient", cs.backgroundImage);
    // Every side: accent borders are often one side only (a card's 3px left edge), and a walk
    // that reads the top border alone never sees them. One count per distinct edge per element.
    const edges = new Set();
    for (const side of ["Top", "Right", "Bottom", "Left"]) {
      const w = cs[`border${side}Width`];
      if (parseFloat(w) > 0) edges.add(`${w} ${hex(cs[`border${side}Color`])}`);
    }
    for (const edge of edges) bump("border", edge);
    if (parseFloat(cs.borderTopLeftRadius) > 0) bump("radius", cs.borderTopLeftRadius);
    const pad = [cs.paddingTop, cs.paddingRight, cs.paddingBottom, cs.paddingLeft];
    if (pad.some((p) => parseFloat(p) > 0)) bump("padding", pad.join(" "));
    if (cs.display.includes("flex") || cs.display.includes("grid")) {
      if (cs.rowGap !== "normal" && parseFloat(cs.rowGap) > 0) bump("gap", cs.rowGap);
      if (cs.columnGap !== "normal" && parseFloat(cs.columnGap) > 0 && cs.columnGap !== cs.rowGap)
        bump("gap", cs.columnGap);
    }
    if (cs.boxShadow !== "none") bump("shadow", cs.boxShadow);
    if (Number(cs.opacity) < 1) bump("opacity", cs.opacity);
    for (const d of `${cs.transitionDuration},${cs.animationDuration}`.split(",")) {
      if (parseFloat(d) > 0) bump("duration", d.trim());
    }
  }
  return out;
}
