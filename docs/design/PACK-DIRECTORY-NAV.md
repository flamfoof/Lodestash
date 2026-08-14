# Pack Directory Navigation — design

> **What this is.** The visual/layout design for **browsing inside a `.unitypackage` like a file
> explorer** — the *Unity Assets file view*. It is the source of truth for the pack browser's
> **target** layout; the behavioural spec lives in [`../UI-SPEC.md`](../UI-SPEC.md) and the feature
> narrative in [`FEATURES.md`](FEATURES.md) §5.4, both of which link back here. Where a pixel detail
> and [`../DESIGN-GUIDELINES.md`](../DESIGN-GUIDELINES.md) disagree, the design system wins.
>
> **Provenance.** Captured from the published mockup
> `https://claude.ai/code/artifact/357c39f4-0809-4f06-adc2-53c1d3be95b0` ("Lodestash — Package
> Directory Navigation"), stored 2026-07-28. Shown against a real pack, **Total Music Collection**.
> Single dark theme **by intent** — it matches the app's committed look. Palette + glyphs are the
> existing Lodestash design system; recreate through `docs/design/tokens.css` variables (the `theme.rs` mirror was retired at the B7 cutover), never inline hex.

Opening a `.unitypackage` drops you into its directory, **indexed in place** — files are pulled on
demand, never fully unpacked (DATA-MODEL §7 / M14). This design replaces the bolted-on horizontal
folder-chip strip with three things: a **persistent left directory tree**, a **pack-rooted
breadcrumb**, and a **folders-first contents list**, with the global **inspector** on the right.

---

## 1. Implementation status (what's built vs the target)

The pack browser ships as the **pack mode of `FoldersView.svelte`** (DESIGN-GUIDELINES §7 "Browse
views"): one three-pane view (244px tree · contents · global inspector) driven by the `pack_dir_tree`
command (`PackDirNode[]`, `api.ts`) whenever a pack filter is active. The earlier egui `SidePanel` /
`TopBottomPanel` / `pack_contents_list` build was deleted at the B7 cutover; the target layout below
is now realised in the Svelte shell. Remaining gaps (KANBAN-SVELTE 🟡): the tree has no user-collapsible
state (expansion derives from the current path) and no responsive fallback below ~900px (§2/§5/§9).

| Region | State |
|---|---|
| **A** Left directory tree | **Shipped** — the `FoldersView.svelte` tree pane (`.tree`, 244px) renders the `pack_dir_tree` result: hierarchy indented by `depth`, expand chevrons (`ls-action-expand-open/closed`), folder glyphs (`ls-folder`/`ls-folder-open`, open = current path), `direct · recursive` counts, gold selection rail, click-to-navigate. **Gap (🟡):** no user-collapsible state. |
| **B** Breadcrumb bar | **Shipped** — `FoldersView.svelte` renders the nav (`← → ↑`, `view.back`/`view.fwd` in `nav.svelte.ts`) + the pack-rooted breadcrumb (pack name as a gold `ls-unitypackage` pill, never "All libraries") + folder segments above the panes; the ordinary Folders view keeps its inline body breadcrumb. **Gap (🟡):** no responsive fallback below ~900px. |
| **Center** Contents list | **Shipped in List mode** — the folders-first `Name / Size / Items` list (`.flist`): folder rows first with `direct (recursive)`, then files with a real size + a duration/dimensions third column. Grid mode still shows thumbnail cards (the toggle). |
| **C** Counts + type glyphs | **Done** — folder rows read `direct (recursive)`; file rows carry a colour-coded type glyph (`typeIcon`) + size + duration/dimensions (the `third` column from `folder_page`). Folded into the Center list above. |
| **Right** Inspector | **Reused** — the global `Inspector.svelte` shows preview + in-archive `Path` + `Package` + stats + Add-to-project. No bespoke inspector; keep reusing it. |

---

## 2. Overall layout

An app window (titlebar → toolbar → body), body is a **three-pane grid**:

```
┌───────────────────────────────────────────────────────────────┐
│ ◈ LODESTASH                                            · · ·   │  titlebar (44px)
├───────────────────────────────────────────────────────────────┤
│ ← → ↑   [◈ Pack] / Folder / Current            [▦ Grid│☰ List] │  toolbar (46px)
├───────────────┬───────────────────────────────┬───────────────┤
│ DIRECTORY     │ Name          Size      Items  │  ┌─────────┐  │
│ ▾ 📂 Assets   │ 📁 Full Tracks  —     22 (41)   │  │ preview │  │
│  ▾ 📂 …        │ 📁 Loopable     —     14 (14)   │  └─────────┘  │
│   ▾ 📂 …       │ ♪ Abstract…mp3 4.8MB  2:41  ◄─┐ │  name/type   │
│    ▸ 📂 [sel] │ ♪ Neon…mp3     5.2MB  2:58    │ │  Path  ⧉     │
│   ▸ 📂 …       │ 🖼 cover…png    1.1MB  2048²   │ │  Package ⧉   │
│  tree (244)   │      contents (1fr)           │ │  stats 2×2   │
│               │                               │ │ ⇢ Add to proj│
└───────────────┴───────────────────────────────┴───────────────┘
   left pane              center pane                right pane (268)
```

Grid columns ≈ **244px · 1fr · 268px**; body height fills. Below ~900px the tree + inspector
collapse and the center list stands alone (responsive fallback).

---

## 3. Titlebar
Gold **◈ diamond** + Cinzel **LODESTASH** wordmark (letter-spaced, gold, uppercase), a spacer, and a
three-dot window affordance at the right. Standard app chrome; unchanged from the rest of the app.

## 4. Toolbar (nav + breadcrumb + view toggle) — **target: full-width, above all three panes**
Left to right:

- **Nav buttons** — `←` Back, `→` Forward (disabled state = dimmed, no pointer), `↑` Up one level.
  Icon buttons, 30px, hover = overlay fill.
- **Breadcrumb** — starts at the **pack name in a gold pill** (`◈` cube glyph + name, gold text on a
  gold-tinted fill with a gold border), **never "All Libraries."** Then `/` separators and folder
  segments: ancestor folders are muted links (hover → brighter); the current folder is plain `TEXT`,
  not a link. Truncates with ellipsis, never wraps.
- **View toggle** — a segmented control **`▦ Grid` / `☰ List`**; the active segment is gold-on-overlay.

## 5. Left pane — directory tree (`A`)
- Header **DIRECTORY** (mono, dim, letter-spaced, uppercase).
- Rows: `[chevron] [folder glyph] [name] [count]`.
  **Iconography (shipped 2026-08-07):** the chevron is the sprite's `ls-action-expand-open/closed`;
  the folder glyph is `ls-folder` (closed, dim) or `ls-folder-open` (open, gold — the current path),
  both in `docs/design/icons/` + the `icons.svg` sprite (Glyph Catalog §07).
  - **Chevron** `▾` expanded / `▸ ` collapsed / blank for a leaf.
  - **Folder glyph** 📂; **name** ellipsizes.
  - **Count** = mono, dim, tabular — the node's **recursive** asset count (a leaf that is the
    selected folder may read `direct · recursive`, e.g. `3 · 58`).
  - **Indent** by depth (≈16px per level).
- **Selected row**: gold-tinted fill + an **inset gold rail** on the left edge (`inset 2px 0 0 gold`);
  its count turns gold.
- Persistent + collapsible: jump anywhere without stepping folder by folder. Recess background,
  right border. Replaces the old horizontal chip strip.

## 6. Center pane — contents, **folders first** (`Center` + `C`)
- **Sticky list header**: three columns **`Name` · `Size` (right) · `Items` (right)** (mono, dim,
  uppercase), bottom border, sticks on scroll.
- **Rows** (`grid: 1fr 92px 128px`), hover = raised fill, selected = gold tint + inset gold rail:
  - **Folder row** (sorts **first**): round **gold swatch dot** + 📁 glyph + **bold** name; Size `—`;
    Items reads **`direct (recursive)`** — e.g. **`22 (41)`** = 22 files directly inside, 41 in total
    below. This `direct (recursive)` convention is the counts rule (`C`).
  - **File row**: a small square **type swatch** + a **colour-coded type glyph** + name; **Size**
    (real, e.g. `4.8 MB`, `31.4 MB`); the third column is **duration** for audio (`2:41`) or
    **dimensions** for image (`2048²`).
- **Type colour encoding** (glyph colour): **sound = aqua** (`TYPE_SOUND`), **image = sapphire**
  (`TYPE_IMAGE`), **model = amethyst** (`TYPE_MODEL`), **material = gold/amber** (`TYPE_MATERIAL`),
  sprite = verdant (`TYPE_SPRITE`).

## 7. Right pane — inspector (reuse the global inspector)
- **Preview** (≈150px): checker-backed. Per type — **audio**: waveform bars (aqua) + a **transport**
  (round gold ▶, elapsed / total time, a seek bar with gold fill); **image**: zoom/pan; **3D**: orbit
  viewport.
- **Name** (bold) + a type swatch; **type sub-line** (e.g. `Audio · MP3`, dim).
- **`Path (in package)`** field — the full in-archive path in a mono code box + a **`⧉` copy** action
  (arcane).
- **`Package`** field — the source `.unitypackage` absolute path (mono, ellipsized) + `⧉` copy. Two
  separate rows on purpose: *where the file really lives in the archive* vs *which archive on disk*.
- **Stats** — a 2×2 grid of `label / value` tiles (e.g. Duration, Size, Sample rate, Channels for
  audio; tri/vert/dims for others).
- **`⇢ Add to project (with dependencies)`** — full-width arcane button; copies the file **plus its
  resolved dependency closure** into the open Unity project (the one action that materializes payloads
  to disk; ENGINE-API §5).

---

## 8. The three annotations (from the mockup)
- **A — Directory tree, always there.** Persistent, collapsible left tree; jump anywhere without
  stepping folder by folder. Each node shows its recursive count; the selected folder carries the
  **gold rail**. Replaces the horizontal chip strip.
- **B — Rooted at the pack.** Breadcrumb starts at the **pack name** (gold pill), never "All
  Libraries." Back/forward/up sit left of it; the grid⇄list toggle sits right. **Folders always sort
  first, then files.**
- **C — Counts read `direct (recursive)`.** A folder shows **3 (58)** = three files directly inside,
  58 in total below. Files show real size + duration/dimensions. Type is a coloured glyph (aqua =
  sound, sapphire = image, amethyst = model).

## 9. Interactions
- **Navigate**: click a tree node **or** a folder row **or** a breadcrumb segment → sets
  `current_dir`; tree selection, breadcrumb, and contents stay in sync.
- **History**: `←`/`→` walk back/forward; `↑` (and Backspace) go up one level; forward disabled at
  the tip.
- **Expand/collapse**: tree chevrons; seed open down the single-child spine to the first branch.
- **Grid ⇄ List**: the toolbar toggle; List is the design shown here (folders-first rows), Grid is
  the thumbnail cards.
- **Select a file** → previews it in the inspector (audio transport / image zoom / 3D orbit) with the
  in-archive `Path` and source `Package` as the two copyable rows.
- Browsing/preview pull **one payload on demand** (a single gzip skip-scan into the cache dir);
  nothing else is unpacked, browse writes no catalog rows (Add to Library / Add to project do).

## 10. Tokens
Dark theme only, drawn from `design/tokens.css` (the executable token file the Svelte frontend
imports; mapping table in
[`README.md`](README.md) "Design Tokens"): `BG`/`SURFACE`/`SURFACE_RAISED`/`OVERLAY`/`RECESS` for the
pane stack, `BORDER`/`BORDER_STRONG` hairlines, `TEXT`/`TEXT_MUTED`/`TEXT_DIM` type, `GOLD`/
`GOLD_BRIGHT` for the pack pill + selection rail + transport, `ARCANE` for copy actions + Add-to-
project, and the `TYPE_*` element colours for the file glyphs. Mono for counts/paths/times (tabular
numerals), Cinzel for the wordmark.
