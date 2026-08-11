# ADR-0002 — Single-writer thread owns every catalog mutation

- **Status:** accepted
- **Date:** 2026-08-07

## Context

Mutations come from commands, drains, the protocol (previews writing the cache) and later the
embedded REST/MCP engines. Interleaving them against SQLite invites lock-order bugs and torn
reads between the grid and the writers.

## Decision

One writer thread owns all catalog writes. Frontend commands enqueue `Op`s and await a reply;
the writer's idle pass runs the drains (metrics → previews → deps → embeddings → promote) with
bounded budgets (`max_assets`, AI-PIPELINE §5.1) so a long backlog never blocks writes. The
engine planes (REST/MCP) hold their own read-only catalog connection.

## Consequences

- The API never writes the catalog; write-shaped agent ops (B-series) must go through the
  writer bridge when they land.
- The 33 ms Engine pump (B1) is deferred — the writer idle pass + pump events cover the spirit.
