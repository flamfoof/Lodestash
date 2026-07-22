# UI Spec — Screens & Functionality

The **screen-by-screen functional spec** for the Lodestash desktop app, derived from the interactive
prototype (`design/Lodestash Prototype.dc.html`) and **normalized against the owning design docs**.
Where the prototype and an owning doc disagreed, the resolution is stated inline as **⚖ Resolution**
and collected in §20. Visual language (tokens, type, icons, motion) stays owned by
[`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md); data shapes by [`DATA-MODEL.md`](DATA-MODEL.md);
search behavior by [`IDENTIFICATION-AND-SEARCH.md`](IDENTIFICATION-AND-SEARCH.md); the agent/engine
surface by [`MCP-AND-ENGINES.md`](MCP-AND-ENGINES.md) / [`ENGINE-API.md`](ENGINE-API.md); ingest by
[`INGESTION.md`](INGESTION.md); AI by [`AI-PIPELINE.md`](AI-PIPELINE.md). This doc owns **what each
screen shows and does**.

Vocabulary used throughout (see the owning docs): **Library Source** ("library" in UI copy — e.g.
Synty, Asset Store cache) → DATA-MODEL §10; **package** (publisher → pack) → DATA-MODEL §7;
**status** `discovered/indexed` = **Unidentified**, `identified`, `confirmed` → DATA-MODEL §8;
**trait chip** = tag presentation → §13.4.

---

## 0. App shell

One window, three fixed regions plus overlays:

```
┌────────────────────────────────────────────────────────────┐
│ Top bar (48px, --ls-surface-raised)                        │
├──────────┬─────────────────────────────────┬───────────────┤
│ Sidebar  │ Main column                     │ Inspector     │
│ 210px    │  [type tab row]                 │ 300px         │
│          │  [screen content]               │ (when an      │
│          │                                 │  asset is     │
│          │                                 │  selected)    │
├──────────┴─────────────────────────────────┴───────────────┤
│ overlays: staging tray (fixed BR) · toasts · modals        │
└────────────────────────────────────────────────────────────┘
```

- **Background**: `--ls-bg` with two faint radial gradients (gold `rgba(231,183,101,.06)` top-right,
  arcane `rgba(73,214,201,.05)` bottom-left). Never behind text directly — it reads as ambience only.
- **Fonts**: Cinzel / Inter / JetBrains Mono per DESIGN-GUIDELINES §3, **bundled with the app**
  (offline invariant — no runtime font fetch).

### 0.1 Top bar (left → right)

1. **Logo** — rotated gold diamond (18px, 2px `--ls-gold` border, inner `--ls-gold-bright` dot, faint
   glow) + `LODESTASH` in Cinzel 16px gold, letter-spaced.
2. **Search mode toggle** — segmented `⟨⟡ Tags⟩ ⟨✦ AI⟩` (DESIGN-GUIDELINES §7). Switching to Tags
   clears AI results.
3. **Search box** (max-width 400px) — border + focus glow follow the active mode (gold / arcane).
   - **Tags mode**: live substring filter over name + pack + tags
     (IDENTIFICATION-AND-SEARCH §5A; grows into FTS5/BM25).
   - **AI mode**: placeholder *"describe what you need — e.g. »pickup sound for rocks« — then
     Enter"*; **Enter runs the search** (§5B), an animated `✦ sieving…` pulse shows while running,
     results replace the grid as a **ranked list** with per-card `✦ NN%` match badges.
     Unidentified assets are never returned (DATA-MODEL §8). Empty result → toast + empty state.
4. **Type tab row** — see §0.3. *(⚖ Resolution: the prototype squeezed 4 category chips inline; the
   real enum has 7 types + All, so the type filter is its own row under the top bar.)*
5. **Health meter** (min-width 180px, right-aligned group) — label `Identified n / total` (mono) over
   a 6px gold-gradient progress bar with glow. Read-only; reflects non-Unidentified share.
6. **Project chip** — pill: colored project dot + `Name · Engine` + `▾`. Opens the **project
   switcher** (§16). Dot = per-project accent color; dims when the editor is stale
   (DESIGN-GUIDELINES §7 "Project chip").
7. **AI-profile pill** — arcane outline pill `✦ local · <GPU name>` showing the active identify
   profile tier (§11.2); links to Settings → AI.

### 0.2 Sidebar (210px, `--ls-surface`)

**⚖ Resolution:** the prototype's single labeled sidebar **replaces** the earlier icon-only nav rail.
One vertical panel, three stacked groups:

1. **Navigation** — 10 destinations, each a 32px row: 16px glyph + label + optional warn-badge
   (amber pill, mono count). Active row: `--ls-overlay` fill + 2px gold left-border.

   | Destination | Icon (`ls-*`) | Badge |
   |---|---|---|
   | Library | `nav-library` | — |
   | Identify | `nav-identify` | Unidentified count |
   | Collections | `nav-collections` | — |
   | Packages | `nav-packages` | — |
   | Unity Assets | `unitypackage` | — |
   | Traits | `nav-traits` | — |
   | Stats | `nav-stats` | — |
   | Duplicates | `nav-duplicates` | open dup-group count |
   | Trash | `nav-trash` | trashed count |
   | Settings | `nav-settings` | — |

   **Unity Assets** is a distinct destination from **Packages** (FEATURES.md §5 vs §4.1): Packages is
   the aggregate view of *already-indexed* assets; Unity Assets manages `.unitypackage` archive
   sources — browse/preview one package, surgical per-asset extraction, and **Add to Library** pinning
   (`package.pinned`, DATA-MODEL §7). Code adds a `Screen::Unity` variant to the existing nine.

   Clicking **Library is the reset-to-everything affordance**: it clears library/type/status/pack/
   collection filters, the query, AI results, and the ⚑ content filters.
   Unbuilt destinations render the **stub screen** (§12) — never hide the row.
2. **STATUS facet group** (letter-spaced dim label): All / Unidentified / Identified / Confirmed,
   each with a rotated status-color diamond dot + count (mono, dim). Single-select; selecting also
   navigates to Library. Maps to `status` (DATA-MODEL §8; `discovered|indexed` fold into
   Unidentified).
3. **LIBRARIES facet group**: All libraries + one row per Library Source (name + count), single-
   select, navigates to Library. Below the rows: a dashed **`+ Add source…`** button (same action as
   Settings → Sources → Add).

Facet groups collapse when a screen other than Library/Identify is active (they still render; they
are Library filters). Sidebar scrolls as one column.

### 0.3 Type tab row

Under the top bar, on Library and Identify: **four coarse category tabs** — `All · Models · Sounds ·
Images` (FEATURES.md §0.2) — each showing its count and colored by its bucket token when active (2px
underline). `All` never hides; a bucket with a zero count hides. The tabs are a **presentation
grouping over the unchanged 7-value `media_type` enum** (the enum stays
`model_3d | texture | audio | image | sprite | material | font` — a data-model invariant); finer
type distinctions live in the ⚑ Filters popover, not the top bar. Bucket → `media_type`:

| Tab | `media_type`(s) |
|---|---|
| Models | `model_3d`, `material` |
| Sounds | `audio` |
| Images | `image`, `texture`, `sprite`, `font` |

*(The earlier second-row Library tabs are gone — the sidebar LIBRARIES facet group (§0.2) absorbed
the per-source filter.)*

### 0.4 Keyboard map (global)

| Key | Context | Action |
|---|---|---|
| `Space` | asset selected | toggle Quick Look |
| `Esc` | Quick Look open → else selection → else | close QL / clear selection / stop audio |
| `← →` | Quick Look open | step prev/next within the visible set |
| `Backspace` | Folders view | up one level |
| `Ctrl/Cmd-A` | grid | select all in current filter |
| `Ctrl/Cmd-Z / Shift-Z` | catalog mutations | undo / redo (DESIGN-GUIDELINES §7) |

Keys never fire while an input/textarea has focus.

---

## 1. Onboarding (first-run)

Modal over the dimmed Library (scrim `rgba(10,12,17,.6)` + blur; Library renders behind it).
Panel: 640px, `--ls-surface-raised`, 1px gold border, **clipped corners** (JRPG frame, §4 of
DESIGN-GUIDELINES), `lsRise` entry. Header: large logo diamond + `LODESTASH` Cinzel 24px.
Shown when `skip_onboarding` is false (Settings §11.7 toggles it; the panel's **"Don't show this
next time"** checkbox writes the same flag).

**Step 1 — "Point it at your hoard."**
- Subtitle: *"Lodestash indexes everything you own — packs, folders, loose files — so you can find
  the one asset you need. Nothing leaves this machine."*
- **Detected/added source rows**: diamond dot (source accent) + name + mono path + pack count +
  outline tag pill (`auto-detected` arcane / `folder` slate). The Unity **Asset Store cache** is
  auto-detected via the discovery order in INGESTION §2 and listed first when present; caption
  "Unity cache auto-detected" bottom-right.
- **Existing-index banner** (returning users): verdant info row — *"Existing index found — N assets,
  last scanned X. **M new files** on disk aren't indexed yet."*
- Actions: **Open library →** (gold primary) · **⌕ Scan N new assets** (arcane outline; → step 2) ·
  **+ Add another folder…** (neutral outline; OS folder picker, appends a source row).

**Step 2 — scan progress.**
- Subtitle: *"Only new and changed files are scanned — the existing index is untouched. Non-content
  junk (scripts, DLLs, editor tools) is hidden automatically."* (INGESTION §3.)
- Gold progress bar + a mono log line per pipeline stage (diff → per-source discoveries → hashing/
  thumbnails/non-content count → summary line with totals and flagged duplicates).
- **On completion**, if new assets exist: an amber callout — *"All N assets start **Unidentified** —
  AI search can't see them until you run identification."* with **✦ Run identification now**
  (arcane; → Identify screen, auto-starts Identify-all) and **Skip to Library**.

---

## 2. Library (home screen)

The browse-everything grid. Composition top→bottom:

1. **Unidentified nudge** (when count > 0): slim amber banner — *"N assets are Unidentified —
   invisible to AI search until appraised."* + **Open Identify mode →**. Dismisses for the session.
2. **Result header row**: result label (`N assets · <library>` or `✦ N ranked matches for "q"`);
   active **pack** and **collection** filter chips (gold outline pill with ✕ to clear — activate
   once the P2 packages model / P3 collections exist); hint text
   `Space quick-looks · ⤢ full size · ⟡ stage` (dim, hidden < 1200px); **sort select**
   (⚖ resolution: shipped sort keys are name / date / size / type ± direction per
   DESIGN-GUIDELINES §7 — richer than the prototype's list; tri count joins with P1 metadata;
   sort is disabled while AI-ranked results are shown); **⚑ Filters** button; **grid-density
   segmented** (▪ Compact ▦ Cozy ▣ Large — card min-widths **150 / 205 / 288px**); **view
   toggle** ⊞ grid / ☰ list (list view still pending — the shipped toggle is Grid / Folders,
   DESIGN-GUIDELINES §7 "Browse views").
3. **Content filter popover** (⚑, 328px, elevation 2) — DESIGN-GUIDELINES §7: TRAIT chip cloud with
   search (arcane select-state), TRIANGLES buckets `≤1k · 1k–3k · 3k–6k · 6k+` (violet, models
   only), FILE SIZE tiers S/M/L/XL with live ranges from §11.4 (tier-colored). Multi-select within
   and across groups (AND across groups, OR within). Active filter count shows as a gold badge on
   the ⚑ button; "Clear all" resets.
4. **Bulk bar** (when ≥1 card is check-picked): `N selected` + tag-name input + **+ Add tag** /
   **− Remove tag** (skips Unidentified) + **⟡ Stage all** + ✕ Clear. Bulk tag edits are undoable.
5. **The grid / list** (virtualized):
   - **Card** (DESIGN-GUIDELINES §7 anatomy): 3px status left-border; thumbnail region (104px, dark
     recess) with: status gem badge (top-right), **pick checkbox** (top-left, appears on
     hover/when any pick is active), `✦ NN%` AI-match pill (top-left, AI results only),
     **S/M/L/XL size badge** + `✓ in <project>` imported badge (bottom-left), **⤢ Quick Look** +
     **⟡ stage/✓ staged** actions (bottom-right, hover-revealed); body: type diamond + name
     (Unidentified: slate italic), mono pack line (pack name is a link → Pack stats §15) +
     short stats, trait chips (first 3 + `+N` expander; chips are click-to-search).
   - **Sound cards**: in place of the thumbnail — round arcane **▶/◼** play button + a 5-bar
     mini-waveform (slate when Unidentified, aqua otherwise); while that card is the active audio,
     an inline scrubber + mono time readout appears. One asset plays at a time (§13.2).
   - **Unidentified cards**: thumbnail at 30% opacity + grayscale behind a large slate mono `?`;
     chips replaced by a single `???` chip. The **identify shimmer** (gold sweep, ~0.6s) plays when
     an asset flips to Identified (respects reduced-motion by flipping instantly).
   - **List rows** (32–38px, zebra): thumb 26px · type diamond · name · pack (link) · size badge ·
     stats · status gem+label · ⤢ · stage. Same selection/ring behavior.
   - Selected card/row: 2px gold ring. Confirmed cards: faint 1px gold ring at rest.
6. **Empty state**: large ⌕ + *"Nothing matches"* / (AI) *"Nothing close in your hoard"* + hint —
   when Unidentified exist: *"N assets are still Unidentified — they can't be found yet."*, else
   *"Try a broader term or clear the facet filters."*

Card click = select (inspector opens); second click deselects. All filters compose:
type ∧ library ∧ status ∧ pack ∧ collection ∧ ⚑ filters ∧ query/AI.

**Pagination (thumbnail views):** the grid renders one page of the filtered result at a
time — a bottom pager bar shows `‹ Prev · Page x / y · Next ›` plus an **assets-per-page**
selector (50 default / 75 / 100 / Custom…, persisted in the view prefs). Only the current
page decodes thumbnails; keyboard navigation follows the selection across page boundaries.
The future list view is exempt (rows are cheap; it stays a single virtualized scroll).

---

## 3. Identify (the appraisal workspace)

Backlog triage per DATA-MODEL §8. Arcane-tinted header:
- Cinzel title **"Appraise the Backlog"**; subtitle *"Fresh loot is Unidentified — no tags, no
  embeddings, invisible to AI search. Appraisal runs once per asset, locally."*
- Right: large mono **remaining count**; **✦ Identify all (N)** primary (arcane) — while running it
  reads **✦ Appraising…** and an arcane progress bar + `done / total appraised` label appears.
- Grid shows the **session set**: everything still Unidentified **plus** assets identified this
  session (so results don't vanish as they flip). Cards are the §2 card minus stage/import actions
  (identification first). Each flip: tags fill in from the tagger, status gem `? → ✓`, shimmer.
- Per-asset identify: the inspector's **✦ Identify now** (§13) runs one asset.
- **Empty state** (backlog clear, nothing this session): verdant diamond ✓ + Cinzel *"Backlog
  clear"* + *"Every asset is appraised and searchable."* + **Go to Library →**. A toast also fires
  when Identify-all drains the backlog.
- Engine: `identify_asset` per IDENTIFICATION-AND-SEARCH §3 tiers; the run is a cancellable job
  (jobs/toasts per DESIGN-GUIDELINES §7). **Auto-identify new files** (§11.2) feeds this screen.

---

## 4. Packages

> **Packages vs Unity Assets** (§0.2): Packages is the aggregate view of *already-indexed* assets
> grouped by publisher → pack; **Unity Assets** manages `.unitypackage` archive *sources* (browse,
> surgical extraction, Add-to-Library pinning). FEATURES.md §4.1 / §5.

Everything you own, by **publisher → pack** (DATA-MODEL §7). Header: Cinzel "Packages" + subtitle.
One card per publisher: header row (▤ gold + name + total asset count) then a row per pack:
name · amber `N unid` outline pill (when > 0) · asset count · **◉ Stats** (→ Pack stats modal §15)
· **Browse →** (→ Library filtered to that pack). Package rows for still-packed Asset Store
`.unitypackage`s (status `discovered`) also offer **Extract** per INGESTION §2 — browse-without-
unpack stays cheap, extraction is on demand. Packs also carry a **tick box**, with an all/none
toggle on each publisher header and an **▶ Extract selected (N)** bar above the list: the ticked
packs queue and unpack **one at a time**, each through the normal job tray row (so each is
individually cancellable), and the bar shows the remaining queue with a **Cancel queue** for the
packs not yet started.

*(Shipped slice: cache discovery/validation with persisted custom roots, publisher→pack browse
straight off the download cache with pack counts + sizes, per-root "Use as library source",
per-pack Extract through the pending-package flow, the batch tick/queue described above, and —
for packs extracted this session — the `N assets` / `N unid` counts and **Browse →**. The `N unid` pill, ◉ Stats, and Browse→ need
catalog `package` rows — they land with the P2 Packages model.)*

---

## 5. Duplicates

Review queue per DATA-MODEL §5 / DESIGN-GUIDELINES §7 — **never auto-delete**. Header subtitle:
*"Near-duplicates found across packs. Nothing is ever auto-deleted — you decide."*
One card per group: reason line (mono — e.g. `perceptual hash · 96% similar · different packs`);
member tiles (thumb + name + pack · stats) with the **keeper** marked by a gold `★ keeper` ribbon
(keeper = highest status, tie → earliest indexed); actions **Keep both** (dismisses group) ·
**Merge tags** (union of tags onto keeper, arcane) · **Move duplicate to Trash** (danger outline;
respects confirm-before-Trash §11.6). Empty state: ⧉ *"No duplicates left to review."*

---

## 6. Collections

Pinned loadouts (DATA-MODEL §9 collections). Header: Cinzel title + subtitle *"…stash tabs for your
hoard. Add from any asset's inspector."* + inline **create** (name input + gold `+ Create`).
Grid of collection cards: ⟡ + name + count; strip of up to 5 member thumbs (type-color border;
♪ tile for sounds); **Browse →** (Library filtered by the collection, shown as a header chip) and
✕ delete (toast confirms *"assets untouched"*). Membership is toggled from the inspector (§13.5).

---

## 7. Trash

Catalog-only soft delete (`asset.deleted_at` — DESIGN-GUIDELINES §7 "Trash & restore"). Subtitle:
*"Removed from the catalog only — files stay on disk. Restore any time."* Table rows: type diamond ·
name (muted) · `library · pack · stats` (mono) · **↩ Restore**. Empty state: ⊘ *"Trash is empty."*
Trashed assets leave search, dup groups, and staging everywhere. (Explicit delete-from-disk stays a
separate danger modal per DESIGN-GUIDELINES §7; not on this screen's happy path.)

---

## 8. Stats

Two tabs (segmented in the header): **▦ Overview** and **◉ Tag Graph**.

### 8.1 Overview
- **Stat tiles** (auto-fit ≥170px): assets indexed (`sub: N libraries · M packs`) · on disk
  (gold; avg sub-line) · triangles total (violet; model count) · identified % (amber while
  Unidentified remain, verdant at 100%).
- **SIZE BY TYPE**: per media_type — type diamond + label + total size, proportional type-color
  bar, mono meta (models: count/tris/avg; sounds: count/total runtime; images: count/avg size).
- **DISK WEIGHT BY PACK**: per pack — name + count + size + gold bar scaled to the heaviest;
  row click → Pack stats modal.
- **HEAVIEST ASSETS**: top 5 by size — tier badge + name + `library · pack — description` + stats;
  click selects in Library.
- **INSIGHTS**: generated advisory rows (accent glyph + message + action link), including at least:
  Unidentified backlog (→ Identify), open dup groups (→ Duplicates), heaviest model's share of
  total tris (→ View), heaviest pack's share of disk (→ Browse), traits teaching the identifier
  (→ Vocabulary, Traits tab).

### 8.2 Tag Graph
Full-bleed force-directed canvas of the identified library: **gold tag nodes** (radius ∝ usage,
labeled), small type-colored asset nodes, slate edges (asset↔tag), **dashed arcane edges** for
dependency links (model → texture, from `asset_relationship`). Interactions: scroll = zoom
(0.4×–3×, zoom widget top-right with reset), drag a node to untangle (pinned while held), hover
highlights + labels, **click a tag → Library filtered to that tag; click an asset → its inspector**.
Legend bottom-left; header shows `tags · links · assets` counts. Amber footnote when Unidentified
exist: *"? N Unidentified assets have no tags yet — they're not in the graph."*

---

## 9. Traits (Traits & Mapping)

> Nav label is **Traits** (matching the prototype); this screen still hosts both the Traits and
> Mapping tabs — "Traits" is the destination name, Mapping is a tab within it.

**One screen, two tabs** (header segmented control, same pattern as Stats §8): **❖ Traits** — the
vocabulary you tag with — and **⇢ Mapping** — the rules that interpret tags into canonical
categories. Both sides of "what do my words mean here" live together; raw tags are never modified
by either. Sidebar destination **Vocabulary** (`ls-nav-traits`; `ls-nav-mapping` becomes the
Mapping **tab** glyph). Deep links land on a tab (e.g. Stats' "Trait library →" insight → Traits).

### 9.1 Traits tab

The user's tagging vocabulary, and the human side of AI-PIPELINE §6.
**⚖ Resolution:** "Traits" is the user-facing evolution of the Tag manager (DESIGN-GUIDELINES §7):
a **trait** is a `tag` row promoted to first-class with `scope`, `usage_note`, `teach`, `color`,
`icon` (DATA-MODEL delta, §20). Free-form tags keep working underneath.

- Tab subtitle: *"Your own tagging vocabulary. Traits marked ✦ teaching are fed to the
  identifier — future scans tag with your words, not just its own."*
- **NEW TRAIT** card: name input (normalized: lowercase, spaces→dashes; dup-guarded) · **applies
  to** scope pills (`any / model / sound / image` — extend with the remaining media types as their
  taggers land) · **⚙ advanced** reveals the **USAGE** field (*"when should this trait apply? The
  identifier reads this during scans."*) · gold **+ Add trait**.
- **Trait cards** (grid ≥300px): colored chip (icon + name) + scope label + `N tagged` count;
  italic usage quote; **✎ style** expander (6-color palette + 8-glyph icon picker); **✦ teaching
  identifier / ⏸ paused** toggle (toasts explain the effect); **⌕ find tagged** (→ Library query);
  ✕ delete (*"existing tags on assets are kept"*).
- **✦ PROMOTE EXISTING TAGS**: chips for tags already in use (manual ones, or AI tags with ≥2 uses)
  not yet traits — one click promotes (`+ label · count`).
- **Teaching contract**: traits with `teach=1` are injected into the identifier's vocabulary/prompt
  (per-modality by scope) so future `identify_asset` runs can emit them (AI-PIPELINE §6);
  paused traits are excluded but keep their existing tags.

---

### 9.2 Mapping tab

The controlled-vocabulary editor over DATA-MODEL §4 (taxonomy / category / alias / rule /
`asset_category`). Not in the prototype — specced from DESIGN-GUIDELINES §7 ("two-pane: raw
tags → categories tree, rules between") and the §4 workflow (tab stubbed until built).

- **Tab header row**: subtitle *"Rules that sort messy tags into your clean categories — swap
  whole vocabularies without touching a single tag."* Right: **taxonomy switcher** (select +
  `● active` marker — multiple taxonomies coexist, one active), `v<version>` (mono),
  **+ New taxonomy** / **Import starter pack**.
- **Left pane — RAW TAGS**: the live tag vocabulary with usage counts and a search box (same data
  as the Traits tab, read-only here; traits show their chip styling). Rows show which category the
  active taxonomy currently maps
  them to (dim `→ Environment/Foliage`) or an amber `unmapped` pill. **Drag a tag onto a category**
  (right pane) to create an `exact_tag` rule; drop with `Alt` to create an **alias** instead.
- **Right pane — CATEGORY TREE**: the hierarchy (`path`-materialized), expandable rows with
  per-category mapped-asset counts. Row actions: rename, add child, delete (assignments cascade;
  confirm). Selecting a category opens its **detail card**:
  - **ALIASES**: chips (✕ to remove) + add field.
  - **RULES**: one row per `mapping_rule` — match-type badge (`exact_tag · alias · regex ·
    glob_path · media_type · embedding_knn`) + mono pattern + priority (editable number, lower
    wins) + min-confidence + enable toggle + ✕. **+ Add rule** opens an inline form; `embedding_knn`
    rules pick seed assets via a mini asset-picker ("near these examples").
- **Footer bar — apply pipeline**: **Preview** runs a dry-run (*"would assign 1,204 assets · 37
  conflicts · 12 pinned overrides untouched"*) with a diff list (asset, old → new category, winning
  rule); **Apply** materializes `asset_category` rows (`assigned_by='rule'`), bumps
  `taxonomy.version`, toasts. Conflicts resolve by priority then confidence; **pinned (manual)
  assignments always survive re-runs** and show a small lock glyph wherever they render.
- **Manual assignment** happens in the asset **inspector** (§13): a CATEGORY row under TRAITS —
  current category chip(s) + a picker; manual picks write `assigned_by='manual', is_pinned=1`.
- **Search integration**: categories appear as a facet group in the sidebar once a taxonomy is
  active (`asset_category` pre-filter, IDENTIFICATION-AND-SEARCH §5A); `apply_mapping` is the
  agent-plane equivalent (MCP surface).

---

## 10. (merged into §9)

Traits and the Mapping editor share the single **Vocabulary** screen (§9). Number reserved so
later sections keep their references.

---

## 11. Settings

Scrollable single column (max 760px) of section cards (DESIGN-GUIDELINES §7 "Settings" groups —
this section is the authoritative expansion). Header subtitle: *"Sources, the identify profile
matched to your hardware, storage caps, and the live engine bridge."*

### 11.1 WATCHED SOURCES
Row per `import_source`: status dot (dimmed when paused) · name + mono path · meta (`N packs ·
auto-detected / folder / watched`) · **⊘ blacklist chip** (`N excluded`, amber when non-empty;
expands the panel) · **enable toggle** (pause/resume watching — paused sources stay cataloged but
stop scanning). Expanded blacklist panel: excluded chips (folder amber / asset red diamond, ✕ to
remove) + input + **⊘ folder** / **⊘ asset** add buttons (DATA-MODEL §10 per-source blacklist).
Header: **+ Add source** (folder picker; Asset Store cache is offered via auto-detect).

*(Shipped interim: watching is real but **library-wide**, not per source — a **SOURCES ▸ Watch
sources for changes** checkbox (`AppConfig.watch_sources`, default on) drives one `notify` watcher
over every open root, debounced 1 s, rescanning only while the library is idle and at most once
every 30 s. The per-source pause toggle here needs `import_source.watch_enabled` (DATA-MODEL §2) to
be read by the app, which lands with the Sources-manager card.)*

### 11.2 IDENTIFY PROFILE
Three selectable tier cards — **Integrated / Balanced / Dedicated GPU** — each naming its model set
and preview quality. **⚖ Resolution:** tiers bind to the real per-GPU model sets in
[`AI-MODELS-BY-GPU.md`](AI-MODELS-BY-GPU.md) (the prototype's "TagNet-M" etc. are placeholders);
any cloud-verify step exists **only** when cloud AI is opted in per modality (AI-PIPELINE §5 —
local-first invariant). Picking a tier also sets on-screen quality (§11.3) to its recommendation.
Below: **Auto-identify new files** toggle — watched-source drops run `identify_asset`
automatically instead of only landing in the backlog.

### 11.3 ON-SCREEN QUALITY
Low / Medium / High tiles (fast·pixelated / balanced / sharp·heavier GPU). Grid-only; **Quick Look
always renders full detail**. Changing it re-renders grid thumbnails (toast).

### 11.4 FILE-SIZE LABELS
The S/M/L/XL thresholds (already implemented — `SizeTiers`): draggable 3-handle track over colored
segments, numeric S→M / M→L / L→XL fields, editable cap, **Reset defaults**
(1 / 4 / 8 MB, cap 10 MB). Invariants: 128 KB step, ≥128 KB gap, cap ≥ XL + 128 KB. Legend chips
show live ranges. Feeds the card badge, the ⚑ FILE SIZE filter, and Stats tier badges.

### 11.5 MODEL CONTEXT PROTOCOL
Status pill (`● listening · 127.0.0.1:<port>` verdant / `○ offline`). Body: **Enable MCP server**
toggle. **⚖ Resolution vs prototype:** this card controls the **optional Streamable-HTTP
transport** of the one MCP server (MCP-AND-ENGINES §2; stdio needs no config and no token). When
enabled: **ENDPOINT** (`http://127.0.0.1:<port>/mcp`, ⧉ Copy) on a **stable configurable port
(default 4319)** — distinct from the engine API's ephemeral port; **ACCESS TOKEN** (mono, ↻
Regenerate rotates it; per-session token semantics per ENGINE-API §1); **ALLOWED ACTIONS** — three
permission-group toggles gating tool groups (not literal tool names):

| Group | Gates (MCP-AND-ENGINES §2 tools) |
|---|---|
| **search & read** | `search_assets`, `semantic_search`, `find_similar`, `get_asset*`, `list_*` |
| **tag & traits** | `add_tag`, `retag`, `apply_mapping` |
| **identify** | `identify_asset`, `suggest_tags`, `unpack_package` |

Side-effecting import/export tools stay confirm-gated per call (MCP-AND-ENGINES §5) and are not a
blanket toggle. MCP ↔ REST 1:1 invariant unaffected.

### 11.6 STORAGE CAPS

**Keep extracted packs cached** toggle (default **off**): on, extracted packages rehydrate from
the workspace + catalog at startup (`asset_file.workspace_rel`, DATA-MODEL §13 M6) — no
re-extraction; retention-evicted files simply don't return. Off: extractions are session-only.
**Preview cache limit** slider (1–16 GB) + usage bar (verdant → amber >80% → red over) with
`used by N thumbnails` / headroom labels; **Auto-purge over cap** toggle (drop least-recently-viewed
previews). The extracted-archive **workspace** cap + retention (INGESTION §2) and **Clear cache /
Clear workspace** actions live in this card too (DESIGN-GUIDELINES §7 Storage group).

### 11.7 ENGINE BRIDGE
Live status row (verdant dot glow when bridged): `Live — bridged to <project>` / `Bridge paused`,
sub-line `<engine> · imports sync to <dest> as you stage them`; enable toggle (paused ⇒ staged
items queue). Plus: **Confirm before moving to Trash** toggle; **Show the first-run setup screen**
toggle (binds `skip_onboarding`, same flag as §1).

---

## 12. Stub screens

Any navigable destination whose screen isn't built renders: large dim glyph · Cinzel title · one
sentence of what it will be · *"Not wired yet."* · **← Back to Library**. Never a dead click, never
a hidden nav row.

---

## 13. Inspector (right panel, 300px)

Opens when an asset is selected; ✕ / Esc closes. Top→bottom:

1. **Preview** (160px): thumbnail (Unidentified: 30% + grayscale + big `?`); sounds show a static
   waveform, animated EQ bars while playing; **⤢ full size** pill (→ Quick Look); ✕ close.
2. **Audio transport** (sounds): round ▶/◼ + scrubber + mono `pos / dur` (scrubber appears after
   first play; *"play to scrub the timeline"* hint before). One global audio at a time; switching
   assets/screens stops playback.
3. **Identity**: type diamond + name; status line (gem + label + `· Type`); a 2px gradient rule in
   the status color.
4. **Stat block**: mono key/value rows (per-modality: Format/Tris/Materials/Rig/Size;
   Format/Duration/Sample rate/Channels/Size; Format/Resolution/Channels/Color space/Size — from
   INGESTION §4 metadata).
5. **TRAITS**: chips (manual = trait-colored w/ icon; AI = arcane outline + confidence underbar,
   AI-PIPELINE §6); Unidentified → *"No traits yet — run Identify."*; below, dashed **`+ trait`**
   quick-add chips from the Trait Library (scope-matched, not yet applied).
6. **LINKED FILES** (when the asset has dependencies — `asset_relationship`, DATA-MODEL §10):
   caption notes the parse source (`parsed from FBX bindings` / `.mtl sidecar` / `.meta GUID refs`);
   rows: kind glyph (◇ material violet / ▦ texture sapphire, textures indented) + mono name +
   state: **✓ in pack** (verdant) · **⤷ in library** (arcane; click opens that asset) ·
   **✕ missing** (red). Missing rows expand a **relink panel**: declared path, name-match
   suggestions from the library (`NN% · use →`), **⌕ Browse disk…**, **⊘ Ignore link** (drops the
   edge). Footer warning when any missing: *"✕ missing links are skipped on export — click one to
   relink."* (⚖ requires the unresolved-reference delta, §20.)
7. **COLLECTIONS**: toggle chips (`+ name` / `✓ name` gold) — membership per §6.
8. **SIMILAR IN YOUR HOARD**: top 3 by shared tags + same-type bonus (thumb + name + pack, → that
   asset). Backed by `find_similar` (embeddings) once available.
9. **Provenance footer**: `Source · <library · pack>` (pack → Pack stats), `License · <effective
   license>` (asset → package fallback, DATA-MODEL §9), and when imported: verdant
   `✓ imported in <project> → <dest>/`.
10. **Actions**: Unidentified → full-width **✦ Identify now**. Otherwise: **⟡ Stage / ✓ Staged**
    (gold outline) + **↧ Import / ✓ Imported** (gold primary; dup-guard toast when already in the
    active project) side-by-side, then **✦ Re-identify** (arcane outline; re-queues identification).

---

## 14. Quick Look

Centered modal (780×420, elevation 2, blur scrim; click-outside/Esc/Space closes; ←/→ step through
the current visible set, syncing selection). Left pane (440px): image/texture full preview;
models: the pre-render + caption *"low-res pre-render · orbit viewport in app"* (the real orbit
viewport per INGESTION §5 replaces it); sounds: large 30-bar waveform with played-portion coloring +
transport. **Always renders full detail** regardless of §11.3. Right pane: identity + stat block +
chips (as inspector §13.3–5, read-only) and **Stage / Import** for identified assets; footer
`← prev · Esc closes · ← → step · next →`.

---

## 15. Pack stats (modal)

Opened from any pack-name link (cards, list rows, Packages, Stats). 620px modal:
Cinzel pack name + mono meta line (`library · license · N exported to <project>`); 4 stat tiles
(assets / on disk / triangles / unidentified — amber when > 0); **CONTENTS** per-type rows
(diamond + label + `count · size · type-specific extra`); **TAGS IN THIS PACK** (top 12 chips —
click = search that tag *scoped to the pack*; empty → *"no tags yet — assets in this pack are
Unidentified"*); **HEAVIEST IN PACK** top 3 (tier badge + name + stats, click → select in Library);
footer **Browse <pack> in Library →** (gold primary).

---

## 16. Projects, staging tray & the import pipeline

The GUI face of ENGINE-API §5–6 / MCP-AND-ENGINES §3.

### 16.1 Project switcher (top-bar chip popover, 310px)
- **EXPORT TARGETS**: row per registered project — dot, name + engine, mono `path/dest`; right side:
  gold `N staged` badge (when > 0) + dim `N imported` count; ✓ marks the active project; click
  switches (staging trays swap — see 16.2).
- **RECENT EXPORTS · <project>**: last 4 import batches — `✓ N files → dest` + relative time +
  **↩ undo** (confirm-gated: removes that batch's copied files from the project and deletes its
  `import_record`s; catalog assets untouched).
- **+ Connect Unity project…** (dashed) → wizard (§17).
- Footnote: *"staging trays are per-project — switching never mixes picks."*

### 16.2 Staging tray (fixed bottom-right, 300px, gold border + glow)
**⚖ Resolution:** staging is **per-project** — one staging collection per registered project
(replaces the single shared shortlist; DATA-MODEL §9 / ENGINE-API §6 delta in §20). The tray always
targets the **active** project; MCP `stage_asset`/`list_staged`/`import_staged` gain optional
`project_id` (default: active). Contents:
- Header: `⟡ Staging` + project dot/name + mono count + `clear`.
- Thumb strip (34px tiles, type-color border, name tooltip).
- Dependency line: `N picks → M files incl. materials & textures` (dependency closure minus missing
  links, which are skipped with the §13.6 warning).
- Destination row: `→ <project> /` + editable dest subfolder (persists per project).
- Primary: **Import all → <project>** *(⚖ user-facing verb is **Import** for into-project; "Export
  to folder" is reserved for the arbitrary-path `export_asset` flow)*; caption *"picks + linked
  files only — never the whole pack."*
- **Conflict state** (dup-guard, ENGINE-API §5.4): amber panel *"N of these already exist in
  <project> — exporting again would replace those files."* → **Overwrite / Skip existing / ✕**.
- **Running state**: mono log lines (resolve → overwrite/skip note → missing-links warning →
  copying → done). Completion writes `import_record`s (one **batch**), clears the tray, pushes a
  history row, toasts.

### 16.3 Bridge levels
- **Level 1 — file-copy (no plugin)**: Lodestash copies files into the project; Unity imports on
  next editor focus. Registration via the wizard (§17). `import_record` provenance still written.
- **Level 2 — plugin bridge**: the Unity panel registers/heartbeats (`register_project`) and drains
  the import queue on the main thread with GUID capture (ENGINE-API §5.3). Same UI; richer results.

---

## 17. Connect Unity project (wizard)

Modal, 540px, two steps (`step 1/2 · pick folder` → `step 2/2 · confirm`):
1. **Pick folder**: candidate rows (recent/sibling folders) — name + mono path; right badge
   `✓ Unity project` (verdant, with `<version> · <pipeline>` read from
   `ProjectSettings/ProjectVersion.txt`) or `✕ not a project` (disabled). `⌕ Browse elsewhere…`.
   Caption: *"Nothing is written until you export."*
2. **Confirm**: detected summary (verdant row: name + path); mono detail rows — Editor (from
   ProjectVersion.txt), Render pipeline (feeds the `pipeline_mismatch` advisory, ENGINE-API §5.4),
   Bridge (`imports on editor focus — no plugin` for Level 1); **DESTINATION FOLDER** preset pills
   (`Assets/Imported · Assets/ThirdParty · Assets/Art/External`) + editable field; **← Back** ·
   **Connect project →** (gold). Connecting registers the project (DATA-MODEL §9), assigns its
   accent dot, creates its empty staging collection, and makes it active.

---

## 18. Toasts & jobs

Toasts: bottom-right (left of the tray), `--ls-overlay`, 3px accent left-border by intent
(success verdant / warn amber / info blue / AI arcane / action gold), ~3.2s, `lsRise` in. Every
mutating action confirms itself with a toast naming the object (*"⟡ Staged Beaver_LowPoly →
MyRPG"*), and destructive toasts carry Undo where DESIGN-GUIDELINES §7 permits. Long-running work
(scan, identify-all, extraction, import) surfaces in the jobs tray with progress + cancel
(`cancel_job`).

---

## 19. Screen ↔ engine-surface map

| UI | REST/MCP (ENGINE-API §2) |
|---|---|
| Library search (Tags/AI) | `search_assets` / `semantic_search` |
| Identify all / now / re-identify | `identify_asset`, `get_job_status`, `cancel_job` |
| Trait quick-add, bulk tags | `add_tag` / `retag` |
| Similar in your hoard | `find_similar` |
| Packages browse/extract | `list_packages`, `unpack_package` |
| Stage / tray / import all | `stage_asset`, `list_staged`, `import_staged` (+ `project_id`) |
| Inspector Import | `import_to_project` |
| Project switcher / wizard | `list_projects`, `get_active_project`, `register_project` |
| Quick Look preview | `get_asset_preview` |

---

## 20. Resolutions & required spec deltas

Decisions this spec takes where the prototype and owning docs disagreed — **each is also reflected
in the owning doc** (same-change rule):

1. **Sidebar replaces nav rail** — one 210px labeled sidebar (nav + badges + status/libraries
   facets + add source). *(DESIGN-GUIDELINES §7 updated.)*
2. **Per-project staging** — one staging collection per project (`collection.is_staging=1` +
   `project_id`), not a single shared tray; stage ops gain optional `project_id` (default active).
   *(DATA-MODEL §9, ENGINE-API §6, MCP-AND-ENGINES §2 updated.)*
3. **Import vs Export verbs** — "Import" = into a project (tracked, dup-guarded); "Export to
   folder" = arbitrary path (`export_asset`). The prototype tray's "Export to <project>" label is
   superseded.
4. **MCP settings card** = the optional Streamable-HTTP transport of the one MCP server: stable
   configurable port (default **4319**), rotating token, and a **literal per-tool allow-list**
   (FEATURES.md §6.5) toggling the real MCP tools (`search_assets`, `find_similar`, `identify_asset`,
   `suggest_tags`, `add_tag`/`retag`, …) — names must match the canonical inventory, and toggles keep
   MCP↔REST 1:1; import/export stays per-call confirm. *(MCP-AND-ENGINES §2 updated.)*
5. **Type taxonomy** — the 7-value `media_type` enum stands (data-model invariant). The **top-bar
   category tabs are four coarse buckets** (`All · Models · Sounds · Images`, §0.3) grouping the
   enum — a presentation choice, not an enum reduction (FEATURES.md §0.2). Element-color tokens
   extended: texture `#6FB1FF` (shares sapphire with image), material `#A98BFF` (shares amethyst
   with model), sprite `#5BD897` (verdant), font `#F2966E` (coral). *(DESIGN-GUIDELINES §2 updated.)*
6. **Identify profiles** — presentation tiers bind to AI-MODELS-BY-GPU model sets; cloud steps only
   under per-modality cloud opt-in. Prototype model names are placeholders.
7. **Unresolved dependencies** — `asset_relationship` gains an unresolved form: `to_asset_id`
   nullable + `declared_path TEXT` + `state ('resolved'|'library'|'missing')` so the Linked-Files
   UI (ok / in library / missing + relink/ignore) has backing rows. *(DATA-MODEL §10 updated.)*
8. **Traits** — `tag` gains `scope`, `usage_note`, `teach`, `icon` (color exists); teaching traits
   are injected into the identifier vocabulary. Trait Library supersedes the bare "Tag manager"
   naming; mapping library unchanged. *(DATA-MODEL §2 note + AI-PIPELINE §6 updated.)*
9. **Import batches & undo** — `import_record` gains `batch_id` so Recent Exports can list and
   **undo** a batch (delete copied files + records; confirm-gated; catalog untouched).
   *(DATA-MODEL §9 updated.)*
10. **Grid density stops** — card min-widths 150/205/288 (Compact/Cozy/Large); the implementation's
    thumbnail-px stops map onto these card widths.
11. **Bridge levels** — Level 1 file-copy (wizard-registered, "imports on editor focus", no plugin)
    is a first-class path alongside the Level 2 plugin queue. *(ENGINE-API §5 note.)*

**Deliberately not adopted from the prototype:** fixed MCP endpoint semantics as a second protocol
surface (violates MCP↔REST 1:1); "no plugin" as the *only* bridge; 3-type taxonomy; fake AI model
names; shared staging tray.
