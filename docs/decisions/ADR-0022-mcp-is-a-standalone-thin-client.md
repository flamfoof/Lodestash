# ADR-0022 — The MCP server is a standalone thin client of the app

- **Status:** accepted (2026-11-16), implemented on branch `rewrite/v2`
- **Supersedes:** [ADR-0011](ADR-0011-headless-mcp-forwards-writes.md) and its 2026-09-12 amendment
- **Relates to:** [ADR-0021](ADR-0021-engine-owns-the-catalog-ui-is-a-web-client.md),
  [`ENGINE-API.md`](../ENGINE-API.md) §2, [`MCP-AND-ENGINES.md`](../MCP-AND-ENGINES.md) §2
- **Design:** [v2 architecture spec](../superpowers/specs/2026-11-16-v2-architecture-design.md) §7

## Context

`lodestash-desktop --mcp` served MCP over stdio with its own read-only catalog connection and
forwarded app-owned operations to the running app (ADR-0011). It worked, but the MCP server and
the app were two programs coupled to one schema, the server needed its own handshake, version and
catalog-id checks, and every agent operation existed twice — in-process and forwarded. The owner
asked for the product to be two parts: the app, and an MCP server dedicated to it.

## Decision

`lodestash-mcp` is its own binary. It depends on `lodestash-protocol`, `rmcp`, an HTTP client and
serde — never on the catalog, SQLite or the engine; a build test fails if it does.

- Its tools are the `Agent` operations in the protocol catalog. One generic handler turns a tool
  call into the operation's HTTP request and relays the app's answer.
- It reads `engine.json` on every call. With no app running it launches the app with
  `--background` and waits up to 20 s; otherwise it answers `app_unavailable`.
- Requests are never retried.
- stdio first; streamable HTTP on port 4319, started by the app's "Enable MCP server" setting,
  follows.

## Consequences

- Reads need the app process. ADR-0011 kept reads working with the app closed; auto-launch
  replaces that, at the cost of a start-up delay on the first call.
- There is one implementation of every agent operation, in the engine.
- The MCP binary can be shipped, versioned and registered with hosts independently of the app,
  provided the protocol versions match (`api_version` in the handshake).
- The `--mcp` branch, `agent_bridge.rs`, the read-only reader in the MCP path and the per-call
  catalog-id check are deleted.

## Alternatives rejected

- **Keep direct reads, forward writes (ADR-0011 in its own binary).** Keeps reads working with the
  app closed, keeps the schema coupling that caused the migration and read-only bugs.
- **MCP server as a long-lived engine hub** hosting the engine bridge for Unity and other engines.
  Moves the data owner out of the app and adds a service the user must keep alive.
