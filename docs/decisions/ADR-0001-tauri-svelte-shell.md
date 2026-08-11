# ADR-0001 — Tauri 2 + Svelte 5 shell replaces the egui crate

- **Status:** accepted
- **Date:** 2026-08-07
- **Supersedes:** the egui/eframe-on-wgpu shell (ROADMAP Phase 0 — deleted at the B7 cutover)

## Context

The original shell was an egui crate (`crates/lodestash-ui`) sharing a wgpu context with the
3D viewer. It was slow to iterate on, hard to skin to the design tokens, and its "runtime only
sees glTF" plan required a normalization step that never landed.

## Decision

Delete `crates/lodestash-ui` at the B7 cutover. The app is now `lodestash-desktop`
(Tauri 2) + `lodestash-app` (core commands/drains) + a Svelte 5 shell under
`crates/lodestash-desktop/ui/`, with a WebGL2 viewer (`viewer3d.ts`, no three.js). The former
"Tauri as fallback" option *shipped*.

## Consequences

- Faster iteration + real CSS design tokens (`tokens.css`); the DG sprite convention became
  executable (56 symbols in `ui/public/icons.svg`).
- The protocol moved to a custom `lodestash://` scheme (Range-capable file/mesh/preview).
- ROADMAP/MILESTONES/PACK-DIRECTORY-NAV still named egui components for a while — that drift
  was fixed 2026-08-07; `node scripts/check-docs.mjs` + the `docs-conformance` review gate now
  catch the next instance.
