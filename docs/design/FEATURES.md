# Lodestash — Complete Feature Handoff

Exhaustive, feature-by-feature spec for the **Lodestash Prototype** (`Lodestash Prototype.dc.html`).
This is the behavioural companion to `README.md` (which focuses on the Settings + Library + Unity
work). Everything below is demonstrated live in the prototype — treat the prototype as the source of
truth for interaction, and this doc as the written contract.

**Target stack:** Rust + `egui`. UI in `crates/lodestash-ui/src/app.rs`; persistence in `config.rs`;
tokens in `theme.rs`. Data/ingest in `lodestash-core` / `lodestash-ingest`; AI in `lodestash-ai`;
serving in `lodestash-api` (REST) and `lodestash-mcp` (MCP) — **REST and MCP stay 1:1**.

**Colour tokens** are listed in `README.md → Design Tokens`; this doc names tokens, not hex.

---

## 0. Shell & global chrome

### 0.1 App frame
- Full-height column: **top bar (48px)** → **body row** (sidebar 210px + main column + optional
  inspector 300px). Background is `BG` with two faint radial gold/violet glows (decorative only).
- Screen routing is a single `screen` enum: `onboarding | library | identify | collections |
  packages | unity | vocabulary | stats | dups | trash | settings`. The main column shows exactly one
  screen `sc-if`; `library` and `onboarding` share the library body (onboarding is an overlay on top).
  Ten nav destinations (onboarding is an overlay, not a nav row); `unity` (**Unity Assets**) is its
  own destination distinct from `packages` (§5 vs §4.1).

### 0.2 Top bar
- **Brand** (lodestone diamond + “LODESTASH” in Cinzel).
- **Search box** — the primary query input. Feeds either Tag search or AI search per the mode toggle.
- **Mode toggle** — segmented `◇ Tags` / `✦ AI`. Tags = literal tag/name/pack substring match; AI =
  ranked semantic search (see §1.6). Persist last mode.
- **Category tabs** — `◆ All / ◎ Models / ♪ Sounds / ▦ Images`, a coarse filter over the grid.
  These four tabs are a **presentation grouping over the 7-value `media_type` enum**, which is
  unchanged (`model_3d | texture | audio | image | sprite | material | font` — a data-model
  invariant). Bucket → `media_type` mapping (finer distinctions live in the ⚑ Filters popover
  §1.4, not the top bar):
  | Tab | `media_type`(s) |
  |---|---|
  | ◎ Models | `model_3d`, `material` |
  | ♪ Sounds | `audio` |
  | ▦ Images | `image`, `texture`, `sprite`, `font` |
- **Project chip** (right) — shows the active Unity export target; click opens the **Project switcher**
  (§7.1). Followed by a **`✦ local · RTX 4070`** chip advertising on-device inference.

### 0.3 Sidebar (left rail)
- **Nav list** (`navItems`): Library, Identify (badge = unidentified count), Collections, Packages,
  Unity Assets, Traits, Stats, Duplicates (badge = dup groups), Trash (badge = trashed count),
  Settings. Active row = `TEXT` fg, `SURFACE_RAISED` bg, `GOLD` left-border. **Traits** is the nav
  label (matching the prototype); the screen hosts both Traits and Mapping tabs (§4.6, DATA-MODEL §4).
- **Facet sections** (only while on Library, `filterOpen`): **STATUS** (All/Unidentified/Identified/
  Confirmed with counts), **LIBRARIES** (All + each source library + pinned Unity libraries + `Add
  source…`). See §1.2 and §5.4.
- Nav glyphs are Unicode except **Unity Assets**, which uses the real SVG `ls-unitypackage` mark.

### 0.4 Toasts
- Bottom-right stack, 1px accent left-border coloured by intent (`SUCCESS`/`WARNING`/`DANGER`/
  `ARCANE`). Auto-dismiss. Every mutating action emits one (documented per feature below).

---

## 1. Library (the asset browser)

The core screen: a responsive grid of asset cards with a filter/sort toolbar and an inspector.

### 1.1 Asset card
- **Thumbnail** (procedurally tinted in the prototype; real app renders a cached preview), a **size
  tier badge** (S/M/L/XL, coloured by tier — see §5.3), a **status gem** (`?` unid / `✓` identified /
  `★` confirmed), **stage** button (adds to the staging tray), and a **⤢ full-size** affordance.
- **Body:** name, `pack · stats` line, and **tag chips** (first 3 + “+N” expander). Confirmed/AI tags
  show a confidence bar; unidentified cards show a `???` placeholder.
- **Click** selects (opens inspector); **Space** on a selection opens Quick Look; **⟡ stage** stages.

### 1.2 Sidebar facets (STATUS / LIBRARIES)
- **STATUS** filters by identification state; **LIBRARIES** filters by source library. Counts are live.
- **All libraries** clears the source filter. Selecting a specific source library filters to it and
  **preserves** other active filters. Pinned Unity libraries appear here too (§5.4) with a `⬢` mark
  and an `✕` remove control.

### 1.3 Library toolbar (strip above the grid)
Left→right: **result label** (`N assets · <library>` or `✦ N ranked matches for "q"`), active
**pack**/**collection** filter pills (each with `✕` clear), the **⇢ Add all N to project** action
(only when a pinned Unity library is the active filter — §5.5), **sort** select (default / name /
largest / smallest / tri count), **view mode** (grid / list), **grid-size** selector, and the
**⚑ Filters** popover trigger (badge = active filter count).

### 1.4 Filters popover
Three multi-select sections (an asset passes if it matches **selected traits (any)** AND **selected
tri buckets (any)** AND **selected size tiers (any)**):
- **TRAIT** — chips of the library’s tags, most-frequent first, with a **search field** to filter the
  chip list. Selected = `ARCANE` outline + `ARCANE_BRIGHT` text.
- **TRIANGLES** (models only) — `≤1k / 1k–3k / 3k–6k / 6k+`. Selected tinted `TYPE_MODEL`.
- **FILE SIZE** — S/M/L/XL, ranges driven by the Settings size-tier thresholds (§5.3). Selected
  tinted by tier colour.
- Header has **Clear all** (danger on hover). Implement the pass function as a pure, unit-tested
  predicate.

### 1.5 Unidentified nudge
When any assets are unidentified, a banner spans the top of the library: “N assets are Unidentified —
invisible to AI search until appraised” with a shortcut to **Identify mode**.

### 1.6 Search — Tags vs AI
- **Tags mode:** substring match over name/pack/tags; instant.
- **AI mode:** shows a brief “thinking” state, then a ranked result set (`aiResults`) with match
  scores; empty result emits an informational toast. Only **identified** assets participate. Wire to
  `lodestash-ai` embeddings / `search_assets` (`IDENTIFICATION-AND-SEARCH.md`).

### 1.7 Watched-folder simulation
On first library open, the prototype simulates a watched source dropping in new files (toast + the
Identify backlog grows). Real behaviour: file-watcher on watched sources auto-indexes new assets into
the Unidentified backlog (`INGESTION.md`), honouring per-source blacklists (§5.1).

---

## 2. Inspector (right panel)

Opens on asset select. Contents:
- **Preview header** (thumbnail / sound icon), name, pack, library, license, status gem.
- **Stats rows** (format, tris/duration/resolution, materials, size, …) from the asset’s `rows`.
- **Tags** — existing chips + confidence; **add-tag** field (creates a manual tag). Editing tags here
  feeds the Trait teaching signal.
- **Dependencies** — the asset’s `deps` list (materials/textures), each with a state: `ok`, `lib`
  (resolved to another library asset, clickable), or `miss` (broken — offers a **relink** with a
  suggested match). This is the dependency-graph surface reused by Unity “Add to project”.
- **Actions** — stage to tray, open Quick Look, open pack stats.

---

## 3. Identify mode

The appraisal queue that turns Unidentified assets into searchable, tagged assets.
- **Header** shows backlog size and a **Run/appraise** control; a progress state processes the queue
  (in the prototype, incrementally; real app calls the identify model per the GPU profile — §5.2).
- Each processed asset gains AI tags with confidence and flips `unid → id`. Finishing the backlog
  emits a success toast. Auto-identify (§5.2) runs this automatically on watched-folder drops.
- Wire to `lodestash-ai` identify tool; the same tool is exposed over MCP as `identify_asset`.

---

## 4. Packages, Collections, Duplicates, Trash, Stats, Traits

### 4.1 Packages
Everything owned, grouped **publisher → pack**. Each pack row: name, unidentified badge, asset count,
**◉ Stats** (opens the Pack-stats modal §7.2) and **Browse →** (filters the library to that pack).
> Distinct from **Unity Assets** (§5): Packages is the aggregate view of *already-indexed* assets;
> Unity Assets manages `.unitypackage` archives as sources.

### 4.2 Collections
Pinned loadouts of assets per project/theme.
- **Create** via the header name field + Create. Each collection card shows a thumbnail strip, count,
  **Browse →** (filters library to the collection), and **✕ delete**.
- Assets are added to collections from the inspector. Persist collections in config.

### 4.3 Duplicate review
Near-duplicate groups found across packs. Nothing is auto-deleted.
- Each group: reason (e.g. hash/name similarity), member cards (one flagged **★ keeper**), and actions
  **Keep both** / **Merge tags** / **Move duplicate to Trash**. Empty state when cleared.
- Back with the near-duplicate detector; the Unity import/pin duplicate check (§5.6) reuses the same
  normalized-name + hash matching.

### 4.4 Trash
Catalog-only removal (files stay on disk). Rows show name, pack, stats, and **↩ Restore**. Empty
state when empty. A confirm-trash flag governs whether destructive actions prompt.

### 4.5 Stats
Overview dashboard: stat tiles (assets indexed, on-disk size, total triangles, identified %),
heaviest packs (bar chart), heaviest assets, a **trait/link graph** canvas, and generated **insight
rows** (unidentified count, duplicate groups, heaviest-model share, heaviest pack, teaching traits).
Tabs switch between overview and graph. All derived from the live asset set.

### 4.6 Traits — Trait library (+ Mapping tab)
The **Traits** destination hosts two tabs: **Traits** (below) and **Mapping** (the
taxonomy/category/alias/rule library — DATA-MODEL §4, unchanged here). This section specs the
Traits tab: user-defined traits that teach the identifier.
- **Create** a trait: name, scope (any/model/sound/image), optional usage description (advanced), and
  it starts **teaching** by default. Traits list shows colour/icon, scope, usage.
- Per trait: **teach/pause** toggle (whether the identifier suggests it), **Find** (library search for
  it), **Edit** (colour/usage), **Delete**. Teaching traits are fed into the next identify scan.

---

## 5. Unity Assets (`.unitypackage` libraries)

Dedicated flow for Unity Asset Store packages (e.g. `%AppData%/Roaming/Unity/Asset Store-5.x`).
**Indexed in place — nothing is copied; files are surgically extracted on demand** (the archive is
never fully unpacked). Nav glyph = the SVG `ls-unitypackage` cube. Two entry points: the sidebar nav
item **and** the Add-a-source chooser (§7.4).

### 5.1 Sources & per-source blacklist (Settings → Sources)
Watched sources each have: status dot, name, mono path, meta, an **exclusions/blacklist** disclosure,
and an enable/disable toggle. The blacklist holds `Folder` or `Asset` excludes (chips with `✕`);
indexing skips them. `+ Add source` opens the chooser (§7.4). Persist blacklist on the source model
(`DATA-MODEL.md §1`); honour it on crawl (`INGESTION.md`).

### 5.2 Import dialog (folder → package list)
- **Folder row** with the chosen path + **Choose folder…**; on pick, scan for `.unitypackage` files.
- **Grouped by publisher / containing folder** (Asset Store-5.x layout). Each group collapses and has
  **select all / clear**.
- **Per package:** checkbox, name, `vX · size · N assets`, an **auto-detected content-category** badge
  that is also a **dropdown override** (3D Models / Audio / Code / UI / Materials), and — for multi-
  version packages — a **“N older versions · newest shown”** disclosure (older versions are not
  imported). Packages that likely overlap existing content show a `⧉ possibly already indexed as “…”`
  flag (§5.6).
- **Category filter tabs** (All / 3D / Audio / Code / UI / Materials) filter the list.
- **Footer:** “indexed in place — no files are copied”, a running **N selected** count, and **Import
  as libraries**. Each selected `.unitypackage` becomes **one Library** (named by the package,
  publisher as subtitle).

### 5.3 Unity Assets landing
Cards for each imported package: `ls-unitypackage` mark, name, publisher · version, category badge,
mono path, `N assets · size`, **↻ Sync** (re-scan archive index), **Open →** (browse/preview §5.4).

### 5.4 Browse / preview one package
- **Header:** back, mark, name + category badge, path, `N assets · size · indexed in place`, **↻
  Sync**, and **＋ Add to Library / ✓ In Libraries · remove** (§5.5).
- **Left rail:** the **surgically-extracted** file tree, grouped by in-archive folder (Prefabs /
  Models / Textures / Materials / Audio / Scripts), each entry typed with a glyph+colour.
- **Right pane — per-asset preview**, extracted without unpacking the whole archive:
  - **3D model** → orbit/zoom viewport with tri/vert/material readout.
  - **Texture / material** → checker-backed swatch + dimensions/format.
  - **Audio** → peak waveform + transport: circular play/pause, a **seek timeline** (drag/click
    to scrub), elapsed/total time, sample-rate/channels, and a **global volume** control
    (persisted, applies to all playback). *(App-added beyond the prototype's static waveform +
    play/pause; the prototype has no volume control.)*
  - **Prefab** → resolved object hierarchy tree.
  - **Script** → line-numbered code peek.
- **⇢ Add to project** (per asset) copies that file **plus its resolved dependencies** into the open
  Unity project — resolved from the package’s own dependency graph, not the whole archive.

### 5.5 Add to Library (grid promotion) + remove
- **Add to Library** pins the package into the sidebar **LIBRARIES** list and surfaces its assets in
  the **normal library grid** — same cards, sort, filters, Quick Look, staging as any other library.
  The *Unity Assets* screen keeps the preview/browse view; pinning is what makes it a first-class
  library. A pinned library **persists until removed** — the pin is a **`package.pinned` flag on the
  catalog `package` row** (DATA-MODEL §7, M7), not a separate config list; its assets surface in the
  grid through the existing `asset.package_id` join.
- **Remove** two ways: the sidebar facet `✕`, or the browse-header toggle (**✓ In Libraries ·
  remove**). Removing drops the facet, pulls its assets out of the grid, and resets the filter to All
  if that library was active.
- **⇢ Add all N to project** (toolbar, when a pinned Unity library is active) copies every asset in
  the library plus dependencies in one step.

### 5.6 Duplicate check
On both import and Add-to-Library, packages whose contents likely overlap an already-indexed pack are
flagged (`⧉` note in the import list + warning toast). Non-blocking — the user decides. Matching is
normalized package/pack name (+ content hashes in the real app), shared with §4.3.

---

## 6. Settings

Scrollable, max content width ~760px, section cards on `SURFACE`. Sections:

- **6.1 Sources** — watched sources + per-source blacklist + `+ Add source` (§5.1, §7.4).
- **6.2 Identify profile** — Integrated / Balanced / Dedicated GPU tiles; selecting one sets the
  identify model set **and** preview render quality together (`AI-MODELS-BY-GPU.md`). Plus an
  **Auto-identify new files** toggle (runs Identify on watched-folder drops).
- **6.3 On-screen quality** — Low / Medium / High; affects **grid thumbnail render only** — the
  full-detail preview is reserved for full-size Quick Look.
- **6.4 File-size labels** — draggable S→M / M→L / L→XL thresholds behind the size-tier badges, plus
  an editable upper **cap** (`cap_kb ≥ xl_kb + 128`; larger tiers keep a 128 KB gap so ranges never
  overlap). **Reset defaults** restores them. These thresholds drive the Filters size section (§1.4).
- **6.5 MCP** — enable flag + endpoint/token (copy endpoint, regenerate token) + a **literal
  per-tool allow-list**. **Do not invent a new protocol surface** — the allow-list toggles the
  **real** MCP tools that exist on both planes (`search_assets`, `find_similar`, `identify_asset`,
  `suggest_tags`, `add_tag`/`retag`, …); keep **MCP↔REST 1:1** (`MCP-AND-ENGINES.md §2`,
  `ENGINE-API.md §2`). (Names must match the canonical inventory — §1.6 already uses `search_assets`.)
- **6.6 Storage caps** — preview-cache limit slider + live usage bar (scales with on-screen quality;
  `WARNING` near / `DANGER` over cap) + **auto-purge over cap** toggle.
- **6.7 Engine bridge** — live bridge to the connected engine/project.
- **6.8 First-run setup toggle** — the Settings-side control for `skip_onboarding`; bound to the
  onboarding’s “Don’t show this next time” checkbox (they are one persisted boolean, inverse of each
  other). Off = app opens straight to the library on launch.

---

## 7. Overlays, modals & wizards

- **7.1 Project switcher** — top-bar popover listing export targets (Unity projects); shows recent
  exports per project with **↩ undo** (removes those files from the project), and **Connect Unity
  project…** (opens the wizard §7.5). Staging trays are per-project and never mix on switch.
- **7.2 Pack stats modal** — per-pack rollup: stat tiles, tag breakdown (each **Find**s in library),
  top assets (each **Open**s), and **Browse** the pack. Opened from Packages/Stats/inspector.
- **7.3 Quick Look** — full-size preview modal (Space / ⤢). Renders at **full detail regardless of the
  on-screen quality setting**. Arrow keys step through the current visible set; sound assets play.
- **7.4 Add-a-source chooser** — modal with **Watch a folder** (existing watched-source path) and
  **Unity Asset Store packages** (opens the import dialog §5.2). Reached from the sidebar `Add
  source…` and Settings `+ Add source`.
- **7.5 Connect Unity project wizard** — multi-step: pick project, engine, destination folder; on
  finish, registers an export target and starts the bridge watching it.
- **7.6 Staging tray** — fixed bottom-right; accumulates staged assets per project. Shows conflict
  warnings (files already exported), an **export** run with progress lines, and writes a recent-export
  history entry (undoable via §7.1). Uses the same file+deps export path as Unity “Add to project”.

---

## 8. First-run onboarding — the “Point it at your hoard” splash

The splash menu is the **First-run onboarding** overlay (screen key `onboarding`,
`data-screen-label="First-run onboarding"`); its headline is **“Point it at your hoard.”** It is a
centred modal **overlay on top of a fully-rendered (dimmed) library** — not a blank page. Its only
graphic is the brand **lodestone diamond** (`ls-logo` / `logo.svg`); everything else is text + source
rows.
- Lists detected sources (Unity Asset Store cache auto-detected, plus added folders), an “existing
  index found / N new files” status, and actions: **Open library**, **Scan N new assets**, **Add
  another folder…**, and a **Don’t show this next time** checkbox (drives `skip_onboarding`, §6.8).
- On real launch, when `skip_onboarding` is set the app opens straight to the library. The prototype
  always shows onboarding for demo purposes; there the checkbox only reflects/persists the preference.

---

## 9. Cross-cutting behaviour

- **Library nav reset:** clicking the top-level **Library** nav returns to All libraries and clears
  ALL active filters (library, status, category, query, trait/tri/size sets). Switching between source
  libraries via the facet does **NOT** clear filters — only the Library nav is the “reset to
  everything” affordance. Keep these two code paths separate in `app.rs`.
- **Staging is per-project;** switching projects never mixes picks.
- **Nothing is destructive without intent:** Trash is catalog-only and restorable; duplicates are
  never auto-deleted; imports/pins copy nothing.
- **Motion:** honour `prefers-reduced-motion`; popovers ~160ms fade+rise; no gratuitous animation.
- **Derived logic** (`size_tier_of`, filter predicate, grid column math, duplicate match, dependency
  resolution) lives as **pure, unit-tested functions** — the egui render loop stays thin
  (`TECH-STACK.md §5.1`).

---

## 10. Persisted state (config.rs / AppConfig)

- `thumb_size` grid stop; on-screen preview quality (`Low|Medium|High`).
- `SizeTiers { m_kb, l_kb, xl_kb }` + `cap_kb` (`cap_kb ≥ xl_kb + 128`).
- `skip_onboarding: bool`.
- Sources with `blacklist: Vec<Exclude { kind: Folder|Asset, value }>`.
- Unity libraries live in the **catalog**, not config: each is a `package` row (name, publisher,
  category, version, path, asset_count, size — DATA-MODEL §7, indexed in place; category
  auto-detected + override; newest version only) with a **`package.pinned`** flag marking the ones
  promoted to the sidebar. Config persists only what isn't catalog-derived (custom Asset Store cache
  roots, `cache_extracted`).
- Collections; user traits (name, scope, usage, teach flag, colour/icon).
- MCP (enabled, transport/port, token, per-tool allow-list); auto-identify, auto-purge, cache cap,
  engine-bridge, confirm-trash flags; active export target + per-project staging/history.
- **Transient (not persisted):** active filter sets, trait-search string, popover/expanded state,
  AI “thinking”/results, current selection, current screen.

---

## 11. Icons

From `icons/icons.svg` by symbol id (24×24 stroke, coloured per token). Nav, type, status-gem, and
action ids as listed in the **Glyph Catalog**. The one bespoke SVG is **`ls-unitypackage`** (isometric
cube, `SUCCESS` green) for the Unity Assets nav item, library cards, and the Add-a-source chooser. All
other prototype glyphs are Unicode; the real app substitutes the matching `ls-*` vector symbols.
</content>
