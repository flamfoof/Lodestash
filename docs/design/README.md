# Handoff: Settings screen + Library filtering, grid controls, and MCP

## Overview

This package specifies the interaction and visual design for a batch of Lodestash UI work that turns
the **Settings** screen from a stub into a functional screen, and adds **library-level filtering and
view controls** to the asset browser. Concretely:

1. **First-run onboarding renders over the live library** (not on a blank page).
2. **Library toolbar controls** — a dedicated Filters popover, a grid-size selector, and the existing
   view/sort toolbar.
3. **A dedicated Trait / Triangles / File-size filter** (distinct from the sidebar facet nav).
4. **Library nav-glyph reset** — clicking the top-level *Library* nav returns to All libraries and
   clears active filters; switching between *source* libraries preserves the current filter.
5. **Settings screen** with these groups: **Sources** (with per-source blacklist), **Identify
   profile / on-screen quality**, **File-size labels**, **MCP**, **Storage caps**, **Engine bridge**.
6. **On-screen preview quality** that only affects the grid — the **full-detail preview is reserved
   for full-size Quick Look**.

## About the design files

The file in this bundle — `Lodestash Prototype.dc.html` — is a **design reference built in HTML/JS**.
It is a prototype that demonstrates the intended layout, states, and micro-interactions. **It is not
production code and must not be shipped or ported line-for-line.**

The Lodestash app is a **Rust + `egui`/`eframe` desktop application** (`crates/lodestash-ui`). The
task is to **recreate these designs in that existing environment**, using its established patterns:

- Colours come **only** from `crates/lodestash-ui/src/theme.rs` constants — never inline hex. The
  prototype's hex values were taken from those tokens; the mapping table below is exact.
- Persisted UI state goes through `crates/lodestash-ui/src/config.rs` (`AppConfig` → `config.json`),
  following the existing `ViewPrefs` pattern.
- **Per `CLAUDE.md`, the `docs/` files are the source of truth.** Where this design adds or changes
  behaviour, the governing doc must be updated in the same change (noted per-feature below). This
  README documents intent; it does not override the specs.

## Fidelity

**High-fidelity.** Colours, type roles, spacing, status vocabulary, and interaction states are final
and already conform to `DESIGN-GUIDELINES.md`. Recreate the structure and behaviour faithfully, but
express them through egui widgets and `theme.rs` tokens — do **not** try to reproduce HTML/CSS
specifics (drop shadows, gradients) that the design system says to avoid. When a pixel detail and the
design system disagree, **the design system (`DESIGN-GUIDELINES.md`) wins.**

---

## Screens / Views

### 1. First-run onboarding over the live library

- **Purpose:** the "Point it at your hoard" first-run source picker.
- **Change:** the onboarding is a **centred modal overlay on top of a fully-rendered library**, not a
  standalone page. The library (top bar, nav rail, asset grid) paints behind it, dimmed.
- **Layout:** full-window scrim over the library; modal centred. Scrim ≈ `rgba(14,17,23,.6)` (i.e.
  `BG` at 60%) with a subtle blur; in egui use an `Area`/`Window` over the central panel with a
  painted dimming rect (avoid heavy blur — a flat dim is fine and cheaper).
- **Behaviour:** dismissing via any action drops straight into the live Library. Governed by
  `DESIGN-GUIDELINES.md` §7 "Drag & drop / Open folder" empty-state flow; keep the gold dashed drop
  affordance.
- **"Don't show this next time" (new):** a checkbox on step 1 sets a persisted `skip_onboarding` flag.
  It is the same setting exposed in Settings → Engine (view 5.7) — the two controls are two faces of
  one boolean and must stay in sync. On launch the real app **reads this flag and, when set, opens
  straight to the library, skipping the panel entirely** (the prototype always shows onboarding for
  demo purposes, so there the checkbox only reflects/persists the preference). Persist in
  `config.json` (`AppConfig`).

### 2. Library toolbar (the strip above the grid)

Extends the existing view/sort toolbar (`DESIGN-GUIDELINES.md` §7 "View / sort toolbar"). Left-to-right:

- **⚑ Filters button** — toggles the filter popover (see view 3). Shows an active state (gold fill +
  border, token `GOLD` / `GOLD.linear_multiply(0.25)`) and a **count badge** (gold pill, dark text)
  when any filter is active.
- **Grid-size selector** — a 3-way segmented control (Compact / Cozy / Large) shown only in grid view.
  Maps to `ViewPrefs.thumb_size` (existing field). Suggested stops: Compact ≈ 96px, Cozy ≈ 128px
  (current default), Large ≈ 176px card min-width. This is the discrete-button equivalent of the
  existing zoom slider — keep **one** source of truth (`thumb_size`) so both agree; either replace the
  slider with these three stops or drive both from `thumb_size`. **Persist** it (already persisted).
- **View toggle** (grid / list / detail) and **sort dropdown** — unchanged, already in the toolbar.

### 3. Trait / Triangles / File-size filter popover

- **Purpose:** a dedicated, multi-select **content filter** — distinct from the left sidebar's
  Libraries/Status/Type facet tree.
- **Trigger:** the ⚑ Filters button; opens a popover anchored under the button (egui `popup_below_widget`
  / `Area`). Closing: click-away or toggle. **Anchor it so it never clips off the right edge** of the
  window.
- **Sections (each a chip row, multi-select):**
  - **TRAIT** — chips of the library's tags/traits, most-frequent first, **with a search field** to
    the right of the "TRAIT" label to filter the chip list (placeholder "search traits…"). Selected
    chip = arcane-teal (`ARCANE` outline + `ARCANE_BRIGHT` text). Note: the label is **"Trait"**, not
    "Tag" — matches the trait vocabulary in `DESIGN-GUIDELINES.md` §7 "Tag / trait chips".
  - **TRIANGLES** (models only) — buckets `≤ 1k`, `1k–3k`, `3k–6k`, `6k +`. Selected = violet
    (`TYPE_MODEL`). Read tri-count from the model stat block; assets without a tri count are excluded
    from this facet.
  - **FILE SIZE** — the **S / M / L / XL** tiers, whose thresholds come from the File-size labels
    setting (view 5.3). Each chip shows its tier letter + range. Selected chip tinted by tier colour
    (S `SUCCESS`, M `TYPE_SOUND`, L `WARNING`, XL `DANGER`).
- **Header:** "FILTER LIBRARY" + a **Clear all** action (danger on hover) that empties all three
  filter sets.
- **Filtering logic:** an asset passes if it matches the **selected traits (any)** AND **selected
  tri-buckets (any)** AND **selected size tiers (any)** — empty section = no constraint. This is a
  pure predicate over the current asset list; keep it a testable function in `lodestash-ui` per
  `TECH-STACK.md` §5.1 ("pure helpers only"). It composes **on top of** the sidebar facet selection
  and search — it does not replace them.
- **Spec touch:** this is new UI behaviour → reflect it in `DESIGN-GUIDELINES.md` §7 (add a "Content
  filter popover" note distinguishing it from the facet sidebar).

### 4. Library nav-glyph reset behaviour

- **Top-level Library nav glyph** (`ls-nav-library`): clicking it returns to the Library screen and
  **resets to All libraries and clears all active filters** — library filter, status/category facet,
  search query, AND the trait/tris/size sets from view 3. This fixes the reported bug where selecting
  a source (e.g. "Synty") and then clicking the Library glyph did **not** return to the unfiltered
  full view.
- **Source-library switching** (choosing a specific source in the Library facet, e.g. Synty → Human3):
  **must NOT clear the current filter** — only the top-level Library glyph is the "reset to
  everything" affordance.
- **Implementation note:** in `app.rs`, the Library nav action should reset the library/category/
  status/query state **and** the new filter sets; the source-facet selection path must leave the
  filter sets untouched. Keep these two code paths clearly separate.

### 5. Settings screen (`ls-nav-settings`)

A scrollable screen, max content width ≈ 760px, section cards on `SURFACE` with `BORDER`, 10px radius
(`--r-lg`). Section-label style: 10px, letter-spaced, `TEXT_DIM`, 600 weight. Governed by
`DESIGN-GUIDELINES.md` §7 "Settings / Preferences" and "Sources manager" — align group names to that
doc (Library / Appearance / AI / Storage / Engine) as you implement; the prototype's grouping is
below and maps onto those.

#### 5.1 Sources (maps to "Library" / Sources manager)

- A list of watched sources; each row: status dot (source colour), name, mono path, meta
  ("N packs · auto-detected" / "watched · …"), an **exclusions** control, and an enable/disable
  toggle.
- **Add source** enables a paused source or triggers an add + rescan.
- **Per-source blacklist (new):** each source row expands to a **Blacklist** panel — a list of
  exclusion chips (folder or asset), an input, and **⊘ folder** / **⊘ asset** buttons to add the typed
  value as that kind of exclusion. Folder exclusions tinted `WARNING`, asset exclusions `DANGER`; each
  chip has an ✕ to remove. Empty state: "Nothing excluded — everything under this source is indexed."
  - **Semantics:** a blacklist entry means "skip this folder/asset under this source when indexing."
    Each source owns its **own** independent list.
  - **Data/spec:** persist per `import_source`. This is a **new field on the source model** → update
    `DATA-MODEL.md` §1 (import_source) and `INGESTION.md` (crawl must honour per-source excludes) in
    the same change. Non-destructive rule still holds — excluding only affects cataloguing, never
    files on disk.

#### 5.2 Identify profile / on-screen quality

- **Identify profile** — 3 tiles: Integrated / Balanced / Dedicated GPU. Selecting one sets the
  identify model set **and** the preview render quality together. This is the GPU-tier / model-set
  chooser from `AI-MODELS-BY-GPU.md`; wire it to the AI settings, not to a new concept.
- **On-screen quality (new, grid-only)** — Low / Medium / High. Controls **how sharply the grid
  thumbnails render** (thumbnail decode/render resolution), a GPU/perf tradeoff. Maps onto the
  thumbnail cache/render path.
  - **Critical rule:** on-screen quality affects **only the grid**. The **full-detail preview is
    reserved for full-size Quick Look** (`DESIGN-GUIDELINES.md` §7 "Quick Look"): Space on a selected
    asset always renders at full detail regardless of the grid quality setting. The grid never renders
    full-detail previews.
  - **Auto-identify toggle** — run identify automatically when a watched source drops in a new asset.

#### 5.3 File-size labels (new)

- **Purpose:** let the user define the byte thresholds behind the **S / M / L / XL** badge shown on
  every thumbnail and used by the File-size filter (view 3).
- **Control:** a single horizontal track with three draggable handles = the S|M, M|L, L|XL boundaries.
  Segments are tinted by tier colour; a legend below lists each tier's resolved range.
- **Range & editing (updated requirement):** the track runs **0 KB → 10 MB** (max cap **10 MB**, i.e.
  10240 KB — down from the earlier 50 MB). Each boundary must **also be an editable numeric field**
  (type an exact MB/KB value), not drag-only. In egui use a `DragValue`/text field per handle beside
  the track.
- **Overlap rule:** boundaries keep a **128 KB minimum gap** — dragging or typing a smaller category
  up pushes each larger category to at least 128 KB above it, so ranges never overlap. Clamp all
  values to `[128 KB, 10 MB]`.
- **Reset defaults** action restores the default thresholds. Suggested defaults within the 0–10 MB
  range: S < 1 MB, M < 4 MB, L < 8 MB, XL ≥ 8 MB.
- **Editable cap (new):** the track's upper bound ("cap __ MB", default 10 MB) is **itself an editable
  numeric field**, not fixed. Changing it rescales the whole track and every S/M/L/XL segment. The cap
  is **clamped so it can never be smaller than the current XL threshold** (enforced as `XL + 128 KB`
  minimum); the XL range label and all segment fill percentages follow the live cap.
- **Persist** these in `config.json` (new `ViewPrefs`-adjacent struct, e.g.
  `SizeTiers { m, l, xl }` + `cap` in KB).
- **Keep it a pure function:** `size_tier_of(bytes, tiers) -> S|M|L|XL` in `lodestash-ui`, unit-tested
  (`TECH-STACK.md` §5.1). The badge, the filter, and the label legend all call it.

#### 5.4 MCP (new)

- **Purpose:** expose Lodestash to the user's own agents over MCP so a client can **identify assets and
  tag traits** directly in their library.
- **IMPORTANT — do not invent a new protocol surface.** Lodestash **already** has an MCP plane
  (`rmcp`, stdio by default + optional Streamable HTTP) specified in `MCP-AND-ENGINES.md` §2, and the
  **MCP ↔ REST 1:1 invariant** in `CLAUDE.md` must hold. This Settings section is the **UI to
  enable/configure that existing server**, not a second one. Map controls to the real tools:
  - "identify_asset" → the existing identify tool/endpoint.
  - "tag_trait" → the existing tag mutation tool/endpoint.
  - "search_library" (read-only) → `search_assets` / `get_asset`.
  Any capability toggle here must correspond to a real MCP tool; if a per-tool allow-list doesn't exist
  in the spec yet, add it to `MCP-AND-ENGINES.md` **and** the REST side (1:1) before shipping the
  toggles.
- **Controls (prototype):** Enable MCP server toggle; when on → **Endpoint** (copyable), **Access
  token** (regenerate), and **Allowed actions** toggles for the three tools above. Note the prototype
  shows an HTTP endpoint on a loopback port; the spec's **default transport is stdio** — present stdio
  registration (the `claude mcp add …` command from §2) as the primary path and treat a loopback HTTP
  endpoint as the optional Streamable-HTTP mode (same loopback + token + origin rules). Follow
  `ENGINE-API.md` for the token handshake; token stays local.
- Status line: listening / offline, with the loopback address when HTTP mode is on.

#### 5.5 Storage caps

- **Preview cache limit** slider + a live usage bar that scales with the current on-screen quality
  (higher quality = larger per-asset cache) and turns `WARNING` near / `DANGER` over the cap.
- **Auto-purge over cap** toggle — drop least-recently-viewed previews when over the limit.
- Maps to `DESIGN-GUIDELINES.md` §7 "Storage" (thumbnail/embedding cache + extracted-archive
  workspace, retention days).

#### 5.6 Engine bridge

- **Live-sync toggle** — shows the active project's engine and import destination; staged assets sync
  as you stage them. Maps to the Unity loopback bridge in `ENGINE-API.md`.
- **Confirm before moving to Trash** toggle — pairs with the soft-delete / Trash flow
  (`DESIGN-GUIDELINES.md` §7 "Trash & restore"; files always stay on disk).

#### 5.7 First-run setup toggle

- **Show the first-run setup screen** toggle — the Settings-side control for the onboarding
  `skip_onboarding` flag (view 1). Toggling it here and checking "Don't show this next time" in the
  onboarding panel drive the **same** persisted boolean (inverse of each other); keep them bound to
  one source of truth. Off = the app opens straight into the library on launch.

---

## Interactions & Behaviour

- **Toggles / segmented controls:** instant state change; honour `prefers-reduced-motion`
  (`DESIGN-GUIDELINES.md` §8). No gratuitous animation.
- **Filter popover:** open/close is a 160ms fade+rise per §8; must not clip the viewport edge.
- **Size-label handles:** dragging is live; the 128 KB-gap push happens continuously; typed values
  clamp on commit.
- **Toasts** confirm mutating actions (source paused/watched, profile changed, quality changed,
  exclusion added, MCP token regenerated) — bottom-right, 1px accent left-border by type
  (`DESIGN-GUIDELINES.md` §7 "Feedback").
- **On-screen quality vs Quick Look:** re-render grid thumbnails at the chosen quality; Quick Look
  (Space) always renders full detail.

## State Management

New/changed persisted state (via `config.rs` `AppConfig`, mirroring `ViewPrefs`):

- `thumb_size` grid stop (existing — drive the new grid-size selector from it).
- `SizeTiers { m_kb, l_kb, xl_kb }` + `cap_kb` — file-size label thresholds and the editable upper
  cap; `cap_kb ≥ xl_kb + 128` (new).
- `skip_onboarding: bool` — suppress the first-run setup panel on launch (new).
- On-screen preview quality (`Low|Medium|High`) (new).
- Per-source `blacklist: Vec<Exclude { kind: Folder|Asset, value: String }>` on the source/import_source
  model (new — see `DATA-MODEL.md` §1).
- MCP: enabled flag, transport/port, token, per-tool allow-list (new — mind the MCP↔REST 1:1 rule).
- Auto-identify, auto-purge, cache cap, engine-bridge, confirm-trash flags.

Transient (in-app, not persisted): active filter sets (`trait[]`, `tri_bucket[]`, `size_tier[]`),
trait-search string, filter popover open state, expanded-source id.

Keep the derived predicates (`size_tier_of`, the filter pass function, grid column math) as **pure,
unit-tested functions** in `lodestash-ui` — the render loop stays thin (`TECH-STACK.md` §5.1).

## Design Tokens

Use `crates/lodestash-ui/src/theme.rs` constants — **do not inline hex.** Prototype → token mapping:

| Prototype hex | `theme.rs` const | Use in this work |
|---|---|---|
| `#0E1117` | `BG` | app bg, onboarding scrim base, inset wells |
| `#161B26` | `SURFACE` | section cards, sidebar |
| `#1E2533` | `SURFACE_RAISED` | popover, header |
| `#232C3D` | `OVERLAY` | segmented-control track, hover fills, menus |
| `#2C3547` | `BORDER` | card/hairline borders |
| `#3A465C` | `BORDER_STRONG` | inputs, chip outlines (unselected) |
| `#E6E9F0` | `TEXT` | primary text |
| `#A7B0C0` | `TEXT_MUTED` | secondary text, meta |
| `#6E7689` | `TEXT_DIM` | labels, captions, placeholders |
| `#E7B765` / `#FFD98A` | `GOLD` / `GOLD_BRIGHT` | brand/action: Filters active, grid-size active, size-label gold |
| `#49D6C9` / `#7BF0E5` | `ARCANE` / `ARCANE_BRIGHT` | AI/MCP accents, selected Trait chips |
| `#A98BFF` | `TYPE_MODEL` | selected Triangles chips |
| `#5BD897` | `SUCCESS` / `STATUS_IDENTIFIED` | size tier **S**, success |
| `#49C9E0` | `TYPE_SOUND` | size tier **M** |
| `#F2B84B` | `WARNING` / `TYPE_MATERIAL` | size tier **L**, folder-exclusion, near-cap, caution |
| `#F2666E` | `DANGER` | size tier **XL**, asset-exclusion, over-cap, destructive |

Spacing / radius / type: use the existing egui `Style` set in `theme::apply` and the scales in
`DESIGN-GUIDELINES.md` §3–§4 (13px body, 32px rows, radius md=6 default / lg=10 cards, 4px spacing base).

## Assets / Icons

Icons come from `docs/design/icons.svg` by symbol id (`DESIGN-GUIDELINES.md` §5). Relevant ids:
`ls-nav-settings`, `ls-nav-library`, `ls-filter` (Filters button), `ls-search`/`ls-search-tags`,
`ls-project`, `ls-import`, `ls-stage`, the status gems, and the type icons. No new bitmap assets are
introduced by this work.

## Files

- **`Lodestash Prototype.dc.html`** (in this bundle) — the interactive design reference. Open it in a
  browser; navigate to Settings via the Settings nav glyph, and use the ⚑ Filters button in the
  library toolbar. Treat it as the behavioural spec, not code to copy.
- **Target code to modify:** `crates/lodestash-ui/src/app.rs` (screens, toolbar, filter predicate,
  nav reset), `crates/lodestash-ui/src/config.rs` (persisted settings), `crates/lodestash-ui/src/theme.rs`
  (tokens only — reuse, don't add unless the design system gains a token).
- **Governing docs to conform to / update** (per `CLAUDE.md`): `DESIGN-GUIDELINES.md` (§7 Settings,
  Sources, filters, Quick Look, view toolbar), `DATA-MODEL.md` §1 (per-source blacklist),
  `INGESTION.md` (honour excludes on crawl), `AI-MODELS-BY-GPU.md` (identify profile / quality),
  `MCP-AND-ENGINES.md` §2 + `ENGINE-API.md` §2 (MCP enable/allow-list, keeping MCP↔REST 1:1),
  `TECH-STACK.md` §5.1 (pure-function tests). Add/track a card in `docs/KANBAN.md`.
