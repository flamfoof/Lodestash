/**
 * Launch the real desktop app and hand back a Playwright page attached to it.
 *
 * WebView2 is Edge, so it speaks CDP natively — no WebDriver, no mocks. The app runs with its
 * real catalog connection and its real `ls://` handler, which is the whole point: these checks
 * exist to catch the failures that unit tests structurally cannot see, like an image element
 * whose src 404s.
 *
 * The debugging port is opened here and only here, by env var. It must never be configured in
 * `tauri.conf.json` — anything on localhost can drive the app and read the catalog through it.
 */
import { spawn, spawnSync } from "node:child_process";
import { setTimeout as sleep } from "node:timers/promises";
import { fileURLToPath } from "node:url";
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
 * Start Vite (unless already running) and the app, then attach.
 * Returns `{ page, close }`; always call `close`.
 */
export async function launch() {
  const spawned = [];

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
    },
  });
  spawned.push(app);

  if (!(await reachable(`http://127.0.0.1:${CDP_PORT}/json/version`, 30_000))) {
    throw new Error("WebView2 did not expose CDP — is the app built? (cargo build -p lodestash-desktop)");
  }

  const browser = await chromium.connectOverCDP(`http://127.0.0.1:${CDP_PORT}`);
  const page = browser.contexts()[0].pages()[0];
  await page.waitForSelector(".card, .msg", { timeout: 15_000 });

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
