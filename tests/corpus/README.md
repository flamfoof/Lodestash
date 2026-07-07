# Regression corpus

Small, license-clean asset samples used by ingest/catalog tests.
Spec: [`docs/TECH-STACK.md`](../../docs/TECH-STACK.md) §5.1.

**Rules for samples committed here:**
- **Small** — a few KB each; this is a Git repo, not an asset store.
- **License-clean** — self-authored or CC0 only. No Asset Store / purchased content.
- **One concern per sample** — each exercises a specific parser/path.

**Planned samples** (add alongside the test that consumes it; each pairs with an
`insta` snapshot of its parsed output so a parser regression shows as a reviewable diff):

| File | Exercises |
|---|---|
| `tiny.unitypackage` | gzip+tar untar → GUID→pathname map; `.meta` `labels:` + GUID refs |
| `bc1.dds` | BCn/DXT texture decode |
| `cube.fbx` | mesh detect + normalize |
| `blip.wav` | audio detect + duration metadata |
| `layers.psd` | PSD parse / composite |
| `mat+textures/` | relationship harvest (material → its textures), incl. cross-source |

Until these land, ingest tests run against `tempfile` fixtures generated in-test
(see `crates/lodestash-ingest/src/crawler.rs` tests). The corpus is for the
*real-format* cases that can't be synthesized cheaply.
