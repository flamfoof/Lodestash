# ADR-0009 — A stated responsive ladder; facet groups stay expanded only where they fit

- **Status:** accepted
- **Date:** 2026-08-22
- **Supersedes (narrows):** the 2026-08-06 owner call in UI-SPEC §0.2 — *"Facet groups stay expanded
  on **every** screen"*

## Context

The shell spends a fixed 210px sidebar + 300px inspector before the grid gets anything, and the
library toolbar carries about ten controls. The entire responsive behaviour in the shipped UI was
one media query: the toolbar's keyboard hint hides below 1200px. Everything else simply crowded, so
each new control was a fresh ad-hoc decision about what to hide.

The 2026-08-06 owner call — facet groups never collapse — was made about *screens*, to stop the
sidebar's filters vanishing when a non-Library screen was active. Read literally it also forbids
collapsing them at any window width, which makes a narrow window unusable rather than dense.

## Decision

One ladder, applied in order as the window narrows. Nothing is removed outright; each step moves
something behind a gesture.

| Below | What changes |
|---|---|
| 1400px | The toolbar's keyboard hint hides (was 1200px). |
| 1280px | The inspector overlays the grid instead of pushing it, keeping its edge tab. |
| 1120px | The sidebar becomes a 48px icon rail; labels, counts and both facet groups move into a flyout the rail opens. |
| 960px | Sort, density and undo/redo fold into one `⋯` overflow. ★, ⚑ and the view toggle never fold. |

Supported floor: **900 × 600**. Below that the shell scrolls.

The owner call is narrowed to: **facet groups stay expanded on every screen, at every width where
they fit.** They are never hidden by *screen*; below 1120px they are one click away for everyone,
on every screen, in the rail's flyout.

## Consequences

- Ordering principle, for whoever adds the next control: decoration first, then chrome that can be
  recalled on demand, then chrome that changes state. The grid never shrinks to make room for a
  control.
- The icon rail and the overlay inspector are real components, not media queries — four more states
  to keep working.
- DESIGN-GUIDELINES §4 gains the ladder; UI-SPEC §0.2 records the narrowing at the owner call.
