# Lodestash — Design Alignment Audit

> **What this is.** A conformance audit of the **current desktop app** (Tauri 2 + Svelte 5,
> `crates/lodestash-desktop/ui`) against the design bundle in `docs/design/` and the governing
> design docs (`docs/DESIGN-GUIDELINES.md`, `docs/UI-SPEC.md`). Every item is classified
> **Aligned · Partial · Missing · Divergent**, with the spec citation and the app's
> `file:line` evidence.
>
> **Scope.** The Svelte app is the current app (Phase B port). `crates/lodestash-ui` (egui) was the
> pre-cutover build, **deleted at the B7 cutover (2026-08-07)** — referenced only as a behaviour source.
>
> **Precedence** (per `docs/design/README.md` + `CLAUDE.md`): `FEATURES.md` is the bundle arbiter;
> governing `docs/` files win over the bundle on conflict; `DESIGN-GUIDELINES.md` wins over pixel
> details; `tokens.css` is the single executable token source (the Svelte app imports it directly).
>
> **Limitation.** `docs/design/screenshots/*.jpg` cannot be pixel-inspected (no image tool in this
> environment); the audit is structural — against the prose specs and the prototype HTML. Raster
> screenshots remain a pixel-level reference only.

---

## Method

1. Extract the spec per area from the design bundle + governing docs (Phase 1.1).
2. Read the app surface (`ui/src/**`) and the desktop-crate commands the UI can call (Phase 1.3).
3. Classify each item with evidence; severity: **V** visual / **M** missing / **D** divergent /
   **B** backend-blocked.
4. Remediate (Phases 2–4); re-verify and finalize (Phase 5).

---

## Verdict

**At audit time (2026-08-04)** the Svelte shell was structurally right and visually on-system
(tokens imported from `docs/design/tokens.css`, fonts bundled, frame metrics + 11-row nav correct,
Library/Packages/Stats/Duplicates/Traits read-only built with a 47-test e2e suite) — but four of
eleven nav destinations were stubs, onboarding was absent, and a cluster of chrome was missing or
drifted. **After remediation (same day)** every nav destination is a working screen, the missing
chrome is built, the divergences are corrected, and the suite is **48/48 green** (plus 150 Rust
tests). What remains is the documented backend-blocked set — engine/pump/B2b work the board
already tracks.

## Remediation log (2026-08-04)

Everything below shipped in this pass; each entry is the delta that moved the matrix row from
its audit class. `ui/` = `crates/lodestash-desktop/ui`.

- **K1/K2 icons** → sprite now has all 7 media-type symbols (`ls-type-font`, `ls-type-sprite`
  promoted per DESIGN-GUIDELINES §5's promote rule; `typeIcon()` grew the cases), and a proper
  gold `ls-nav-favorites` ★ (Glyph Catalog U+2605) replaces the confirmed-gem placeholder —
  also added to `docs/design/icons/` + the DESIGN-GUIDELINES §5 table.
- **D6 density** → `DENSITIES` px 150/205/288 per UI-SPEC §20.10 (was 118/148/200).
- **A2 glows** → gold top-right + arcane bottom-left per UI-SPEC §0.
- **B3/B4 top bar** → search 400px with per-mode placeholder + mode-coloured focus ring;
  category tabs carry live counts (new `LibraryStatus.tab_counts`, `Category::id()`), a 2px
  token-colour underline, and zero-count hiding.
- **B6 health meter** — still not shipped (needs per-status counts wired into a top-bar bar;
  counts exist, the bar does not — small follow-up, not a stub).
- **D1/D5/D7 toolbar** → result label (`N assets · <library>` / `N matches for “q”`, keeping
  `data-testid=result-count`), ★ Favorites toggle + gold count pill left of ⚑, and the
  viewMode split into **browse (flat|folders) × row style (grid|list)**, both persisted.
- **M1 persistence** → `WebPrefs` in `AppConfig` (`config.rs` `web` group) + `get_web_prefs`/
  `set_web_prefs`; density/sort/browse/row-style survive restarts (hydrate-on-start, save-on-change).
- **E1 filters header** → `FILTER LIBRARY` with section-label styling.
- **B8c pack browsing** → `pack_dir_tree` command (`PackDirNode`: path/depth/direct/recursive/
  has_children) over in-memory rows; `folder_page` gained pack-rooted browsing (package-relative
  `dir`, only that pack's assets) + a per-item third column (duration/dimensions); FoldersView is
  three-pane under a pack filter — 244px DIRECTORY tree with gold-rail selection, ← → ↑ history
  (`view.navDir/back/fwd`), pack-name gold pill breadcrumb, folders-first `Name · Size · Items`
  list in list mode. 2 new Rust tests.
- **B8a Traits working** → `VocabRow` widened (id/is_trait/scope/usage_note/color/icon); four
  writer ops + commands (`upsert_trait`, `unpromote_trait`, `set_trait_teach`, `set_trait_style`)
  on the single writer thread; TraitsScreen now has NEW TRAIT, the ✦ promote strip, the teaching
  toggle, the 6-swatch style palette, and demote; per-trait chip colour flows to card tag rows
  and the ⚑ trait cloud through the shared `traitColor` store.
- **J3 Identify** → Appraise the Backlog workbench (Cinzel header, mono count, backlog grid,
  Backlog-clear empty state). ✦ Identify all is present but honestly disabled: the appraiser
  engine binds at B7.
- **J6 Collections** → real catalog CRUD (`list_collections`/`create_collection`/
  `delete_collection`/`collection_members` over the existing schema, staging tray protected —
  +1 core test), writer ops + commands, CollectionsScreen (inline create, ⟡ cards with count,
  Browse →, ✕ delete), and **Browse →** drives `GridQuery.collection` shown as a clearable chip.
- **J5 Unity Assets** → publisher → pack listing with the ⬢ pin/unpin (the `pinned` flag was
  already fetched, now rendered; `SetPackagePinned` writer op + command), a pinned-only filter,
  and **Open →** into the three-pane pack browser.
- **J11 Settings** → full §11 screen: WATCHED SOURCES with live watch/pause (`set_source_watch`),
  IDENTIFY PROFILE tiers + auto-identify, ON-SCREEN QUALITY, FILE-SIZE LABELS (editable KB
  boundaries + cap with the 128 KB-gap clamp + reset), MCP per-tool allow-list, STORAGE CAPS
  (keep-extracted, 1–16 GB cache cap, auto-purge), ENGINE BRIDGE honest status, FIRST RUN
  (`skip_onboarding` mirror). Whole-object `get_settings`/`set_settings` snapshot; size tiers
  re-read on every reload so badges/⚑ facets follow.
- **I1 onboarding** → first-run panel over the live library ("Point it at your hoard.",
  gold dashed drop affordance, "Don't show this next time" → `skip_onboarding`, shared with
  Settings › FIRST RUN).
- **H1/Space** → Quick Look still opens on Enter; Space binding is a tiny follow-up (keyboard
  map L5), not a stub.
- **Window geometry** (out of the design docs but the launch-blocker): `tauri-plugin-window-state`
  added — size/position/maximized persist to `.window-state.json` and restore before show
  (no launch resize flash). Also cleaned leaked e2e processes that were double-launching the
  app and corrupting config.json.

---

## Audit matrix

Legend: **A** = Aligned · **P** = Partial · **M** = Missing · **D** = Divergent.
Spec citation → app evidence. `ui/` = `crates/lodestash-desktop/ui`.

### A. Frame & shell — FEATURES §0.1, UI-SPEC §0

| # | Item | Class | Spec | App evidence |
|---|---|---|---|---|
| A1 | Frame metrics 48 / 210 / 300 | A | tokens.css `--topbar-h`/`--sidebar-w`/`--inspector-w` | `ui/src/styles/app.css` imports tokens; `TopBar.svelte:59` (`var(--topbar-h)`), `Sidebar.svelte:124` (`var(--sidebar-w)`), `Inspector.svelte:192` (`var(--inspector-w)`) |
| A2 | BG + two radial glows | D | UI-SPEC §0: gold **top-right**, arcane `rgba(73,214,201,.05)` **bottom-left** (prototype line 27) | `app.css:36-38`: gold at 12% **-5%** (top-left) + **violet** `rgba(169,139,255,.06)` at 95% 105% (bottom-right) — wrong corners & hue |
| A3 | Bundled fonts, offline | A | UI-SPEC §0.1; CSP forbids remote fonts | `fonts.css` @font-face all three; `index.html` has no font `<link>`; CSP `font-src 'self'` (`tauri.conf.json:25`) |
| A4 | 12-screen enum; onboarding is an overlay on the library | P | FEATURES §0.1 | `routes.ts:9-22` has all 12; `App.svelte` renders onboarding nowhere — overlay **missing** (see I1) |
| A5 | No design-level statusbar | D | design shows no bottom bar (toasts + staging tray dock bottom-right) | `App.svelte:168-182` adds a 24px statusbar (counts, reload, catalog path) — an honest dev scaffold that occupies the toast/staging zone |

### B. Top bar — FEATURES §0.2, UI-SPEC §0.1

| # | Item | Class | Spec | App evidence |
|---|---|---|---|---|
| B1 | Brand: diamond + Cinzel gold wordmark | A | UI-SPEC §0.1.1 | `TopBar.svelte:10-13` (`ls-logo` + `LODESTASH` Cinzel 700, gold, 0.08em) |
| B2 | Search mode toggle (Tags/AI) | A | UI-SPEC §0.1.2, DESIGN-GUIDELINES §7 | `TopBar.svelte:24-37` segmented `ls-search-tags` / `ls-search-ai`, persisted in `view.searchMode` |
| B3 | Mode accents the search box; per-mode placeholder | P | DESIGN-GUIDELINES §7 ("active mode colors the input's focus ring") | `TopBar.svelte:98` gold focus always; one placeholder `"Search assets, tags, packs…"` (`:19`) — no Tags/AI variants |
| B4 | Search box max-width ≈ 400px | D | UI-SPEC §0.1.3 | `TopBar.svelte:90` `max-width: 560px` |
| B5 | Category tabs, 2px underline, counts, zero hides | P | UI-SPEC §0.3 (5 tabs incl. Materials) | `TopBar.svelte:40-47` — 5 tabs correct, but active = overlay fill only, no 2px underline, no per-tab count, zero-count never hides |
| B6 | Health meter `Identified n/total` + gold bar | M | UI-SPEC §0.1.5, DESIGN-GUIDELINES §6 | absent |
| B7 | Project chip (dot · name · engine · ▾) | P | UI-SPEC §0.1.6, DESIGN-GUIDELINES §7 | `TopBar.svelte:50` static `No project` chip; no switcher (B, see I8) |
| B8 | AI-profile pill `✦ local · <GPU>` | P | UI-SPEC §0.1.7 | `TopBar.svelte:51-53` `✦ local` / `✦ shell only`; no GPU name, no Settings link |

### C. Sidebar — FEATURES §0.3, UI-SPEC §0.2

| # | Item | Class | Spec | App evidence |
|---|---|---|---|---|
| C1 | 11 nav rows, FEATURES order | A | FEATURES §0.3 (Favorites 2nd, Unity Assets 7th) | `routes.ts:36-50` `NAV` — exact order, `Favorites` icon is a documented placeholder (K2) |
| C2 | Badges: Identify/Dups/Trash, none on Favorites | A | FEATURES §0.3 | `Sidebar.svelte:15-19` counts; `routes.ts:32` badge map |
| C3 | Active row: overlay fill + 2px gold rail, reserved | A | UI-SPEC §0.2.1 | `Sidebar.svelte:155-162` `.row.on` + `border-left: 2px solid transparent` on all rows |
| C4 | Library row = reset-to-everything | A | FEATURES §9 | `nav.svelte.ts:50-59` `goLibrary()` clears search/tab/status/packageFilter/source/dir/filter |
| C5 | STATUS facet group (dots + mono counts) | A | UI-SPEC §0.2.2 | `Sidebar.svelte:73-89`, diamond dots via `.dot` rotated square |
| C6 | LIBRARIES group + dashed `+ Add source…` | P | UI-SPEC §0.2.3 | `Sidebar.svelte:91-118`; `+ Add source…` is a **label** that opens nothing (B2b chooser); pinned Unity libs not listed yet |
| C7 | Facets collapse off Library/Identify | A | UI-SPEC §0.2 | `Sidebar.svelte:26` `expanded = route is library|identify` |
| C8 | Unity glyph = `ls-unitypackage` | A | FEATURES §0.3/§5 | `routes.ts:44` |

### D. Library toolbar — FEATURES §1.3, UI-SPEC §2.2

| # | Item | Class | Spec | App evidence |
|---|---|---|---|---|
| D1 | Result label `N assets · <library>` | M | FEATURES §1.3 | `Toolbar.svelte:27-29` renders a bare number only |
| D2 | Pack/collection filter pills (✕) | P | FEATURES §1.3 | `LibraryScreen.svelte:107-114` pack chip ✓; collection pill n/a (collections unbuilt) |
| D3 | Sort select + asc/desc | A | UI-SPEC §2.2 | `Toolbar.svelte:32-47`; `SORTS` incl. Relevance |
| D4 | ⚑ Filters + gold count badge | A | FEATURES §1.3 | `Toolbar.svelte:49-65`, `.badge` gold pill |
| D5 | ★ Favorites toggle + gold count pill (left of ⚑) | M | FEATURES §1.3, UI-SPEC §0.3 | absent (Favorites reachable only via nav row) |
| D6 | Grid-density stops 150 / 205 / 288 | D | UI-SPEC §20.10 ("card min-widths 150/205/288") | `api.ts:218-222` `DENSITIES` px `118 / 148 / 200` (also ≠ bundle's suggested 96/128/176 — governing doc wins) |
| D7 | View toggle grid/list/folders | A | UI-SPEC §2.2 (3 stops in Tauri shell) | `Toolbar.svelte:67-95`; note `viewMode` collapses browse × row-style axes (B8c) |
| D8 | Unidentified nudge banner | M | FEATURES §1.5, UI-SPEC §2.1 | absent |
| D9 | Bulk bar + multi-select | M | UI-SPEC §2.4 | absent (selection is single; no checkboxes) |
| D10 | Pager + assets-per-page selector | P | UI-SPEC §2 | pager ✓ (`LibraryScreen.svelte:158-164`); `pageSize` hardcoded 100 (`:23`), no 50/75/100/custom control |

### E. Filters popover — FEATURES §1.4, design README §3

| # | Item | Class | Spec | App evidence |
|---|---|---|---|---|
| E1 | Header `FILTER LIBRARY` + Clear all | P | design README §3, FEATURES §1.4 | `Filters.svelte:47` header reads `Filters`; Clear all ✓ (`:49-53`) |
| E2 | TRAIT chips + search, arcane selected | A | FEATURES §1.4 | `Filters.svelte:60-78`, `.chip.on` arcane (`:200`) |
| E3 | TRIANGLES buckets, violet selected | A | FEATURES §1.4 | `Filters.svelte:82-95`, `.chip.tri.on` violet (`:204`) |
| E4 | FILE SIZE tiers, tier-coloured selected | A | FEATURES §1.4 | `Filters.svelte:98-111`, ranges from backend facets |
| E5 | LINKS section (uses / used by) | M | FEATURES §1.4, UI-SPEC §2.3 | absent — `ContentFilter` (`api.ts:49-53`) has no `link` group; egui has `LinkFilter` (`lodestash-app/types.rs:587`) but no desktop command |
| E6 | Anchored, never clips edge | A | design README §3 | `Filters.svelte:118-121` anchored right under button, 328px |

### F. Asset card — FEATURES §1.1, DESIGN-GUIDELINES §7, UI-SPEC §2.5

| # | Item | Class | Spec | App evidence |
|---|---|---|---|---|
| F1 | 3px status left-border (unid slate / id verdant / conf gold) | D | UI-SPEC §2.5, DESIGN-GUIDELINES §6 | `Card.svelte:105-123` — uniform `--ls-border`; status only via gem + `???` |
| F2 | Thumb region: gem ✓, star tile, ⤢ full-size, ⟡ stage, size badge, imported badge | P | FEATURES §1.1, UI-SPEC §2.5 | gem ✓ (`:68-79`); star ✓ (`:69-71`); size badge ✓ bottom-left (`:80-82`, per UI-SPEC §2.5); ⤢ **missing**; ⟡ stage **missing** (B); imported badge n/a |
| F3 | Body: diamond + name; pack · stats; chips 3+N with confidence; `???` | P | FEATURES §1.1, DESIGN-GUIDELINES §7 | diamond bullet ✓ (`:219-225`); pack-led meta ✓ (`:36-44`); chips 3+N ✓ (`:46-50`, `:94-99`); `???` ✓ (`:92`); AI confidence bars **missing** (`GridItem` has no confidence) |
| F4 | Selection gold ring; pick checkbox; confirmed ring at rest | P | UI-SPEC §2.5 | gold ring ✓ (`:120-123`); checkbox **missing**; confirmed rest ring **missing** |
| F5 | Hover raise + type glow; model preview spins on hover | P | DESIGN-GUIDELINES §7 | hover raise + tint border ✓ (`:116-119`); vector SVG preview is **static** — no 4-view hover spin |
| F6 | List row anatomy | P | UI-SPEC §2.5, DESIGN-GUIDELINES §7 | `AssetList.svelte:29-70` — name/type/status/size/tris; no pack, no status gem (text), no stage |

### G. Inspector — FEATURES §2, UI-SPEC §13

| # | Item | Class | Spec | App evidence |
|---|---|---|---|---|
| G1 | 300px; ✕ / Esc closes | P | UI-SPEC §13 | `Inspector.svelte:191-198` width ✓; no ✕, Esc doesn't close (toolbar icon toggles) |
| G2 | Identity: diamond + name + status gem + 2px status rule | P | UI-SPEC §13.3 | `Inspector.svelte:75-84` name + text badges; no gem, no status-colour rule |
| G3 | Stat block, mono | A | UI-SPEC §13.4 | `Inspector.svelte:86-105` `dl` mono rows (Size/Tris/Dims/Duration/Source) |
| G4 | TRAITS chips + confidence + `+ trait` | M | UI-SPEC §13.5 | absent — `AssetDetail` (`api.ts:175-193`) has no traits |
| G5 | LINKED FILES panel | M | UI-SPEC §13.6 | absent — `related_assets` exists (`catalog.rs:3598`) but no command |
| G6 | COLLECTIONS toggle chips | M | UI-SPEC §13.7 | absent |
| G7 | SIMILAR IN YOUR HOARD | M | UI-SPEC §13.8 | absent |
| G8 | Provenance footer (source · pack · license) | P | UI-SPEC §13.9 | Source row ✓ (`:101-104`); license absent |
| G9 | Actions: Identify now / Stage / Import / Re-identify | P | UI-SPEC §13.10 | `Inspector.svelte:112-145` — Favorite/Trash/Restore/Remove/Delete instead (catalog lifecycle); Stage/Import B; Identify-now addable |

### H. Quick Look — FEATURES §7.3, UI-SPEC §14

| # | Item | Class | Spec | App evidence |
|---|---|---|---|---|
| H1 | Space opens; modal; full detail; ←/→ step; Esc; click-outside | P | UI-SPEC §14, §0.4 | `LibraryScreen.svelte:42-45` opens on **Enter** (Space unbound); `QuickLook.svelte:64-70` Esc/arrows ✓, click-outside ✓, full detail ✓ (`fileUrl`) |

### I. Overlays & modals — FEATURES §7–8, UI-SPEC §1/§12/§16

| # | Item | Class | Spec | App evidence |
|---|---|---|---|---|
| I1 | Onboarding overlay over the dimmed library | M | FEATURES §8, UI-SPEC §1, design README §1 | absent |
| I2 | Confirm-delete danger modal | A | DESIGN-GUIDELINES §7 | `ConfirmDelete.svelte` — names paths, remove-alternative, Cancel-focused |
| I3 | Command palette Ctrl/Cmd-K | A | UI-SPEC §0.4 | `App.svelte:94-99`, `Palette.svelte` |
| I4 | Drag-in drop overlay, gold dashed | A | DESIGN-GUIDELINES §7 | `DropZone.svelte:78-85` |
| I5 | Toasts (bottom-right, accent border, auto-dismiss) | M | FEATURES §0.4, UI-SPEC §18 | absent — only `actionError` bar (`App.svelte:164-166`) + DropZone status card |
| I6 | Staging tray | M | FEATURES §7.6, UI-SPEC §16.2 | absent (B: ENGINE-API §6) |
| I7 | Pack stats modal | M | UI-SPEC §15 | absent; `PackagesScreen.svelte` rows have **no Stats button** (spec §15 says one ships on pack rows) |
| I8 | Project switcher + Connect wizard | M | UI-SPEC §16.1/§17 | absent (B) |

### J. Screens — UI-SPEC per-screen

| # | Screen | Class | Notes |
|---|---|---|---|
| J1 | Library | P | core built (grid/list/folders/pager/empty states); gaps D1/D5/D8/D9/D10/F1/F4/F5 |
| J2 | Favorites | P | filter over library; FEATURES §4.7 wants Cinzel `★ Favorites` header + strap + mono count + diamond-☆ empty state with Browse Library →; app has plain h1 + text-only empty state |
| J3 | Identify | M | **stub** (`Screen.svelte`); UI-SPEC §3 (Appraise the Backlog, session set, Identify all, empty state) |
| J4 | Packages | P | publisher→pack, unid pill, Browse→ ✓; ◉ Stats → pack-stats **missing** (I7) |
| J5 | Unity Assets | M | **stub**; FEATURES §5 + PACK-DIRECTORY-NAV + UI-SPEC §15 |
| J6 | Collections | M | **stub**; FEATURES §4.2, UI-SPEC §6 |
| J7 | Traits | P | read-only vocabulary ✓ (namespaces, counts, teach pill, browse-to-filter); NEW TRAIT card / scope / promote / teach toggle / style picker **missing** (B8a); Mapping tab absent (core slice) |
| J8 | Stats | P | overview ✓ (tiles/size-by-type/disk/heaviest/insights); Tag Graph absent (B, §8.2) |
| J9 | Duplicates | P | groups ✓, keeper ribbon ✓, Keep all ✓, Move-to-Trash ✓; Merge tags missing (needs tag ops); exact-only (perceptual arrives with embeddings) |
| J10 | Trash | P | filter over library + Restore in inspector; UI-SPEC §7 wants table rows with inline ↩ Restore |
| J11 | Settings | M | **stub**; FEATURES §6, UI-SPEC §11, design README §5 |
| J12 | Stub-screen pattern | P | `Screen.svelte` ✓ (title + milestone + one-liner); UI-SPEC §12 wants a `← Back to Library` affordance — absent |

### K. Icons — DESIGN-GUIDELINES §5, Glyph Catalog, FEATURES §11

| # | Item | Class | Spec | App evidence |
|---|---|---|---|---|
| K1 | Sprite coverage | D | DESIGN-GUIDELINES §5 lists `ls-type-sprite`, `ls-type-font` in the sprite; `docs/design/icons/type-{font,sprite}.svg` exist | `public/icons.svg` lacks both symbols; `api.ts:308-321` `typeIcon()` has no `sprite`/`font` case → falls back to `ls-type-image` |
| K2 | Favorites glyph | P | design has no star symbol | `routes.ts:39-40` documented placeholder (`ls-status-confirmed`); swap when the design gains a star |
| K3 | `ls-unitypackage` present | A | FEATURES §11 | `public/icons.svg:52` |
| K4 | 24×24 / 1.8 stroke style | A | DESIGN-GUIDELINES §5 | every symbol matches |

### L. Motion, feedback, a11y — DESIGN-GUIDELINES §8–9

| # | Item | Class | App evidence |
|---|---|---|---|
| L1 | `prefers-reduced-motion` | P | only `DropZone.svelte:190-194` gates; no global motion policy |
| L2 | Focus ring | A | `app.css:58-62` `:focus-visible` = `--el-focus-ring` |
| L3 | Toasts | M | see I5 |
| L4 | Appraise shimmer | M | lands with Identify (UI-SPEC §3) |
| L5 | Keyboard map | P | Ctrl/Cmd-K ✓; Enter→QL (Space missing); Esc ✓; ←/→ QL ✓; Backspace folders ✓; `F` favourite **missing**; `/` search focus **missing**; Ctrl/Cmd-A **missing** (no multi-select); undo **missing** (B) |

### M. Persisted state — FEATURES §10, design README "State Management"

| # | Item | Class | Notes |
|---|---|---|---|
| M1 | Density / sort / view persisted | D | `nav.svelte.ts` is pure `$state` — nothing persists across launches (no localStorage, no config command). `AppConfig.view` (egui `ViewPrefs`) is never read/written by the desktop crate |
| M2 | SizeTiers UI | M | `AppConfig.size_tiers` exists (`config.rs:369`) with the exact invariants (128 KB gap, cap ≤ 10240 KB); no Settings UI, no command |
| M3 | `skip_onboarding` | M | `config.rs:375` exists; no UI (onboarding), no command |
| M4 | On-screen quality | M | `config.rs:372` `preview_quality` exists; no UI/command; grid ignores it |
| M5 | MCP / storage / engine flags | M | `config.rs` has `mcp_disabled_tools` (:451, allow-list engine shipped per KANBAN), `preview_cache_cap_gb`, `auto_purge_preview_cache`, `watch_sources`, `confirm_before_trash`, `auto_identify` — **no Settings UI, no config command** |

---

## Open gaps — remaining (post-remediation)

The remediation log above fixed every remediable item in this audit, and the **B9 pass
(2026-08-06, KANBAN B9a–g)** shipped the non-engine slices: Stats Tag Graph, Mapping tab,
per-source blacklist editor (now fed to the crawler via the config sync), LINKS filter +
bulk-bar tag edits, the staging tray's collect side, the Inspector tag-editing slice, and undo
for tag/collection/trash. What remains below is the genuinely engine-blocked set.

### Backend-blocked (board already tracks; not remediated here)
- **Identify engine**: ✦ Identify all, per-asset Identify now, the appraise shimmer (UI-SPEC §3)
  — the tagger/embeddings/job pump is the B7 engine build; the screen shows the backlog honestly.
- **B2b ingest path**: per-pack Extract, the §5.2 import dialog, the job tray, `+ Add source…`
  folder chooser, the DropZone's post-import flow — all ride the crawl/pump.
- **Project switcher + Connect wizard** (I8), import/export actions, and the **staging tray's
  commit-to-project half** — ENGINE-API/engine-project work. (Undo L5 and the tray's collect side
  shipped in B9.)
- **LINKS dependency *graph* harvest** — `GridQuery.links` filters the one-hop edges that exist;
  harvesting GUID relationships from `.unitypackage` contents is ingest work.
- **Mapping rule authoring** — the B9b tab lists rules and applies the engine; create/edit
  rules is a later slice over `list_rules`/`apply_mapping`.
- **Inspector collection membership read** — the B9f chips toggle but track membership locally;
  a `collection_members` read command is the next slice.

### Small follow-ups (visual/keyboard, not stubs)
- **B6 health meter** in the top bar (UI-SPEC §0.1.5) — counts already flow to `LibraryStatus`.
- **Space → Quick Look** (L5) — Enter works today; add Space per UI-SPEC §0.4.
- **Toasts** (I5) — mutating actions currently confirm via the actionError bar and DropZone card.
- **Unidentified nudge banner** (D8) and the card **status left-border** (F1) — design-level
  polish, both specified.
- **Pack stats modal** (I7) — the ◉ Stats button on pack rows.
- **Model hover-spin** on card previews (F5) and AI confidence bars (F3) — wait on the tagger.
- **Pinned Unity libraries in the sidebar LIBRARIES facet** (C6) — the `pinned` flag now
  renders on the Unity Assets screen; listing them in the facet is a small addition.
- **`preview_quality` render-path wiring** — Settings persists it; the grid still decodes at
  one resolution (needs the thumb cache to honour the quality).

### Doc follow-ups (same-change rule)
- UI-SPEC §0.2's table omits the Favorites row (FEATURES §0.3 — the arbiter — includes it, and
  the app implements it); add the row so the two specs agree.
- DESIGN-GUIDELINES §5's sprite table was updated with `ls-nav-favorites`; the Glyph Catalog's
  nav list still reflects the pre-Favorites 10-row prototype — add a Favorites ★ row.
  *(Both shipped with the B8 commit 2026-08-06.)*
