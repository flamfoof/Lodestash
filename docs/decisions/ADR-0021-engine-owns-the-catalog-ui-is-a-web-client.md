# ADR-0021 — One engine owns the catalog; one operation catalog; the UI is a web client of `/v1`

- **Status:** accepted (2026-11-16), implemented on branch `rewrite/v2`
- **Supersedes:** the IPC and custom-scheme half of [ADR-0001](ADR-0001-tauri-svelte-shell.md)
  (Tauri 2 and Svelte 5 stay); the "convention" form of [ADR-0002](ADR-0002-single-writer-thread.md)
  (the single writer stays, now enforced by type)
- **Relates to:** [ADR-0016](ADR-0016-coalesced-change-publication.md) (the change bus moves into
  the engine unchanged), [ADR-0022](ADR-0022-mcp-is-a-standalone-thin-client.md)
- **Design:** [v2 architecture spec](../superpowers/specs/2026-11-16-v2-architecture-design.md) §2–§6, §9

## Context

The [2026-11-16 review](../reviews/2026-11-16-architecture-review.md) found three overlapping
facades over the catalog — the desktop writer and snapshot read model, the HTTP `ServerState`, and
107 Tauri commands — with different shapes, string errors and refresh rules (R1). The UI could not
know when data changed except through counters bumped by hand (R2). Every Tauri command ran on
the window's UI thread (R6). The single-writer rule held by convention; the "read-only" GUI model
opened the catalog with the migrating constructor (R5).

## Decision

1. A new `lodestash-engine` crate is the only code that opens the catalog for writing. It owns the
   writer thread, a reader pool, one job registry, the change bus, the config store and the model
   manager. `lodestash-core` splits `CatalogWriter` from `CatalogReader` so a reader cannot write.
2. A new `lodestash-protocol` crate declares every operation once. HTTP routes, MCP tools and the
   web client's types are derived from that declaration.
3. The UI calls the same `/v1` API as agents, over loopback HTTP, served same-origin by the app's
   own server with a cookie session. Bytes (thumbnails, meshes, files) are `/v1` routes. The UI
   needs no Tauri IPC; the shell only owns the window, single instance and lifecycle.

## Consequences

- One code path per operation, whoever calls it. A missing implementation does not compile.
- Agent and background writes reach the UI through the change feed; the read model rebuilds when
  the catalog generation moves.
- The window cannot freeze on a slow operation; the server is async and long work is a job.
- The UI can run in a plain browser against `lodestash-desktop --serve`, which is what makes the
  fixture-catalog e2e and the design gate possible (ADR-0023).
- **Cost:** the loopback server must authenticate two kinds of caller and defend against DNS
  rebinding and CSRF (spec §6). OS integrations the UI used to reach through Tauri (folder picker,
  reveal in Explorer) become UI-only operations the engine runs in-process.
- **Cost:** the engine is the largest crate. It is organised by area so no module grows into a
  second `writer.rs`.

## Alternatives rejected

- **Keep Tauri commands for the UI, HTTP for agents.** Two adapters to keep equivalent forever —
  the state that produced R1.
- **One generic Tauri command that dispatches the operation catalog.** Removes the drift but keeps
  the UI tied to a WebView2 runtime for every test, so the design gate would still need CDP into
  the user's app.
- **A separate daemon process owning the engine.** Survives window close, but adds a lifecycle the
  user must manage. `--background` gives the MCP server what it needs without it.
