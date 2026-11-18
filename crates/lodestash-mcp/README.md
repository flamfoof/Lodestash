# lodestash-mcp

Written 2026-11-16 (v2 plan P4). Lodestash's MCP server: a standalone binary that an MCP host
(Codex, Claude Code, …) starts over **stdio**. It is a thin client of the running Lodestash app
([ADR-0022](../../docs/decisions/ADR-0022-mcp-is-a-standalone-thin-client.md), v2 spec §7): it never
opens the catalog, and every tool is answered by the app's `/v1` API.

- **Tools** are the `Agent` operations of the protocol catalog (`lodestash-protocol`'s
  `OPERATIONS`): name, description and input schema come from the catalog. One generic path serves
  them all — input → the operation's HTTP request → the app → the answer. A new agent operation in
  the catalog is a new tool here with nothing to write.
- **The app's REST answer is the tool's answer.** The shapes are `docs/ENGINE-API.md` §2–§8:
  a JSON body becomes the structured content plus the same JSON as text (REST's containers —
  `{results}`, `{tags}`, `{packages}` … — and REST's default limits); an `image/*` body
  (`get_asset_preview`) becomes an image block; a non-2xx answer becomes `isError: true` carrying
  the app's error envelope `{error: {code, message, detail}}`.
- **`get_server_info`** adds `mcp: {version, transport, handshake, port}` to the app's answer, so a
  host can see which server and which `engine.json` it is talking through. Paths a model reads —
  here and in errors — show the home directory as `~`; the full path is in the log on stderr.
- **One resource:** `asset://{id}/file` (the `file_resource_uri` asset summaries carry) reads the
  app's `GET /v1/assets/{id}/file`, inline up to 4 MiB.
- **Errors.** Input the operation refuses → `isError`, code `invalid`, nothing sent. No app
  answering and none could be started → `isError`, code `app_unavailable`, with `detail` naming the
  handshake file and whether the request was sent (`sent`). The app this server started is still
  coming up → `isError`, code `app_starting`, `sent: false`: ask again. An unknown tool name → a
  JSON-RPC error. **Nothing is retried:** a request that reached the app and got no answer may have
  been applied.
- **Leaving.** The server exits within about a second of the host closing stdin, even with a call
  still waiting on the app.

## Finding and starting the app

On **every** call the server reads the app's handshake file (`engine.json`, ENGINE-API §1):
`LODESTASH_ENGINE_JSON` if set, else `%LOCALAPPDATA%\Lodestash\engine.json` (Windows),
`~/Library/Application Support/Lodestash/engine.json` (macOS), `~/.local/share/lodestash/engine.json`
(Linux).

**The file is trusted only while the process it names is running**
([ADR-0024](../../docs/decisions/ADR-0024-the-boundary-fails-closed.md)). Every request carries the
session token, and a crashed app leaves its file behind while its port goes to whoever binds it
next. So the handshake's `pid` is checked first — before the version, before a connection — and a
file whose process is gone is treated as no file. (Windows: `OpenProcess` + `GetExitCodeProcess`;
Linux and macOS: `kill(pid, 0)`. A pid this user cannot query is not the app. The OS reuses pids,
so a dead app's pid can come to name some other live process; that leaves a narrow window, which
the app closes on a clean exit by removing its file.) A *running* app whose `api_version` is not
`v1` is refused.

If the file is missing, stale, or its port refuses the connection, the server launches the app with
`--background` — detached, in the app's own directory, no stdio, outside the host's job where
Windows allows it — and the call waits up to **20 s** for a `v1` handshake, written by a live
process, whose port answers. The binary is, in order:

1. `LODESTASH_APP` — when set, the only candidate (if it names nothing, the answer is
   `app_unavailable`; it never falls through to another binary);
2. `lodestash-desktop` (`.exe` on Windows) beside this binary;
3. the `exe` the last handshake recorded — the least trusted, since it is a string in a file. It
   is used only when it is an absolute path on a local disk (no UNC or network-drive path on
   Windows; on Linux and macOS a network mount is not told apart) to a file named
   `lodestash-desktop` or `lodestash-desktop.exe`.

**One launch at a time.** The process the server started is remembered while it runs. If it takes
longer than one call's 20 s to come up, that call and the ones after it answer `app_starting` and
wait for the same process — none starts a second app. A start that exits with a failure before
publishing is reported at once as `app_unavailable`.

The launched app is given `LODESTASH_ENGINE_JSON` naming the file the server waits on.

> **Today's app (until v2 plan P5).** `lodestash-desktop` does not implement `--background` yet: a
> launch opens the normal window, which then serves `/v1` as usual. It also writes its handshake to
> the platform path only and does not record `exe` — so set `LODESTASH_ENGINE_JSON` only for tests,
> and install `lodestash-mcp` beside the app (or set `LODESTASH_APP`) for launching to work.

## Build

```sh
cargo build -p lodestash-mcp --release
# → target/release/lodestash-mcp(.exe); install it beside lodestash-desktop(.exe)
```

Logs go to stderr (stdout is the JSON-RPC channel); `RUST_LOG=lodestash_mcp=debug` for more.

## Register with a host

Use the absolute path of the built binary. Syntax checked against the CLIs' own `--help` on
2026-11-16 (Codex CLI 0.166.0, Claude Code 2.1.326).

**Codex** (`codex mcp add <NAME> [--env KEY=VALUE] -- <COMMAND>...`):

```sh
codex mcp add lodestash -- "C:\path\to\lodestash-mcp.exe"
# with an explicit app binary:
codex mcp add lodestash --env LODESTASH_APP="C:\path\to\lodestash-desktop.exe" -- "C:\path\to\lodestash-mcp.exe"
```

**Claude Code** (`claude mcp add [options] <name> <commandOrUrl> [args...]`; stdio is the default
transport, `-s user` makes it available in every project):

```sh
claude mcp add -s user lodestash -- "C:\path\to\lodestash-mcp.exe"
# with an explicit app binary:
claude mcp add lodestash -e LODESTASH_APP="C:\path\to\lodestash-desktop.exe" -- "C:\path\to\lodestash-mcp.exe"
```

**Any other stdio host** — the usual JSON shape:

```json
{
  "mcpServers": {
    "lodestash": {
      "command": "C:\\path\\to\\lodestash-mcp.exe",
      "args": [],
      "env": { "LODESTASH_APP": "C:\\path\\to\\lodestash-desktop.exe" }
    }
  }
}
```

The app's own Settings → MCP allow-list and approval gates apply to every call (ENGINE-API §8.4):
a tool switched off there answers `tool_disabled`, and gated tools wait for approval in the app.

## Tests

`cargo test -p lodestash-mcp` — the built binary over real stdio JSON-RPC against today's `/v1`
router on a temp catalog (`tests/e2e.rs`), launching fake app executables (`tests/launch.rs`,
unit tests in `src/launch.rs` and `src/forward.rs`), `tools/list` against the catalog
(`tests/stdio.rs`), and the dependency rule (`tests/deps.rs`: no catalog, SQLite, engine or server
crate in the normal tree). The trust rules are tested against a raw listener that is *not* the app
(`tests/common/raw.rs`): a dead process's handshake gets no connection, a dropped request is sent
exactly once, another API version is sent nothing. Every test points the server at a temp
`engine.json` and an app binary of its own; none touches the real handshake file.
