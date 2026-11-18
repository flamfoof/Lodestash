# ADR-0024 — The boundary fails closed

- **Status:** accepted (2026-11-17), implemented on branch `rewrite/v2`
- **Amends:** [ADR-0022](ADR-0022-mcp-is-a-standalone-thin-client.md) (how the MCP server finds
  and starts the app)
- **Relates to:** [ADR-0020](ADR-0020-egress-is-a-parsed-endpoint-and-a-per-modality-consent.md),
  [ADR-0021](ADR-0021-engine-owns-the-catalog-ui-is-a-web-client.md),
  [`ENGINE-API.md`](../ENGINE-API.md) §1–§2, [`MCP-AND-ENGINES.md`](../MCP-AND-ENGINES.md) §2
- **Design:** [v2 architecture spec](../superpowers/specs/2026-11-16-v2-architecture-design.md)
  §4.5, §6, §7

## Context

A review of the v2 boundary code (2026-11-17) — the protocol crate, the MCP server, the HTTP API
and the config store — found the same shape of defect in several places: where an input was
absent, stale or ambiguous, the code resolved it in the permissive direction. A handshake file
left by a dead app was still an address the session token was sent to. A string in that file was
run as a program. A launch that outlasted one call was forgotten, so the next call started
another app.

None of these is a design change. Each is a default that pointed the wrong way, and the rule that
replaces them is one rule, so it is recorded once.

## Decision

**At the boundary, what cannot be verified is not trusted, and what is not said changes nothing.**

### The handshake is an address only while its process is alive

- `lodestash-mcp` checks that the handshake's `pid` is a running process **before** the version
  check and before any connection. A file whose process is gone is treated as no file: it leads
  to the launch path, never to a request. The same test applies to the handshake a launched app
  publishes.
- Liveness uses the OS directly, with no new dependency: `OpenProcess` + `GetExitCodeProcess` on
  Windows, `kill(pid, 0)` on Linux and macOS. A pid the user cannot query is not the app (the app
  runs as that user) and reads as dead.
- The app removes its handshake on a clean exit, and only while the file still names its own
  pid, so it cannot remove a newer instance's.
- A **running** app of another API version is refused. A dead one's file is stale, whatever
  version it names — otherwise an old file blocks the new app from ever being started.

### The handshake's `exe` is the last resort, and constrained

The launch order is `LODESTASH_APP` → the `lodestash-desktop` installed beside the MCP binary →
the handshake's `exe`. The `exe` is used only as an absolute path on a local disk to a file named
`lodestash-desktop` or `lodestash-desktop.exe`. (ADR-0022's order put `exe` second and checked
only that the file existed.)

### One launched app at a time

The MCP server remembers the process it started while it runs. Calls that arrive meanwhile wait
for that process and answer the retryable `app_starting` when their own 20 s ends; none starts a
second app. `app_unavailable` keeps its meaning: nothing answers and nothing could be started.

### A Settings write changes only what it says

- **The API key.** `api_key` absent, `null`, empty or blank leaves the stored key alone; a value
  replaces it; only `clear_api_key: true` removes it, and it wins over a key sent with it. (An
  empty string used to clear the key, so a key field that was typed into and erased wiped the
  stored key on the next save of anything.)
- **The spend cap and the endpoint.** `api_max_per_run` and `api_base_url` are optional on a write
  and absent keeps the stored value. The DTO no longer has a default of its own for either. (An
  omitted cap used to read as `0` — uncapped — while the config's default is 500.)
- Every other field a write may omit is "off" or "none configured" when absent, and the
  allow-list (`mcp_disabled_tools`) is required. A test in `lodestash-protocol` finds the optional
  fields by leaving each out and fails when the list changes.

### A config that was not read is not written over, and only the catalog's owner writes it

- Only a file that was **read** and fails to parse is corrupt, and is quarantined. A file that
  cannot be read, or that is corrupt and cannot be moved aside, blocks the store: updates are
  refused until the file loads, and meanwhile the store holds a fallback that switches nothing on
  — every agent tool off, no background identification, no rescans. The update that finds the
  file readable again takes its config and is itself refused once: it was decided against the
  fallback.
- `Engine::start` takes the catalog lock before it opens the config. An engine that is refused
  the lock gets a read-only store, so a second instance cannot save its stale copy over the
  first's changes.
- Config observers are told of commits in commit order, and a save that fails restores the
  credential store.

## Consequences

- A crashed app's `engine.json` costs a launch, not a leaked token. The remaining window is pid
  reuse: the dead app's pid must come to name another of the user's processes *and* a third party
  must hold its old port. The clean-exit removal keeps stale files rare.
- A user who relied on the handshake's `exe` to find an app in an unusual place — a renamed
  binary, a network share — must set `LODESTASH_APP`.
- Hosts see a new error code, `app_starting`, on a slow first start. It carries `sent: false`,
  so repeating the call is safe.
- On Linux and macOS a network mount is not told apart from a local directory; the file-name
  rule still applies there.
- A client that cleared the key by sending `api_key: ""` now changes nothing; it must send
  `clear_api_key`. The old shell's Settings screen gained a **Remove saved key** button for it.
- A client that relied on an omitted `api_max_per_run` meaning "uncapped" must send `0`.
- While `config.json` is unreadable the app is deliberately quiet: agent tools answer
  `tool_disabled` and nothing identifies or rescans in the background, until the file loads.
  That state is reported (`config_unreadable` in the engine report) rather than silent.
- A second Lodestash instance shows the settings it read at start and cannot change them.

## Alternatives rejected

- **Give the DTO the config's defaults** for the cap and the endpoint (one shared constant).
  Removes the `0` trap, but an omitted field would still *change* something: a tighter cap would
  loosen to the default, and a local endpoint would become the default remote one — with the
  stored key then sent to it. "Unchanged" has neither problem and needs no shared default.
- **Run on plain defaults while the config is unreadable.** The defaults switch on what the user
  may have switched off (every agent tool, auto-identify).
- **Apply the pending update once an unreadable config loads.** A whole-object Settings write
  made from the fallback would then replace the user's settings with it.

- **Verify the process image behind the pid** (that it really is `lodestash-desktop`). Closes the
  pid-reuse window, but needs per-OS process inspection and a rule for development builds; the
  window is narrow enough to leave.
- **Authenticate the app to the MCP server** (a challenge over the port before the token is
  sent). The right answer if the handshake file ever leaves the user's own profile; more protocol
  than a loopback, same-user channel needs today.
- **Wait for the launched app without a per-call limit.** Simpler than `app_starting`, but a host
  with a tool-call timeout would see a hang instead of an answer it can act on.
