<script lang="ts">
  /**
   * The ⚑ content-filter popover (UI-SPEC §2.3): TRAIT cloud, TRIANGLES buckets, FILE SIZE tiers.
   * Multi-select — OR within a group, AND across groups.
   *
   * Every chip is drawn from `filter_facets`, never from a list written out here: the ids are the
   * Rust enums' own serde names, so a chip cannot come to mean something the backend does not
   * parse. That also makes the size ranges honest — they are the user's own boundaries (§11.4),
   * not constants that go stale the moment someone edits them.
   */
  import { filterFacets, type Facets, type ContentFilter } from "./api";
  import { traitColor } from "./traits.svelte";
  import { view } from "./nav.svelte";

  let { onclose }: { onclose: () => void } = $props();

  let facets = $state<Facets | null>(null);
  let traitSearch = $state("");

  $effect(() => {
    filterFacets()
      .then((f) => (facets = f))
      .catch(() => (facets = { traits: [], tri_buckets: [], sizes: [] }));
  });

  const on = (group: keyof ContentFilter, id: string) => view.filter[group].includes(id);

  // The cloud is long on a tagged library; the search box is what makes it usable. Capped so an
  // untyped popover cannot render nine hundred chips.
  let shownTraits = $derived(
    (facets?.traits ?? [])
      .filter((t) => t.name.toLowerCase().includes(traitSearch.trim().toLowerCase()))
      .slice(0, 60),
  );

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      onclose();
      e.preventDefault();
    }
  }
</script>

<svelte:window on:keydown={onkeydown} />

<div class="popover" data-testid="filters-popover">
  <header>
    <h2>FILTER LIBRARY</h2>
    <button
      class="clear"
      onclick={() => view.clearFilter()}
      disabled={view.filterCount === 0}
      data-testid="filters-clear">Clear all</button
    >
  </header>

  {#if !facets}
    <p class="msg">Loading…</p>
  {:else}
    <section>
      <h3>Trait</h3>
      {#if facets.traits.length === 0}
        <p class="msg">
          Nothing is tagged yet — traits arrive when you run identification on an asset.
        </p>
      {:else}
        <input class="search" placeholder="Search traits" bind:value={traitSearch} />
        <div class="cloud">
          {#each shownTraits as t (t.name)}
            <button
              class="chip"
              class:on={on("traits", t.name)}
              style={traitColor.has(t.name) ? `--chip: var(${traitColor.get(t.name)})` : ""}
              onclick={() => view.toggleFacet("traits", t.name)}
            >
              {t.name}<span class="count num">{t.count}</span>
            </button>
          {/each}
        </div>
      {/if}
    </section>

    <section>
      <h3>Triangles</h3>
      <p class="hint">Models only — an asset with no mesh is excluded while any bucket is on.</p>
      <div class="cloud">
        {#each facets.tri_buckets as b (b.id)}
          <button
            class="chip tri"
            class:on={on("tri_buckets", b.id)}
            onclick={() => view.toggleFacet("tri_buckets", b.id)}
            data-testid="facet-tri-{b.id}"
          >
            {b.label}
          </button>
        {/each}
      </div>
    </section>

    <section>
      <h3>File size</h3>
      <div class="cloud">
        {#each facets.sizes as s (s.id)}
          <button
            class="chip"
            class:on={on("sizes", s.id)}
            onclick={() => view.toggleFacet("sizes", s.id)}
            data-testid="facet-size-{s.id}"
          >
            {s.label}
          </button>
        {/each}
      </div>
    </section>
  {/if}
</div>

<style>
  .popover {
    position: absolute;
    top: calc(100% + var(--sp-1));
    right: 0;
    z-index: 40;
    width: 328px;
    max-height: 70vh;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: var(--sp-3);
    padding: var(--sp-3);
    background: var(--ls-surface-raised, var(--ls-surface));
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-lg);
    box-shadow: 0 12px 32px rgb(0 0 0 / 0.45);
    text-align: left;
  }
  header {
    display: flex;
    align-items: baseline;
    gap: var(--sp-2);
  }
  h2 {
    font-size: var(--fs-xs);
    letter-spacing: 0.08em;
    color: var(--ls-text-dim);
    margin: 0;
  }
  h3 {
    font-size: var(--fs-xs);
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--ls-text-dim);
    margin: 0 0 var(--sp-2);
  }
  .clear {
    margin-left: auto;
    font-size: var(--fs-sm);
    color: var(--ls-gold);
  }
  .clear:disabled {
    opacity: 0.4;
    color: var(--ls-text-dim);
  }
  .msg,
  .hint {
    margin: 0 0 var(--sp-2);
    font-size: var(--fs-sm);
    color: var(--ls-text-dim);
  }
  .search {
    width: 100%;
    margin-bottom: var(--sp-2);
    padding: var(--sp-1) var(--sp-2);
    background: var(--ls-bg);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-md);
    color: var(--ls-text);
    font: inherit;
    font-size: var(--fs-sm);
  }
  .cloud {
    display: flex;
    flex-wrap: wrap;
    gap: var(--sp-1);
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-1);
    padding: 2px var(--sp-2);
    border: 1px solid var(--chip, var(--ls-border));
    border-radius: var(--r-pill);
    background: var(--ls-bg);
    color: var(--chip, var(--ls-text-dim));
    font: inherit;
    font-size: var(--fs-sm);
    cursor: pointer;
  }
  .chip:hover {
    color: var(--ls-text);
  }
  /* Selected state is arcane, per DESIGN-GUIDELINES §7 — gold is reserved for the primary action. */
  .chip.on {
    border-color: var(--ls-arcane, var(--ls-gold));
    color: var(--ls-text);
    background: color-mix(in srgb, var(--ls-arcane, var(--ls-gold)) 18%, transparent);
  }
  .chip.tri.on {
    border-color: var(--ls-type-model);
    background: color-mix(in srgb, var(--ls-type-model) 18%, transparent);
  }
  .count {
    color: var(--ls-text-muted);
    font-size: var(--fs-xs);
  }
</style>
