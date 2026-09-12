# ADR-0019 — Textured previews, keyed by what they paint

- **Status:** accepted
- **Date:** 2026-09-12
- **Amends:** [DATA-MODEL](../DATA-MODEL.md) §10's one-albedo-per-model preview, and the mesh cache's
  claim that a proxy load is untextured (it was not, on a cache miss).
- **Plan:** [v1 architecture](../superpowers/plans/2026-09-07-v1-architecture.md), S08 (the preview
  identity and fidelity tiers it opens are S14–17)

## Context

S07 resolved what a model wears — per material slot, by property, from the prefab renderer that
declared it. Nothing rendered it. A model's preview was a grey, decimated hero view, and Quick Look
painted a single albedo over every slot of the model. Judging a prop from that picture means judging
its silhouette.

Three constraints shaped the answer:

1. **The embedding must not move.** Its vectors are only comparable to each other, and a re-coloured
   input would silently re-encode the library. Worse, the proxy loader already broke its own rule: a
   fresh parse resolved the model's declared textures, so a model's *first* encode saw a textured
   render and every later one — served from the geometry cache — saw flat colour.
2. **The simplifier is for silhouettes.** It averages UVs across each vertex cluster, which is
   invisible on a flat-shaded proxy and unmistakable on a palette atlas.
3. **Cold and warm must agree.** A thumbnail and the viewer that opens from it painting a model
   differently is worse than either being plain.

## Decision

- **One resolver, both paths.** `mesh::load_textured_with` splits a model by material slot and paints
  each: the file's own declared or embedded texture first, then the albedo the catalog resolved for
  that slot (S07). The cold renderer draws those meshes; the warm viewer is served the same slot's
  texture over `lodestash://slottex`. Neither side resolves anything on its own.
- **Full geometry for textured previews**, decimation only for the untextured proxy. A model past the
  triangle cap falls back to that proxy rather than spending a minute rasterising.
- **Keyed by what it paints.** A textured preview's cache key carries the render version and the
  identity of every texture in it, so swapping an albedo is a different picture, and previews
  regenerate lazily as they are asked for rather than being invalidated in bulk. The untextured
  render keeps its old key and stays the fallback, so nothing loses a picture in the meantime.
- **The wire carries slots, not textures.** `lodestash://mesh` appends a `"SLT1"` trailer naming each
  mesh's slot; textures are separate requests, cached beside the thumbnails. Geometry caches on disk
  are untouched and still parse in every existing reader.
- **The embedding route is fenced off and made deterministic.** `load_preview_cached` returns geometry
  only on a cache miss as well as a hit. `embedding_dirty` is not raised: nothing asks the library to
  re-encode.

## Consequences

- A textured preview costs a parse of the source model (~0.2 s for a Synty prop, ~1.9 s for a dense
  one, measured on the reference library) on a thumbnail miss, against a proxy-cache hit before. It is
  paid once per model per size, and the picture is the point.
- Models whose first encode happened before S08 hold a vector taken from a textured render. They are
  not re-encoded here; the fix stops the drift rather than rewriting history.
- The grid still shows a model's vector preview (M15), not the textured raster. Flipping that primacy,
  a shared eviction policy and hash-based preview identity are S14–17 — this ADR only settles how a
  textured preview is produced and identified.
- A slot the graph paints nothing of stays its flat material colour in both paths, rather than
  borrowing a neighbouring slot's atlas.
