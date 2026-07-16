# Lodestash — Design Guidelines

A design system for **developers and designers** building Lodestash. **Dark-mode first**, themed with
restrained **JRPG inventory/appraisal** flavor, and tuned to present **a lot of information that stays
easy to digest**.

> 🎨 **See it live:** open [`design/Lodestash Prototype.dc.html`](design/Lodestash%20Prototype.dc.html)
> in a browser for the rendered prototype, and [`design/icons/index.html`](design/icons/index.html)
> for the icon set. Icons ship as a sprite in [`design/icons/icons.svg`](design/icons/icons.svg).

---

## 1. Design principles

1. **Information density without clutter.** This is a power tool for people with 10k–100k assets. Favor
   compact rows, tabular figures, and scannable "stat blocks" over airy marketing layouts — but use
   whitespace, dividers, and hierarchy so density never becomes noise.
2. **Dark-first.** Designed for long sessions in a dark workspace. Light surfaces are *raised* panels,
   not the page. Use **glow** (focus rings, status auras) instead of heavy drop shadows.
3. **Theme in the accents, never in the legibility.** The JRPG flavor lives in color, borders, icons,
   naming, and micro-motion — **not** in body text, contrast, or core controls. If flavor ever fights
   readability, readability wins.
4. **Status is the spine of the UI.** Every asset is somewhere on `Unidentified → Identified →
   Confirmed`. Color, icon, and border communicate it instantly (see §6).
5. **One glance = type, status, identity.** A card/row must answer "what kind, how processed, what is
   it" before you read a word.
6. **Accessible by default.** Meet WCAG AA contrast on text; **never rely on color alone** — always pair
   with an icon, label, or shape.

### The JRPG metaphor (why it fits)
Lodestash's core loop *is* a classic JRPG mechanic: you acquire **unidentified items** (`???`) and
**appraise/identify** them to reveal their traits. We lean into that:

| Lodestash concept | JRPG framing | UI expression |
|---|---|---|
| Unidentified asset | An unidentified item (`???`) | Grey gem badge with `?`, muted card |
| Identification (AI) | Appraisal / identify spell | "Identify" action + shimmer reveal |
| Asset metadata | Item **stat block** | Tabular stats (tris, size, duration…) |
| Tags / traits | Item traits/affixes | Trait chips with confidence |
| Category (Model/Sound/Image) | **Element type** | Type color + type icon |
| Status (Unid/Id/Confirmed) | **Rarity tier** | Border/aura color + gem icon |
| Collections | Party loadout / stash tabs | Pinned groups |

Keep it tasteful: think **Octopath / Persona menu polish**, not cluttered fantasy skeuomorphism.

---

## 2. Color tokens (dark mode)

CSS custom properties are the source of truth (prefix `--ls-`). All pairings below meet AA for their
intended text/background use.

### Surfaces & text
| Token | Hex | Use |
|---|---|---|
| `--ls-bg` | `#0E1117` | App background (deepest) |
| `--ls-surface` | `#161B26` | Panels, sidebar, cards |
| `--ls-surface-raised` | `#1E2533` | Raised cards, popovers, header |
| `--ls-overlay` | `#232C3D` | Modals, menus, hover fills |
| `--ls-border` | `#2C3547` | Hairline dividers, card borders |
| `--ls-border-strong` | `#3A465C` | Inputs, emphasized separators |
| `--ls-text` | `#E6E9F0` | Primary text |
| `--ls-text-muted` | `#A7B0C0` | Secondary text, labels |
| `--ls-text-dim` | `#6E7689` | Tertiary, placeholders, captions |

### Brand & action accents
| Token | Hex | Meaning |
|---|---|---|
| `--ls-gold` | `#E7B765` | **Lodestone** — primary brand/action accent ("the guiding light") |
| `--ls-gold-bright` | `#FFD98A` | Hover/active highlight, glow |
| `--ls-arcane` | `#49D6C9` | **Arcane teal** — AI / "Search with AI" / magic actions |
| `--ls-arcane-bright` | `#7BF0E5` | AI hover/glow |
| `--ls-violet` | `#A98BFF` | Special / rare emphasis |

### Element-type colors (categories)
| Token | Hex | Category |
|---|---|---|
| `--ls-type-model` | `#A98BFF` | **Models** (3D) — amethyst |
| `--ls-type-sound` | `#49C9E0` | **Sounds** (audio) — aqua |
| `--ls-type-image` | `#6FB1FF` | **Images** — sapphire |
| `--ls-type-texture` | `#6FB1FF` | **Textures** — shares sapphire with images |
| `--ls-type-material` | `#A98BFF` | **Materials** — shares amethyst with models |
| `--ls-type-sprite` | `#5BD897` | **Sprites** — verdant |
| `--ls-type-font` | `#F2966E` | **Fonts** — coral |

One row per `media_type` value (the 7-value enum, [`DATA-MODEL.md`](DATA-MODEL.md)); shared hues
keep the palette calm — the type **icon** disambiguates within a hue family (§9: never color alone).

### Status / rarity colors
| Token | Hex | Status |
|---|---|---|
| `--ls-status-unidentified` | `#8A93A6` | **Unidentified** — slate (the `???`) |
| `--ls-status-identified` | `#5BD897` | **Identified** — verdant |
| `--ls-status-confirmed` | `#E7B765` | **Confirmed** — gold seal |

### Semantic
| Token | Hex | Use |
|---|---|---|
| `--ls-success` | `#5BD897` | Success/done |
| `--ls-warning` | `#F2B84B` | Caution, "Unidentified backlog" nudges |
| `--ls-danger` | `#F2666E` | Errors, destructive |
| `--ls-info` | `#5AA9F0` | Neutral info |
| `--ls-focus` | `#FFD98A` | Focus ring (use as `0 0 0 2px` glow) |

> **Dark-mode elevation rule:** surfaces get *lighter* as they rise (`bg → surface → raised →
> overlay`). Reserve shadows for modals; use a 1px `--ls-border` + optional accent glow elsewhere.

---

## 3. Typography

| Role | Font | Notes |
|---|---|---|
| Display / titles | **Cinzel** (fantasy serif caps) | Logo, page/section titles **only**. Sparing — never body. |
| UI / body | **Inter** (fallback: Segoe UI, system-ui) | Everything functional. Default body **13px**. |
| Numeric / stats | **JetBrains Mono** (or Inter `tabular-nums`) | Stat blocks, counts, sizes, durations — aligned columns. |

### Scale (`--fs-*`)
`xs 11` · `sm 12` · **`md 13` (body)** · `lg 15` · `xl 18` · `2xl 24` · `3xl 32`
Line-height: 1.45 body, 1.2 headings. Weight: 400 body, 500 labels, 600 headings, 700 display.

> Density tip: at 13px body with tabular numerals, a card can show name + type + 4 stats + 3 trait
> chips and still breathe. Don't go below 11px for any readable text.

---

## 4. Spacing, radius, density, elevation

- **Spacing scale** (`--sp-*`, 4px base): `1=4 · 2=8 · 3=12 · 4=16 · 5=24 · 6=32 · 7=48`.
- **Radius** (`--r-*`): `sm=4 · md=6 (default) · lg=10 (cards) · pill=999 (chips)`.
- **Density:** default **compact** — list row height **32px**, grid card padding `--sp-3`, control
  height 30–34px. Offer a "Comfortable" toggle (+4px) for accessibility.
- **JRPG frame (optional, sparing):** header/hero panels may use a **beveled/clipped corner** treatment
  (`clip-path` cut top-left & bottom-right) with a 1px `--ls-gold` edge to evoke a menu frame. Never on
  dense list cards.
- **Elevation:** `0` flat (`--ls-surface`), `1` raised (`--ls-surface-raised` + 1px border), `2`
  popover (`--ls-overlay` + soft shadow `0 8px 24px rgba(0,0,0,.45)`), `focus` (2px `--ls-focus` glow).

---

## 5. Iconography

- **Style:** 24×24 grid, **stroke-width 1.8**, round caps/joins, minimal fill.
  A faint **gem/diamond motif** ties the set to the theme (status gems, logo star).
- **Sizes:** 16 (inline/rows), 20 (toolbar), 24 (nav), 32 (empty states).
- **Color:** each icon's stroke is **baked as its theme-token hex** in the SVG source (the sprite has
  no viewer context to inherit from); an icon that must follow the surrounding text color swaps the
  baked hex for `currentColor` at the point of use.
- **Delivery (current):** the SVG set is the source of truth — `docs/design/icons/` holds one file
  per icon plus the **`icons.svg` sprite** (symbol ids below) and an `index.html` preview. The app
  still renders **Unicode glyph** stand-ins (see the design bundle's **Glyph Catalog** for the
  codepoint + `theme.rs` token per glyph) until SVG rendering lands in the UI.

**In the sprite** (`docs/design/icons/icons.svg` — reference by `<use href="icons.svg#ls-…">`):

| id | Meaning | | id | Meaning |
|---|---|---|---|---|
| `ls-nav-library` | Library | | `ls-type-all` | All types |
| `ls-nav-identify` | Identify mode (appraise) | | `ls-type-model` | Models (3D) |
| `ls-nav-collections` | Collections | | `ls-type-sound` | Sounds |
| `ls-nav-packages` | Packages | | `ls-type-image` | Images |
| `ls-nav-traits` | Traits | | `ls-type-texture` | Textures |
| `ls-nav-stats` | Stats | | `ls-type-material` | Material (sphere) |
| `ls-nav-duplicates` | Duplicates | | `ls-type-sprite` | Sprites (frame strip) |
| `ls-nav-trash` | Trash | | `ls-type-font` | Fonts |
| `ls-nav-settings` | Settings | | `ls-status-unidentified` | Unidentified `?` gem |
| `ls-view-grid` / `ls-view-list` | View toggle | | `ls-status-identified` | Identified ✓ gem |
| `ls-grid-compact/cozy/large` | Grid density | | `ls-status-confirmed` | Confirmed ★ gem |
| `ls-action-filters` | Filters/facets popover | | `ls-action-fullsize` | Quick Look / full size |
| `ls-action-stage` | Staging tray / shortlist | | `ls-action-clear` | Clear (✕) |
| `ls-action-copy` | Copy | | `ls-action-regenerate` | Regenerate thumbnail |
| `ls-action-exclude` | Exclude / blacklist (⊘) | | `ls-action-expand-open/closed` | Expander chevrons |
| `ls-action-ai-sieve` | AI sieve | | | |

**Individual files only** (drawn in `docs/design/icons/` as standalone SVGs, not yet folded into
the sprite): `logo`, `nav-mapping`, `search`, `search-ai`, `search-tags`, `project`, `import`,
`download`, `play`/`pause`, `tag-add`, `dependency`, `non-content`, `color`, `variants`, `similar`.
When any of these is first referenced in code, promote it into `icons.svg` under its `ls-*` id.

---

## 6. Status system (the spine)

The single most important visual language. Every asset shows status three ways (color **+** gem icon
**+** label) so it's never color-only.

| Status | Token | Gem icon | Card treatment |
|---|---|---|---|
| **Unidentified** | `--ls-status-unidentified` | `ls-status-unidentified` (`?`) | Muted card, dashed/slate border, name shown as `???`-styled until identified |
| **Identified** | `--ls-status-identified` | `ls-status-identified` (✓) | Normal card, 1px verdant left-border |
| **Confirmed** | `--ls-status-confirmed` | `ls-status-confirmed` (★) | Gold left-border + faint gold aura on hover |

- **Identify mode** filters to Unidentified and offers batch "Identify." On completion, play a brief
  **appraise shimmer** (≤400ms gold sweep) as the card transitions to Identified — satisfying, not
  distracting; respect `prefers-reduced-motion`.
- A persistent **library health meter** (e.g., "8,420 / 10,000 Identified") lives in the header as a
  thin gold progress bar — turns the backlog into a visible quest.

---

## 7. Core components

> **Screen-by-screen functional spec:** [`UI-SPEC.md`](UI-SPEC.md) owns what each screen shows and
> does; this section owns the reusable components and their visual rules.

### Top bar
Logo + global search (with mode toggle, §below) + library health meter (`Identified n/total` over a
gold progress bar) + **project chip** (§below) + active AI-profile pill (shows GPU tier / model set,
links to AI-MODELS-BY-GPU). Height 48px, `--ls-surface-raised`. The media-type filter is its own
**tab row** beneath the bar ([`UI-SPEC.md`](UI-SPEC.md) §0.3), not inline chips.

### Search bar with **mode toggle**
A segmented control: **⟨ Tags ⟩ ⟨ AI ⟩** (`ls-search-tags` / `ls-search-ai`).
- **Tags** mode → gold accent, placeholder "filter by tag…".
- **AI** mode → arcane-teal accent + subtle glow, placeholder "describe what you need…".
The active mode colors the input's focus ring so you always know which engine you're querying.
### Sidebar (navigation + facets)
A single **210px labeled sidebar** (`--ls-surface`), three stacked groups
([`UI-SPEC.md`](UI-SPEC.md) §0.2): **navigation** (Library / Identify / Collections / Packages /
Traits / Mapping / Stats / Duplicates / Trash / Settings — glyph + label + amber warn-badges for Identify/
Duplicates/Trash counts; Library doubles as the reset-to-everything affordance), then a **STATUS**
facet group (All / Unid / Identified / Confirmed with diamond dots), then a **LIBRARIES** facet
group (one row per Library Source) with a dashed **+ Add source…**. Each facet row shows a
**count** in dim tabular figures; the active row gets a 2px gold left-border + `--ls-overlay` fill.
Unbuilt destinations render a stub screen — never hide the row. *(Supersedes the earlier icon-only
nav-rail treatment.)*

### Asset card (grid) — the "item card"
The hero component. Compact, scannable:
```
┌───────────────────────────┐  ← left border = status color
│      [ thumbnail ]   ◈?    │  ◈ = status gem (top-right)
│                            │
│ ⬡ Beaver_LowPoly      ▾    │  ⬡ = type icon (amethyst=Model)
│ Forest Pack · 1.2k tris    │  stat line (tabular)
│ ⟨animal⟩⟨lowpoly⟩⟨+3⟩      │  trait chips (+overflow)
└───────────────────────────┘
```
- Thumbnail/icon auto-generated; placeholder shimmer while pending.
- Hover: raise + faint type-colored glow; quick actions appear (Preview, Import, Tag).
- Density target: ≥ 5 columns at 1440px; virtualized grid.

### Asset list row (table)
For max density: `[type] name | package | stats (tabular cols) | status gem | tags…`. 32px rows,
zebra via `--ls-surface`/`--ls-surface-raised`, sortable columns.

### Detail / preview pane — the "stat block"
Split: preview (3D viewport / audio player+waveform / image viewer) over a **stat block**:
- **Header:** type icon + name + status gem + rarity-colored rule.
- **Stats grid** (2-col, tabular): format, dimensions/tris/verts/materials/bones (3D); resolution/
  channels/color space (image); duration/sample-rate/channels (audio); file size; source package.
- **Traits:** tag chips with confidence; AI vs manual vs imported distinguished (see chips).
- **Provenance & license:** a quiet footer line — **source pack(s)**, a **license/attribution** value
  (`ls-non-content`-muted style, never shouty), and any **"imported in <project> → <path>"** records.
- **Actions:** **Stage** (`ls-action-stage`), Import (`ls-import`, with dependency count), Add to collection,
  Re-identify, Edit tags. If the asset is already imported into the active project, the Import button
  shows an **"already imported"** state (see badge below) and asks before duplicating.

### Tag / trait chips
Pill, `--r-pill`, 11–12px. Provenance + confidence encoded:
- **Manual** — solid gold-tinted, no meter.
- **AI** — arcane-teal outline + a tiny confidence meter (filled arc/bar); low confidence = dimmer.
- **Imported** (Unity label) — neutral slate.
- Editable: hover reveals ✕ (remove) / ✓ (confirm). `ls-tag-add` to add.

### Mapping library editor
Two-pane: raw tags (left) → canonical categories tree (right), with rules between. Drag to alias;
rules list with priority. Pinned (manual) assignments get a small lock.

### Staging tray (the shortlist)
A **collect-then-import** dock (`ls-action-stage`), pinned bottom-right. Holds the **active
project's** shortlist — staging is **per-project** ([`DATA-MODEL.md`](DATA-MODEL.md) §9): switching
projects swaps trays, never mixes picks. Items you (or Claude over MCP) stage while browsing appear
in the GUI tray and the Unity panel alike. Compact thumbnail strip with a count badge, a dependency
summary line (`N picks → M files incl. materials & textures`), a **dest subfolder** field, and a
primary **Import all → <project>** (`ls-import`) action; dup-guard conflicts offer Overwrite / Skip
existing ([`UI-SPEC.md`](UI-SPEC.md) §16.2). Empty state: "Stage candidates as you browse, then
import the keepers together." Use `--ls-gold` for its accent (it's an action surface, not an AI one).

### "Already imported" badge
When an asset already has an `import_record` for the active project, mark it with a small **`ls-import`
check badge** in `--ls-success` on the card/row and an inline "in <project>" note in the stat block.
On Import, this turns the action into a confirm ("Re-import? It's already in Assets/…") — never a
silent duplicate. Pair the color with the icon (never color-only, per §9).

### Project chip
The active registered Unity project shows as a small **`ls-project` chip** in the top bar (next to the
AI-profile chip) — name + a connected/stale dot. Stale (editor closed) dims it and disables "into
project" actions, which then fall back to **Export to folder**.

### Variants, color & 2D previews
- **Variant tile** — a collapsed group renders as a *stacked* tile (`ls-variants`) with a count badge;
  click to fan out members. Reuses the card; the stack edge uses `--ls-border-strong`.
- **Color-coded tags** — chips may carry a `tag.color`; nested groups indent under a parent. Keep AA
  contrast on the label regardless of chip color.
- **Color search** — a row of dominant-color swatches in the filter sidebar; selecting one filters by
  `dominant_color` proximity. Pair each swatch with a hex label (never color-only).
- **Animated sprite preview** — play/pause + FPS + frame scrubber (mirrors the audio transport);
  **pixel-perfect** (nearest-neighbor) by default for pixel art.
- **Material sphere** — preview PBR/MaterialX map sets on a lit sphere in the shared viewport.
- **Font specimen** — alphabet + a sample string at a few sizes as preview/thumbnail.

### Jobs / progress
Identification, ingestion, exports, **and imports** show as a **job tray** (bottom) with per-job
progress + an aggregate header meter. Use `--ls-arcane` for AI jobs, `--ls-gold` for ingest/IO and
import.

### Feedback
- **Toasts** bottom-right, `--ls-overlay`, 1px accent left-border by type (success/warn/danger/info).
- **Empty states** use a 32px icon + one line + a primary action (e.g., "No Identified assets yet —
  run Identify").
- **Destructive confirms** are modals with a danger accent; never a bare toast.

### Selection & context menu
- **Multi-select:** click selects; Ctrl/Cmd-click toggles; Shift-click range; marquee drag selects;
  Ctrl/Cmd-A selects all within the current filter. Selected cards get a 2px `--ls-gold` border + a
  check overlay (top-left); a selection count shows in the top bar. Bulk actions operate on the set.
- **Right-click context menu** (`--ls-overlay`, §4 elevation 2): Open, **Quick Look** (Space),
  **Reveal in Explorer/Finder**, **Open with default app**, **Copy path**, Stage, Import…, Add to
  collection, Edit tags, Re-identify, Find similar, **Move to Trash**. Engine actions gray out (with a
  tooltip) when no project is active. The same menu serves single and multi selection.

### View / sort toolbar
A strip above the grid: a **⚑ Filters** button (opens the content-filter popover below; gold fill +
count badge when any filter is active) · a **view toggle** (grid / list / detail) · a **grid-size
selector** (Compact / Cozy / Large segmented control, grid only — the discrete-stop equivalent of a
zoom slider, driving the single persisted `thumb_size`) · a **sort** dropdown (Name · Date added ·
Date modified · Size · Type · Status) with an asc/desc caret. View, sort, and grid size **persist**
(app config) so a library reopens the way you left it.

### Content filter popover (⚑ Filters)
A dedicated multi-select **content filter**, distinct from the sidebar facet nav — opened from the ⚑
Filters button, anchored below it (never clipping the viewport edge). Three chip sections that **AND**
together (empty section = no constraint), over the current asset list, composing on top of the facet
selection and search:
- **TRAIT** — the library's tags/traits (most-frequent first) with a search field; selected chips
  arcane-teal. *(Backed by AI tags — P4; shown disabled until then.)*
- **TRIANGLES** (models only) — buckets `≤1k · 1k–3k · 3k–6k · 6k+`; selected violet. Assets without
  a tri count are excluded when the facet is active. *(Backed by 3D metadata — P1; disabled until then.)*
- **FILE SIZE** — the **S / M / L / XL** tiers with their resolved ranges; selected chip tinted by
  tier colour (S success · M sound-aqua · L warning · XL danger). Fully backed by `size_bytes`.
Header "FILTER LIBRARY" + **Clear all**. Every thumbnail carries its size-tier badge (top-right,
tier-coloured). The filter predicate is a **pure, unit-tested** function ([`TECH-STACK.md`](TECH-STACK.md) §5.1).

### Browse views (Flat · Folders) & the Library filter
Assets from many sources must never blur into one anonymous pile — "where is this from?" is answerable
at every level. Two **browse views**, toggled in the view/sort toolbar and persisted:
- **Flat** — every asset in the current filter as one virtualized grid (the default). With more than
  one source in the project, a **Library filter** ("Library: All · Synty · Human3" — the
  `import_source.name` facet, [`DATA-MODEL.md`](DATA-MODEL.md) §10) narrows the grid to one source,
  and each asset's Library Source shows in the inspector stat block.
- **Folders** — a directory browser over the sources: a **breadcrumb** (`All libraries / Synty /
  PolygonCity / …`, clickable segments; **Backspace** = up one level), **folder tiles** (📁 name +
  contained-asset count) for the subdirectories at the current level, then the assets that live
  directly in that folder. The top level lists the project's sources as tiles (single-source projects
  descend automatically). Typing in search while in Folders searches the **current subtree**; the
  media-type tabs keep filtering both tiles' counts and assets. This is how a pack-structured library
  (`Synty/PolygonCity/…`) is browsed pack-by-pack before P2 packages land; the sidebar
  Libraries/Packages tree (publisher → category → pack, §above) complements it once package metadata
  exists.

### Drag & drop
- **Drag out:** drag selected asset(s) to the OS — Explorer/Finder, a DCC, or the **Unity Editor** — as
  a native file drag of the underlying path(s). Complements `import_to_project` (drag = a manual drop;
  the panel = a *tracked* import with provenance).
- **Drag in:** drop files/folders onto the window to add a source and ingest (the "Open folder" path).
  Show a full-window drop overlay with a gold dashed border.

### Quick Look (preview peek)
**Space** on the focused/selected asset opens a centered, dismissible large-preview overlay (the
preview-pane content above, read-only) without leaving the grid; Space/Esc closes; arrow keys step to
the next asset. Mirrors OS "quick look" muscle memory.

### Trash & restore
Soft delete (`asset.deleted_at`, [`DATA-MODEL.md`](DATA-MODEL.md) §8) surfaces as a **Trash** view in
the sidebar. **Move to Trash** removes an asset from the catalog views (recoverable); **Restore**
returns it. A separate, explicit **Delete file from disk** is a danger-accent modal — rare, and it
breaks the non-destructive default, so warn clearly. Emptying Trash purges catalog rows only, never
originals unless the on-disk delete was explicitly chosen.

### Settings / Preferences
A scrollable screen (max content width ≈ 760px, section cards on `--ls-surface`, `--r-lg` radius,
letter-spaced 10px section labels) behind `ls-nav-settings` in the **sidebar** (§Sidebar; the
Library entry is the reset-to-everything affordance, §Browse views). The card-by-card expansion is
[`UI-SPEC.md`](UI-SPEC.md) §11. Groups:
- **Library** (the Sources manager below), incl. a **per-source blacklist** — folder/asset exclusions
  that skip cataloguing (never touch disk); a new field on `import_source`
  ([`DATA-MODEL.md`](DATA-MODEL.md) §1) honoured by the crawler ([`INGESTION.md`](INGESTION.md)).
- **Appearance** — density (§4), follow-OS reduced-motion, and **on-screen (grid) quality**
  (Low/Medium/High): the grid-thumbnail render size, a GPU/perf tradeoff. **Grid-only** — full-detail
  previews are reserved for full-size **Quick Look** (§below), which always renders at full detail.
- **File-size labels** — the byte thresholds behind the **S / M / L / XL** badge and the File-size
  filter: three editable boundaries + an editable cap (0 KB → 10 MB), a live preview track, and a
  reset. Boundaries keep a ≥128 KB gap and the cap stays ≥ XL + 128 KB. A pure `size_tier_of`
  ([`TECH-STACK.md`](TECH-STACK.md) §5.1) feeds the badge, the filter, and the legend. Persisted.
- **AI** — per-modality local/API provider + active GPU profile / identify tier
  ([`AI-MODELS-BY-GPU.md`](AI-MODELS-BY-GPU.md)).
- **MCP** — enable/configure the **existing** MCP server ([`MCP-AND-ENGINES.md`](MCP-AND-ENGINES.md) §2;
  stdio default, optional loopback HTTP) with a per-tool allow-list (identify / tag / search); never a
  second protocol surface, and the MCP↔REST 1:1 invariant holds.
- **Storage** — thumbnail/embedding cache + the **extracted-archive workspace** (location, size caps,
  **retention days** default 7, [`INGESTION.md`](INGESTION.md) §2, **Clear cache / Clear workspace**).
- **Engine** — default export folder, loopback port/token, live-sync, confirm-before-Trash.
- **First-run** — a **Show the first-run setup screen** toggle bound to the same persisted
  `skip_onboarding` flag as the onboarding panel's "Don't show this next time"; off = open straight
  into the library on launch.

### Sources manager
Manage the catalog's `import_source` roots ([`DATA-MODEL.md`](DATA-MODEL.md) §1): **Add folder / Asset
Store cache**, **Rescan**, **toggle Watch**, **Remove from library** (keeps files), with per-source
counts and last-scanned time, plus a recent-libraries list for quick reopen. This is the multi-library
home that the M0 startup-restore builds toward.

### Tag manager
Curate the raw tag vocabulary ([`DATA-MODEL.md`](DATA-MODEL.md) §2): **rename**, **merge** (two tags →
one, re-points `asset_tag`), **delete**, set `tag.color` / nesting (`parent_id`), with usage counts.
Tag-entry fields get **autocomplete** over existing `norm_name`. Distinct from the controlled-vocabulary
**mapping library editor** above — tags stay free-form.

### Duplicate review
Surface dedup groups (BLAKE3 exact + perceptual near-dup, [`DATA-MODEL.md`](DATA-MODEL.md) §5) as a
**review queue**: grouped cards with the keeper highlighted and per-group **keep / merge tags / move to
trash** — **never auto-delete**. Reachable from the sidebar with a count.

### Undo / redo
Catalog mutations — tag add/remove/confirm, collection membership, mapping apply, move-to-trash — are
**undoable** (Ctrl/Cmd-Z / Ctrl/Cmd-Shift-Z) with a brief toast ("Removed 3 tags · Undo"). Ingest/scan
and engine imports are **not** undoable (jobs / external side effects) — confirm those up front instead.

---

## 8. Motion

Subtle, fast, purposeful. Durations: micro 120ms, standard 200ms, reveal 360–400ms.
- **Appraise shimmer** on Unidentified→Identified (gold sweep).
- Hover raise 120ms; popovers 160ms fade+rise.
- Progress meters animate smoothly; never spin gratuitously.
- **Always honor `prefers-reduced-motion`** (drop shimmer/transforms, keep instant state changes).

---

## 9. Accessibility

- **Contrast:** body text ≥ 4.5:1, large/secondary ≥ 3:1 against its surface. Verified for the tokens
  above on `--ls-surface`/`--ls-surface-raised`.
- **Never color-only:** status, type, provenance always also carry an icon/label/shape.
- **Focus:** visible 2px `--ls-focus` glow on every interactive element; full keyboard nav (grid is
  arrow-navigable; **Space** = Quick Look, **Enter** = open, **Del** = move to Trash; `/` focuses
  search; Ctrl/Cmd-click + Shift-click + Ctrl/Cmd-A select; mode toggle is a real segmented control).
- **Hit targets:** ≥ 28px in compact, ≥ 32px comfortable.
- **Motion/again:** reduced-motion respected; no essential info conveyed by animation alone.

---

## 10. Developer handoff

- **Tokens are CSS custom properties** (`--ls-*`, `--fs-*`, `--sp-*`, `--r-*`) — mirror them as a
  single source (e.g. a `tokens.css` / a Rust `theme.rs` constants module for the egui build). The
  style guide HTML defines the canonical values in `:root`.
- **egui mapping:** translate tokens to an egui `Visuals`/`Style` (dark base; `widgets` fills =
  surface/raised/overlay; `selection`/`hyperlink` = gold; accent overrides per component). Keep the
  named constants 1:1 with the CSS tokens so design and app never drift.
- **Icons:** consume `design/icons.svg` by symbol id; size via CSS `width/height`, color via
  `color`/`currentColor`.
- **Naming:** components and states use the status vocabulary from this doc (Unidentified/Identified/
  Confirmed) and the icon ids — so design specs, code, and tickets share one language.

---

### Quick reference card
- **Dark-first**, glow over shadow, surfaces lighten as they rise.
- **Gold = brand/action, Teal = AI, type colors = category, status colors = rarity.**
- **Every asset = type icon + status gem + stat block.** One glance tells you kind/status/identity.
- **Density is a feature** — tabular numerals, 13px body, 32px rows — but always paired with hierarchy.
- **Theme in accents, never legibility.** Honor reduced-motion and AA contrast.
