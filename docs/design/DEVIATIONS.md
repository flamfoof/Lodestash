# Deliberate deviations from the prototype

> Created 2026-11-16 (ADR-0023). Read by the gate, `web/e2e/gate.mjs` (`scripts/design/lib/compare.mjs`).

The app must match [`Lodestash Prototype.dc.html`](Lodestash%20Prototype.dc.html) as measured by
the design-contract gate ([ADR-0023](../decisions/ADR-0023-one-visual-authority-and-a-contract-gate.md)).
A difference is allowed only if it has a row here. The gate reads this table: an anchor listed
here (in backticks) is exempt for the properties named in its row (in backticks: `w`, `h`, `x`,
`y`, `fontFamily`, `color`, `padding`, …), in the states its first cell names (`all`, or the
states in backticks), and nothing else is. A row with no backticked anchor documents a difference
the contract does not measure.

Add a row when behaviour owned by `FEATURES.md` / `UI-SPEC.md` requires the app to draw something
the prototype does not, or when the owner decides against the prototype. Remove the row when the
prototype is updated to agree.

| Screen / state | Anchor | Properties | Prototype | App | Reason | Decided |
|---|---|---|---|---|---|---|
| `library` | `library.sort` | `fontFamily`, `w` | Arial — the browser's default for a `<select>` the prototype never styled; 128 px wide | Inter, the UI font; 6 px wider, because the longest option ("sort · largest first") is set in Inter | An unstyled-control artefact, not a design choice: every other control is Inter. The height is pinned to the drawn 24 px | 2026-11-17 |
| all | nav and action glyphs (no anchor; glyph shape is not measured) | glyph shape only — box, colour and spacing are still checked | Unicode characters (⊞ ★ ✦ ⟡ ▤ …) rendered by whatever system font has them | the designed SVG icon set, `docs/design/icons/icons.svg`, at the glyph's box size | [ADR-0012](../decisions/ADR-0012-one-icon-ladder.md): one icon ladder from the designer's set; Unicode glyph shapes vary by OS font | 2026-11-16 |
