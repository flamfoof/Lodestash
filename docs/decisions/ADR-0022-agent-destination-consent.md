# ADR-0022 — Agent destination and modality consent

- **Status:** accepted
- **Date:** 2026-09-16
- **Supersedes:** the agent-switch-only permission in
  [ADR-0020](ADR-0020-egress-is-a-parsed-endpoint-and-a-per-modality-consent.md); its cloud-tagger policy remains unchanged.
- **Contract:** [ENGINE-API §8.2](../ENGINE-API.md#82-evidence-and-file-access).

## Decision

Keep the agent byte switch as a master deny, and require a named, app-configured destination with
explicit modality consent. Old switch-only configurations become metadata-only. Apply the shared
policy to MCP images/resources and authenticated REST previews/files, before source work and again
before delivery. Long-lived processes read current app settings, so revocation does not depend on
reconnecting. Audio-derived images inherit audio policy and never go to a remote destination.

Use one active profile for the existing shared MCP/REST surface. The user configures the host's
intended destination; a loopback transport or caller-supplied `local=true` cannot prove it. This
preserves the existing authentication model and avoids inventing client accounts. The tradeoff is
that separate concurrent-host permissions require separate credentials/identities later. Hosts
remain responsible for where they send bytes after an authorized release.

Settings clears modality consent on destination changes. The existing Settings payload also now
round-trips both byte switches: previously the frontend exposed them but Rust silently dropped them.
No live configuration is changed by implementation or tests. The current pilot stop remains in force.
