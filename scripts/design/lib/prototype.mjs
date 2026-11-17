// Drive the design prototype in a headless browser: serve docs/design over loopback, route its
// Google Fonts request to the app's bundled TTFs (so measurement is identical offline), and step
// it into a named state.

import { createRequire } from "node:module";
import { pathToFileURL, fileURLToPath } from "node:url";
import http from "node:http";
import fs from "node:fs";
import path from "node:path";

export const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
export const DESIGN_DIR = path.join(ROOT, "docs", "design");
export const PROTOTYPE = "Lodestash Prototype.dc.html";
export const VIEWPORT = { width: 1440, height: 900 };

/** playwright-core from the web UI's node_modules (P6), falling back to the old UI's. */
export function loadPlaywright() {
  const roots = [
    process.env.PLAYWRIGHT_ROOT,
    path.join(ROOT, "web"),
    path.join(ROOT, "crates", "lodestash-desktop", "ui"),
  ].filter(Boolean);
  for (const r of roots) {
    try {
      return createRequire(pathToFileURL(path.join(r, "package.json")).href)("playwright-core");
    } catch {
      /* try the next root */
    }
  }
  throw new Error(`playwright-core not found under: ${roots.join(", ")} — run bun install in web/`);
}

/** The bundled fonts, wherever the UI keeps them. */
export function fontsDir() {
  const dirs = [path.join(ROOT, "web", "public", "fonts"), path.join(ROOT, "crates", "lodestash-desktop", "ui", "public", "fonts")];
  const found = dirs.find((d) => fs.existsSync(path.join(d, "Inter.ttf")));
  if (!found) throw new Error(`bundled fonts not found in: ${dirs.join(", ")}`);
  return found;
}

const TYPES = {
  ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml",
  ".png": "image/png", ".jpg": "image/jpeg", ".json": "application/json", ".ttf": "font/ttf",
};

/** Serve `dir` on 127.0.0.1 with path-traversal refused. Returns { base, close }. */
export async function serveDir(dir) {
  const rootDir = path.resolve(dir);
  const server = http.createServer((req, res) => {
    const rel = decodeURIComponent(new URL(req.url, "http://x").pathname).replace(/^\/+/, "");
    const full = path.resolve(rootDir, rel);
    if (!full.startsWith(rootDir) || !fs.existsSync(full) || fs.statSync(full).isDirectory()) {
      res.writeHead(404);
      res.end();
      return;
    }
    // The fonts are served from a second loopback origin: without CORS the browser refuses them and
    // the page silently renders in fallback fonts (which is how contract.json was first measured).
    res.writeHead(200, {
      "content-type": TYPES[path.extname(full)] ?? "application/octet-stream",
      "access-control-allow-origin": "*",
    });
    fs.createReadStream(full).pipe(res);
  });
  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  return { base: `http://127.0.0.1:${server.address().port}/`, close: () => server.close() };
}

/** CSS equivalent to the prototype's Google Fonts link, pointing at local files. */
function localFontCss(fontBase) {
  const face = (family, file, weights) =>
    `@font-face{font-family:"${family}";src:url("${fontBase}${file}") format("truetype");font-weight:${weights};font-display:block}`;
  return [
    face("Inter", "Inter.ttf", "100 900"),
    face("Cinzel", "Cinzel.ttf", "400 900"),
    face("JetBrains Mono", "JetBrainsMono.ttf", "100 800"),
  ].join("\n");
}

/**
 * Launch a browser on the prototype. Returns { browser, page, close, errors }.
 * The browser channel defaults to Edge (always present on Windows); set LS_BROWSER_CHANNEL=""
 * to use Playwright's bundled Chromium.
 */
export async function openPrototype({ viewport = VIEWPORT } = {}) {
  const { chromium } = loadPlaywright();
  const site = await serveDir(DESIGN_DIR);
  const fonts = await serveDir(fontsDir());
  const channel = process.env.LS_BROWSER_CHANNEL ?? "msedge";
  const browser = await chromium.launch({ headless: true, ...(channel ? { channel } : {}) });
  const page = await browser.newPage({ viewport, deviceScaleFactor: 1 });
  const errors = [];
  page.on("pageerror", (e) => errors.push(`pageerror: ${e.message}`));
  await page.route(/fonts\.googleapis\.com/, (route) =>
    route.fulfill({ status: 200, contentType: "text/css", body: localFontCss(fonts.base) }),
  );
  await page.route(/fonts\.gstatic\.com/, (route) => route.abort());
  await page.goto(site.base + encodeURIComponent(PROTOTYPE), { waitUntil: "networkidle", timeout: 30000 });
  await assertFontsLoaded(page);
  await settle(page);
  const close = async () => {
    await browser.close();
    site.close();
    fonts.close();
  };
  return { browser, page, close, errors };
}

/**
 * Load the three bundled families and fail unless every one of them is in use. A page whose fonts
 * did not load still reports the declared family in `getComputedStyle`, so a measurement taken in
 * fallback fonts looks right everywhere except in its widths — check, never assume.
 */
export async function assertFontsLoaded(page) {
  const problems = await page.evaluate(async () => {
    const wanted = ['400 12px "Inter"', '700 16px "Cinzel"', '400 11px "JetBrains Mono"'];
    await Promise.all(wanted.map((f) => document.fonts.load(f).catch(() => [])));
    await document.fonts.ready;
    const failed = [...document.fonts].filter((f) => f.status === "error").map((f) => `${f.family} failed to load`);
    const missing = wanted.filter((f) => !document.fonts.check(f)).map((f) => `${f} is not available`);
    return [...failed, ...missing];
  });
  if (problems.length) throw new Error(`bundled fonts: ${problems.join("; ")}`);
}

/** Let animations (lsRise is 360 ms) finish before measuring. */
export async function settle(page, ms = 700) {
  await page.waitForTimeout(ms);
}

/** Click the nth visible element whose own text equals `text` exactly. */
export async function clickText(page, text, nth = 0) {
  const ok = await page.evaluate(
    ([text, nth]) => {
      const own = (el) =>
        [...el.childNodes].filter((n) => n.nodeType === 3).map((n) => n.textContent).join(" ").replace(/\s+/g, " ").trim();
      const hits = [...document.querySelectorAll("*")].filter((e) => {
        const r = e.getBoundingClientRect();
        return own(e) === text && r.width > 0 && r.height > 0;
      });
      const el = hits[nth];
      if (!el) return false;
      el.click();
      return true;
    },
    [text, nth],
  );
  if (!ok) throw new Error(`clickText: no visible element with own text "${text}" (#${nth})`);
  await settle(page);
}
