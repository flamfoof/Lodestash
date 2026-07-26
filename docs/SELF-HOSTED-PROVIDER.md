# Self-Hosted Lodestash Model Service — spec stub (SEPARATE / FUTURE)

> **Status: not scheduled, not built.** This is a placeholder capturing intent so it isn't lost. It
> is an **entirely separate spec** from the app's tagging pipeline. Nothing here blocks or changes the
> shipped identification features. Do not implement from this doc without a dedicated design pass and
> its own Kanban epic.

## Intent

Offer a **hosted, subscription-based inference endpoint** that other Lodestash users (or the public)
can point their app at instead of running models locally or paying a big cloud provider — a managed,
Lodestash-branded model service tuned for game-asset tagging.

## Why it's nearly free on the client side

The app's `ApiTagger` already speaks the **OpenAI-compatible** chat+vision shape
([`AI-PIPELINE.md`](AI-PIPELINE.md) §5). A hosted service is therefore just **another Endpoint URL +
key** in the *Settings ▸ AI · IDENTIFY (CLOUD)* card — no client code required to consume it. If the
hosted service exposes a standard `/chat/completions` (or `/v1`) vision route, today's build works
against it unchanged.

The work of this spec is entirely **server-side / commercial**, not in this repo's core:

- **Serving**: an OpenAI-compatible gateway (vLLM / TGI / LiteLLM proxy) in front of the chosen
  VLM(s); autoscaling; a vision model tuned or prompted for game-asset facets.
- **Accounts & billing**: subscription tiers, API-key issuance/rotation, per-key quotas and rate
  limits, usage metering, abuse controls.
- **Privacy/legal**: a clear data policy (previews are uploaded during tagging), retention promise,
  ToS, regional hosting. Must not undercut the app's **local-first, opt-in cloud** invariant — the
  hosted service is one more opt-in provider, never a default that sends data off-box silently.
- **Trust surface**: TLS, key scoping, no training on user assets without explicit consent.

## Possible small client niceties (later, optional)

- A one-click **"Lodestash Hosted"** preset in the CLOUD card that fills the Endpoint URL and links to
  sign-up (still just an OpenAI-compatible target under the hood).
- A key-validation ping and a remaining-quota readout, if the service exposes those.

## Explicitly out of scope here

Model training, GPU procurement, pricing, the billing stack, and the marketing site. Those belong to
the separate business/infra spec this stub is a pointer to.

## See also

- [`MODEL-GUIDE.md`](MODEL-GUIDE.md) — user-facing "which model" guide (mentions this as future).
- [`AI-PIPELINE.md`](AI-PIPELINE.md) §5 — the OpenAI-compatible `ApiTagger` a hosted plan plugs into.
