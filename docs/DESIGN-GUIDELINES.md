# Lodestash — Design Guidelines

A design system for **developers and designers** building Lodestash. **Dark-mode first**, themed with
restrained **JRPG inventory/appraisal** flavor, and tuned to present **a lot of information that stays
easy to digest**.

> 🎨 **See it live:** open [`design/style-guide.html`](design/style-guide.html) in a browser for the
> rendered palette, type, components, and icons. Icons ship as a sprite in
> [`design/icons.svg`](design/icons.svg).

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
| `--ls-type-image` | `#6FB1FF` | **Images** (textures) — sapphire |

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

- **Style:** 24×24 grid, `stroke=currentColor`, **stroke-width 1.75**, round caps/joins, minimal fill.
  A faint **gem/diamond motif** ties the set to the theme (status gems, logo star).
- **Sizes:** 16 (inline/rows), 20 (toolbar), 24 (nav), 32 (empty states).
- **Color:** inherit text color by default; type/status icons take their token color.
- **Delivery:** single sprite [`design/icons.svg`](design/icons.svg) — `<svg class="icon"><use
  href="icons.svg#ls-search"/></svg>`.

| id | Meaning | | id | Meaning |
|---|---|---|---|---|
| `ls-logo` | Brand mark (lodestone star) | | `ls-type-model` | Models (3D) |
| `ls-nav-library` | Library | | `ls-type-sound` | Sounds |
| `ls-nav-identify` | Identify mode (appraise) | | `ls-type-image` | Images |
| `ls-nav-collections` | Collections | | `ls-status-unidentified` | Unidentified `?` gem |
| `ls-nav-mapping` | Mapping library | | `ls-status-identified` | Identified ✓ gem |
| `ls-nav-packages` | Packages | | `ls-status-confirmed` | Confirmed ★ gem |
| `ls-nav-settings` | Settings | | `ls-import` | Import to engine |
| `ls-search` | Search | | `ls-download` | Download (cache) |
| `ls-search-ai` | Search with AI | | `ls-play` / `ls-pause` | Audio transport |
| `ls-search-tags` | Search by Tags | | `ls-tag-add` | Add/edit tag |
| `ls-filter` | Filters/facets | | `ls-dependency` | Dependency link |
| | | | `ls-non-content` | Non-content (tool/script) |

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

### Top bar
Logo + global search (with mode toggle, §below) + library health meter + active AI-profile chip
(shows GPU tier / model set, links to AI-MODELS-BY-GPU) + settings. Height 48px, `--ls-surface-raised`.

### Search bar with **mode toggle**
A segmented control: **⟨ Tags ⟩ ⟨ AI ⟩** (`ls-search-tags` / `ls-search-ai`).
- **Tags** mode → gold accent, placeholder "filter by tag…".
- **AI** mode → arcane-teal accent + subtle glow, placeholder "describe what you need…".
The active mode colors the input's focus ring so you always know which engine you're querying.
A **category segmented filter** (All · Models · Sounds · Images) sits beside it, each with its type
color + icon.

### Sidebar (facets)
Collapsible groups: Libraries/Packages (tree, publisher → category → pack), Status (Unid/Id/Confirmed
counts), Type, Tags (top + search), Mapping categories. Each facet row shows a **count** in dim
tabular figures. Selected facet gets a gold left-border.

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
- **Actions:** Import (with dependency count), Add to collection, Re-identify, Edit tags.

### Tag / trait chips
Pill, `--r-pill`, 11–12px. Provenance + confidence encoded:
- **Manual** — solid gold-tinted, no meter.
- **AI** — arcane-teal outline + a tiny confidence meter (filled arc/bar); low confidence = dimmer.
- **Imported** (Unity label) — neutral slate.
- Editable: hover reveals ✕ (remove) / ✓ (confirm). `ls-tag-add` to add.

### Mapping library editor
Two-pane: raw tags (left) → canonical categories tree (right), with rules between. Drag to alias;
rules list with priority. Pinned (manual) assignments get a small lock.

### Jobs / progress
Identification, ingestion, exports show as a **job tray** (bottom) with per-job progress + an aggregate
header meter. Use `--ls-arcane` for AI jobs, `--ls-gold` for ingest/IO.

### Feedback
- **Toasts** bottom-right, `--ls-overlay`, 1px accent left-border by type (success/warn/danger/info).
- **Empty states** use a 32px icon + one line + a primary action (e.g., "No Identified assets yet —
  run Identify").
- **Destructive confirms** are modals with a danger accent; never a bare toast.

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
  arrow-navigable; `/` focuses search; mode toggle is a real segmented control).
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
