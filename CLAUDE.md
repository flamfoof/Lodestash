# Lodestash — Project Rules

> Last reviewed: 2026-08-12 (unchanged — the conformance rule, invariants and conventions below
> are the standing operating agreement; see `docs/HANDOFF.md` §1.9 for the current state of play).

Lodestash is a **local-first desktop app to identify, view, and retrieve game assets (3D/audio/image)
for Unity** (Godot/Unreal later). The `README.md` + `docs/` are the **design spec and the source of
truth**. This file is the rule set for working in this repo.

## The design-conformance rule (read this first)

1. **Designs are the source of truth.** Before implementing or changing any feature, **read the
   governing design doc(s)** in `docs/` and conform to them. The doc map is in `README.md`; pick the
   doc that owns the area (e.g. schema → `DATA-MODEL.md`, search → `IDENTIFICATION-AND-SEARCH.md`,
   engine/REST → `ENGINE-API.md`, MCP → `MCP-AND-ENGINES.md`, ingest → `INGESTION.md`, AI →
   `AI-PIPELINE.md`, deps → `TECH-STACK.md`, UI → `DESIGN-GUIDELINES.md`).
2. **Code follows the spec; if you must diverge, update the spec in the same change.** Never let code
   and docs drift. A divergence that isn't reflected back into the design doc is a bug.
3. **Keep the docs cohesive.** When you change one doc, fix the others it touches — cross-links, the
   README doc map, and shared vocabulary must stay consistent.
4. **Track work on the board.** Use `docs/KANBAN.md`; move a card Ready → In Progress → Review → Done
   as you work it. Don't start work that isn't a card without adding one.
5. **New dependencies require updating `docs/TECH-STACK.md`.** Don't introduce a crate/library that
   isn't listed without adding it there with a one-line rationale.
6. **Conformance review before "done" (docs-vs-diff gate).** Run a review that checks the diff
   against the governing doc — every requirement implemented, edge cases tested, nothing outside
   scope, and **no claim in `docs/` contradicted by the change** (a contradiction means the doc
   updates in the same commit). Invoke the `docs-conformance` skill for the playbook. Report gaps,
   not style.
7. **Decisions get recorded.** Pivotal cuts (a doc superseded, a default flipped, an invariant
   relaxed) get a numbered ADR in `docs/decisions/` with status + supersedes links — commit
   messages are not a decision log.
8. **Docs freshness is checked.** Run `node scripts/check-docs.mjs` after doc-touching work (and
   in CI when one exists): it flags dead `file:line` citations and cross-links. A dead citation is
   drift — fix the doc or the code.
9. **Board claims are checked too.** Run `node scripts/check-board.mjs` after board-touching work.
   A checkbox is a claim about the code and it rots in both directions: a **ticked** card can cite a
   symbol that has since been renamed, and — the one that costs real time — an **open** card can
   keep asserting something is missing long after it was built, because building a thing never
   touches the card that says it does not exist. Five cards were found in that state in one session
   (2026-08-22); three of them sent work toward something already finished. Before starting from a
   card, confirm its claim against the code.

## Invariants (do not break without an explicit spec change)

- **Local-first & private.** Fully functional offline; cloud AI (Claude vision, etc.) is strictly
  opt-in, per-modality. Embeddings stay local.
- **Non-destructive.** Original asset files and the Unity Asset Store cache are never modified or
  moved; the catalog only references them.
- **MCP ↔ REST are 1:1** for the agent-facing surface (`ENGINE-API.md` §2); add an operation to both
  planes or neither.
- **Status lifecycle** is `discovered → indexed → identified → confirmed` (`DATA-MODEL.md` §8); the
  user-facing terms are **Identified / Unidentified**.
- **`media_type` enum** is `model_3d | texture | audio | image | sprite | material | font` — keep every
  reference (DATA-MODEL, ENGINE-API, IDENTIFICATION-AND-SEARCH) in sync.
- **Naming:** the product is **Lodestash**; crates are `lodestash-*`; never reintroduce the old codename.
- **Engine surface is versioned** (`/v1/`); breaking changes get a new version.

## Conventions

- **Stack:** Rust core (single binary) + optional Python sidecar (UnityPy) + ONNX/Ollama/cloud AI —
  see `docs/TECH-STACK.md`. Keep dependencies permissive (MIT/Apache/BSD); GPL only as a separate
  subprocess.
- **Git:** branch off `main`; commit/push only when asked. End commit messages with the
  `Co-Authored-By` trailer.
- **Claude/LLM specifics:** verify model IDs, pricing, structured-output and batch/caching details
  against the `claude-api` reference before writing them — don't rely on memory.

When in doubt, the relevant `docs/` file wins over assumptions.

## MCP asset-analysis work (2026-09-10)

For agent search, library selection, evidence/file access, Unity-package analysis and LLM-authored
tags, start with [`docs/MCP-ASSET-ANALYSIS.md`](docs/MCP-ASSET-ANALYSIS.md). It links the owning
contracts and the implementation handoff. Exact wire shapes belong only in `docs/ENGINE-API.md` §8;
do not duplicate them in prompts or this file. The existing v1 architecture plan remains the owner
of shared FTS, provenance/history, privacy and startup prerequisites.

The design and implementation plan are **proposed, not shipped**. This session authorized planning
and documentation only; runtime implementation, MCP registration and real asset writes are deferred.
On a later implementation request, confirm board claims against the current code, implement the
corresponding MCP/REST operation together, and prove the real host can call it. Equal tool-name sets
alone do not establish behavioral parity or successful persistence.

Keep three connections distinct: Vite serves the UI, the Engine API serves authenticated `/v1/`
operations, and `lodestash-desktop --mcp` is the host-spawned stdio service. A computer-use helper
pipe failure is not evidence of a Lodestash MCP failure. Use the existing launch guide in
[`crates/lodestash-desktop/README.md`](crates/lodestash-desktop/README.md) for the GUI.

For completed analysis, independently read back the selected asset IDs and report persisted values
and evidence limitations. Never substitute proposed tags for stored tags, a file link for an image
actually inspected, or an agent mutation approval for human confirmation of tag correctness.

The confirmed first host is Codex, consuming 3D preview images and metadata and writing tags plus
a short description. The plan must include existing-tag reporting, identification and vocabulary-
guided accept/reject review, with approved proposals for new traits/categories (ENGINE-API §8.5).
Traits, taxonomy categories and schema namespaces are distinct; agent review never sets human confirmation.
