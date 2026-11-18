# UI Spec — Screens & Functionality

> Last reviewed: 2026-08-11 (check-docs verified every citation resolves)

The **screen-by-screen functional spec** for the Lodestash desktop app, derived from the interactive
prototype (`design/Lodestash Prototype.dc.html`) and **normalized against the owning design docs**.
Where the prototype and an owning doc disagreed, the resolution is stated inline as **⚖ Resolution**
and collected in §20. Visual language (tokens, type, icons, motion) stays owned by
[`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md); data shapes by [`DATA-MODEL.md`](DATA-MODEL.md);
search behavior by [`IDENTIFICATION-AND-SEARCH.md`](IDENTIFICATION-AND-SEARCH.md); the agent/engine
surface by [`MCP-AND-ENGINES.md`](MCP-AND-ENGINES.md) / [`ENGINE-API.md`](ENGINE-API.md); ingest by
[`INGESTION.md`](INGESTION.md); AI by [`AI-PIPELINE.md`](AI-PIPELINE.md). This doc owns **what each
screen shows and does**.

**Proposed MCP analysis controls (2026-09-10):**
[MCP-ASSET-ANALYSIS](MCP-ASSET-ANALYSIS.md) adds a focused workflow to the existing MCP settings,
approval and inspector surfaces. Show configured producer/destination and per-modality evidence
consent; expose actual tool/connection/writer availability separately. A batch-tag approval shows
selected asset IDs/names, AI producer, tag changes and optional description replacements before any
annotation write. Approval grants that operation, not human-confirmed status. Inspector readback must
show stored AI provenance even when confidence is null. No shell redesign is planned; exact wire
fields and approval lifecycle belong to ENGINE-API §8. These controls are not yet implemented.

The clarified workflow also needs vocabulary reads, tag accept/reject review and approved new
traits/categories (ENGINE-API §8.5). Reuse Vocabulary §9's distinct Traits/Mapping views. Show the
canonical match, usage guidance, evidence limitations and reason for each review decision; protected
manual/imported tags are flagged for human action. Vocabulary proposals show scope, parent taxonomy
and any future teaching effect separately from asset assignments. Creating a category never silently
remaps the library. Present existing tags, identification output and final reviewed assignments as
distinct stages; all remain AI-authored unless a human authored the underlying assignment.

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
2. *(removed — the search-mode toggle. ADR-0007. The slot is kept so the §0.1.n citations in
   ALIGNMENT-AUDIT and the boards keep pointing at the controls they were written about.)*
3. **Search box** (max-width 460px) — **one box, no mode toggle** (ADR-0007). Typing runs the live
   substring filter over name + pack + tags (IDENTIFICATION-AND-SEARCH §5A; grows into FTS5/BM25).
   Placeholder *"Search your hoard"*; gold border + focus glow.
   - **AI escalation** — a row pinned under the filtered results reads `✦ Ask AI for »query« —
     searches meaning, not names` with an `⏎` affordance; Enter or a click runs the semantic search
     (§5B), with the animated `✦ sieving…` pulse while it runs. It is present the whole time, so the
     AI is never a mode you had to predict.
   - **AI results** replace the grid as a **ranked list** with per-card `✦ NN%` match badges; the box
     takes the **arcane** border and the result header (§2.2) reads `✦ N ranked matches` with
     `← back to name filter`. Unidentified assets are never returned (DATA-MODEL §8); an empty result
     falls to the §2.6 empty state, which offers the escalation as its primary action.
   - **AI never runs on a keystroke.** Clearing the query drops back to the unfiltered grid and
     resets the result mode. The mode is a consequence of what you did — legible in the border and
     the header — not a switch you set first.
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

1. **Navigation** — 10 destinations, each a 32px row: 24px glyph + label + optional warn-badge
   (amber pill, mono count). Active row: `--ls-overlay` fill + 2px gold left-border.

   | Destination | Icon (`ls-*`) | Badge |
   |---|---|---|
   | Library | `nav-library` | — |
   | Favorites | `nav-favorites` | — |
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

Facet groups stay expanded on **every** screen — owner call 2026-08-06: the sidebar's
navigation must not vanish when a screen other than Library/Identify is active (they are Library
filters, and every facet click still navigates to Library). Sidebar scrolls as one column.
**Narrowed by ADR-0009** to *"on every screen, at every width where they fit"*: below 1120px the
whole sidebar becomes a 48px icon rail and both facet groups move into its flyout — one click away,
on every screen, for everyone. They are still never hidden by *screen*. DESIGN-GUIDELINES §4 owns
the ladder.

An **unavailable** Library Source (§11.1) renders its diamond as a red `✕` and dims the row; it
still selects.

### 0.3 Type tab row

Under the top bar, on Library and Identify: **five coarse category tabs** — `All · Models ·
Materials · Sounds · Images` (FEATURES.md §0.2) — each showing its count and colored by its bucket
token when active (2px underline). `All` never hides; a bucket with a zero count hides. The tabs are a **presentation
grouping over the unchanged 7-value `media_type` enum** (the enum stays
`model_3d | texture | audio | image | sprite | material | font` — a data-model invariant); finer
type distinctions live in the ⚑ Filters popover, not the top bar. Bucket → `media_type`:

| Tab | `media_type`(s) |
|---|---|
| Models | `model_3d` |
| Materials | `material` |
| Sounds | `audio` |
| Images | `image`, `texture`, `sprite`, `font` |

Materials were originally folded into Models; they became their own tab once the dependency graph
made them a thing you reach *for* (a material is what carries an export's textures — DATA-MODEL
§10), not just a by-product of a mesh.

*(The earlier second-row Library tabs are gone — the sidebar LIBRARIES facet group (§0.2) absorbed
the per-source filter.)*

**Favourites** (DATA-MODEL §12) — a **★ Favorites toolbar toggle** left of ⚑ Filters, with a gold
count pill when any exist; *not* a section inside the Filters popover, because it is a one-click view
switch rather than a facet to combine. Cards carry a `☆`/`★` tile top-right of the thumbnail (left of
the ⤢ affordance, on a `BG`-at-75% backdrop); list rows carry an outlined pill. `F` toggles the pin on
the selection (or the lead card), toasting *"★ Added to Favorites"* / *"Removed from Favorites"*.
A **Favorites screen** sits second in the nav (§0.2), header `★ Favorites` + starred count, with the
"Nothing starred yet → Browse Library" empty state.

### 0.4 Keyboard map (global)

| Key | Context | Action |
|---|---|---|
| `Space` | asset selected | toggle Quick Look |
| `Esc` | Quick Look open → else selection → else | close QL / clear selection / stop audio |
| `← →` | Quick Look open | step prev/next within the visible set |
| `Backspace` | Folders view | up one level |
| `Ctrl/Cmd-A` | grid | pick every card on the **current page** |
| `← → ↑ ↓` | grid | move the lead selection (↑↓ by one row); focus follows it |
| `Shift` + arrow / click | grid | extend the **pick set** (what the bulk bar acts on) from the lead |
| `Home` / `End` | grid | first / last of the visible page |
| `Ctrl/Cmd-Z / Shift-Z` | catalog mutations | undo / redo (DESIGN-GUIDELINES §7) |
| `Ctrl/Cmd-K` | anywhere | open the command palette: asset search + navigation (ROADMAP P7) |
| `/` | anywhere | focus the search box |
| `Delete` | asset selected | move it to Trash (through the §11.7 confirm gate) |

Keys never fire while an input/textarea has focus — **except `Ctrl/Cmd-K`**, since the search
box is where people press it.

Three of these rows were reconciled on 2026-08-26, after an audit found the two docs disagreeing
about what the keyboard does and the app implementing neither answer:

- **`/` and `Delete`** were only ever in DESIGN-GUIDELINES §9's keyboard-nav paragraph, never in
  this table — and this table is what anyone implementing the keyboard map reads. Both were
  missing from the app. They are here now, and `Delete` is deliberately **lead-only**: the bulk
  bar offers no trash action, and a key that can bin fifty assets at once should not arrive
  without a bar that says so.
- **`Ctrl/Cmd-A` is scoped to the current page**, not the whole filter, which is a narrowing of
  what this row used to claim. Pagination (§2) arrived after that line was written: a filter can
  hold eight thousand assets, one page holds fifty to a hundred, and the pick set this fills is
  what the bulk bar writes tags to. One keystroke arming a write over eight thousand rows the
  user cannot see is a footgun, not a shortcut. The page is already what `← →` in Quick Look
  treats as "the visible set".
- **`F` is a toggle**, per §0.3 — it set the star unconditionally until the same audit, so it
  could only ever add, while the card's ☆/★ tile and the context menu both toggled.

### 0.5 Status bar

A 24px strip under everything: asset count · vector-preview count · source count · `reload` · the
catalog path (right). It is also where **another writer's change** surfaces
([ADR-0016](decisions/ADR-0016-coalesced-change-publication.md)). When an agent (through the engine
API) or the background lane (identification, previews, watch rescans, a finished unpack) changes the
catalog, the grid does **not** re-read under the user; the bar shows **Library updated · Refresh**
(gold, inside a polite live region, 28px hit target). Refresh re-reads the catalog snapshot, the
counts and every open screen's query, and clears only what it had seen when it started — a change
landing mid-read stays pending. The user's own writes refresh where they were made and are never
announced back. `reload` is the same action, always offered.

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
- **On completion**, if new assets exist: an **arcane** callout — *"✦ Appraisal has started. All N
  assets begin Unidentified. They are being appraised in the background, whatever you are looking at
  first — search gets better as it goes, and you do not have to wait here."* with **Open library →**
  (gold primary) and **Watch progress** (→ Identify §3).
  *(ADR-0008: the old amber copy — "AI search can't see them until you run identification" — and its
  **✦ Run identification now** / **Skip to Library** fork are gone. The fork asked a new user to
  choose between two things they cannot yet evaluate; under ADR-0008 there is nothing to choose. The
  honest statement that assets start Unidentified is kept — softening it would hide why early AI
  searches come back thin.)*

**Shipped 2026-08-28**, and it replaces a **static explainer**: three numbered sentences and a
"Get started" that described the app instead of setting it up — and whose third sentence promised
you could *"pull the right asset into Unity in one step"*, which nothing in the app can do until
the plugin exists. Step 1 now lists the sources actually indexed and the Asset Store cache detected
on disk but not yet added (read-only detection: `unity_cache_candidates`, an `is_dir` per candidate
root, never a crawl — running the importing version to find out whether there was anything to find
would index the machine to answer a question). Step 2 runs the scan.

**Two narrowings, both forced by what the backend can honestly report:**

- **No "M new files" count** in the returning-user banner. Counting un-indexed files means walking
  every source root, which costs the same as the scan the number is advertising. The banner states
  what it knows — assets indexed, sources, when the last crawl finished — and the button reads
  **⌘ Scan sources** rather than naming a size it would have to do the work twice to learn.
- **The progress bar measures sources completed, not files.** `add_source` is a synchronous disk
  walk that reports nothing mid-crawl; its own doc comment says *"a fake bar that jumps 0 → 100 is
  worse than an honest spinner"*, and that is still true. Sources are scanned one at a time —
  sequential because they contend on the same disk — so the bar and each mono log line correspond
  to a source that genuinely finished, with the count it returned. A source that fails is logged
  and the run continues: one unplugged drive must not stop the others. A per-file bar needs the
  crawl to report progress, which is the job-tray work in ROADMAP, not this screen's.

**Shipped 2026-08-28**, and it replaces a **static explainer**: three numbered sentences and a
"Get started" that described the app instead of setting it up — and whose third sentence promised
you could *"pull the right asset into Unity in one step"*, which nothing in the app can do until
the plugin exists. Step 1 now lists the sources actually indexed and the Asset Store cache detected
on disk but not yet added (read-only detection: `unity_cache_candidates`, an `is_dir` per candidate
root, never a crawl — running the importing version to find out whether there was anything to find
would index the machine to answer a question). Step 2 runs the scan.

**Two narrowings, both forced by what the backend can honestly report:**

- **No "M new files" count** in the returning-user banner. Counting un-indexed files means walking
  every source root, which costs the same as the scan the number is advertising. The banner states
  what it knows — assets indexed, sources, when the last crawl finished — and the button reads
  **⌘ Scan sources** rather than naming a size it would have to do the work twice to learn.
- **The progress bar measures sources completed, not files.** `add_source` is a synchronous disk
  walk that reports nothing mid-crawl; its own doc comment says *"a fake bar that jumps 0 → 100 is
  worse than an honest spinner"*, and that is still true. Sources are scanned one at a time —
  sequential because they contend on the same disk — so the bar and each mono log line correspond
  to a source that genuinely finished, with the count it returned. A source that fails is logged
  and the run continues: one unplugged drive must not stop the others. A per-file bar needs the
  crawl to report progress, which is the job-tray work in ROADMAP, not this screen's.

---

## 2. Library (home screen)

**Card art for 3D models is a vector preview that spins on hover** — see
[`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md) §7 "Asset card" for the visual contract (traced
contour + tone bands in `--ls-type-model`, four stored viewpoints, ~0.45 s per frame, resting cards
return to the hero view). Grid behaviour that belongs here rather than there: only the **hovered**
card animates, hover-spin applies in the grid only (a **list row** shows the hero view and does not
spin), and the spin is presentation-only — it changes no selection, no filter, and nothing on disk.
A model whose preview hasn't been generated yet falls back to its rendered raster thumbnail, so the
grid never shows a hole while a scan is still draining.

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
   toggle** ⊞ grid / ☰ list / 📁 folders — three stops in the Tauri shell (the egui build still
   offers only Grid / Folders), DESIGN-GUIDELINES §7 "Browse views". The **active-filter bar**
   (item 7) renders directly under this row whenever anything is filtered.
3. **Content filter popover** (⚑, 328px, elevation 2) — DESIGN-GUIDELINES §7: TRAIT chip cloud with
   search (arcane select-state), TRIANGLES buckets `≤1k · 1k–3k · 3k–6k · 6k+` (violet, models
   only), FILE SIZE tiers S/M/L/XL with live ranges from §11.4 (tier-colored). Multi-select within
   and across groups (AND across groups, OR within). Active filter count shows as a gold badge on
   the ⚑ button; "Clear all" resets.
4. **Bulk bar** (when ≥1 card is check-picked): `N selected` + tag-name input + **+ Add tag** /
   **− Remove tag** (skips Unidentified) + **⟡ Stage all** + ✕ Clear. Bulk tag edits are undoable.
5. **The grid / list** (virtualized):
   - **Card** (DESIGN-GUIDELINES §7 anatomy): **1px** status left-border (this said 3px until
     2026-08-22; DESIGN-GUIDELINES §6 says 1px, the app renders 1px, and DG wins on pixel details
     per `docs/design/README.md` — ALIGNMENT-AUDIT F1); thumbnail region (104px, dark
     recess) with: status gem badge (top-right), **pick checkbox** (top-left, appears on
     hover/when any pick is active), `✦ NN%` AI-match pill (top-left, AI results only),
     **S/M/L/XL size badge** (the **letter only** — it sits on the artwork and has nowhere to
     grow; it read `S · 412 KB` until 2026-08-28, which repeated the byte size already in the stat
     line below and could wrap the badge onto a second row over the thumbnail. Exact size on hover,
     matching what the list row has always done) + `✓ in <project>` imported badge (bottom-left), **⤢ Quick Look** +
     **⟡ stage/✓ staged** actions (bottom-right, hover-revealed); body: type diamond + name
     (Unidentified: slate italic), mono pack line (pack name is a link → Pack stats §15) +
     short stats, trait chips (first 3 + `+N` expander; chips are click-to-search).
   - **Sound cards**: in place of the thumbnail — round arcane **▶/◼** play button + a 5-bar
     mini-waveform (slate when Unidentified, aqua otherwise); while that card is the active audio,
     an inline scrubber + mono time readout appears. One asset plays at a time (§13.2).
   - **Unidentified cards**: thumbnail at 30% opacity + grayscale behind a large slate mono `?`;
     chips replaced by a single `???` chip. The **identify shimmer** plays when an asset flips to
     Identified (respects reduced-motion by flipping instantly).

     *(Shipped 2026-08-29 — **400ms**, not the 0.6s written here: it fires per card and a grid
     that has just appraised a page has dozens going at once, where six-tenths of a second reads
     as the grid stuttering rather than as one card reporting. A ring in the status colour rather
     than a sweep across the card, for the same reason. Driven off the status the card is handed,
     not off an event: identification lands as a catalog write the grid re-reads, so there is no
     click to hang it on. Only the Unidentified → Identified edge fires — a re-identify queueing
     an asset back to `indexed` is not an accomplishment. Reduced motion is honoured globally, and
     that block **had to be fixed to honour it**: it neutralised `transition-duration` only, so a
     keyframe animation would have played in full for someone who asked for less motion.)*
   - **List rows** (32–38px, zebra): thumb 26px · type diamond · name · pack (link) · size badge ·
     stats · status gem+label · ⤢ · stage. Same selection/ring behavior.
   - Selected card/row: 2px gold ring. Confirmed cards: faint 1px gold ring at rest.
6. **Empty state**: large ⌕ + *"No name or trait contains that"* / (AI) *"Nothing close in your
   hoard"*. The primary action is the **AI escalation** (§0.1.3) — *"✦ Search meaning instead"* —
   because a description is not a filename and the engine that can answer it is right there
   (ADR-0007). Under it, when Unidentified exist: *"N assets are still Unidentified and cannot be
   reached this way"* → Identify. A filtered-empty result never shows a screen's teaching copy
   (DESIGN-GUIDELINES §7 "Empty states") — the screen is not empty, the filter is.
7. **Active-filter bar** — one strip under the toolbar listing **every** active constraint as a
   removable chip, because the filters live in four places (search box, type tabs, two sidebar facet
   groups, ⚑ popover, ★ toggle) and only pack and collection were ever visible. Left → right in the
   order filters compose: `query · type · status · library · pack · collection · ⚑ · ★`.
   Right-aligned: `n of N shown` + **Clear all** — the same reset the sidebar's Library row performs
   (§0.2), now with a name and a place.
   - **Chip vocabulary** (colour says which axis, never colour alone): query = gold + magnifier;
     type = its type token + rotated diamond (matching the §0.3 underline); status = its status token
     + the same gem shape the facet and card corner use; library / pack / collection = neutral chip
     with a mono axis prefix (`lib` / `pack`); trait = neutral chip keeping the type-coloured dash it
     carries on cards; triangles = violet; size tier = its §11.4 tier colour **with the live range**
     (`M 1–8 MB`), so the letter is never a mystery.
   - Removing a chip drives the control it came from, so there is exactly one source of truth per
     axis. Nothing active → **the bar is not rendered** (no empty rail). Beyond one row the tail
     collapses into a `+N more ▾` popover rather than wrapping the grid downward; **Clear all** stays
     pinned right.
   - Cost, accepted: one row of vertical space above the grid whenever anything is filtered.

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
- Right: large mono **remaining count**.

**Appraisal is a background job, not a screen you sit on** (ADR-0008). It starts automatically after
a scan (respecting the `auto_identify` setting) and runs in the **job tray**, so the screen is a
dashboard you can walk away from:

- **Running row** (arcane): *"Running in the background — you can browse, stage and import while it
  works."* + **⏸ Pause**. Paused keeps its place (*"Stopped at 1,284 / 3,912 — nothing is lost"*) and
  **▶ Resume** continues; done collapses to `◆ All N appraised`.
- **Progress**: `done / total appraised`, throughput and an estimate over an arcane bar.
- **Queue order**, shown explicitly because it is no longer obvious: (1) what is on screen right
  now, (2) the rest of the current filter, (3) everything else, newest first. **Browsing re-orders
  the queue** — the assets you are about to want are appraised first, so the backlog stops being
  all-or-nothing.
  Implemented as an ordering the **whole** appraisal shares, not one queue's: tags, embeddings
  (text, visual, audio), metrics, previews and dependency scans all order by the same expression,
  because an asset that gets tagged and then waits behind 3,000 others for its embedding is never
  promoted and never flips on screen. The focus is a hint held in memory, never a column — it
  changes on every scroll, and a browse that dirtied rows to record where the eye is would be a
  write storm for something that stops being true the moment the user moves. It reaches the writer
  through shared state rather than an op, so browsing never queues behind a running pass, and the
  second tier is capped (500 ids) because the list is inlined into every queue query.
  Absent a focus the order is **newest first**, which is the third tier standing alone.
- **Where the backlog is**: one row per Library Source (name + a slate bar + remaining count) with
  its own **✦ Appraise** — a source or a pack is the unit people think in, not "all 3,912".
- Any single asset jumps the queue with the per-card **✦** (§2.5); it returns Identified in place
  with the shimmer.
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
**A queue, not a scrolling page.** Header carries `n / N groups reviewed` + a gold progress bar.
Left: a **group list** (280px) — one row per group (type diamond + keeper name + member count),
resolved rows dimmed with their outcome (`kept both` / `trashed 1`); `↑ ↓` moves and the list keeps
your place after every decision. Right: the group itself, in the **§13 compare pane** — reason line
(mono — e.g. `perceptual hash · 96% similar · different packs`), the two members side by side with
the **keeper** marked by a gold `★ keeper` ribbon (keeper = highest status, tie → earliest indexed),
and the same differ-is-gold stat table. Groups of three or more hold the keeper on the left and
cycle the right pane (`2 of 3 ›`) — always one comparison against the keeper, never a three-way
stare.

Actions (footer): **Keep both** (dismisses group) · **Merge tags** (union of tags onto keeper,
arcane) · **Move duplicate to Trash** (danger outline; respects confirm-before-Trash §11.6).
Empty state: ⧉ *"No duplicates left to review."* + *"Lodestash flags byte-identical and
near-identical assets across packs. Nothing is ever deleted for you."*

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

**Source state is three-valued** — `available · paused · unavailable` (`import_source.state`,
DATA-MODEL §1). Adding a folder that does not exist is already refused; a source whose root goes
away **after** indexing (external drive unplugged, folder moved, pack uninstalled) is the normal end
state for this product, which only ever points at files it never moves. It is checked on library
open and after every scan — one `is_dir` per source **root**, never per asset, off the UI thread
with a timeout, defaulting to `available` while pending (a false "unavailable" flash on every open
would be worse than the gap it closes). A single missing file is a per-asset fact and never marks
the source.

While a source is unavailable the catalog stays fully browsable — search, facets, stats, stars and
traits all still work, which is the point of an index:

- **Banner** (once, top of the Library, dismissible for the session): `✕ <name> is unavailable — N
  assets are still listed, but their files cannot be reached.` + declared path (mono) + **Relink…**
- **Sidebar facet** (§0.2): the diamond becomes a red `✕` and the row dims — shape, not colour alone
  (DESIGN-GUIDELINES §9). The row still selects; filtering to an unavailable library is legitimate.
- **Cards / rows** (§2.5): the artwork becomes a stated `file unavailable` plate rather than a broken
  image; `⟡` goes inert but stays focusable and explains itself — staging a file the import would
  skip is a promise the tray cannot keep. Everything the catalog knows still reads.
  **`⤢` stays live** (narrowed 2026-08-22, shipped D7b): it is the route to the Quick Look panel
  below, which carries the declared path and **Relink**, and is the one useful action left on an
  unreachable asset. An inert `⤢` beside a live double-click on the same card is a contradiction,
  and it would leave the panel this section specifies reachable only by accident. The Inspector's
  **⇢ Add to project** follows `⟡`, not `⤢`.
- **Quick Look** (§14): an honest panel — *"This file isn't reachable"*, the library and the date it
  was last seen, the declared path, **Relink library…** / **Copy path**.
- **Staging / import** (§16.2): refuse out loud and partially — *"3 of 6 staged assets are
  unavailable — the import skipped them"*. The ones that imported are not rolled back; the ones that
  did not stay staged.
- **Never** auto-remove, auto-trash or purge on the next scan. The non-destructive invariant covers
  absence too.

**Relink panel** (expands from an unavailable row): declared path, **Choose…** for the new root, and
a **match count shown before committing** — *"◆ 4,198 of 4,206 files matched by relative path — 8
would stay unavailable"* — then **Relink N assets**. A relink that would strand half the library is
a thing you see first.

A relink is a **rename, not a re-scan**: `asset_file.rel_path` is relative by definition
(DATA-MODEL §2), so every row keeps its identity, tags, stars, status and import history. Only two
things hold an absolute path and both are rewritten — `import_source.root_path` and the
`package.package_path`s beneath it. A pack that does **not** sit under the declared old root is left
exactly as it is rather than re-anchored to a guess: it then reads as unavailable, which is true.
The count walks both location kinds, because counting only one lies — a loose file resolves as
`<new root>/<rel_path>`, an in-pack asset resolves through its **archive** (memoised per pack, so
thousands of entries cost one `stat` per pack rather than one each). Assets already extracted into
the workspace cache are outside both numbers: moving the root does not move them.
The Library banner's **Relink…** and Quick Look's **Relink library…** both navigate *here* rather
than opening a second copy of the panel — one place commits the change.

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
Regenerate rotates it; per-session token semantics per ENGINE-API §1); **ALLOWED ACTIONS** — a
**literal per-tool allow-list** (card 178, **superseding** the earlier three-permission-group
resolution): one toggle per real agent tool from the canonical inventory
(`lodestash_api::AGENT_OPERATIONS` — `get_server_info`, `search_assets`, `semantic_search`,
`get_asset`, `get_asset_preview`, `find_similar`, `list_packages`, `export_asset`, `get_job_status`,
`cancel_job`; the list grows as tools land). **Default: every tool enabled** — a fresh install
behaves exactly as before, and the user switches tools **off**. A disabled tool is gated on **both**
planes at once: REST answers `403 tool_disabled`, MCP refuses the tool — because both consult one
allow-list on `ServerState` (`set_allowed` / `allows`), which is what keeps the gate MCP↔REST 1:1
rather than two lists that can drift. Persisted as `config.mcp_disabled_tools` (the names switched
off). Side-effecting export/import stays **additionally** confirm-gated per call server-side
(MCP-AND-ENGINES §5): the allow-list decides whether a tool exists for the agent at all; the approval
modal still gates each mutation. The REST-only `…/file` byte route carries no tool name and is never
on the allow-list — it is always reachable behind the auth token.

**Shipped (interim).** The allow-list **engine** is in — both-plane gating, the config field, and
tests (the parity test still asserts MCP↔REST 1:1). Not yet built: the per-tool toggle **UI** in this
card, and the optional Streamable-HTTP transport (stable port 4319, rotating token). Until the UI
lands, the list is driven by editing `config.mcp_disabled_tools` directly.

### 11.6 STORAGE

**Two cards, not one** (reworked 2026-08-29). They were a single **STORAGE CAPS** card governing
two unrelated stores, and folding them together is what let three controls sit inert without
anyone noticing: `cache_extracted`, `preview_cache_cap_gb` and `auto_purge_preview_cache` were
loaded from config, written back to config, and read by nothing.

**PREVIEW CACHE** — thumbnails, decimated meshes, in-pack scratch. All regenerable from the source
asset, so it is capped and disposable: a cap slider (1–16 GB), **Auto-purge over cap**, live usage,
and **Clear preview cache**. Purge is oldest-by-mtime rather than truly
least-recently-*viewed* — reads do not touch mtime, and recording access times would mean a write
on every preview read for files that are cheap to rebuild. Applied at startup: the cache grows one
thumbnail at a time, and checking on that path would stat the whole tree per preview.

**EXTRACTED PACKS — WORKSPACE** — payloads pulled out of a `.unitypackage`. Not a cap but a
choice, per pack. **Keep every extracted pack cached** governs the sweep globally; each pack also
carries its own mark (§15, and on the Packages row). A pack that is *not* kept is still fully
searchable, filterable and previewable — its assets resolve out of the archive by surgical pull
instead of off disk, which is slower per open, not absent.

When anything in the workspace is unreferenced, the card says so in the size it would free —
*"3.55 GB is not referenced by the catalog · 1,179 folders left behind by packs that were
re-imported or removed"* — with a **Reclaim** button. It reports before it removes and appends
what went to `reclaimed.log` beside the workspace, because deleting gigabytes and keeping no
record is not something to do quietly. Below it, cached packs largest-first with their marks; a
pack whose archive has gone reads **archive missing**, is never reclaimed, and offers
**Retarget…** (§15).

**Reclaim refuses rather than guesses.** Both the size line and the sweep answer only when the
engine is Ready and the catalog can say what is still referenced; otherwise nothing is measured
(the size line stays at zero and the button disabled) and a sweep answers with the reason instead
of removing anything — "could not find out" is never read as "nothing is referenced". The sweep
also refuses while a pack is being analysed or extracted, and a file
belonging to an asset in the Trash counts as referenced ([`INGESTION.md`](INGESTION.md) §2).

*(The old text promised a workspace **cap** and a **Clear workspace** action. Neither shipped, and
neither is planned: a size cap needs last-used times the workspace does not record, and "clear
everything" is the wrong verb for a store where one folder may be a pack's only surviving copy.
Reclaim-what-is-unreferenced is the safe form of the same intent.)*

### 11.6b Agent approval prompt (shipped)
A modal over a dimmed library, warning-bordered: **"An agent is asking to write files"**, the job's
one-line summary (`Export N asset(s) to <path>`), the reassurance that originals are only copied,
then **Deny** / **Approve**. No click-to-dismiss — the decision is deliberate. One prompt at a time,
oldest first; the window repaints itself when a request arrives rather than waiting for input.
Approving or denying raises a toast. This is the desktop half of ENGINE-API's server-side
confirmation gate.

Agent tag writes (`write_tags`, ENGINE-API §8.3) wait on the same gate. The job tray's approval row
lists, per asset, that producer's current tags → the proposal, and any proposed caption, so the
person approves what will actually be written. Approving never confirms an asset.

### 11.7 ENGINE BRIDGE
Live status row (verdant dot glow when bridged): `Live — bridged to <project>` / `Bridge paused`,
sub-line `<engine> · imports sync to <dest> as you stage them`; enable toggle (paused ⇒ staged
items queue). Plus: **Confirm before moving to Trash** toggle; **Show the first-run setup screen**
toggle (binds `skip_onboarding`, same flag as §1).

**Shipped (interim).** The card shows what actually exists: `Listening on 127.0.0.1:<port> · API v1`
with a **Copy** button for the session token (the manual-paste flow of ENGINE-API §1), or
"Not listening" when the server couldn't start. The project/bridge status row and the two toggles
arrive with project registration (ENGINE-API §5).

---

## 12. Stub screens

Any navigable destination whose screen isn't built renders: large dim glyph · Cinzel title · one
sentence of what it will be · *"Not wired yet."* · **← Back to Library**. Never a dead click, never
a hidden nav row.

---

## 13. Inspector (right panel, 300px)

Opens when an asset is selected; ✕ / Esc closes.

**Collapsed (nothing selected).** The rail collapses to a **26px edge tab** carrying a vertical
`INSPECTOR ‹` label — it never holds 300px open to print an instruction. Selecting anything reopens
it; the tab reopens it manually (for a pinned read of the last selection). A manual collapse sticks
until the next selection. *(Replaces the "Select an asset." empty state — ALIGNMENT-AUDIT N5.3.)*

**Compare (exactly two picked).** The rail widens to **460px** and shows the two side by side:
preview over preview, then one stat table with a column each. Rows where the two **differ** are gold
on both sides; rows where they agree stay dim — the eye lands on the facts that decide it. Footer:
**Keep left** / **Keep right**. Entered by check-picking exactly two cards, or `C` on a duplicate
group (§5); three or more falls back to the bulk bar. This is the pane §5 reviews groups in.

Selected, one asset — top→bottom:

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
   *(Shipped 2026-07-23: the chips — one per facet value, coloured by facet (subject / style /
   material / mood / usage / audio / free), AI tags carrying a drawn confidence underbar and a
   hover reading `facet · NN% confident`. Empty state is "No traits yet — run Identify." Not yet:
   the dashed `+ trait` quick-add chips from the Trait Library, and accept/reject.)*

6. **LINKED FILES** (when the asset has dependencies — `asset_relationship`, DATA-MODEL §10):
   caption notes the parse source (`parsed from FBX bindings` / `.mtl sidecar` / `.meta GUID refs`);
   rows: kind glyph (◇ material violet / ▦ texture sapphire, textures indented) + mono name +
   state: **✓ in pack** (verdant) · **⤷ in library** (arcane; click opens that asset) ·
   **✕ missing** (red) · **⊘ ignored** (dim). Unresolved rows expand a **relink panel**: the
   declared reference and **⊘ Ignore link** / **↺ Restore link**. Footer warning when any missing.
   (⚖ requires the unresolved-reference delta, §20.)

   *(Shipped 2026-07-22: the read-only panel — caption `parsed from .meta GUID refs · prefab
   bindings`, one row per edge with a type glyph, direction arrow (`→` this asset uses it, `←` it
   uses this asset) and the linked name; clicking a row selects that asset in the grid, and when
   the current filter hides it, a toast says so rather than silently doing nothing. A `missing`
   edge rendered `✕ missing` and nothing else. Glyphs are limited to ◆/◇/• because the bundled
   text fonts have no chain glyph — see §15's dropped ◉, and the correction in
   [`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md) §5: the glyphs are **not** missing, the sprite is
   simply the icon system.)*

   *(Completed 2026-08-29. The **four row states** ship — `same_pack` on `related_assets` answers
   ✓ vs ⤷, and that is not decoration: a dependency inside the pack travels with it, while one
   resolving across packs is what [`ENGINE-API.md`](ENGINE-API.md) §5.5 flags, since Unity
   reassigns GUIDs per import. The **relink panel** ships showing the declared reference and
   ⊘ Ignore / ↺ Restore, and the export-skip warning — which this note claimed was already there
   and was not — now exists and stops counting ignored rows.*

   ***Narrowed:*** *name-match suggestions (`NN% · use →`) and ⌕ Browse disk… are **not
   implementable as specified**, and are dropped rather than faked. Every edge in the harvest
   declares a **GUID**, not a path — `unity_meta` and `unity_prefab` are the only sources, and both
   record the referenced GUID in `declared_path`. A GUID has no name to match against the library,
   and no file chosen off disk resolves it, because what makes an asset the right target is its own
   `.meta` GUID, not where it sits. They become buildable the day an edge is declared by path (an
   `.mtl` sidecar or an FBX binding, [`DATA-MODEL.md`](DATA-MODEL.md) §10 lists both as intended
   `declared_path` forms); until then ⊘ is the honest action, and it is reversible.)*

7. **COLLECTIONS**: toggle chips (`+ name` / `✓ name` gold) — membership per §6.
8. **SIMILAR IN YOUR HOARD**: top 3 (thumb + name + pack, → that asset).

   *(Shipped 2026-08-29, and it is **embeddings, not shared tags**. The spec offered tags with
   "backed by `find_similar` (embeddings) once available" as the upgrade; the embeddings arrived
   first, and on a real library the tag half cannot work — 24 of 8,237 assets carry a tag against
   6,807 carrying an embedding, so a tag ranking would render this panel empty almost everywhere.
   Cosine KNN in the asset's own embedding space, self excluded, trashed rows excluded.

   The index is built once per embedding model and held in `Library`, dropped on reload: this
   panel fires on **every selection change**, and rebuilding per click would re-read every
   embedding out of SQLite to answer it — measured 221 ms cold against ~47 ms warm here. Brute
   force is deliberate at this size (`TECH-STACK.md` §6 has `sqlite-vec`/LanceDB for when the
   scan itself becomes the cost rather than the load).

   Before this the panel drew the asset's **dependency edges** — the same rows LINKED FILES shows
   directly above it — under a SIMILAR heading.)*
9. **Provenance footer**: `Source · <library · pack>` (pack → Pack stats), `License · <effective
   license>` (asset → package fallback, DATA-MODEL §9), and when imported: verdant
   `✓ imported in <project> → <dest>/`.

   For an **in-pack** asset, two more rows: **Path in pack** (the `Assets/…` path the pack
   declared for it) and **Archive** (the `.unitypackage` holding it) — both mono, selectable, each
   with a copy button. They exist because an in-pack asset has no file to reveal in Explorer, so
   *"where is this?"* is answerable nowhere else in the app, and that is the majority state (5,963
   of 7,691 here). An asset that has been **extracted** shows neither: it has a real file on disk,
   and pointing at the archive would send you where the bytes are no longer read from. The Archive
   row names the file §15's **Retarget…** repoints when it goes missing.

   *(Shipped 2026-08-27: the library · pack row, the pack linking to §15 the way it already does
   from every card and list row — this panel named the library and stopped, so the place you open
   to learn about an asset said less about where it came from than the card you opened it from.
   **License landed 2026-08-28** with the harvest behind it ([`INGESTION.md`](INGESTION.md) §2):
   the row reads the *effective* licence — the asset's own override falling back to its pack's
   ([`DATA-MODEL.md`](DATA-MODEL.md) §9) — and links it when a URL is known. It is **omitted, not
   shown as "unknown"**, when nothing in that chain states one; a row that always reads unknown is
   not provenance, which is why it waited for the harvest rather than shipping empty.)*
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

**Keep cached** row (2026-08-29), beside the size it governs: *kept* — its assets open from
disk — or *evictable*, reclaimed once nothing references it (§11.6). Marking a pack that is not
extracted starts **no work**; it promises the bytes survive if you extract later, so one click can
never begin a gigabyte of disk writes. Backed by `package.keep_cached`, deliberately not `pinned`,
which already means Add to Library ([`DATA-MODEL.md`](DATA-MODEL.md) §13 M23).

**Retarget…** appears on a pack whose `.unitypackage` no longer resolves (on the Packages row,
where a missing archive is visible in the list). It counts how many of the pack's recorded entries
a candidate archive still accounts for **by GUID** and offers the swap — the same shape as the
source relink in §11.1, and for the same reason the count is *evidence, not a verdict*: a publisher
updating a pack legitimately reshuffles GUIDs, so a poor match is shown and the user decides. Only
`package.package_path` moves; every asset, locator and extracted byte stays where it is.

*(Shipped: tiles, CONTENTS, HEAVIEST, footer — opened by a plain **Stats** button on the Packages
and Unity Assets
pack rows, for packs whose assets are in the library. Triangles show `—` when nothing in the pack
has been measured yet (DATA-MODEL §14: unknown, not zero). Not yet: the license/exported meta line
(needs the license harvest + `import_record` join) and TAGS, which shows the empty state above
until P4 tagging. The spec's ◉ prefix is dropped for now: the bundled text fonts have no glyph
for it and it renders as tofu — it returns with the icon sprite, DESIGN-GUIDELINES §5.)*

> **The tofu premise was wrong** (measured 2026-08-29 — see
> [`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md) §5). ◉, ⌕ and ⧉ all render. The sprite is still
> the right call for these — one icon system, scalable, themeable — but "the font cannot show it"
> is not the reason, and must not be cited as one for the next glyph.

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
`project_id` (default: active).

**…and one tray before there is a project at all** ([ADR-0010](decisions/ADR-0010-one-staging-tray.md),
2026-08-27). Per-project was read as *only* per-project, which made the tray unusable for anyone
without a registered editor — which, while the Unity plugin does not exist, is everyone. The app
worked around it by keeping its own in-memory list, so the tray on screen and the one ENGINE-API
§6 reads were different objects: `list_staged` answered `[]` while six picks sat in the tray.
There is now a single tray with no project, resolved by the same function as the per-project ones,
and **the first Connect wizard adopts it** rather than dropping the picks it holds. Picks also
survive a restart, which the in-memory list never did.

Contents:
- Header: `⟡ Staging` + project dot/name + mono count + `clear`.
- Thumb strip (34px tiles, type-color border, name tooltip).
- Dependency line: `N picks → M files incl. materials & textures` (dependency closure minus missing
  links, which are skipped with the §13.6 warning).
- Destination row: `→ <project> /` + editable dest subfolder (persists per project).
- Primary: **Import all → <project>** *(⚖ user-facing verb is **Import** for into-project; "Export
  to folder" is reserved for the arbitrary-path `export_asset` flow)*; caption *"picks + linked
  files only — never the whole pack."*
- Secondary: **⤓ Export to folder…** (shipped 2026-08-22). The same picks, copied to a folder the
  user chooses — no editor, no plugin, no registered project. It is level 1 of §16.3 made
  reachable, and while the Unity plugin does not exist it is the **only** way a file leaves the
  library, so it sits beside Import rather than behind a menu. Also offered on the Inspector and
  the card context menu, because a single pick should not have to be staged first.
  The native folder dialog **is** the confirmation: the approval gate in ENGINE-API §4 exists
  because an *agent* asked, and prompting the user to approve their own click would train them to
  dismiss the prompt that matters. The **sandbox is not skipped with it** — absolute, no `..`,
  never inside a library source root — because that rule protects the non-destructive invariant,
  not the agent boundary.
  The toast reports what happened, not that the request was accepted, and names skips by **reason**:
  *in a pack* (extract it first) and *unreachable* (§11.1) are different problems with different
  fixes, and one merged "missing" number would be useless. An export never unpacks an archive on
  the user’s behalf — extraction stays an explicit action (§5).
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
   **Narrowed 2026-08-27** ([ADR-0010](decisions/ADR-0010-one-staging-tray.md)): *plus* exactly one
   tray with `project_id IS NULL`, for the picks made before an editor is connected. Without it
   "per-project" means "no tray at all" for every user until the plugin ships, which is how the
   shell came to keep a second, invisible tray of its own.
   *(DATA-MODEL §9 + M20, ENGINE-API §6, MCP-AND-ENGINES §2 updated.)*
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
   nullable + `declared_path TEXT` + `state` so the Linked-Files UI (ok / in library / missing +
   relink/ignore) has backing rows. **Amended 2026-08-29:** the state enum is
   `('resolved'|'missing'|'ignored')`, not `'library'` — whether a resolved edge is *in pack* or
   *in library* is a fact about the two ends' packages, derived per query (`same_pack`), not a
   state an edge can be stored in; it would go stale the moment either end moved. `'ignored'`
   replaces it, carrying §13.6's ⊘ mute, which must be a state rather than a `DELETE` because
   `record_dependencies` re-derives a harvest wholesale. *(DATA-MODEL §10 updated.)*
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
