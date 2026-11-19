<script lang="ts">
  import { FilterChip, Input, SectionHeading, Text, TextLink } from "../../ui";
  import type { SizeThresholds } from "../asset/model";
  import { filterCount, NO_FILTERS, SIZE_TIERS, sizeRanges, toggleFilter, TRI_BUCKETS, type FilterGroup, type LibraryFilters, type TraitCount } from "./filters";

  /**
   * What the ⚑ Filters popover holds (UI-SPEC §2 item 3, FEATURES §1.4): three groups of toggles —
   * TRAIT (the library's tags, most used first, with a search that narrows the chips), TRIANGLES
   * and FILE SIZE — and Clear all. Whoever opens it owns the popover; this is its content.
   *
   * It holds no filter: `filters` comes in, and every toggle reports the whole next set
   * (`onchange`), so the grid, the count on the ⚑ button and the active-filter bar all read one
   * value. The only state here is the text in the trait search, which nobody else needs.
   */
  let {
    filters,
    traits,
    thresholds,
    anchored = false,
    onchange,
  }: {
    filters: LibraryFilters;
    /** The tags that can be filtered on, in the order to offer them. */
    traits: readonly TraitCount[];
    thresholds: SizeThresholds;
    /** Mark the parts the contract gate measures. */
    anchored?: boolean;
    onchange?: (next: LibraryFilters) => void;
  } = $props();

  let search = $state("");

  const needle = $derived(search.trim().toLowerCase());
  const shown = $derived(needle ? traits.filter((t) => t.name.toLowerCase().includes(needle)) : traits);
  const ranges = $derived(sizeRanges(thresholds));
  const active = $derived(filterCount(filters) > 0);

  const toggle = <G extends FilterGroup>(group: G, value: LibraryFilters[G][number]) => onchange?.(toggleFilter(filters, group, value));

  // The gate marks one chip that is on and one that is off in each group.
  function mark(base: string, on: boolean, seen: { on: boolean; off: boolean }) {
    const key = on ? "on" : "off";
    if (!anchored || seen[key]) return undefined;
    seen[key] = true;
    return on ? `${base}.on` : base;
  }
  const marks = $derived.by(() => {
    const groups = { trait: { on: false, off: false }, tris: { on: false, off: false }, size: { on: false, off: false } };
    return {
      trait: shown.map((t) => mark("filters.trait", filters.traits.includes(t.name), groups.trait)),
      tris: TRI_BUCKETS.map((b) => mark("filters.tris", filters.tris.includes(b.id), groups.tris)),
      size: SIZE_TIERS.map((tier) => mark("filters.size", filters.sizes.includes(tier), groups.size)),
    };
  });
  const anchor = (name: string) => (anchored ? name : undefined);
</script>

<div class="panel">
  <div class="title">
    <SectionHeading text="FILTER LIBRARY" flush textAttrs={{ "data-anchor": anchor("filters.title") }} />
    <TextLink tone={active ? "muted" : "dim"} hover="danger" size="10" disabled={!active} onclick={() => onchange?.(NO_FILTERS)} data-anchor={anchor("filters.clear")}>
      Clear all
    </TextLink>
  </div>

  <div class="trait-head">
    <SectionHeading text="TRAIT" size="sm" flush textAttrs={{ "data-anchor": anchor("filters.heading") }} />
    <span class="search">
      <Input label="Search traits" hideLabel size="sm" fill placeholder="search traits…" bind:value={search} data-anchor={anchor("filters.search")} />
    </span>
  </div>
  <div class="chips" role="group" aria-label="Trait">
    {#if shown.length === 0}
      <Text variant="nothing">No traits match.</Text>
    {/if}
    {#each shown as trait, i (trait.name)}
      <FilterChip
        label={trait.name}
        selected={filters.traits.includes(trait.name)}
        title="{trait.count} {trait.count === 1 ? 'asset' : 'assets'}"
        onclick={() => toggle("traits", trait.name)}
        data-anchor={marks.trait[i]}
      />
    {/each}
  </div>

  <SectionHeading text="TRIANGLES" size="sm" note="· models only" noteAttrs={{ "data-anchor": anchor("filters.heading.note") }} />
  <div class="chips" role="group" aria-label="Triangles">
    {#each TRI_BUCKETS as bucket, i (bucket.id)}
      <FilterChip kind="tris" label={bucket.label} selected={filters.tris.includes(bucket.id)} onclick={() => toggle("tris", bucket.id)} data-anchor={marks.tris[i]} />
    {/each}
  </div>

  <SectionHeading text="FILE SIZE" size="sm" />
  <div class="chips last" role="group" aria-label="File size">
    {#each SIZE_TIERS as tier, i (tier)}
      <FilterChip
        kind="size"
        {tier}
        label={tier}
        detail={ranges[tier]}
        selected={filters.sizes.includes(tier)}
        aria-label="Size {tier}, {ranges[tier]}"
        onclick={() => toggle("sizes", tier)}
        data-anchor={marks.size[i]}
        tierAttrs={{ "data-anchor": i === 0 ? anchor("filters.size.tier") : undefined }}
        detailAttrs={{ "data-anchor": i === 0 ? anchor("filters.size.range") : undefined }}
      />
    {/each}
  </div>
</div>

<style>
  /* Layout only: the popover's width and the rhythm of its groups. */
  .panel {
    width: var(--filters-w);
  }

  .title {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: var(--space-10);
  }

  .trait-head {
    display: flex;
    align-items: center;
    gap: var(--space-8);
    margin-bottom: var(--space-6);
  }

  .search {
    display: flex;
    width: var(--filters-search-w);
    margin-left: auto;
  }

  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-5);
    margin-bottom: var(--space-13);
  }

  .last {
    margin-bottom: 0;
  }
</style>
