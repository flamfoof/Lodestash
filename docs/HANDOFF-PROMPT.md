# Handoff prompt — paste this to start the next session

Copy everything in the block below into a fresh Claude Code session at the repo root
(`G:\GameDev\asset_viewer`). It orients the next agent, states the working agreements, and turns on
the caveman / cavecrew / claudenator tooling.

---

```
Lodestash — session handoff. You're continuing the build of Lodestash, a local-first Rust/egui
desktop app to identify, view, and retrieve game assets (3D / audio / image / texture / material /
font) for Unity (Godot/Unreal later).

FIRST, read in this order before touching code:
1. CLAUDE.md (repo root) — the design-conformance rule: docs/ is the SOURCE OF TRUTH, code conforms,
   and any divergence is written back into the owning doc IN THE SAME CHANGE.
2. docs/HANDOFF.md — current state of play, settled decisions, environment traps (NOT a spec).
3. docs/KANBAN.md — the board. Every card carries a detailed shipped/remaining note; read the card
   for your area before starting. Move cards Ready → In Progress → Review → Done.
4. The doc that OWNS your area (AI-PIPELINE, DATA-MODEL, IDENTIFICATION-AND-SEARCH, INGESTION,
   ENGINE-API, MCP-AND-ENGINES, UI-SPEC, TECH-STACK, MODEL-GUIDE).

WHERE THE BUILD IS (2026-07-26): Search-with-AI complete; agent plane (REST + MCP) open; P4 tagging
mostly in — accept/reject → confirmed, the Traits screen + teaching, CLAP zero-shot audio tagging,
and an OpenAI-compatible API tagger (Gemini/Groq/OpenRouter/local, Claude-native deferred) all
shipped. Open P4: YAMNet/PANNs + Whisper, the Mapping tab (card 170), Claude-native + Batches,
`+ trait` quick-add, trait colour on inspector chips. See docs/HANDOFF.md §2 and §7 for the full list.

INVARIANTS (don't break without a spec change in the same commit):
- Local-first & private; cloud AI is strictly opt-in per modality; embeddings stay local; audio is
  never uploaded to any provider.
- Non-destructive: original files and the Unity Asset Store cache are never modified or moved.
- MCP ↔ REST are 1:1 (a parity test enforces it); add an op to both planes or neither.
- Status lifecycle: discovered → indexed → identified → confirmed. `confirmed` is human-set.
- media_type enum: model_3d | texture | audio | image | sprite | material | font.
- Crates are lodestash-*; the product is Lodestash (never the old codename).

WORKING AGREEMENTS:
- Direct-commit workflow: branch is main; commit slices straight to main with [skip ci] in the
  message; end every commit with the Co-Authored-By trailer. PRs/CI are suspended until app-complete.
- Verify by BREAKING it: for non-trivial logic, mutation-check (break the code, confirm the test
  fails, revert via the Edit tool — NEVER `git checkout` a file with uncommitted work).
- Keep docs cohesive: when you change one doc, fix its cross-links and the README doc map.
- New dependency ⇒ add it to docs/TECH-STACK.md with a one-line rationale. Keep licences permissive
  (MIT/Apache/BSD); GPL only as a separate subprocess.
- Verify Claude/LLM model IDs, pricing, and request shapes against the claude-api reference or the
  provider's current docs — never hard-code from memory.

ENVIRONMENT TRAPS (see docs/HANDOFF.md §6): Windows + PowerShell primary (Bash tool also available);
egui runs on the Glow/OpenGL renderer, so screenshots via PrintWindow come back blank and CopyFromScreen
comes back black — UI layout needs a human eyeball, don't trust captured pixels. app.rs is large and a
second agent lane may edit it concurrently: do any code move as ONE atomic content-based edit, not
multi-step line-number surgery.

TOOLING — turn these on and use them:
- /caveman:caveman — terse "smart caveman" prose for all your replies (drop articles/filler/hedging;
  fragments OK). Code, commits, and security warnings stay written normally. Already active this repo.
- /caveman:cavecrew — delegate BOUNDED sub-tasks to specialist subagents:
    • cavecrew-investigator (read-only "where is X / what calls Y" → file:line table, ~60% fewer tokens)
    • cavecrew-builder (surgical 1–2 file edits: typo, single-fn rewrite, mechanical rename)
    • cavecrew-reviewer (diff/branch/file review, one line per finding, severity-tagged)
  Use them for well-scoped work; do NOT use for new features or cross-file refactors.
- /claudenator — decompose a larger task into delegatable NEW-FILE units and orchestrate them; don't
  default to doing everything inline. NEVER --cmd a crate the human is mid-edit on.
- ponytail (lazy-senior mode) is also active: climb the YAGNI ladder — does it need to exist? already
  in the codebase? stdlib/native? one line? — then the minimum that works. Read the whole problem
  first; laziness shortens the solution, never the comprehension.

Sanity check before you start and before you hand off: `cargo test --workspace` and
`cargo clippy --workspace --tests` (both green at handoff), then `cargo fmt --all`.

Now pick the top unstarted item from docs/KANBAN.md's "Next up" (or whatever I ask for next), read its
governing doc, and go. If intent is ambiguous, STOP and ASK before writing code.
```

---

*(Regenerate this file whenever the "where the build is" line or the open-work list drifts — it's the
first thing the next session reads, so it must not go stale.)*
