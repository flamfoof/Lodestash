# ADR-0007 — One search box; the Tags/AI mode toggle is removed

- **Status:** accepted
- **Date:** 2026-08-22
- **Supersedes:** the segmented `⟨⟡ Tags⟩ ⟨✦ AI⟩` mode toggle (UI-SPEC §0.1.2, DESIGN-GUIDELINES §7
  "Search bar with mode toggle")

## Context

The top bar carried a segmented control that chose the search engine *before* the user typed. That
asked for a decision they cannot make yet: whether their question is answerable by substring over
name/pack/tags, or needs the embedding search. The two failure modes were both silent —

- Tags mode with a descriptive query ("something to light a dungeon corridor") returned
  *"Nothing matches"* and stopped, with the engine that could have answered it one toggle away and
  unmentioned.
- AI mode with a filename fragment ran a semantic search for a string the user could see with a
  substring filter.

Switching to Tags also cleared AI results, so the toggle was destructive as well as premature.

## Decision

One box. Typing always runs the instant substring filter (IDENTIFICATION-AND-SEARCH §5A). The AI
search (§5B) becomes an **escalation** reachable from inside the results:

- A pinned row under the filtered results — `✦ Ask AI for »query« — searches meaning, not names` —
  activated by Enter or by clicking it.
- The empty state offers the same escalation as its primary action instead of dead-ending.
- While AI results are shown the box keeps the arcane border and the result header reads
  `✦ N ranked matches`, with `← back to name filter` returning to the substring result.

The mode is therefore a *consequence* of what the user did, still legible in the border colour and
the result header, rather than a switch they had to set first. AI never runs on a keystroke.

## Consequences

- `view.searchMode` stops being user-set state; it becomes a result state (`"tags" | "ai"`) that the
  escalation sets and clearing the query resets.
- The unidentified-backlog note moves into the empty state, where it explains a thin result rather
  than sitting in a placeholder.
- **Lost:** a sticky per-session preference for people who want AI first every time. If that turns
  out to matter, remember the last escalation for the session rather than restoring the toggle.
- UI-SPEC §0.1.2–3, §2.2 and §2.6 change with this ADR; DESIGN-GUIDELINES §7's "Search bar with
  mode toggle" is rewritten as "Search bar".
