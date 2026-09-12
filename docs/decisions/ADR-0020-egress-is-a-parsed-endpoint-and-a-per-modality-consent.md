# ADR-0020 — Egress is a parsed endpoint and a per-modality consent

- **Status:** accepted
- **Date:** 2026-09-12
- **Amends:** [AI-PIPELINE](../AI-PIPELINE.md) §5's single "cloud tagging is opt-in" switch, and the
  engine planes' assumption that metadata access implies byte access.
- **Plan:** [v1 architecture](../superpowers/plans/2026-09-07-v1-architecture.md), S10

## Context

"Local-first, cloud strictly opt-in" was one boolean and one substring test:

1. **Locality was a substring.** `is_free_endpoint` asked whether the URL *contained* `localhost`,
   `127.0.0.1` or `[::1]`. `https://localhost.example.com/v1` is someone else's server and passed;
   `http://127.1.2.3:8000` is this machine and failed; `http://localhost@evil.example/` is a request
   to `evil.example` with a username that happens to read as local.
2. **Price and place were the same answer.** A free remote endpoint was treated as if it were local,
   which is true about the bill and false about the bytes.
3. **One switch covered every modality.** Enabling cloud tagging enabled sending whatever the pass
   rendered, including pixels derived from audio.
4. **Agents got bytes for free.** `GET …/preview`, `GET …/file` and `get_asset_preview` served the
   file to anything that could reach the port with a token, because reading metadata had been taken
   as permission to read the asset.
5. **Redirects were followed** by the HTTP client's default, so a `3xx` could move the key and the
   image to an origin nobody configured.

## Decision

- **Parse the host, judge the host.** `lodestash_ai::endpoint::locality` extracts the authority
  (dropping userinfo, port and brackets) and calls loopback only the literals and the reserved
  `localhost` name (RFC 6761). Anything unparseable is remote. Deliberately textual: a name that
  resolves to 127.0.0.1 now can resolve elsewhere on the next request, and this decides whether bytes
  are sent at all.
- **Price is a separate question.** `is_free_endpoint` still answers it, for the budget confirm only.
- **Consent is per modality and defaults to off.** `egress::may_send` is the one gate: images (and
  the pixels derived from models, materials and sprites) need `remote_image_bytes`; audio bytes never
  leave, whatever is configured, and neither do pixels derived from audio. A remote model may receive
  text about a clip. A loopback endpoint needs no consent.
- **Refuse before the request exists.** The cloud pass asks the gate before it renders a preview, so
  a denied modality leaves no outbound traffic to observe — the negative case is zero requests, not
  an error after the upload.
- **Redirects are refused.** The call carries the key and the image; a moved endpoint is a
  configuration change the user makes knowingly.
- **Bytes to agents are their own switch.** `mcp_bytes_enabled`, off by default, gates REST's
  preview/file routes (`403 bytes_not_released`) and MCP's preview tool, which still answers with
  metadata and links. A stdio transport proves the client *process* is local and nothing about the
  model behind it.

## Consequences

- Someone whose cloud tagging worked yesterday must tick **Allow image bytes to leave this machine**
  before it works again. That is the point: the old setting chose an endpoint, not a permission, and
  reading it as consent would be exactly the silent widening this ADR exists to prevent.
- An agent that fetched previews must be granted bytes explicitly; until then it sees the same
  metadata and links it always did, with a refusal that names the switch.
- A self-hosted endpoint keeps working untouched — it needs no consent, and the parsing fixes the
  spellings (`127.1.2.3`, `[::1]`, `api.localhost`) the substring test used to reject.
- A provider that relies on redirects will fail with a message naming the target rather than
  silently following it. Configure the final URL.
- Locality is judged by text, not by resolution: a hostile DNS entry pointing `localhost` elsewhere
  would be trusted. Resolving at request time was rejected as worse — it invites TOCTOU between the
  check and the connection, and this decision is made before every send.
