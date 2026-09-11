# ADR-0018 — Prefabs are scoped evidence for v1

- **Status:** accepted
- **Date:** 2026-09-11
- **Supersedes:** [DATA-MODEL](../DATA-MODEL.md) §10's whole-file prefab pairing (step 1b) and its
  wholesale rebuild of `unity_prefab` edges.
- **Plan:** [v1 architecture](../superpowers/plans/2026-09-07-v1-architecture.md), S06

## Context

A stock FBX embeds its materials, so the `.prefab` is the only place where "this mesh is painted with
that material" is written down. The pass that read it had four defects, each checked against the
code:

1. **Whole-file pairing.** Every `m_Mesh` and every `m_Materials` item in a file went into two lists
   that were multiplied: a prefab holding two models linked each one to the other's materials. A
   `MeshCollider`'s mesh, and the mesh a particle system emits from, counted as painted ones.
2. **Wholesale rebuild.** Each pass deleted every `unity_prefab` edge and inserted what it found.
   That un-muted every edge a person had ignored, and dropped the edges of any source whose drive was
   not connected at the time.
3. **Blind to change.** The pass ran only when an asset row changed. A prefab is not an asset, so
   editing one changed nothing until something else did. Prefabs extracted into the package workspace
   were never read at all.
4. **Nothing to resolve later.** A pair whose mesh was not catalogued yet was dropped, not kept, so
   the edge never appeared once the mesh arrived.

## Decision

- **Read per document.** Each component is its own `--- !u!` YAML document. A `MeshRenderer` pairs
  with the `MeshFilter` on its own GameObject (`m_GameObject`), and a `SkinnedMeshRenderer` carries
  its own mesh. A `ParticleSystemRenderer` in mesh mode paints its meshes with its first material
  (the second slot is its trail's). Every binding keeps its renderer and material slot. A material's texture bindings
  keep their property names (`_BaseMap`, `_MainTex`, `_BumpMap`), which S07 needs.
- **Keep the evidence** (M28). `prefab_file` records each prefab under the scope it was walked in (a
  source root or the package workspace) with the size and mtime it was read at. `prefab_binding` is
  what the prefab declares, one row per renderer slot. Only a prefab whose revision moved is re-read.
- **Derive the edges.** `reconcile_prefab_edges` makes one `uses_material` edge per distinct pair of
  catalogued mesh and material GUID, and removes an edge no binding backs, but never a muted one. A
  mesh catalogued late gets its edge on the next pass. A material catalogued late is late-bound as
  before.
- **Delete only what a walk proved.** A walk that completed forgets the prefabs it no longer found. A
  root that does not resolve, or a directory the walk could not open, keeps its evidence. The evidence
  of scopes that are no longer walked, such as a removed source, is forgotten only after a walk that
  covered every scope and completed in each.
- **Refresh where prefabs change.** The pass runs after a dependency drain that scanned something,
  after a watch rescan or a manual scan (over their roots), and after an extraction (over the
  workspace).
- **Migration.** The existing edges came from whole-file pairings, and no row says which prefab
  produced them. M28 files them as the evidence of a reserved scope, `legacy`. A source's legacy
  pairings are retired when a walk of that source completes, because its prefabs were just re-read;
  whatever is left goes with the first complete walk of every scope. A legacy pairing is taken to
  belong to its mesh's source, so a prefab in one source that painted a mesh from another loses that
  edge while its own source is unplugged, until that source is walked. The reharvest is tried once
  per process, so an unplugged drive does not turn every idle tick into a library walk. In-pack
  assets take the GUID their package manifest names, because an extracted pack has no `.meta`
  beside its files.

## Consequences

- Prefabs remain evidence, not assets. A first-class prefab (selected, imported and shown as one unit)
  is a separate schema/API/UI decision. It is parked until users need that, rather than a textured
  mesh.
- Nested prefab instances and variant overrides are counted and left unresolved. An override is a
  property change on another prefab's renderer, and reading it as a binding would be a guess.
- A mesh that several prefabs paint differently has several evidenced materials. Choosing one for a
  preview, deterministically and with the ambiguity shown, is S07's job.
- A material embedded in the model file names the model's own GUID, so no self-edge is kept.
- Evidence for a pack evicted from the workspace stays until the next walk of the workspace.
- Measured on the reference library, the reharvest kept 1,172 of the 1,228 whole-file pairings and
  added none. The 56 it dropped were 24 collision meshes, 15 particle emission-shape meshes, 11
  pairings across renderers (a candle painted with its flame's material) and 6 self-edges. A first
  walk of 1,298 prefabs took 1.6–5.6 s; a walk with nothing changed took 26–56 ms.
