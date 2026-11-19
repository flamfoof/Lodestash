# v2 P7b — Library Overlays: Filters, Bulk Bar, List View, Quick Look, Staging Tray

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development or
> superpowers:executing-plans. Tasks state interfaces, tests and done criteria (owner's global
> CLAUDE.md §6); they do not script every line.

**Goal:** The parts of the Library screen that [P7](2026-11-17-v2-p7-library.md) left under "Not
built here" and that the prototype draws: the ⚑ Filters popover and the active-filter bar, the
bulk-selection bar, list rows (the view toggle's second stop), Quick Look, and the staging tray.
Static compositions measured against the design contract in seven new states, **with positions**.
Components take typed props and emit intents; wiring to `/v1` is P6b.

**Architecture:** [v2 spec](../specs/2026-11-16-v2-architecture-design.md) §9–§10,
[ADR-0023](../../decisions/ADR-0023-one-visual-authority-and-a-contract-gate.md). Builds on
[P7](2026-11-17-v2-p7-library.md) (its rules apply unchanged) and the
[P6 foundation](2026-11-16-v2-p6-web-foundation.md). Parent plan:
[`2026-11-16-v2-rewrite.md`](2026-11-16-v2-rewrite.md). Behaviour owners:
[`UI-SPEC.md`](../../UI-SPEC.md) §0.4, §2 (items 3, 4, 5, 7), §14, §16.2;
[`FEATURES.md`](../../design/FEATURES.md) §1.1, §1.3, §1.4, §7.3, §7.6;
[`DESIGN-GUIDELINES.md`](../../DESIGN-GUIDELINES.md) §7, §9;
[ADR-0009](../../decisions/ADR-0009-responsive-ladder.md),
[ADR-0010](../../decisions/ADR-0010-one-staging-tray.md).

**Rules this plan holds itself to** (P7's, restated because every task leans on them):

- Visual CSS lives in `web/src/ui` only; `features/` and `screens/` carry layout. A look a feature
  needs becomes a primitive (or a variant of one) with a specimen on the gallery sheet.
- Nothing here fetches or keeps a private copy of server data. The filter set, the Quick Look
  target, the playback position and the staged set are **props**; a control reports the change.
- Every asset intent comes from `features/asset/actions.ts`. New things a person can ask for
  (add / remove a tag on the pick set) are new actions there, not callbacks beside it.
- One keymap: an overlay pushes a blocking scope; its own keys are bound in that scope.
- Tokens only. A fixed box size is a named metric in the `app-only` block of `tokens.css`.
- Pixels follow the prototype; behaviour follows UI-SPEC / FEATURES. A difference is a
  `DEVIATIONS.md` row or a line in "Where the prototype and the behaviour docs disagree" below —
  never a silent pick.

**Gates before every commit:** `bun run --cwd web check`, `node scripts/design/check-styles.mjs`,
`node scripts/design/check-tokens.mjs`, `node scripts/design/gen-icons.mjs --check`,
`node --test "scripts/design/**/*.test.mjs"`, `bun run --cwd web test`,
`node scripts/design/extract-contract.mjs --check`, `bun run web:gate -- --gallery`,
`node scripts/check-docs.mjs`.

---

### Task 7b.1: The contract grows — seven states and their anchors

**Files:** Modify `scripts/design/lib/prototype.mjs` (`clickCss`), `scripts/design/states.mjs`,
`scripts/design/anchors.mjs`, `docs/design/contract.json` and `vocabulary.json` (regenerated),
`docs/design/tokens.css` (only if the walk finds values with no token), `web/e2e/gate.mjs`
(`DEFERRED`).

- States, each from a fresh load of the prototype:
  - `library+filters` — ⚑ open with one trait (`rock`), one triangle bucket (`≤ 1k`) and one size
    tier (`M`) chosen, so the on and off form of each chip is drawn and the button carries its
    count;
  - `library+picks` — the first three cards picked: the bulk bar, and a pick box that is on;
  - `library+list` — the ☰ stop of the view toggle;
  - `quicklook` (a model, `Beaver_LowPoly`), `quicklook+sound` (`rock_pickup_01`),
    `quicklook+image` (`bark_oak_albedo`);
  - `staging` — three staged assets (two models and a sound), measured after the "Staged …" toasts
    have left, so the vocabulary does not depend on timing.
- Anchors per state name what makes each part recognisable (a container's box, fill and edge; a
  label's type and colour) and take `pos` where it proves a rhythm or an alignment: the popover and
  its first chip, the bar and its right-aligned ✕ Clear, the first and second row, the dialog and
  its action row, the tray. Anchors of `library` that a state moves are measured again in that
  state (`library.grid` under the bulk bar; `library.view` in list view).
- [x] `extract-contract.mjs` writes the new states; `--check` clean; `check-tokens` 0
- [x] Gate green with every new anchor in `DEFERRED`, keyed `state/anchor` to the task below that
  removes it
- [x] Commit `feat(design): contract states for filters, picks, list view, Quick Look and staging`

*As built:* 107 anchors over the seven states (17 `library+filters`, 9 `library+picks`, 17
`library+list`, 21 `quicklook`, 17 `quicklook+sound`, 10 `quicklook+image`, 16 `staging`); 181 in
all. The vocabulary gained no value without a token — the source walk had already named what the
overlays draw. Two things about the ruler itself:

- **A radius of 99 px and up is one value.** The prototype writes "fully rounded" as `99px`
  (waveform bars, the play button, the project dot) and as `999px` (chips); `tokens.css` has one
  `--r-pill` and `check-tokens.mjs` already folds both into it. The gate's comparator now does the
  same (`scripts/design/lib/compare.mjs`, with a test) — otherwise `--r-pill` could never match a
  99 px anchor.
- **`gate.mjs --only <page>`** measures one page while it is being built. It prints that it is a
  partial run and is not a gate result.

### Task 7b.2: Filters — the predicate, the popover, the active-filter bar

**Files:** Create `web/src/features/filters/filters.ts`, `active.ts`, `FilterPanel.svelte`,
`ActiveFilterBar.svelte`; tests beside them. Create `web/src/ui/FilterChip.svelte`,
`FilterStrip.svelte`. Modify `web/src/screens/LibraryScreen.svelte`,
`web/src/features/library/LibraryToolbar.svelte`, `web/src/gallery/*`, `web/e2e/gate.mjs`,
`docs/design/DEVIATIONS.md`.

- `filters.ts` (pure): `LibraryFilters { traits, tris, sizes }`, `NO_FILTERS`, `toggleFilter`,
  `filterCount`, `triBucket(tris)` (`≤1k · 1k–3k · 3k–6k · 6k+`), `passes(asset, filters,
  thresholds)` — OR within a group, AND across groups, an asset with no triangle count fails an
  active TRIANGLES group (FEATURES §1.4, DESIGN-GUIDELINES §7) — `traitCounts(assets)` (most
  frequent first, ties in first-seen order), `sizeRanges(thresholds)` (the live ranges of §11.4).
- `FilterPanel`: the popover's content — FILTER LIBRARY + Clear all, TRAIT with a search field,
  TRIANGLES, FILE SIZE. Props `filters`, `traits: {name, count}[]`, `thresholds`; events
  `onchange(next)`. The screen owns the `Popover` and its anchor (the ⚑ button).
- `active.ts` (pure): `activeFilters(constraints, filters, favoritesOnly, thresholds)` → one entry
  per active constraint in UI-SPEC §2 item 7's order (`query · type · status · library · pack ·
  collection · ⚑ · ★`), each with its axis, label and what removing it means.
- `ActiveFilterBar`: one strip under the toolbar, a removable chip per entry, `n of N shown` and
  **Clear all** pinned right; not rendered when nothing is active; the tail folds into
  `+N more ▾`. Removing a chip reports the axis and value — the owner drives the control it came
  from.
- `LibraryScreen` gains `filters`, `traitOptions`, `constraints`, `total`; events `onfilterschange`,
  `onunconstrain(axis)`, `onclearfilters`. `onfilters(anchor)` goes: the screen opens the popover.
- [x] Vitest: filters compose (OR within, AND across) and clear; the count on ⚑ is the number of
  chosen values; an unmeasured model fails an active triangles filter; the bar lists constraints in
  the documented order, is absent with none, and removing a chip emits the axis it came from; the
  popover closes on Escape and no global key fires behind it
- [x] Gate: `#library-filters` passes every `library+filters` anchor with positions or has a
  DEVIATIONS row; its deferrals are gone
- [x] Commit `feat(web): the Filters popover and the active-filter bar`

*As built:* 17/17 `library+filters` anchors with positions; one measured exemption
(`filters.search` `fontFamily` — the prototype's input is unstyled Arial, as its `<select>` is).
Two more rows for what the contract cannot measure: the active-filter bar itself, and the XL
chip's range ("8.0 MB+", where the prototype prints its Settings track's "50+").

- New primitives: `FilterChip` (the popover's three kinds of toggle) and `FilterToken` (a removable
  chip in the bar, in the prototype's pack-pill look). `SectionHeading` has a 9 px size and a
  note; `TextLink` can be a quiet action of its own ("Clear all"); `Input` has stated heights for
  the fields the prototype left unstyled; `Text` has `nothing`. The bar is a second `Toolbar`.
- The strip was planned as `FilterStrip`; `Toolbar` already is that strip (and gives it ← →).
- The screen's `filterCount` and `onfilters(anchor)` props are gone: the count is derived from
  `filters`, and the screen opens the popover it owns. `onclearfilters` is the bar's Clear all —
  the owner resets every axis, because the screen owns only two of them.
- `Popover` returned focus to its anchor even when the anchor was a wrapper `<span>`, which cannot
  take it, so Escape left focus on the document. It now returns it to the button inside. Found by
  the test, not by the gate.
- The bar's overflow is measured, not a fixed head: the row is one chip tall and clips; the chips
  that wrapped are counted from their `offsetTop` and offered under `+N more ▾`.

### Task 7b.3: The bulk bar

**Files:** Create `web/src/features/library/BulkBar.svelte`; test `BulkBar.test.ts`. Modify
`web/src/features/asset/actions.ts` (+ test), `web/src/ui/Banner.svelte` or a new strip primitive,
`web/src/ui/Button.svelte`, `web/src/ui/Input.svelte`, the screen, the specimen, the gate.

- `actions.ts`: `tag-add` and `tag-remove` — intents that carry a `tag`, skip Unidentified targets
  (UI-SPEC §2 item 4) and are unavailable with a reason when the name is empty or nothing in the
  set is identified. `intentFor(id, ctx, { tag })`.
- `BulkBar`: `N selected`, a tag-name field, **+ Add tag**, **− Remove tag**, **⟡ Stage all**,
  **✕ Clear**. Props `targets` (the pick set, resolved), `project`; events `onintent`, `onclear`.
  The tag name is the bar's own draft (nobody else's business until it is submitted). Enter in the
  field adds.
- The screen shows it while anything is picked; ✕ Clear empties the pick set.
- [x] Vitest: each button emits the intent `actions.ts` produces for the pick set; Unidentified
  assets are left out of a tag intent; an empty name emits nothing and says why; Stage all on a
  fully staged set unstages (the one stage toggle); ✕ Clear clears picks only
- [x] Gate: `#library-picks` passes every `library+picks` anchor with positions
- [x] Commit `feat(web): the bulk bar acts on the pick set`

*As built:* 9/9 `library+picks` anchors with positions; `bulk.tag` joins the `fontFamily` row
(the same unstyled input). The strip is `Toolbar` with `tone="selection"` rather than a `Banner`
variant — it is a toolbar, and gets ← → for free. `Button` gained `quiet-gold` and the `bar`
size, `Input` the `bar` size, `Text` the `selection` variant.

- **An intent can leave targets out.** `ActionDef.applies` narrows what an intent carries, so
  "skips Unidentified" is a property of the action, not of the bar: `tag-add` on a set of five
  with two Unidentified carries three ids. `intentFor` and `whyNot` take `{ tag }`.
- **The intent's tag is trimmed, not normalised.** Case and the vocabulary are the engine's; the
  specimen's stand-in lowercases, as the prototype does.
- **The tag buttons are never disabled for an empty name** (the prototype's are not): the name is
  typed after the picks are made, so the bar would open with two dead buttons. Pressing one
  without a name says why in a status line beside the buttons and puts focus in the field.
- **Picks reach the bar in page order**, whatever order they were picked in, so an intent's ids
  read the way the grid does.

### Task 7b.4: List view

**Files:** Create `web/src/features/asset/AssetRow.svelte`, `web/src/ui/ItemRow.svelte`,
`web/src/ui/RowFrame.svelte`; tests `AssetRow.test.ts`, additions to `AssetGrid.test.ts`. Modify
`AssetGrid.svelte` (a `layout` prop), the screen, the specimen, the gate.

- `ItemRow`: the 38 px row as a container (`role="gridcell"`, roving `tabindex`) with one primary
  action — the name, stretched over the row — and slots for the controls beside it. Zebra, status
  edge, selection ring.
- `AssetRow`: thumb · type mark · name · pack (link) · size · stats · status · ☆ · ⤢ · stage —
  the card's parts, the card's events.
- `AssetGrid` renders rows instead of cards when `layout="list"`, with one column. The selection,
  the one tab stop, the arrow / Shift / Home / End / Enter / Space / F2 / Menu-key handling and the
  focus hand-over are the same code for both layouts — which is the point.
- [x] Vitest: in list layout there is one tab stop and none inside rows; ↑ ↓ move the lead by one;
  Shift extends the pick set; F2 enters a row's buttons and Escape returns; a row's buttons emit
  the intents the card's do; switching layout keeps the lead and the picks
- [x] Gate: `#library-list` passes every `library+list` anchor with positions
- [x] Commit `feat(web): the list view — rows on the grid's selection and focus model`

*As built:* 17/17 `library+list` anchors with positions and no measured exemption. Three rows
for what the contract does not measure: the pick box over a row's thumb, "Unidentified" in full,
and the rings drawn inside the row.

- **The model is proved, not asserted.** `AssetGrid.layouts.test.ts` runs one set of
  expectations over both layouts (`describe.each`); the only thing stated per layout is how far ↓
  reaches. `AssetGrid` renders `AssetRow` or `AssetCard` from one loop; the key handling, the tab
  stop and the focus hand-over did not gain a branch.
- **Picking in list view.** The prototype's rows cannot be picked at all. The row carries the
  card's pick box over its 26 px thumb — on hover, with focus inside, and on every row while
  anything is picked — so no measured column moves.
- **Rings are inside the row.** The frame's `overflow: hidden` cuts the prototype's outer ring off
  at both ends; three app-only inset ring tokens draw selection, focus, and both together.
- `Button` learned two things rows need: `accent` without `onMedia` (⤢ turns arcane, stage turns
  gold, with no backdrop) and the `favorite` variant (the ☆/★ pin). "✓ staged" is one rule for
  cards and rows now (`.accent.active`), with the thumbnail's backdrop added on top.
- New primitives `ItemRow` and `RowFrame`; `StatusLine` has `inline`; the zebra tone is the token
  `--ls-row-alt`. Fixed column widths are app-only metrics (`--list-*`).
- Not virtualised: it renders the rows it is handed. Windowing needs a row source with a total,
  which is P6b's library query (see "Not built here").

### Task 7b.5: Quick Look

**Files:** Create `web/src/features/quicklook/QuickLook.svelte`, `step.ts`; tests. Create
`web/src/ui/Waveform.svelte`, `AudioTransport.svelte`; modify `Dialog.svelte` (a bare, wide form),
`Thumb.svelte`, `PanelTitle.svelte`, `IconButton.svelte`, the screen, the specimen, the gate,
`web/src/fixtures/library.ts` (waveform peaks).

- `QuickLook`: a `Dialog` — scrim, focus trap, `inert` background, focus restore. Left pane by
  type: an image in full; a model's pre-render with its caption (the orbit viewport is
  `features/viewer3d`'s, later — this is the surface it will fill); a sound's 30-bar waveform with
  the played part coloured, over the transport. Right pane: identity, stat block, chips, **Stage /
  Import** for identified assets (through `actions.ts`), and the footer `← prev · Esc closes · ← →
  step · next →`. Props `asset`, `project`, `traits`, `playback`, `peaks`; events `onintent`,
  `onstep(±1)`, `onseek`, `onclose`.
- Keys, in the dialog's scope: `Esc` and `Space` close (Space is left to a focused button); `←`
  `→` step (left to the scrubber when it has focus). Nothing global fires behind it.
- The screen owns which asset is shown as a prop (`quickLook`, `onquicklook`), steps through the
  visible page in order, moves the lead with it (UI-SPEC §14 "syncing selection"), and on close
  puts focus on the card or row of the asset last shown.
- [ ] Vitest: Tab cycles inside; Escape and Space close; focus returns to the card that opened it,
  and to the card of the asset stepped to; ← → step and wrap; `F`, `Delete` and the grid's arrows
  do not act while it is open; Stage and Import emit `actions.ts` intents; an unidentified asset
  shows neither
- [ ] Gate: `#quicklook`, `#quicklook-sound`, `#quicklook-image` pass their states with positions
- [ ] Commit `feat(web): Quick Look`

### Task 7b.6: The staging tray

**Files:** Create `web/src/features/staging/StagingTray.svelte`, `staging.ts`; tests. Create
`web/src/ui/Tray.svelte`; modify `Thumb.svelte`, `TypeMark.svelte`, `TextLink.svelte`, the
specimen, the gate.

- One tray (ADR-0010): it shows the staged assets it is handed, with or without a project. With
  none, the header names no project, **Import all** is unavailable with the reason `actions.ts`
  gives, and **Export to folder…** still works.
- Header (`⟡ Staging`, project dot and name, count, `clear`), 34 px tiles, the dependency line
  (`N picks → M files incl. materials & textures`), the destination row, **Import all →
  <project>**, its caption, **⤓ Export to folder…**. Props `items`, `fileCount`, `project`; events
  `onintent`, `ondest`. Not rendered when nothing is staged.
- `staging.ts` (pure): `dependencyLine(picks, files)`.
- [ ] Vitest: the count follows the items; staging and unstaging through `actions.ts` intents adds
  and removes a tile; `clear` emits one unstage intent for every item; Import all is disabled
  without a project and Export is not
- [ ] Gate: `#staging` passes every `staging` anchor with positions or has a DEVIATIONS row;
  `DEFERRED` is empty again
- [ ] Commit `feat(web): the staging tray`

### Task 7b.7: Reconcile the docs

**Files:** `docs/UI-SPEC.md` (§0.4, §2, §14, §16.2 — "as built in the v2 web UI" notes),
`docs/DESIGN-GUIDELINES.md` (§7, §9 where a pattern was extended), `docs/design/DEVIATIONS.md`,
`docs/KANBAN.md` (V2-P7), the parent plan, the P7 plan's "Not built here", the v2 spec §9.1 / §10.3
if the tree or the state list changed, this plan ("As built", "Outcome").

- [ ] `node scripts/check-docs.mjs` 0 dead; `node scripts/check-board.mjs` clean
- [ ] Commit `docs: the Library overlays as built`

---

## Where the prototype and the behaviour docs disagree

Known before building (each is decided in its task by the rule above, and the decision recorded
here and in `DEVIATIONS.md` where a measured anchor is involved):

- **Active-filter bar** — UI-SPEC §2 item 7 specifies it; the prototype has only pack and
  collection pills inside the toolbar. Built from UI-SPEC; the contract cannot measure it.
- **LINKS filter** — FEATURES §1.4 lists a fourth section (*uses others / used by others*);
  UI-SPEC §2 item 3 and the prototype have three.
- **Tray verb** — the prototype's primary reads "Export to MyRPG →"; UI-SPEC §16.2 resolved the
  verb as **Import** ("Import all → <project>") and reserves Export for the folder flow.
- **Export to folder…** — UI-SPEC §16.2 (shipped 2026-08-22) puts it in the tray; the prototype
  predates it.
- **List row size** — the prototype writes "L · 4.1 MB" in the row; UI-SPEC §2 item 5 says the
  badge is the letter alone "matching what the list row has always done".
- **List row status** — the prototype abbreviates "Unid."; the user-facing term is
  "Unidentified" (CLAUDE.md invariants, DATA-MODEL §8).
- **Picking in list view** — UI-SPEC says rows have the "same selection/ring behavior"; the
  prototype's rows have no pick box.
- **Quick Look at the ends of the set** — the prototype wraps; UI-SPEC §14 does not say.

## Not built here (and where it goes)

The pager (UI-SPEC §2 pagination) and list virtualisation (spec §9.3) — both need paged data, so
they arrive with P6b's library query; the Folders view (DESIGN-GUIDELINES "Browse views"); the tray's
conflict and running states (UI-SPEC §16.2 — they are the import run's, and need ENGINE-API §5.4's
dup-guard answer); the real orbit viewport (`features/viewer3d`, INGESTION §5) and real audio
playback (`state/player`) — Quick Look draws the surface and the transport and reports what was
asked; the inline scrubber on a playing sound card (UI-SPEC §2 item 5); the `✦ NN%` pill and
AI-ranked results; marquee selection; everything else P7's list still names (relink, SIMILAR,
compare, the inspector's edge tab, the identify shimmer).
