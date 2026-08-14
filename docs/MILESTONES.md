# Milestones

> Last reviewed: 2026-08-12 (reconciled against [`KANBAN-SVELTE.md`](KANBAN-SVELTE.md)).

High-level checkpoints for Lodestash — each a shippable capability. This is the release view over the
detailed [`ROADMAP.md`](ROADMAP.md) (phases) and [`KANBAN.md`](KANBAN.md) (cards). Core milestones
M0–M6 are the path to v1; **Extra Wins** are opt-in bonuses scheduled *after* the core is scaffolded.

## Core path to v1

| # | Milestone | Phase | Goal / exit criterion | Governs |
|---|---|---|---|---|
| **M0** | **Walking Skeleton** | P0 | Cargo workspace + SQLite catalog + folder scan → searchable-by-name thumbnail grid. *Exit:* point at a folder of images, see them in the grid. | ARCHITECTURE, DATA-MODEL, TECH-STACK |
| **M1** | **View Everything** | P1 | Interactive 3D viewport, audio playback + waveform, image/texture viewer (+ pixel-perfect, material sphere, font specimen, animated sprites). *Exit:* preview all media types from the grid. | INGESTION |
| **M1.5** | **Browser QoL** | P1+ | Asset-browser plumbing layered on M1: virtualized grid, detail/inspector pane, **browse views (flat / folders + breadcrumb + Library filter)**, list-row view, multi-select + context menu, view/sort + thumbnail-zoom, drag in/out, Quick Look, jobs/activity panel, undo/redo, trash, sources manager, settings, tag manager, dup-review. *Exit:* the grid feels like a real asset browser at 50k+ assets. | DESIGN-GUIDELINES, DATA-MODEL |
| **M2** | **Unity Ingestion** | P2 | Import real libraries: `.unitypackage`, **Asset Store download cache** (multi-root, browse, batch unpack), broader formats, dedup, non-content filtering. *Exit:* browse + selectively unpack the owned downloaded library. | INGESTION, DATA-MODEL |
| **M3** | **Search & Identification** | P3 | Local embeddings + hybrid search; **Search by Tags** and **Search with AI** (NL→filters). *Exit:* keyword + tag + natural-language search on a 10k+ library. | IDENTIFICATION-AND-SEARCH, DATA-MODEL |
| **M4** | **Auto-Tagging & Mapping** | P4 | Local/API faceted identification; mapping library; confidence + accept/reject. *Exit:* auto-tag a library and map onto a clean taxonomy. | AI-PIPELINE, IDENTIFICATION-AND-SEARCH |
| **M5** | **Unity Retrieval (v1)** | P5 | Loopback API + MCP server + Unity plugin: selective import (asset + deps) into the open project, from the GUI **and** from Claude. *Exit:* the v1 "Definition of done" in ROADMAP. | MCP-AND-ENGINES, ENGINE-API |
| **M6** | **More Engines & Polish** | P6 | Godot + Unreal (best-effort), packaging/installer, auto-update, first-run setup. | INGESTION, MCP-AND-ENGINES, TECH-STACK |

> **v1 ships at M5** (Unity end-to-end, local-first) — *per [`KANBAN-SVELTE.md`](KANBAN-SVELTE.md) the desktop shell covers M0–M4's app surface (B-series port) and M5's server half (loopback REST + MCP, with the 10 doc'd agent ops live on both planes); the Unity plugin (editor) half and the staging-commit/batch-undo round-trip are the open engine-project work.* M6 broadens engines and hardens distribution.
>
> **M1.5 (Browser QoL)** is a cross-cutting UX lane, not a gate — pull its cards (the `[QoL]` tag in
> [`KANBAN.md`](KANBAN.md)) alongside M1–M3 as the library grows; the genuinely-new interactions are
> specced in [`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md) §7/§9. The **rating UI** is parked by
> design (KANBAN-SVELTE 🔵) — the column stays, but no rating interaction is open work.

## Extra Wins (post-scaffolding, opt-in)

Scheduled **after the core is scaffolded** (≈ after M5) — standalone bonuses, not on the critical path.

| # | Milestone | Phase | Goal | Governs |
|---|---|---|---|---|
| **M7** | **Power Features & Insights** | P7 | "More like this"/example search, auto-collections + tag propagation, credits/attribution export, project asset audit, batch ops, command palette, insights dashboard, pack-update diff, catalog snapshot. | ROADMAP P7 |
| ★ **MX** | **Unity Account Sync** *(the extra win)* | P7 | See below. | INGESTION §2, DATA-MODEL §7, MILESTONES |

### ★ MX — Unity Account Sync (extra win)

Use the user's **Unity login** to go beyond the local cache — list, bulk-download, and update **owned**
Asset Store packs — solving the "can't download multiple at once" pain. **Opt-in; unofficial; not on
the v1 path.** Sequenced after M0–M5.

**Scope**
- **List owned (incl. not-downloaded):** fetch the account's My Assets list (name, publisher, version,
  thumbnail, package id) and show owned-but-absent packs as `status='discovered'` ghost entries.
- **Bulk download queue:** replicate the Package Manager's per-asset download into the cache via the
  job engine, with **bounded concurrency + rate limiting** — the headline win over the one-at-a-time UI.
- **Update detection:** diff owned versions vs the downloaded cache (`package.version`/`content_hash`)
  → "N updates available → re-download"; optional versioned backups.
- **Login & credentials:** Unity ID OAuth (embedded webview) → session/refresh token, or reuse the
  Hub/Editor token, or paste a cookie. Store the refresh token in the **OS keychain**, **local-only**.

**Reuses existing design** (additive, not architectural): downloads land in the Asset Store cache that
M2 already watches/ingests; version diff uses the `package` model; the job engine runs the download
queue.

**Guardrails / risks (must hold):**
- **Unofficial endpoints** — no supported consumer API; expect breakage and budget maintenance.
- **ToS-grey** — automating downloads of *your own* purchases only; be a good citizen (serialize /
  cap concurrency, rate-limit, no hammering); never redistribute.
- **Credential safety** — keychain storage, local-only, never transmitted anywhere but Unity.
- Ship **off by default**; clearly labeled experimental.

> Feasibility confirmed: third-party tools (e.g. Asset Inventory) already do direct + bulk download via
> the account session, so this is buildable — the constraints above are why it stays opt-in and post-v1.
