<script lang="ts">
  import Icon from "./Icon.svelte";
  import Filters from "./Filters.svelte";
  import { SORTS, STATUSES, DENSITIES, recentOps, undo, type OpLogRow } from "./api";
  import { view } from "./nav.svelte";

  let { total, favorites = 0 }: { total: number; favorites?: number } = $props();

  // ---- Undo (L5) ---------------------------------------------------------
  // The newest undo-log row. The effect re-fetches on every `view.dataVersion` change — i.e. after
  // any catalog write — so the button tracks the log's head, and an undo itself bumps the version
  // to advance to the next op (a reverted op is removed from the log).
  let lastOp = $state<OpLogRow | null>(null);
  let undoBusy = $state(false);
  $effect(() => {
    view.dataVersion;
    let stale = false;
    recentOps(1)
      .then((rows) => {
        if (!stale) lastOp = rows[0] ?? null;
      })
      .catch(() => {});
    return () => {
      stale = true;
    };
  });

  /** The op kinds that actually land in the log (catalog.rs), as plain words for the tooltip. */
  function opLabel(kind: string): string {
    switch (kind) {
      case "set_tags":
      case "bulk_tag":
        return "tag edit";
      case "set_trashed":
        return "move to trash";
      case "set_favorite":
        return "favorite change";
      case "create_collection":
        return "collection create";
      default:
        return kind.replace(/_/g, " ");
    }
  }

  async function onUndo() {
    const op = lastOp;
    if (!op || undoBusy) return;
    undoBusy = true;
    try {
      await undo(op.id);
      // The reverted op is gone from the log; bumping the version re-reads the head and re-asks
      // every screen, so the undo lands visibly.
      view.dataVersion += 1;
    } catch {
      // Leave dataVersion untouched — nothing changed, and the log still holds the op.
    } finally {
      undoBusy = false;
    }
  }
</script>

<svelte:window
  onclick={(e) => {
    // Click-away closes it. Scoped to the wrapper rather than a full-screen scrim: a filter
    // popover is not modal, and a scrim would swallow the first click on every card behind it.
    if (view.filtersOpen && !(e.target as HTMLElement).closest(".filters")) view.filtersOpen = false;
  }}
/>

<div class="toolbar">
  <div class="group" role="group" aria-label="Status">
    {#each STATUSES as s (s.id)}
      <button class="chip" class:on={view.status === s.id} onclick={() => (view.status = s.id)}>
        {s.label}
      </button>
    {/each}
  </div>

  {#if view.browse !== "folders"}
    <!-- Result label (FEATURES §1.3). Search keeps the plain form: AI-ranked ✦ results need
         the semantic search the shell does not have yet, and a label that claims ranking that
         never happened is the kind of fake the repo does not ship. -->
    <span class="count num" data-testid="result-count">
      {#if view.search}
        {total.toLocaleString()} matches for “{view.search}”
      {:else}
        {total.toLocaleString()} assets · {view.source ?? "All libraries"}
      {/if}
    </span>
  {/if}

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

    <div class="filters">
      <button
        class="flag"
        class:on={view.route === "favorites"}
        onclick={() => (view.route === "favorites" ? view.go("library") : view.go("favorites"))}
        aria-pressed={view.route === "favorites"}
        data-testid="fav-toggle"
        title="Show only starred assets"
      >
        <Icon name="ls-nav-favorites" size={14} />
        Favorites
        {#if favorites > 0}
          <span class="badge num" data-testid="fav-badge">{favorites}</span>
        {/if}
      </button>

      <button
        class="flag"
        class:on={view.filtersOpen || view.filterCount > 0}
        onclick={() => (view.filtersOpen = !view.filtersOpen)}
        aria-expanded={view.filtersOpen}
        data-testid="filters-button"
      >
        ⚑ Filters
        {#if view.filterCount > 0}
          <span class="badge num" data-testid="filters-badge">{view.filterCount}</span>
        {/if}
      </button>
      {#if view.filtersOpen}
        <Filters onclose={() => (view.filtersOpen = false)} />
      {/if}
    </div>

    <div class="group" role="group" aria-label="View mode">
      <button
        class="icon-btn"
        class:on={view.browse === "flat" && view.rowStyle === "grid"}
        onclick={() => {
          view.browse = "flat";
          view.rowStyle = "grid";
        }}
        title="Grid"
        aria-label="Grid view"
      >
        <Icon name="ls-view-grid" size={16} />
      </button>
      <button
        class="icon-btn"
        class:on={view.browse === "flat" && view.rowStyle === "list"}
        onclick={() => {
          view.browse = "flat";
          view.rowStyle = "list";
        }}
        title="List"
        aria-label="List view"
      >
        <Icon name="ls-view-list" size={16} />
      </button>
      <button
        class="icon-btn"
        class:on={view.browse === "folders"}
        onclick={() => {
          view.browse = "folders";
          view.rowStyle = "grid";
        }}
        title="Folders"
        aria-label="Folders view"
      >
        <Icon name="ls-nav-packages" size={16} />
      </button>
    </div>

    {#if view.rowStyle === "grid"}
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
      class:on={!!lastOp}
      disabled={!lastOp || undoBusy}
      onclick={onUndo}
      title={lastOp ? `Undo ${opLabel(lastOp.op_kind)}` : "Nothing to undo"}
      aria-label={lastOp ? `Undo ${opLabel(lastOp.op_kind)}` : "Nothing to undo"}
      data-testid="undo-button"
    >
      ↶
    </button>

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

  .filters {
    position: relative;
  }
  .flag {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-1);
    height: var(--control-h);
    padding: 0 var(--sp-2);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-md);
    color: var(--ls-text-dim);
    font-size: var(--fs-sm);
  }
  .flag:hover,
  .flag.on {
    border-color: var(--ls-gold);
    color: var(--ls-text);
  }
  .badge {
    min-width: 16px;
    padding: 0 4px;
    border-radius: var(--r-pill);
    background: var(--ls-gold);
    color: var(--ls-bg);
    font-size: var(--fs-xs);
    text-align: center;
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
  /* The undo button dims hard when the log is empty or an undo is in flight; hover must not
     re-brighten a button that cannot act. */
  .icon-btn:disabled {
    opacity: 0.25;
    cursor: default;
  }
  .icon-btn:disabled:hover {
    opacity: 0.25;
  }
</style>
