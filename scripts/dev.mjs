/**
 * Run the desktop app from the repo root — one command instead of two terminals.
 *
 * A debug build loads `devUrl` (`tauri.conf.json`), so Vite has to be listening on :5173 *before*
 * the window opens; that ordering is the only reason the two-terminal dance existed. This starts
 * Vite, waits for it, hands the terminal to `cargo run`, and takes Vite back down on the way out.
 *
 * The binary does the same for itself now (`crates/lodestash-desktop/src/devserver.rs`), so a launch
 * that skips this script still gets a server. This stays for the release path and for watching
 * Vite's output in the same terminal; both reuse a server that is already answering.
 *
 * `--release` skips Vite entirely: a release build loads `ui/dist` from the bundle, so the
 * frontend is compiled first instead and nothing ever listens on :5173.
 *
 * The spawn shapes are the ones `crates/lodestash-desktop/ui/e2e/harness.mjs` paid for: Vite is a
 * grandchild (`cmd.exe` -> `bun` -> `node vite.js`), so only a tree-kill reaches it, and
 * `shell: true` resolves to whatever shell *this* process was started from, which hangs under Git
 * Bash without ever launching anything.
 */
import { spawn, spawnSync } from "node:child_process";
import { once } from "node:events";
import { setTimeout as sleep } from "node:timers/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const REPO = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const UI_DIR = path.join(REPO, "crates", "lodestash-desktop", "ui");
const VITE_URL = "http://localhost:5173";
const release = process.argv.includes("--release");

/** Bun is a shim rather than a plain exe on most Windows installs, so go through `cmd.exe`. */
const bun = (args, opts) =>
  process.platform === "win32"
    ? spawn("cmd.exe", ["/c", "bun", ...args], { cwd: UI_DIR, ...opts })
    : spawn("bun", args, { cwd: UI_DIR, ...opts });

function killTree(child) {
  if (!child.pid) return;
  if (process.platform === "win32") {
    spawnSync("taskkill", ["/PID", String(child.pid), "/T", "/F"], { stdio: "ignore" });
  } else {
    child.kill("SIGKILL");
  }
}

async function reachable(url, ms) {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    try {
      if ((await fetch(url, { signal: AbortSignal.timeout(1000) })).ok) return true;
    } catch {
      /* not up yet */
    }
    await sleep(250);
  }
  return false;
}

const spawned = [];
if (release) {
  // `bun run build` gates on svelte-check, so a type error fails here rather than shipping.
  const build = bun(["run", "build"], { stdio: "inherit" });
  const [code] = await once(build, "exit");
  if (code !== 0) process.exit(code ?? 1);
} else if (await reachable(VITE_URL, 500)) {
  console.log("vite already answering on :5173 — reusing it");
} else {
  console.log("starting vite on :5173 ...");
  spawned.push(bun(["run", "dev"], { stdio: "ignore", windowsHide: true }));
  if (!(await reachable(VITE_URL, 30_000))) {
    for (const p of spawned) killTree(p);
    throw new Error("vite did not start on :5173");
  }
}

// cargo is a real exe on every platform, so it needs no shell; spawning it directly also keeps
// Ctrl+C from landing on a `cmd.exe` that would stop to ask "Terminate batch job (Y/N)?".
const app = spawn("cargo", ["run", ...(release ? ["--release"] : []), "-p", "lodestash-desktop"], {
  cwd: REPO,
  stdio: "inherit",
});

const stopVite = () => {
  for (const p of spawned.splice(0)) killTree(p);
};
process.once("exit", stopVite);
process.once("SIGINT", () => {
  killTree(app);
  stopVite();
  process.exit(130);
});

const [code] = await once(app, "exit");
stopVite();
process.exit(code ?? 0);
