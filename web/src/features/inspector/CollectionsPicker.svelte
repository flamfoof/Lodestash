<script lang="ts">
  import { Chip, SectionHeading } from "../../ui";
  import type { Collection } from "../asset/model";

  /**
   * COLLECTIONS (UI-SPEC §13.7): one toggle chip per collection — "+name" to add the asset,
   * "✓name" (gold) when it is in. Membership is read from the asset every time (`memberOf`); a
   * toggle reports what was asked (`ontoggle`) and waits to be told the result.
   */
  let {
    collections,
    memberOf,
    anchored = false,
    ontoggle,
  }: {
    collections: readonly Collection[];
    /** Ids of the collections the asset is in. */
    memberOf: readonly number[];
    anchored?: boolean;
    ontoggle?: (id: number, on: boolean) => void;
  } = $props();
</script>

<section aria-label="Collections">
  <SectionHeading text="COLLECTIONS" />
  <div class="chips">
    {#each collections as collection, i (collection.id)}
      {@const on = memberOf.includes(collection.id)}
      <Chip
        tone="toggle"
        size="md"
        label="{on ? '✓' : '+'}{collection.name}"
        selected={on}
        aria-label="{on ? 'Remove from' : 'Add to'} {collection.name}"
        onclick={() => ontoggle?.(collection.id, !on)}
        data-anchor={anchored && i === 0 ? "inspector.collection" : undefined}
      />
    {/each}
  </div>
</section>

<style>
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-5);
  }
</style>
