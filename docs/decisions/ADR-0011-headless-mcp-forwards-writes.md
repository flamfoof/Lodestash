# ADR-0011 — The headless MCP server forwards its writes to the running app

- **Status:** accepted (2026-08-29); **amended 2026-09-12** by v1 plan S11 — see
  [Amendment](#amendment-2026-09-12--forward-the-operation-not-the-write)
- **Relates to:** [ADR-0002](ADR-0002-single-writer-thread.md) (single writer),
  [`ENGINE-API.md`](../ENGINE-API.md) §1–2, [`MCP-AND-ENGINES.md`](../MCP-AND-ENGINES.md) §2

## Context

`lodestash-desktop --mcp` serves the agent plane over stdio — the transport Claude Code and Claude
Desktop speak — with its own `ServerState` and its own catalog connection. It was **read-only by
design**: `set_write` is called in the GUI startup path and nowhere else, and the code says so
("a headless server without the desktop is read-only, which is the correct default").

That default made the agent surface half a surface. Measured against the reference library:

```
tools/list                        23 tools, handshake clean
search_assets / get_asset         work
list_tags / find_similar          work
add_tag                           "the engine has no writer bridge — read-only mode"
```

So an agent could read the catalog and could not act on it — it could not tag an asset, stage one,
or start an identify pass. The one thing the tagging workflow needs is the one thing it could not
do.

Simply calling `set_write` in the headless path is the tempting fix and the wrong one. It would put
a **second writer thread in a second process** against the same SQLite file, which is exactly the
`database is locked` ADR-0002 exists to prevent — and it would fire precisely when the user has the
app open, the common case.

## Decision

**The headless server keeps its read-only catalog connection and forwards every mutating op to the
running app** over the loopback REST plane the app already serves, using the port and token the app
already publishes in `engine.json` (ENGINE-API §1).

Reads stay local: SQLite WAL admits any number of readers, so a search or a preview costs no HTTP
hop. Writes take the long way round and land in the same single writer the GUI uses.

```
claude ── stdio ──▶ lodestash-desktop --mcp
                          │  reads: local, read-only connection
                          │  writes: engine.json → port + token
                          ▼
                    running app ──▶ single writer ──▶ catalog
```

With no app running the forward fails to connect and the tool answers honestly — the same
read-only behaviour as before, with a message that says what to do about it.

## Consequences

- **One writer, always**, whoever asked and whichever plane they came in on. ADR-0002 holds without
  a cross-process lock, a lease, or a liveness probe.
- **Agent writes require the app to be open.** That is a real limitation, and the one the
  alternatives trade against. It is acceptable because tagging is something you do *while looking
  at the library*, and because the failure is loud rather than silent.
- **No pid liveness check.** A stale `engine.json` fails to connect; a different process that
  happened to take the port answers `401` on the bearer token rather than being handed someone
  else's write. Both fail in the safe direction, for less code than a per-platform process probe.
- **Two ops cannot be forwarded** — `RemoveTag` and `UnstageAsset` have no REST route (and no MCP
  tool). They are refused with a message rather than reported as zero rows changed, which would
  read as "done, nothing matched". If either ever gets a tool, it needs a route first — which is
  the MCP↔REST 1:1 invariant doing its job.
- **Long-running ops report `1`**, meaning one job accepted, not one row changed: `identify` and
  `unpack` answer with a `job_id` the caller polls. Reporting `0` would read as nothing happening.

## Amendment 2026-09-12 — forward the operation, not the write

The first version forwarded **writes**, one `AgentWrite` at a time, and it left three things wrong
that a real host would hit:

1. **Jobs were nested.** A forwarded `identify_asset` made a job in the headless process, then
   polled the app's job for up to thirty minutes on a thread. The id the agent held was the wrong
   one: `cancel_job` cancelled a wrapper, and the app's tray showed a job no agent could name.
2. **Approvals parked where no window was.** `write_tags`, `export_asset` and the imports waited
   on a gate in the headless process's memory; nothing could approve them.
3. **The handshake was read once, at startup.** An app started — or restarted onto a new port — after
   the server stranded it read-only until the host was restarted.

And two things the decision assumed were not true: the headless catalog connection was opened with
`Catalog::open`, which **migrates**, and the headless branch returned before the managed ONNX
Runtime path was set, so `semantic_search` could never load its model there.

**Now:**

- **App-owned tools are forwarded whole.** `add_tag`, `write_tags`, `apply_mapping`,
  `identify_asset`, `unpack_package`, `stage_asset`, `import_staged`, `import_to_project`,
  `export_asset`, `register_project`, `get_job_status` and `cancel_job` each become the one REST
  request for that operation, and the app's answer — body and status — *is* the tool's answer. The
  planes are 1:1, so this is the same result the tool gives in-process, with the app's own job id.
  Nothing is run twice, and no job or approval exists in the headless process.
- **Every call reads `engine.json` again**, and forwards only when the file names API `v1` and the
  **same catalog** the server reads (M31 `catalog_id`, carried in the handshake). An app with another
  library open is never sent anything.
- **Nothing is retried.** A forward that timed out after it was sent may still land; only a request
  id (`write_tags`) makes a repeat safe, and that decision belongs to the caller.
- **No app is a domain failure**, `app_unavailable` (503, `isError`), not a protocol error — the
  request was well-formed and reads still work. `get_server_info` reports `writer` (`in_process`,
  `app`, `app_unavailable`, `none`) so a client can check before it asks.
- **The reader is genuinely read-only** (`Catalog::open_reader`): `SQLITE_OPEN_READ_ONLY`, no
  pragma that writes, and any schema version other than this build's is refused with what to do.
  The engine's three direct writes (project registration, import records, export history) moved onto
  the bridge, so the in-process engine obeys the same rule.
- **Startup order:** the ONNX Runtime path is set before the `--mcp` branch, and `tracing` writes to
  stderr — stdout is the JSON-RPC channel, and the fmt layer's default writer was stdout.
- **Tools run off the async thread.** Both planes are current-thread runtimes calling synchronous
  state; handlers now run on the blocking pool, so `get_job_status` answers while a preview renders
  or a model loads.

The "two ops cannot be forwarded" and "long-running ops report `1`" consequences above no longer
apply: there is no per-write mapping, and a long-running tool returns the app's job.

## Alternatives rejected

- **Give the headless server its own writer.** Two writers against one file. Guarding it on
  "is the app running?" makes writes work only when the app is *closed* — the opposite of when you
  want them.
- **Both: forward when the app is open, own writer when it is not.** Covers every case and never
  double-writes, but leaves two write paths that must stay behaviourally identical or an agent gets
  different results depending on whether a window is open. Revisit if headless-only tagging becomes
  a real workflow.
- **The Streamable-HTTP MCP transport** on the stable port 4319 that [`UI-SPEC.md`](../UI-SPEC.md)
  §11.5 already specifies — a client would talk to the running app directly with no stdio hop. It
  is the design on paper and remains the eventual answer; it is a larger build, and this decision
  does not block it. When it lands, the forwarding bridge becomes unnecessary rather than wrong.
