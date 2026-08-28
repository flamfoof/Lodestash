# ADR-0010 — One staging tray, in the catalog, that exists before any project does

- **Status:** accepted
- **Date:** 2026-08-27
- **Relaxes:** M5's rule that a staging tray must belong to a project (`MIGRATION_05_UI_SPEC_ALIGNMENT`
  deleted every `is_staging=1 AND project_id IS NULL` row)
- **Supersedes:** the shell's in-memory shortlist (`Library::staged: Vec<i64>`)

## Context

UI-SPEC §16.2 and §20.2 say staging is **per-project**: one `collection(is_staging=1)` per
registered editor, and switching projects swaps trays. M5 implemented exactly that — it added
`collection.project_id`, replaced the single-tray unique index with one per project, reassigned the
legacy shared tray to the active project if there was one, and **deleted it otherwise**.

The reasoning was that a shortlist means nothing without an editor to send it to. That is true of
the agent plane. It is false of the app, for a reason M5 could not have designed around: the Unity
plugin does not exist yet, so **no user has a registered project**, and the state M5 treated as a
migration artefact is the only state anyone is actually in.

So the shell did the obvious thing and kept its own list. `Library::staged` was a `Vec<i64>` behind
the library mutex, and `stage` / `unstage` / `clear_staged` never touched the catalog. That left
two trays:

| | The person's tray | The agent's tray |
|---|---|---|
| Where | `Library::staged`, in memory | `collection(is_staging=1)`, in the catalog |
| Written by | `stage` / `unstage` / `clear_staged` | `Op::StageAsset` / `Op::UnstageAsset` |
| Read by | the tray on screen | ENGINE-API §6 `list_staged`, `import_staged` |
| Survives a restart | no | yes |
| Follows the active project | no — there is only one | yes |

Two answers to "what is staged", neither wrong from where it stood. `list_staged` returned `[]`
while the tray on screen showed six picks. `import_staged` would have imported nothing. And the
agent-plane write path refused outright without a registered editor — *"no active project
registered — register one via the Connect wizard"* — which was the correct rule for an agent and,
today, the answer for every human.

The 2026-08-26 audit found the symptom (the tray never re-read itself, so trashing a staged asset
left its tile behind) and recorded the split as known-and-open rather than fixing it. This is the
fix.

## Decision

**One tray, in the catalog, resolved by one function.**

`Catalog::active_staging_tray_id()` answers the tray for the active project, or the **project-less**
tray when no editor is registered. Every plane resolves through it: the desktop `stage` /
`unstage` / `clear_staged` commands (now writer `Op`s like every other mutation, per ADR-0002),
ENGINE-API §6 `list_staged` and `list_collections`, and the agent-plane stage writes.

Three consequences follow, and each is the point rather than a side effect:

1. **A project-less tray is legal again.** M20 adds
   `UNIQUE(is_staging) WHERE is_staging=1 AND project_id IS NULL`, which admits exactly one. M5's
   index could not express this: it is `UNIQUE(project_id) WHERE is_staging=1`, and SQLite treats
   NULLs as distinct in a UNIQUE index, so it constrained nothing for such a row — which is *why*
   M5 had to delete them rather than rely on the index.
2. **The first `register_project` adopts it**, picks and all, instead of leaving them in a tray
   nothing resolves to. This is the same move M5's data step made for the legacy tray, for the same
   reason: a shortlist built by hand is the expensive thing on screen, and connecting an editor is
   the moment it finally has a destination. A *second* project gets its own empty tray, so trays
   still never mix.
3. **Picks survive a restart**, which the in-memory list never did and which nobody had asked for
   because the tray was understood as transient. It was only transient because of where it was kept.

## What this costs

- **Staging is now a database write.** It goes through the single writer thread, so it queues behind
  whatever that thread is doing — during a scan or an extraction, a `⟡` click is not instantaneous.
  The alternative is a second writer, which ADR-0002 exists to forbid.
- **A pick is a foreign key.** `collection_asset.asset_id` references `asset(id)`, so an id that is
  not in the catalog can no longer be staged. Nothing staged such an id, but the in-memory list
  would have accepted one.
- **Per-project trays are now reachable but still untested against a real editor**, because there is
  no plugin to register one. `register_project` is exercised by tests; the round trip through Unity
  is not, and cannot be from here.

## Alternatives rejected

- **Keep both, and sync them.** Two sources of truth plus a synchroniser is three things that can
  disagree instead of two.
- **Make the shell create a project.** A registered project means a real folder on disk that imports
  are copied into. Inventing one so a shortlist has somewhere to live would put a fake editor in the
  project switcher and in `list_projects`.
- **Leave staging refusing without a project, and tell the user to connect one.** That is the rule
  as written, and it makes the tray unusable for every current user in service of a plane that has
  no client yet.

## Documents updated in the same change

`UI-SPEC.md` §16.2 and §20.2, `DATA-MODEL.md` §9 and §13, `ENGINE-API.md` §6, `KANBAN-SVELTE.md`
(the export-history card's open half closes).
