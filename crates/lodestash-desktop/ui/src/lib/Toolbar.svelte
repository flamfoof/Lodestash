<script lang="ts">
  import Icon from "./Icon.svelte";
  import Filters from "./Filters.svelte";
  import { SORTS, DENSITIES, recentOps, redoOps, redo, undo, type OpLogRow } from "./api";
  import { view } from "./nav.svelte";

  let { total, favorites = 0 }: { total: number; favorites?: number } = $props();

  // ---- Undo (L5) ---------------------------------------------------------
  // The newest undo-log row. The effect re-fetches on every `view.dataVersion` change — i.e. after
  // any catalog write — so the button tracks the log's head, and an undo itself bumps the version
  // to advance to the next op (a reverted op is removed from the log).
  let lastOp = $state<OpLogRow | null>(null);
  let undoBusy = $state(false);
  /**
   * AI results are ranked by match (UI-SPEC §2.2), so sorting them by anything else is not a
   * sort — it is discarding the ranking. `searchMode` alone is not enough: escalating and then
   * clearing the box leaves the mode set with nothing ranked to protect.
   */
  let ranked = $derived(view.searchMode === "ai" && view.search.trim() !== "");

  /** Step 4 of the ladder: below 960px the sort controls live behind this. */
  let overflowOpen = $state(false);
  let redoHead = $state<OpLogRow | null>(null);
  let redoBusy = $state(false);
  $effect(() => {
    view.dataVersion;
    let stale = false;
    recentOps(1)
      .then((rows) => {
        if (!stale) lastOp = rows[0] ?? null;
      })
      .catch(() => {});
    redoOps(1)
      .then((rows) => {
        if (!stale) redoHead = rows[0] ?? null;
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

  /** Redo the newest undone op (UI-SPEC §2 Shift-Z). */
  async function onRedo() {
    const op = redoHead;
    if (!op || redoBusy) return;
    redoBusy = true;
    try {
      await redo(op.id);
      view.dataVersion += 1;
    } catch {
      // Nothing changed; the redo stack still holds the op.
    } finally {
      redoBusy = false;
    }
  }

  /** Ctrl/Cmd-Z undo, Ctrl/Cmd-Shift-Z / Ctrl/Cmd-Y redo (UI-SPEC §2). Never steals from an input. */
  function onkeydown(e: KeyboardEvent) {
    if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) return;
    const mod = e.ctrlKey || e.metaKey;
    if (mod && e.key.toLowerCase() === "z" && !e.shiftKey) {
      onUndo();
    } else if (mod && (e.key.toLowerCase() === "y" || (e.key.toLowerCase() === "z" && e.shiftKey))) {
      onRedo();
    }
  }
</script>

<svelte:window
  onclick={(e) => {
    // Click-away closes it. Scoped to the wrapper rather than a full-screen scrim: a filter
    // popover is not modal, and a scrim would swallow the first click on every card behind it.
    if (view.filtersOpen && !(e.target as HTMLElement).closest(".filters")) view.filtersOpen = false;
  }}
  on:keydown={onkeydown}
/>

<div class="toolbar">
  <!-- The STATUS filter is the sidebar's facet group (UI-SPEC §0.2.2), and UI-SPEC §2.1's result
       header row does not list it. Rendering it in both places gave one piece of state two
       controls that had to be kept in sync by eye. The rail owns it. -->
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
    <!-- UI-SPEC §2.1: the keyboard hint, dim, hidden under 1200px. Three bindings nobody
         discovers by looking at the grid; a line of dim text is cheaper than a help screen. -->
    <span class="hint">Space quick-looks · ⤢ full size · ⟡ stage</span>
  {/if}

<!-- One definition, rendered in the toolbar above 960px and inside the ⋯ popover below it. The
     ladder says *move* chrome behind a gesture, never remove it — a sort you cannot reach on a
     narrow window is a worse answer than a sort that costs one click. -->
{#snippet sortControls()}
  <!-- Inert while AI results are on screen (UI-SPEC §2.2: *"sort is disabled while AI-ranked
       results are shown"*). It was live until 2026-08-26, and picking Size or Name there threw the
       ranking away without saying so: `page_with_ranks` only consults the ranks under
       `SortBy::Relevance`, so the cards kept their `✦ NN%` badges while sitting in an order those
       numbers no longer described. A percentage in the wrong order is worse than no percentage. -->
  <label class="sort" class:inert={ranked}>
    Sort
    <select
      bind:value={view.sortBy}
      disabled={ranked}
      title={ranked ? "AI results are ranked by match — clear the search to sort" : "Sort the grid"}
    >
      {#each SORTS as s (s.id)}
        <option value={s.id}>{s.label}</option>
      {/each}
    </select>
  </label>
  <button
    class="dir"
    disabled={ranked}
    onclick={() => (view.sortAsc = !view.sortAsc)}
    title={ranked
      ? "AI results are ranked by match — clear the search to sort"
      : view.sortAsc
        ? "Ascending"
        : "Descending"}
    aria-label={view.sortAsc ? "Ascending" : "Descending"}
  >
    {view.sortAsc ? "↑" : "↓"}
  </button>
{/snippet}

  <div class="right">
    <span class="sort-inline">{@render sortControls()}</span>

    <div class="overflow">
      <button
        class="flag more"
        aria-expanded={overflowOpen}
        aria-label="More controls"
        onclick={() => (overflowOpen = !overflowOpen)}
        data-testid="toolbar-overflow"
      >⋯</button>
      {#if overflowOpen}
        <button class="scrim" aria-label="Close" onclick={() => (overflowOpen = false)}></button>
        <div class="pop" data-testid="toolbar-overflow-pop">{@render sortControls()}</div>
      {/if}
    </div>

    <div class="filters">
      <button
        class="flag"
        class:on={view.route === "favorites"}
        onclick={() => (view.route === "favorites" ? view.go("library") : view.go("favorites"))}
        aria-pressed={view.route === "favorites"}
        data-testid="fav-toggle"
        title="Show only starred assets"
      >
        <Icon name="ls-nav-favorites" size={20} />
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
        <Icon name="ls-view-grid" size={20} />
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
        <Icon name="ls-view-list" size={20} />
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
        <Icon name="ls-nav-packages" size={20} />
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
          <Icon name={d.icon} size={20} />
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
      class:on={!!redoHead}
      disabled={!redoHead || redoBusy}
      onclick={onRedo}
      title={redoHead ? `Redo ${opLabel(redoHead.op_kind)}` : "Nothing to redo"}
      aria-label={redoHead ? `Redo ${opLabel(redoHead.op_kind)}` : "Nothing to redo"}
      data-testid="redo-button"
    >
      ↷
    </button>
    <button
      class="icon-btn"
      class:on={view.inspectorOpen}
      onclick={() => (view.inspectorOpen = !view.inspectorOpen)}
      title="Inspector"
      aria-label="Toggle inspector"
    >
      <Icon name="ls-action-filters" size={20} />
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
  .count {
    font-size: var(--fs-sm);
    color: var(--ls-text-dim);
  }
  .hint {
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
    opacity: 0.7;
    white-space: nowrap;
  }
  /* Step 1 of the ladder (DESIGN-GUIDELINES §4): the keyboard hint is the only thing on screen
     that teaches nothing you cannot discover, so it goes first — and 1200px was already too late. */
  @media (max-width: 1400px) {
    .hint {
      display: none;
    }
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
  /* Disabled sort: dimmed and un-clickable, but still readable and still focusable-looking —
     the user has to be able to see *which* sort is in force while the ranking holds it. */
  .sort.inert,
  .sort select:disabled,
  .dir:disabled {
    color: var(--ls-text-dim);
    cursor: default;
  }
  .dir:disabled:hover {
    border-color: var(--ls-border);
  }
  .icon-btn:disabled {
    opacity: 0.25;
    cursor: default;
  }
  .icon-btn:disabled:hover {
    opacity: 0.25;
  }

  /*
   * Step 4 of the responsive ladder (DESIGN-GUIDELINES §4). Written here rather than in the global
   * sheet because these rules have to beat this component's own scoped ones — a bare `.toolbar
   * .sort` in `app.css` ties on specificity and loses on source order, which is how the first
   * version of this step did nothing at all at every width.
   *
   * ★, ⚑ and the view toggle never fold: they change *what you are looking at*, and the
   * active-filter bar is what tells you they did.
   */
  .overflow {
    position: relative;
    display: none;
  }
  .more {
    font-size: var(--fs-lg);
    line-height: 1;
  }
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 20;
    background: none;
    border: 0;
    cursor: default;
  }
  .pop {
    position: absolute;
    right: 0;
    top: calc(var(--control-h) + 4px);
    z-index: 21;
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-2);
    background: var(--ls-overlay);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-md);
    box-shadow: var(--el-popover-shadow);
  }

  @media (max-width: 959px) {
    .sort-inline {
      display: none;
    }
    .overflow {
      display: block;
    }
  }
  @media (min-width: 960px) {
    .sort-inline {
      display: contents;
    }
  }
</style>
