# Session handoff prompt

Paste the block below into a fresh session. It is deliberately short — everything durable lives in
[`HANDOFF.md`](HANDOFF.md) and the design docs; this only orients and states the agreements.

---

Lodestash — session handoff (picks up after 2026-07-23, `cc79e64`)

You're continuing work on **Lodestash** at `G:\GameDev\asset_viewer` — a local-first Rust/egui
desktop app to identify, view and retrieve game assets for Unity.

**Read first, in this order:**
1. `CLAUDE.md` — the design-conformance rule: `docs/` is the source of truth. Code conforms; any
   divergence is written back into the doc **in the same change**.
2. `docs/HANDOFF.md` — state of play, settled decisions, traps. Current as of `cc79e64`.
3. `docs/KANBAN.md` — opens with a **Next up** block; every card carries a *shipped / remaining* note.
   Read the card before working it.
4. The doc owning your area (`ENGINE-API.md`, `DATA-MODEL.md`, `IDENTIFICATION-AND-SEARCH.md`,
   `AI-PIPELINE.md`, `INGESTION.md`, `UI-SPEC.md`, `TECH-STACK.md`, `docs/design/FEATURES.md`).

**Working agreements.** Commit straight to `main`, one slice per commit, message ending `[skip ci]`
and the `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>` trailer; PRs/CI stay suspended
until app-complete. Ponytail mode (full): the laziest solution that works, stdlib and existing deps
before new ones, no speculative abstraction; mark deliberate shortcuts with a `ponytail:` comment
naming the ceiling. Caveman mode (full) for prose.

**Where it stands.** Grid, viewers, ingest, technical facets, hybrid search, the agent plane
(nine ops, MCP↔REST 1:1, export gated on a desktop approval) all shipped. Local **tagging** runs
through Ollama (VLM captions → text model writes facets); the **dependency graph** is harvested from
`.meta` GUIDs and prefab bindings, and drives both `AssetDetail.dependencies` and export closure;
materials render as **shader balls**; **favourites** exist in the catalog. This machine's library:
~2274 assets, 2067 embedded, 19 tagged — so the health meter honestly reads 19 identified until a
full Identify pass runs (~5 h of CPU, manual and resumable).

**Invariants — do not break without a spec change.** Local-first (embeddings never leave the
machine); non-destructive, whose **one** documented exception is the confirmation-gated
delete-from-disk; MCP↔REST 1:1 (a new op = `ServerState` method + route + tool + `AGENT_OPERATIONS`
row); `media_type` stays a 7-value enum; status is `discovered → indexed → identified → confirmed`
and `identified` needs an embedding **and** a tag; single writer on the catalog; `/v1/` is versioned.

**Verification standard — keep it.** Anything visual gets *looked at*, not asserted. Anything
semantic gets a discrimination matrix, and the control arm matters. Property tests get
mutation-checked: break the impl, confirm the test fails. Prefer reading the reference to inferring
it. A failing test is not proof the code is wrong — check the fixture.

**Environment traps** (all of these bit someone): another agent lane may be editing
`crates/lodestash-ui` in this same tree — never `git checkout <path>` a file you haven't read, and
never `git add -A`; ORT is strict (`ort 2.0.0-rc.10` ⇄ ORT 1.22.x, mismatch panics); run
`cargo fmt --all` before committing; `cargo build` fails with "Access is denied" while the app runs
(`Stop-Process -Name lodestash -Force`); synthetic clicks into the window hit the foreground lock —
screenshot with `PrintWindow` to see, verify behaviour headlessly; Python heredocs need raw strings
for `\u{…}` in Rust source; Ollama evicts models between calls, so batch by stage.

**Suggested next work** is §7 of `HANDOFF.md`, in the user's stated order: (1) **Unity Assets end to
end** — packs as Libraries, surgical extract → identify → index, and the Unity Assets menu from
`docs/design/`; (2) favourites design conformance (four presentation deltas on the KANBAN card);
(3) the P5 Unity round-trip; (4) P4 remainder (audio taggers, accept/reject, Traits screen);
(5) P2 remainder (naming heuristic, relink panel, dependency search filters).

Start with `cargo test --workspace` and `cargo clippy --workspace --tests` (both green, zero
warnings at handoff).
