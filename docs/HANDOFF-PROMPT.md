# Session handoff prompt

Paste the block below into a fresh session. It is deliberately short — everything durable lives in
[`HANDOFF.md`](HANDOFF.md) and the design docs; this only orients and states the agreements.

---

Lodestash — session handoff (picks up after 2026-07-24, `fd72eb6`)

You're continuing work on **Lodestash** at `G:\GameDev\asset_viewer` — a local-first Rust/egui
desktop app to identify, view and retrieve game assets for Unity.

**Start by invoking these, in this order:** `/caveman:caveman full` (terse prose; code, commits and
security warnings stay written normally), `/ponytail full` (the laziest solution that works — stdlib
and existing deps before new ones, no speculative abstraction, mark deliberate shortcuts with a
`ponytail:` comment naming the ceiling), `/caveman:cavecrew` (read it to decide *when* to delegate:
`cavecrew-investigator` to locate code, `cavecrew-builder` for a bounded 1–2 file edit,
`cavecrew-reviewer` for a diff — their output is compressed, which matters because `app.rs` is ~12k
lines), and `/claudenator` (status/enable — decompose into delegatable new-file units; never point it
at a crate you are mid-edit on).

**Read first, in this order:**
1. `CLAUDE.md` — the design-conformance rule: `docs/` is the source of truth. Code conforms; any
   divergence is written back into the doc **in the same change**.
2. `docs/HANDOFF.md` — state of play, settled decisions, traps. Current as of `fd72eb6`.
3. `docs/KANBAN.md` — every card carries a *shipped / remaining* note. Read the card before working it.
4. The doc owning your area (`ENGINE-API.md`, `DATA-MODEL.md`, `IDENTIFICATION-AND-SEARCH.md`,
   `AI-PIPELINE.md`, `INGESTION.md`, `UI-SPEC.md`, `TECH-STACK.md`, `docs/design/FEATURES.md`).

**Working agreements.** Commit straight to `main`, one slice per commit, message ending `[skip ci]`
and the `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>` trailer; PRs/CI stay suspended
until app-complete. `cargo fmt --all` before every commit. Say plainly what you did **not** verify.

**Where it stands.** Grid, viewers, ingest, technical facets, hybrid search, local tagging, the
dependency graph and the agent plane (nine ops, MCP↔REST 1:1, export gated on a desktop approval)
all shipped. **Unity Assets now works end to end** (FEATURES.md §5): import a folder of
`.unitypackage` files as pinned libraries, open any indexed pack — reading its manifest on demand if
the catalog knows it only by name — walk its contents as a folder tree, and preview one file pulled
straight out of the archive into the grid's own viewers, with no catalog row and no unpack. A pinned
pack survives a restart. This machine's library: ~1728 catalogued assets, 87 packs, 19 tagged — so
the health meter honestly reads 19 identified until a full Identify pass runs (~5 h of CPU, manual
and resumable).

**Invariants — do not break without a spec change.** Local-first (embeddings never leave the
machine); non-destructive, whose **one** documented exception is the confirmation-gated
delete-from-disk; MCP↔REST 1:1 (a new op = `ServerState` method + route + tool + `AGENT_OPERATIONS`
row); `media_type` stays a 7-value enum; status is `discovered → indexed → identified → confirmed`
and `identified` needs an embedding **and** a tag; single writer on the catalog; a pack is its
archive path (M13), so anything minting packages goes through `upsert_package`; `/v1/` is versioned.

**Verification standard — keep it.** Anything visual gets *looked at*, not asserted. Anything
semantic gets a discrimination matrix, and the control arm matters. Property tests get
mutation-checked: break the impl, confirm the test fails. Prefer reading the reference to inferring
it. A failing test is not proof the code is wrong — check the fixture.

**Environment traps** (all of these bit someone): another agent lane may be editing
`crates/lodestash-ui` in this same tree — never `git checkout <path>` a file you haven't read, and
never `git add -A`; ORT is strict (`ort 2.0.0-rc.10` ⇄ ORT 1.22.x, mismatch panics); `cargo build`
fails with "Access is denied" while the app runs (`Stop-Process -Name lodestash -Force`); synthetic
clicks hit the foreground lock, so reach a screen with temporary `// TEMP-SHOT` instrumentation and
`PrintWindow` — **screenshot to a fresh filename each time**, a repeated path served a stale image
twice; Python heredocs need raw strings for `\u{…}` in Rust source; Ollama evicts models between
calls, so batch by stage.

**Suggested next work** is §7 of `HANDOFF.md`: (1) Unity Assets §5 leftovers — pack-row path /
`N assets · size` / **↻ Sync**, the import list's category badge + override and filter tabs, the
sidebar facet `✕`, script/prefab/material previews, and one fresh extraction to demonstrate the
pinned rehydrate; (2) favourites design conformance (four presentation deltas on the KANBAN card);
(3) the P5 Unity round-trip; (4) P4 remainder (audio taggers, accept/reject, Traits screen);
(5) P2 remainder (naming heuristic, relink panel, dependency search filters); (6) list-view parity,
and a card for splitting `app.rs`.

Start with `cargo test --workspace` and `cargo clippy --workspace --tests` (both green, zero
warnings at handoff).
