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

**Bun** is the package manager and script runner — `bun.lock` is the committed lockfile and there is
no `package-lock.json`. Two terminals; no Tauri CLI is installed, deliberately (see
`docs/TECH-STACK.md` §1 on keeping the frontend to four dependencies).

```sh
# once
bun install --cwd crates/lodestash-desktop/ui

# terminal 1 — the frontend dev server on :5173 (debug builds load devUrl)
bun run --cwd crates/lodestash-desktop/ui dev

# terminal 2
cargo run -p lodestash-desktop
```

For a build that runs standalone, compile the frontend first — release builds load `ui/dist`
from the bundle rather than the dev server:

```sh
bun run --cwd crates/lodestash-desktop/ui build
cargo run --release -p lodestash-desktop
```

`bun run build` runs `svelte-check` first, so a type error fails the build rather than shipping.

## Status

B2 scaffold. The app frame, tokens, fonts, icon sprite, the 11 nav destinations and 12 stub routes
render, and one command (`app_info`) proves the IPC channel. There is **no catalog behind it yet** —
the status bar says so, and `engine_ready` is `false` until `lodestash-app` exposes `Engine`.
