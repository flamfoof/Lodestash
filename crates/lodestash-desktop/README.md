# lodestash-desktop

The Tauri 2 desktop shell — the target frontend for Lodestash. Hosts a Svelte 5 + TypeScript app
in a WebView2 window and talks to the Rust core over `#[tauri::command]` plus (from B3) the
`lodestash://` binary protocol.

The B7 cutover (2026-08-07) retired `lodestash-ui` (egui) — this shell is the app; the port plan
and its milestones are in `docs/KANBAN.md`.

## Layout

```
crates/lodestash-desktop/
  src/            Rust: window, commands, (later) the Engine mutex + pump thread
  tauri.conf.json window, CSP, bundle
  icons/          app icon — placeholder, regenerate from docs/design/icons/logo.svg
  ui/             the Svelte frontend
    src/lib/      shell components (TopBar, Sidebar, Icon, Screen) + view state
    src/styles/   app.css (imports docs/design/tokens.css) + bundled @font-face
    public/       icons.svg sprite, the three bundled TTFs
```

Tokens are **not** copied here: `ui/src/styles/app.css` imports `docs/design/tokens.css` at its
canonical path so there is exactly one token file in the repo.

## Running it

**A debug build starts its own frontend server.** `tauri.conf.json` points `devUrl` at
`http://localhost:5173`, so a debug window is the browser's "can't reach this page" unless Vite is
answering. The binary now guarantees it (`src/devserver.rs`): it probes the port, starts
`bun run dev` when nothing answers, waits for it, and stops the one it started when the app exits.
That holds however you launch — `cargo run`, an IDE run button, or the built exe — so there is no
ordering left to get wrong. A server that was already listening is reused and left running.

Everything else runs **from the repo root**. The root `package.json` is a task runner with no
dependencies of its own, and `scripts/dev.mjs` still exists for the one-command release path and for
seeing Vite's own output in the terminal.

**Bun** is the package manager and script runner — `bun.lock` is the committed lockfile and there is
no `package-lock.json`. No Tauri CLI is installed, deliberately (see `docs/TECH-STACK.md` §1 on
keeping the frontend to four dependencies).

```sh
# once
bun run setup         # bun install into ui/

bun run dev           # vite on :5173 + cargo run -p lodestash-desktop, one terminal
bun run dev:release   # bun run build, then cargo run --release — serves ui/dist, no dev server
```

`bun run dev` reuses a Vite already answering on :5173 rather than starting a second one, and takes
down the one it started when the app exits — the same rule the binary itself follows, so the two
never fight over the port. `npm run` works the same — the root scripts only shell
out, so nothing is installed there.

The two halves stay separately runnable when you want them apart; this is all the root scripts do:

```sh
bun run --cwd crates/lodestash-desktop/ui dev
cargo run -p lodestash-desktop
```

`bun run build` runs `svelte-check` first, so a type error fails the build rather than shipping.

## Status

B2 scaffold. The app frame, tokens, fonts, icon sprite, the 11 nav destinations and 12 stub routes
render, and one command (`app_info`) proves the IPC channel. There is **no catalog behind it yet** —
the status bar says so, and `engine_ready` is `false` until `lodestash-app` exposes `Engine`.


## Isolated MCP verification

`cargo test -p lodestash-desktop --test mcp_stdio --test mcp_workflow` launches the real desktop
binary with `--mcp` against disposable fixtures. The workflow uses a local stub identification
writer and fixture-owned approvals, never the configured AI provider or real asset library.

For isolated instances, set `LODESTASH_CATALOG`, `LODESTASH_CONFIG` and `LODESTASH_HANDSHAKE` to
absolute file paths before launching. Unset variables keep the normal OS locations. Both app and
headless child must use the same paths to communicate. These are launch-time configuration,
not agent arguments or an authorization override; catalog identity and normal policy still apply.
Tests set all three so they cannot read user consent or discover the user's running app.
