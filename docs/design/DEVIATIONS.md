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
| all | `topbar.search` | `w` | 340.5 px: the search well takes what four category tabs leave it (◆ All, ⬡ Models, ♪ Sounds, ▦ Images) | 95 px narrower: a fifth tab, ◇ Materials, between Models and Sounds | Behaviour owned by FEATURES §0.2: since 2026-07-30 Materials is its own tab (it carries an export's textures, DATA-MODEL §10). Both prototype branches predate that decision; the well's position and height are still checked | 2026-11-17 |
| `library` | `card.size` | `w` | "L · 4.1 MB" — the tier letter and the size, 68 px wide; beside "✓ in MyRPG" it runs under the "+ stage" button on a 230 px card | the tier letter alone (19.5 px); the exact size is the badge's tooltip, and is already in the stat line under the name | Behaviour owned by UI-SPEC §2.5 (decided 2026-08-28): the badge sits on the artwork and has nowhere to grow, and it repeated the size the stat line states. Height, type, colour, fill, edge and padding are still checked | 2026-11-18 |
| all | nav and action glyphs (no anchor; glyph shape is not measured) | glyph shape only — box, colour and spacing are still checked | Unicode characters (⊞ ★ ✦ ⟡ ▤ …) rendered by whatever system font has them | the designed SVG icon set, `docs/design/icons/icons.svg`, at the glyph's box size | [ADR-0012](../decisions/ADR-0012-one-icon-ladder.md): one icon ladder from the designer's set; Unicode glyph shapes vary by OS font | 2026-11-16 |
