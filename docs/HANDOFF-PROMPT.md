# Handoff prompt — paste this at the start of the next session

> Copy this whole file into the first message of the session (or point the agent at it).
> It is the fast path into [`HANDOFF.md`](HANDOFF.md) §1.9 + the board. The design docs still
> win on any question of what the software *should* do — this file only says where things are.

---

You are picking up **Lodestash** (`G:\GameDev\asset_viewer`), a local-first desktop app that
catalogues Unity game assets (3D/audio/image) — Rust core + a Tauri 2 / Svelte 5 shell.

**Start here, in order:**

1. Read [`docs/HANDOFF.md`](HANDOFF.md) **§1.9** (current handoff, 2026-08-12) — what shipped,
   what's left, the traps. Then `docs/KANBAN-SVELTE.md` (the living board — the 🟡/🔴/⚪/🔵 lanes
   are the backlog) and [`CLAUDE.md`](../CLAUDE.md) (the design-conformance rule — docs are the
   source of truth; any divergence gets written back into the doc in the same change).
2. Sanity-check the tree before touching anything: `cargo test --workspace`, then
   `node scripts/check-docs.mjs` (must print **0 dead reference(s), 0 warning(s)**), then
   `cargo fmt --all`.

**Working agreements (invariants — do not break without a spec change):**

- **Single writer.** Every catalog write goes through the desktop's `Writer` thread (`Op`). Agent
  plane mutations arrive as `AgentWrite` and cross the ADR-0002 write bridge — never write on a
  command thread, never add a second catalog writer.
- **Agent planes are 1:1.** A new agent operation = one declaration in `lodestash-protocol`'s
  `operations!` list + a `ServerState` method + a REST route. The MCP tool and the allow-list gate
  come from the declaration; a route without one — or a declaration without a route — fails
  `lodestash-api/tests/protocol_catalog.rs`, which is the point.
- **Non-destructive.** Original assets + the Unity cache are never modified or moved; `file`/`mesh`
  pulls materialize to our own cache/scratch only. The 256 MB in-pack cap guards the **thumb route
  only** (ADR-0005).
- **Status lifecycle** `discovered → indexed → identified → confirmed`; user-facing terms are
  **Identified / Unidentified**. `media_type` stays the 7-value enum.
- **Verification bar:** Rust green, svelte-check 0/0 (`bun run check` in `ui/`), e2e **52/52**
  (run with **node**, not bun — bun's ws can't handshake WebView2 CDP), `check-docs` 0/0.

**The open work, in priority order** (all carded on the board):

1. **Import dialog §5.2** — `add_source_dialog` is a plain folder picker. Recommended:
   discovery-dialog-only (pick folder → crawl → modal listing discovered packs, group/select-all,
   "import as libraries"); packs browsable in place. The full spec needs a new "pack folder as a
   browsable source" model.
2. **Staging-commit half** — Recent-exports list, batch undo, and the top-bar project switcher
   popover (FEATURES §7.1). Transport + approval gate exist; the shell half is the work.
3. **UPM C# Unity plugin** — needs a real Unity install to build/verify.
4. **Shell polish** — onboarding two-step; eviction/`keep-extracted`; per-entry "Extract
   selected"; trait usage-note editing; inspector relink/provenance; Ctrl/Cmd-A; LINKS full
   harvest; variants + color search.
5. **Doc drift** — the 🟠 row in KANBAN-SVELTE (DESIGN-GUIDELINES §5, ALIGNMENT-AUDIT, ROADMAP,
   MILESTONES, PACK-DIRECTORY-NAV §1, USER-GUIDE).

**Traps (abridged — full list in HANDOFF §1.9/§6):**

- `cargo build -p lodestash-desktop` before e2e (the harness mtime-guards stale binaries).
- The harness polls + dismisses the onboarding panel each run; a new overlay must do the same.
- `promote_statuses` returns `Vec<i64>`; `record_op` takes a redo-forward 3rd arg (`None` when the
  payload is the forward); the migration-count test asserts **17**.
- Commit with explicit paths — `.reasonix/` and `reasonix.toml` stay untracked. `cargo fmt --all`
  before committing. Never `git add -A`.
- Known open defects (2026-08-01 review, still unfixed): in-pack pulls unbounded in RAM;
  `poll_import_queue` double-imports on retry; cancelled extraction stamped unpacked; `gate` 500
  not 404; `ack_import` accepts non-queued ids; `unmaterialize_package` leaks deduped bytes;
  `apply_mapping` alias arm doesn't escape `_`/`%`.
