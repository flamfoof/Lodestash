# ADR-0012 — One icon ladder: 16 / 20 / 24 / 32

- **Status:** accepted (2026-08-30)
- **Relates to:** [`DESIGN-GUIDELINES.md`](../DESIGN-GUIDELINES.md) §5 (iconography),
  [`UI-SPEC.md`](../UI-SPEC.md) §0.1 (the shell frame),
  [ADR-0009](ADR-0009-responsive-ladder.md) (the responsive ladder — same shape of decision)

## Context

Three documents described the icon sizes and all three disagreed:

| source | said |
|---|---|
| `DESIGN-GUIDELINES.md` §5 | 16 (inline/rows), 20 (toolbar), **24 (nav)**, 32 (empty states) |
| `UI-SPEC.md` §0.1 | the nav row is "32px row: **16px** glyph" |
| the Svelte shell | 12, 13, 14, 16, 18, 20, 22, 28, 32, 40 — **nine** sizes, nav at 18 |

This is the same failure as the §0.4-vs-§9 keyboard-map disagreement: not one doc drifting from the
code, but two docs drifting from each other, so *conforming to the spec* was not a well-defined
action. Nothing looked broken in the running app — the shell's own 14/18/22/28/40 was internally
coherent — which is why it survived. A ladder that is coherent and undocumented still costs: every
new icon is a fresh judgement call, and the ninth size arrives without anyone deciding on it.

Both ladders were rendered in the running app and compared by eye (2026-08-30): DG's 24px nav
glyphs nearly fill the 32px row and shift every label right; the shipped 18px reads lighter.
Neither is wrong. This is a weight decision, and the reason to settle it is that it is unsettled.

## Decision

**`DESIGN-GUIDELINES.md` §5 is the ladder.** Four rungs, no others:

| rung | role |
|---|---|
| **16** | inline and row icons — list rows, chips, status gems, tree nodes |
| **20** | toolbar and tab controls |
| **24** | nav (the sidebar's `nav[aria-label="Main"]`) and the brand mark |
| **32** | empty states **and art placeholders** — the type mark drawn where a preview would go |

The 32 rung gained "art placeholders" because the shell had a real role the guidelines did not name
— the card / Inspector / Quick Look type mark, previously 28 and 40 — and inventing a fifth rung for
it would have restarted the argument this ADR closes.

`UI-SPEC.md` §0.1's "16px glyph" is corrected to 24. Every `<Icon size>` in the shell now sits on a
rung: 17 at 16, 12 at 20, 4 at 24, 7 at 32.

## Consequences

- Nav glyphs are visibly heavier. That is the accepted cost of the doc winning over the code.
- A fifth size in a diff is now a bug rather than a judgement call, and it is **checked**: an e2e
  case measures every rendered `svg.icon` and asserts the set of widths is a subset of the ladder,
  plus that the nav glyph is 24 specifically. Prose could not have caught this — `check-docs` reads
  citations, not pixels, and the disagreement lived precisely in what the two docs *said*.
- The board's two icon cards (KANBAN-SVELTE "Icon ladder — three specs, three answers" and "Icon
  sizes diverge") were the same defect filed twice; both close here.
