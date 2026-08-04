<script lang="ts">
  import Card from "./Card.svelte";
  import Toolbar from "./Toolbar.svelte";
  import QuickLook from "./QuickLook.svelte";
  import AssetList from "./AssetList.svelte";
  import FoldersView from "./FoldersView.svelte";
  import { gridPage, DENSITIES, type GridPage, type LibraryStatus } from "./api";
  import { view } from "./nav.svelte";

  let {
    status,
    favoritesOnly = false,
    trashedOnly = false,
    title = "Library",
  }: {
    status: LibraryStatus | null;
    favoritesOnly?: boolean;
    trashedOnly?: boolean;
    title?: string;
  } = $props();

  let page = $state(0);
  let pageSize = $state(100);
  let result = $state<GridPage | null>(null);
  let error = $state<string | null>(null);
  let quickLook = $state<number | null>(null);

  /// Walk the *current page* rather than the whole result: paging inside the viewer would need
  /// to fetch and re-anchor, and the page is what the user can see behind the scrim anyway.
  function step(delta: number) {
    const items = result?.items ?? [];
    const i = items.findIndex((x) => x.asset_id === quickLook);
    if (i < 0) return;
    const next = items[(i + delta + items.length) % items.length];
    quickLook = next.asset_id;
    view.selected = next.asset_id;
  }

  function onkeydown(e: KeyboardEvent) {
    // Never steal keys from the search box.
    if (e.target instanceof HTMLInputElement) return;
    if (e.key === "Enter" && quickLook === null && view.selected !== null) {
      quickLook = view.selected;
      e.preventDefault();
    }
  }

  // Any change to what is being *asked for* resets to the first page. Staying on page 9 after
  // a query that matches two assets is how a grid ends up looking empty; sort order changes
  // reset too, since page 9 of a re-sorted list is a different set of assets entirely.
  let queryKey = $derived(
    `${view.tab} ${view.search} ${view.status} ${view.sortBy} ${view.sortAsc} ${trashedOnly}` +
      // The ⚑ filter belongs in the reset key for the same reason the search does: narrowing
      // the result while the pager sits on page 9 shows an empty grid.
      ` ${JSON.stringify(view.filter)} ${view.packageFilter} ${view.source}`,
  );
  let lastKey = "";
  $effect(() => {
    if (queryKey !== lastKey) {
      lastKey = queryKey;
      page = 0;
    }
    // Re-run after any write: the backend re-read the catalog, so this re-asks rather than
    // patching rows locally to match a guess about what changed.
    view.dataVersion;
    // Folders runs its own query; asking for a page of 100 cards nobody is looking at is a
    // whole-library sort per keystroke for nothing.
    if (view.viewMode === "folders") return;
    gridPage({
      tab: view.tab,
      search: view.search,
      status: view.status,
      sort_by: view.sortBy,
      sort_asc: view.sortAsc,
      favorites_only: favoritesOnly,
      trashed_only: trashedOnly,
      filter: view.filter,
      package: view.packageFilter,
      // Shared with the Folders view, where it means "which source am I inside". Same answer
      // either way: the grid shows one library's assets.
      source: view.source,
      page,
      page_size: pageSize,
    })
      .then((r) => {
        result = r;
        error = null;
      })
      .catch((e) => (error = String(e)));
  });

  let cardPx = $derived(DENSITIES.find((d) => d.id === view.density)?.px ?? 148);
</script>

<svelte:window on:keydown={onkeydown} />

<section class="screen">
  <header>
    <h1>{title}</h1>
    {#if status && !status.open}
      <span class="warn">no catalog — run a scan in the egui build first</span>
    {/if}
  </header>

  <Toolbar total={result?.total ?? 0} />

  {#if view.packageFilter}
    <!-- A filter you cannot see is a library that has quietly gone missing. -->
    <div class="chips">
      <button class="chip" onclick={() => (view.packageFilter = null)} data-testid="pack-chip">
        pack: {view.packageFilter} ✕
      </button>
    </div>
  {/if}

  <div class="body">
    {#if view.viewMode === "folders"}
      <FoldersView onopen={(id) => (quickLook = id)} />
    {:else if error}
      <p class="msg danger">{error}</p>
    {:else if !result}
      <p class="msg">Loading…</p>
    {:else if result.items.length === 0}
      <p class="msg" data-testid="empty">
        {#if view.search}
          Nothing matches “{view.search}”.
        {:else if view.filterCount > 0}
          No assets match those filters.
        {:else if favoritesOnly}
          No favourites yet — star an asset to pin it here.
        {:else if trashedOnly}
          The Trash is empty. Nothing here is deleted from disk — trashed assets keep their files.
        {:else}
          Nothing in this category.
        {/if}
      </p>
    {:else if view.viewMode === "list"}
      <AssetList
        items={result.items}
        selected={view.selected}
        onselect={(id) => (view.selected = id)}
        onopen={(id) => (quickLook = id)}
      />
    {:else}
      <div class="grid" style="--card: {cardPx}px">
        {#each result.items as item (item.asset_id)}
          <Card
            {item}
            selected={view.selected === item.asset_id}
            onselect={(id) => (view.selected = id)}
            onopen={(id) => (quickLook = id)}
          />
        {/each}
      </div>
    {/if}
  </div>

  {#if result && result.pages > 1 && view.viewMode !== "folders"}
    <footer class="pager">
      <button disabled={page === 0} onclick={() => (page = Math.max(0, page - 1))}>‹ Prev</button>
      <span class="num" data-testid="pager">Page {result.page + 1} of {result.pages}</span>
      <button disabled={page >= result.pages - 1} onclick={() => (page = page + 1)}>Next ›</button>
    </footer>
  {/if}

  {#if quickLook !== null}
    <QuickLook assetId={quickLook} onclose={() => (quickLook = null)} onstep={step} />
  {/if}
</section>

<style>
  .screen {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-width: 0;
  }
  header {
    display: flex;
    align-items: baseline;
    gap: var(--sp-3);
    padding: var(--sp-4) var(--sp-5) var(--sp-3);
  }
  h1 {
    font-size: var(--fs-2xl);
  }
  .warn {
    margin-left: auto;
    font-size: var(--fs-sm);
    color: var(--ls-warning);
  }

  .body {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: var(--sp-4) var(--sp-5);
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(var(--card), 1fr));
    gap: var(--sp-3);
  }
  .chips {
    flex: none;
    padding: var(--sp-2) var(--sp-5) 0;
  }
  .chip {
    padding: 2px var(--sp-2);
    border: 1px solid var(--ls-gold);
    border-radius: var(--r-pill);
    background: none;
    color: var(--ls-gold);
    font: inherit;
    font-size: var(--fs-sm);
    cursor: pointer;
  }

  .msg {
    color: var(--ls-text-dim);
  }
  .msg.danger {
    color: var(--ls-danger);
  }

  .pager {
    flex: none;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: var(--sp-4);
    padding: var(--sp-2);
    border-top: 1px solid var(--ls-border);
    font-size: var(--fs-sm);
    color: var(--ls-text-muted);
  }
  .pager button {
    padding: 3px var(--sp-3);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-md);
  }
  .pager button:hover:not(:disabled) {
    border-color: var(--ls-gold);
    color: var(--ls-text);
  }
  .pager button:disabled {
    opacity: 0.4;
  }
</style>
