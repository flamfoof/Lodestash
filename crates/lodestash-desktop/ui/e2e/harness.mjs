/**
 * Launch the real desktop app and hand back a Playwright page attached to it.
 *
 * WebView2 is Edge, so it speaks CDP natively — no WebDriver, no mocks. The app runs with its
 * real catalog connection and its real `lodestash://` handler, which is the whole point: these checks
 * exist to catch the failures that unit tests structurally cannot see, like an image element
 * whose src 404s.
 *
 * The debugging port is opened here and only here, by env var. It must never be configured in
 * `tauri.conf.json` — anything on localhost can drive the app and read the catalog through it.
 */
import { spawn, spawnSync } from "node:child_process";
import { setTimeout as sleep } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import fs from "node:fs";
import path from "node:path";
import { chromium } from "playwright-core";

const UI_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const REPO = path.resolve(UI_DIR, "..", "..", "..");
const BIN = path.join(REPO, "target", "debug", "lodestash-desktop.exe");
const CDP_PORT = 9222;
const VITE_URL = "http://localhost:5173";

/**
 * Kill a spawned process **and its descendants**, synchronously.
 *
 * Two things make this fussier than `child.kill()`:
 *
 * - `kill()` signals only the direct child. Vite is a grandchild — `cmd.exe` -> `bun` ->
 *   `node vite.js` — so killing the shell leaves Vite alive holding :5173.
 * - It must be **synchronous**. An async `spawn("taskkill")` loses the race against
 *   `process.exit()` and never runs, which looks exactly like no cleanup at all.
 *
 * A leaked Vite is not merely untidy: it inherits the runner's stdio, so a piped invocation
 * (`bun run e2e | ...`) never sees end-of-stream and appears to hang forever — long after the
 * checks have finished and reported.
 */
function killTree(child) {
  if (!child.pid) return;
  if (process.platform === "win32") {
    spawnSync("taskkill", ["/PID", String(child.pid), "/T", "/F"], { stdio: "ignore" });
  } else {
    try {
      process.kill(-child.pid, "SIGKILL");
    } catch {
      child.kill("SIGKILL");
    }
  }
}

async function reachable(url, ms) {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    try {
      const r = await fetch(url, { signal: AbortSignal.timeout(1000) });
      if (r.ok) return true;
    } catch {
      /* not up yet */
    }
    await sleep(250);
  }
  return false;
}

/**
 * Refuse to run against a binary older than the Rust it is supposed to be testing.
 *
 * Vite is rebuilt (or served live) every run, but `BIN` is whatever `cargo build` last produced —
 * so a Rust change lands in the frontend's *types* while the running app still serves the old
 * shape. The failure is vicious: a command returns JSON missing a field the UI now reads, the
 * component that reads it throws mid-render, and the DOM freezes half-updated. What you see is a
 * stale grid next to a correct result count, which reads like a caching bug anywhere but here.
 *
 * Cheap mtime comparison, not a build: the suite should say what is wrong, not silently take
 * minutes to fix it.
 */
function assertBinaryIsCurrent() {
  if (!fs.existsSync(BIN)) {
    throw new Error(`no ${BIN} — run: cargo build -p lodestash-desktop --bin lodestash-desktop`);
  }
  const built = fs.statSync(BIN).mtimeMs;
  const srcDirs = ["lodestash-desktop", "lodestash-app", "lodestash-core"].map((c) =>
    path.join(REPO, "crates", c, "src"),
  );
  let newest = 0;
  let newestPath = "";
  const walk = (dir) => {
    for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
      const p = path.join(dir, e.name);
      if (e.isDirectory()) walk(p);
      else if (e.name.endsWith(".rs")) {
        const m = fs.statSync(p).mtimeMs;
        if (m > newest) [newest, newestPath] = [m, p];
      }
    }
  };
  for (const d of srcDirs) if (fs.existsSync(d)) walk(d);
  if (newest > built) {
    throw new Error(
      `${path.relative(REPO, newestPath)} is newer than the built binary — the app would serve ` +
        `the old command shapes. Run: cargo build -p lodestash-desktop --bin lodestash-desktop`,
    );
  }
}

/**
 * Start Vite (unless already running) and the app, then attach.
 * Returns `{ page, close }`; always call `close`.
 */
export async function launch() {
  const spawned = [];
  assertBinaryIsCurrent();

  if (!(await reachable(VITE_URL, 1000))) {
    // A debug build loads devUrl, so the dev server has to be up before the window opens.
    // Spawned through cmd.exe: `shell: true` resolves to whatever shell the parent process was
    // started from, and under Git Bash that hangs without ever launching Vite.
    spawned.push(
      spawn("cmd.exe", ["/c", "bun", "run", "dev"], {
        cwd: UI_DIR,
        stdio: "ignore",
        windowsHide: true,
      }),
    );
    if (!(await reachable(VITE_URL, 30_000))) throw new Error("vite did not start on :5173");
  }

  const app = spawn(BIN, {
    stdio: "ignore",
    env: {
      ...process.env,
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${CDP_PORT}`,
      // The preview backfill is CPU-heavy (decimate + 4-view raster per model); a suite that
      // launches the app dozens of times would grind the machine into click timeouts. The
      // drain still runs for real users — this only skips it under e2e.
      LODESTASH_NO_PREVIEW_DRAIN: "1",
    },
  });
  spawned.push(app);

  if (!(await reachable(`http://127.0.0.1:${CDP_PORT}/json/version`, 30_000))) {
    throw new Error("WebView2 did not expose CDP — is the app built? (cargo build -p lodestash-desktop)");
  }

  const browser = await chromium.connectOverCDP(`http://127.0.0.1:${CDP_PORT}`);
  const page = browser.contexts()[0].pages()[0];
  await page.waitForSelector(".card, .msg", { timeout: 15_000 });

  // First-run onboarding (FEATURES §8) covers the whole UI and swallows clicks when the
  // user's config has `skip_onboarding` off — the suite tests the app, not the welcome
  // panel. The flag arrives over IPC a beat *after* the grid renders, so poll until the
  // panel is gone: dismiss it (a scrim click) whenever it is present, and keep checking
  // for the whole window in case it arrives late.
  const onboardDeadline = Date.now() + 6_000;
  while (Date.now() < onboardDeadline) {
    const dismissed = await page
      .evaluate(() => {
        const el = document.querySelector('[data-testid="onboarding"]');
        if (el) {
          el.click();
          return true;
        }
        return false;
      })
      .catch(() => false);
    if (dismissed) {
      await page.waitForTimeout(300); // let Svelte unmount the panel
    } else {
      await page.waitForTimeout(150);
    }
  }
  await page.waitForSelector('[data-testid="onboarding"]', { state: "detached", timeout: 2_000 }).catch(() => {});

  return {
    page,
    /** Call a `#[tauri::command]` directly, bypassing the UI. */
    invoke: (cmd, args) =>
      page.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a), [cmd, args ?? {}]),
    async close() {
      await browser.close().catch(() => {});
      for (const p of spawned.reverse()) killTree(p);
    },
  };
}
