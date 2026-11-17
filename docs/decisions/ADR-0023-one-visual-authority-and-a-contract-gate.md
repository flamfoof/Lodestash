# ADR-0023 — One visual authority, and a contract gate that measures the app against it

- **Status:** accepted (2026-11-16)
- **Supersedes:** the precedence chain in [`docs/design/README.md`](../design/README.md) and the
  manual drift process of [`docs/design/ALIGNMENT-AUDIT.md`](../design/ALIGNMENT-AUDIT.md)
- **Relates to:** [ADR-0012](ADR-0012-one-icon-ladder.md) (same failure, for icons),
  [ADR-0021](ADR-0021-engine-owns-the-catalog-ui-is-a-web-client.md)
- **Design:** [v2 architecture spec](../superpowers/specs/2026-11-16-v2-architecture-design.md) §10

## Context

The UI kept drifting from the design, and the
[2026-11-16 review](../reviews/2026-11-16-architecture-review.md) (R3, R4) found why:

- The owner's design canvas holds two prototypes that branched from the same 2026-07-14 base. The
  repo carried one (Favorites), the canvas root the other (Unity Assets, Unity import, Add source).
  The design README called the canvas root stale; it was a sibling.
- The token scale could not express what the prototype draws (19 font sizes, 35 colours, radii off
  the scale), so every element forced a choice between the tokens and the design.
- The e2e suite asserted DOM text against the user's own catalog and never compared rendering.
  The August alignment audit found the drift by comparing pixels by hand; nothing kept doing it.

## Decision

1. `docs/design/Lodestash Prototype.dc.html` is the only visual authority: the three-way merge of
   the canvas's two prototypes over their common base, nav resolved to `FEATURES.md`'s eleven rows.
   `docs/design/PROVENANCE.md` records the source hashes; a script re-imports from the canvas.
2. Tokens are extracted from what the prototype draws, and named — not snapped to a scale. A check
   fails on a drawn value without a token or a token nobody uses.
3. A contract (`contract.json` beside the prototype in `docs/design/`, generated in plan P6) records, per screen and state, the measured geometry,
   type and colour of named anchors. The browser e2e runs the real app on a fixture catalog that
   mirrors the prototype's data and fails on any difference not listed in
   `docs/design/DEVIATIONS.md`.
4. Behaviour stays owned by `UI-SPEC.md` and `FEATURES.md`. A deliberate visual difference is a row
   in `DEVIATIONS.md` with its reason.

## Consequences

- Drift is a red build, not a later audit.
- The prototype's one-off values become named tokens. Consolidating them is a design decision,
  taken in the canvas and re-imported, not by an implementer.
- Anchors in the prototype are located by structure and text; copy changes in the canvas need a
  re-import and may need anchor updates. The extractor fails loudly on a missing anchor.

## Alternatives rejected

- **Pixel-diff the app against the prototype directly.** Real thumbnails, fonts and data never
  match a mockup pixel for pixel; the threshold that tolerates that also tolerates drift.
- **Keep the precedence chain and audit periodically.** That is the process that produced the
  drift.
