# Prototype provenance

`Lodestash Prototype.dc.html` and `support.js` in this folder are the visual authority
([ADR-0023](../decisions/ADR-0023-one-visual-authority-and-a-contract-gate.md)). This file
records where they came from, so a re-import can be checked and repeated.

## Sources

| Role | Where | SHA-256 (first 16, LF line endings) |
|---|---|---|
| Common base | repo `1321611:docs/design/Lodestash Prototype.dc.html` (2026-07-14 import) | `f1ea81b4165c3a8a` |
| Branch A — Favorites | repo `cc79e64` (2026-07-23); the canvas page `design_handoff_settings_and_library/Lodestash Prototype.dc.html` has the same byte size (274,880) | `9017ccab65d9bf16` |
| Branch B — Unity Assets, Unity import, Add source | canvas root page `Lodestash Prototype.dc.html`, version `1794867901-ee57`, exported 2026-11-16 | `db510f34c8ad45f8` |
| Runtime | canvas root `support.js`, same version | `5de6a1b840537ef0` |

Canvas: <https://claude.ai/artifact/R3t8jcVqrSz14THqSA9tJF> (exported from Claude Design project
`f1fd4bce-f7bc-4b33-b099-d74b78f31998`).

## Merge

`git merge-file` of A and B over the base produced one conflict: the `navDefs` array in the
component script. It resolves to the union in `FEATURES.md` order — Library, Favorites, Identify,
Collections, Packages, Unity Assets, Traits, Stats, Duplicates, Trash, Settings — keeping branch B's
Unity glyph (`this.unityMark(…)`).

Result: `aa58c48ec649d282`. Checked by rendering it headless with the canvas runtime and visiting
every nav destination; all eleven render their screen.

## Re-importing

When the canvas changes, read the changed page(s) from the canvas, repeat the merge against the
recorded base, re-render, update the hashes above, and re-run the contract extractor
(spec §10.2–§10.3). A change that only one branch makes merges cleanly; a conflict is a design
decision to take in the canvas.
