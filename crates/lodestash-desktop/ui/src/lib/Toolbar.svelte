<script lang="ts">
  import Icon from "./Icon.svelte";
  import { SORTS, STATUSES, DENSITIES } from "./api";
  import { view } from "./nav.svelte";

  let { total }: { total: number } = $props();
</script>

<div class="toolbar">
  <div class="group" role="group" aria-label="Status">
    {#each STATUSES as s (s.id)}
      <button class="chip" class:on={view.status === s.id} onclick={() => (view.status = s.id)}>
        {s.label}
      </button>
    {/each}
  </div>

  <span class="count num">{total.toLocaleString()}</span>

  <div class="right">
    <label class="sort">
      Sort
      <select bind:value={view.sortBy}>
        {#each SORTS as s (s.id)}
          <option value={s.id}>{s.label}</option>
        {/each}
      </select>
    </label>
    <button
      class="dir"
      onclick={() => (view.sortAsc = !view.sortAsc)}
      title={view.sortAsc ? "Ascending" : "Descending"}
      aria-label={view.sortAsc ? "Ascending" : "Descending"}
    >
      {view.sortAsc ? "↑" : "↓"}
    </button>

    <div class="group" role="group" aria-label="View mode">
      <button
        class="icon-btn"
        class:on={view.viewMode === "grid"}
        onclick={() => (view.viewMode = "grid")}
        title="Grid"
        aria-label="Grid view"
      >
        <Icon name="ls-view-grid" size={16} />
      </button>
      <button
        class="icon-btn"
        class:on={view.viewMode === "list"}
        onclick={() => (view.viewMode = "list")}
        title="List"
        aria-label="List view"
      >
        <Icon name="ls-view-list" size={16} />
      </button>
    </div>

    {#if view.viewMode === "grid"}
    <div class="group" role="group" aria-label="Density">
      {#each DENSITIES as d (d.id)}
        <button
          class="icon-btn"
          class:on={view.density === d.id}
          onclick={() => (view.density = d.id)}
          title={d.label}
          aria-label={d.label}
        >
          <Icon name={d.icon} size={16} />
        </button>
      {/each}
    </div>
    {/if}

    <button
      class="icon-btn"
      class:on={view.inspectorOpen}
      onclick={() => (view.inspectorOpen = !view.inspectorOpen)}
      title="Inspector"
      aria-label="Toggle inspector"
    >
      <Icon name="ls-action-filters" size={16} />
    </button>
  </div>
</div>

<style>
  .toolbar {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    padding: var(--sp-2) var(--sp-5);
    border-bottom: 1px solid var(--ls-border);
    background: var(--ls-surface);
  }
  .group {
    display: flex;
    gap: 2px;
    background: var(--ls-bg);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-md);
    padding: 2px;
  }
  .chip {
    padding: 2px var(--sp-3);
    border-radius: var(--r-sm);
    font-size: var(--fs-sm);
    color: var(--ls-text-dim);
  }
  .chip:hover {
    color: var(--ls-text);
  }
  .chip.on {
    background: var(--ls-overlay);
    color: var(--ls-text);
  }

  .count {
    font-size: var(--fs-sm);
    color: var(--ls-text-dim);
  }
  .right {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: var(--sp-2);
  }

  .sort {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    font-size: var(--fs-sm);
    color: var(--ls-text-dim);
  }
  select {
    height: var(--control-h);
    background: var(--ls-bg);
    color: var(--ls-text);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-md);
    padding: 0 var(--sp-2);
    font: inherit;
  }
  .dir {
    width: var(--control-h);
    height: var(--control-h);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-md);
    color: var(--ls-text-muted);
  }
  .dir:hover {
    border-color: var(--ls-gold);
    color: var(--ls-text);
  }

  .icon-btn {
    display: grid;
    place-items: center;
    width: 26px;
    height: 26px;
    border-radius: var(--r-sm);
    opacity: 0.6;
  }
  .icon-btn:hover {
    opacity: 1;
  }
  .icon-btn.on {
    background: var(--ls-overlay);
    opacity: 1;
  }
</style>
