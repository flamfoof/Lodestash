<script lang="ts">
  import { LinkRow, SectionHeading, Text } from "../../ui";
  import type { AssetLink } from "../asset/model";
  import { linksCaption, linkState } from "../asset/present";

  /**
   * LINKED FILES (UI-SPEC §13.6): the asset's dependency edges, one row each, with where each was
   * found — in the pack, elsewhere in the library, missing, or ignored — under a caption saying
   * what they were parsed from. A file found in another pack is a button that opens that asset;
   * a dependency inside the pack travels with it and has nothing to open.
   * Read-only here: the relink panel (⊘ Ignore / ↺ Restore) is its own task.
   */
  let {
    links,
    anchored = false,
    onopenasset,
  }: {
    links: readonly AssetLink[];
    anchored?: boolean;
    onopenasset?: (id: number) => void;
  } = $props();

  const TIP = {
    in_pack: "Resolved inside the pack",
    in_library: "Found in another pack — open it",
    missing: "The declared file was not found",
    ignored: "Ignored: it will not be exported",
  } as const;

  const missing = $derived(links.filter((l) => linkState(l) === "missing").length);
  const kindOf = (link: AssetLink) => (link.kind === "uses_material" ? "material" : link.kind === "uses_texture" ? "texture" : "other");
</script>

<section aria-label="Linked files">
  <SectionHeading text="LINKED FILES" caption={linksCaption(links)} />
  <div class="rows">
    {#each links as link, i (link.id)}
      {@const state = linkState(link)}
      {@const target = state === "in_library" ? link.to_asset_id : null}
      <LinkRow
        name={link.name}
        kind={kindOf(link)}
        {state}
        indent={link.depth === 1}
        title={TIP[state]}
        onclick={target === null ? undefined : () => onopenasset?.(target)}
        anchors={anchored && i === 0 ? { row: "inspector.link", name: "inspector.link.name", state: "inspector.link.state" } : {}}
      />
    {/each}
  </div>
  {#if missing > 0}
    <div class="warning"><Text variant="warning">{missing === 1 ? "A missing link is" : "Missing links are"} skipped on export.</Text></div>
  {/if}
</section>

<style>
  .rows {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }

  .warning {
    margin-top: var(--space-6);
  }
</style>
