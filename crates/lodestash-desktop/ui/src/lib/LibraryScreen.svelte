<script lang="ts">
  import Card from "./Card.svelte";
  import Toolbar from "./Toolbar.svelte";
  import QuickLook from "./QuickLook.svelte";
  import AssetList from "./AssetList.svelte";
  import FoldersView from "./FoldersView.svelte";
  import { gridPage, DENSITIES, bulkTag, type GridPage, type LibraryStatus } from "./api";
  import { view } from "./nav.svelte";

  // Right-click menu (FEATURES §1.1): position + target asset. Closes on any action, Escape,
  // click-away or a scroll.
  let menu = $state<{ x: number; y: number; id: number } | null>(null);
  function closeMenu() {
    menu = null;
  }
  function menuAction(action: "favorite" | "identify" | "trash") {
    const m = menu;
    if (!m) return;
    const favorite = result?.items.find((i) => i.asset_id === m.id)?.favorite ?? false;
    onmenuaction?.({ action, id: m.id, favorite });
    closeMenu();
  }

  let {
    status,
    favoritesOnly = false,
    trashedOnly = false,
    title = "Library",
    onmenuaction,
    onrestore,
  }: {
    status: LibraryStatus | null;
    favoritesOnly?: boolean;
    trashedOnly?: boolean;
    title?: string;
    onmenuaction?: (d: { action: string; id: number; favorite: boolean }) => void;
    /** Trash view: restore one row (UI-SPEC §10). */
    onrestore?: (id: number) => void;
  } = $props();

  let page = $state(0);
  let pageSize = $state(100);
  let result = $state<GridPage | null>(null);
  let error = $state<string | null>(null);
  let quickLook = $state<number | null>(null);
  /** Unidentified nudge (D8 / KANBAN B10d): ✕ hides it for the rest of the session. */
  let nudgeDismissed = $state(false);

  // ---- Bulk-bar tag edits (KANBAN B9d) -----------------------------------
  // The tag value input, in the default `free` namespace. `bulkBusy` guards the writer thread
  // against overlapping writes; the selection survives an error so the user can fix and retry.
  let tagValue = $state("");
  let bulkBusy = $state(false);
  let bulkError = $state<string | null>(null);

  async function bulkEdit(on: boolean) {
    const value = tagValue.trim();
    if (!value || view.selectedIds.length === 0 || bulkBusy) return;
    bulkBusy = true;
    bulkError = null;
    try {
      await bulkTag(view.selectedIds, "free", value, on);
      view.clearSelection();
      tagValue = "";
      // The writer re-read the catalog; re-ask every screen so the added/removed chips land.
      view.dataVersion += 1;
    } catch (e) {
      bulkError = String(e);
    } finally {
      bulkBusy = false;
    }
  }

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
    // Never steal keys from the search box or a focused control.
    if (e.target instanceof HTMLInputElement) return;
    if (e.target instanceof HTMLTextAreaElement) return;
    if (e.target instanceof HTMLButtonElement || e.target instanceof HTMLSelectElement) return;
    // Space toggles Quick Look (UI-SPEC §0.4): opens on the selection, closes when open.
    if (e.code === "Space") {
      e.preventDefault();
      if (quickLook === null) {
        if (view.selected !== null) quickLook = view.selected;
      } else {
        quickLook = null;
      }
      return;
    }
    // Enter opens Quick Look on the selection (Space is the toggle above).
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
      ` ${JSON.stringify(view.filter)} ${view.packageFilter} ${view.collectionFilter?.id ?? ""} ${view.source}`,
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
    if (view.browse === "folders") return;
    gridPage({
      tab: view.tab,
      search: view.search,
      search_mode: view.searchMode,
      ids: [],
      status: view.status,
      sort_by: view.sortBy,
      sort_asc: view.sortAsc,
      favorites_only: favoritesOnly,
      trashed_only: trashedOnly,
      filter: view.filter,
      // The LINKS chip rides on `filter.links` in the store, but the backend field is
      // top-level on GridQuery — forward it here or serde silently drops it.
      links: (view.filter as { links?: number | null }).links ?? null,
      package: view.packageFilter,
      collection: view.collectionFilter?.id ?? null,
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

  <Toolbar total={result?.total ?? 0} favorites={status?.favorites ?? 0} />

  {#if view.browse === "flat" && !nudgeDismissed && (status?.unidentified ?? 0) > 0}
    <!-- Unidentified nudge (D8/B10d): flat browse only, session-dismissable, routes to Identify. -->
    <div class="nudge" role="status" data-testid="unidentified-banner">
      <span class="text">
        ✦ <span class="num">{status!.unidentified}</span> assets still unidentified — run Identify
      </span>
      <button class="go" onclick={() => view.go("identify")} data-testid="unidentified-go">
        Identify
      </button>
      <button
        class="x"
        aria-label="Dismiss unidentified nudge"
        onclick={() => (nudgeDismissed = true)}
        data-testid="unidentified-dismiss"
      >
        ✕
      </button>
    </div>
  {/if}

  {#if view.packageFilter || view.collectionFilter}
    <!-- A filter you cannot see is a library that has quietly gone missing. -->
    <div class="chips">
      {#if view.packageFilter}
        <button class="chip" onclick={() => (view.packageFilter = null)} data-testid="pack-chip">
          pack: {view.packageFilter} ✕
        </button>
      {/if}
      {#if view.collectionFilter}
        <button
          class="chip"
          onclick={() => (view.collectionFilter = null)}
          data-testid="collection-chip"
        >
          collection: {view.collectionFilter.name} ✕
        </button>
      {/if}
    </div>
  {/if}

  {#if view.selectedIds.length > 0}
    <!-- Bulk bar (B9d): appears whenever cards are multi-selected, above the scroll area so it
         stays put while the grid underneath pages and scrolls. Tags land in the `free` namespace. -->
    <div class="bulkbar" role="toolbar" aria-label="Bulk tag">
      <span class="count num" data-testid="bulk-count">{view.selectedIds.length} selected</span>
      <input
        class="tagval"
        placeholder="tag · free"
        aria-label="Tag value"
        bind:value={tagValue}
        data-testid="bulk-tag-input"
        onkeydown={(e) => e.key === "Enter" && bulkEdit(true)}
      />
      <button
        class="gold"
        onclick={() => bulkEdit(true)}
        disabled={bulkBusy || !tagValue.trim()}
        data-testid="bulk-add"
      >
        + Add
      </button>
      <button
        onclick={() => bulkEdit(false)}
        disabled={bulkBusy || !tagValue.trim()}
        data-testid="bulk-remove"
      >
        − Remove
      </button>
      <button onclick={() => view.clearSelection()} disabled={bulkBusy} data-testid="bulk-clear">
        Clear
      </button>
      {#if bulkError}
        <span class="msg danger" data-testid="bulk-error">{bulkError}</span>
      {/if}
    </div>
  {/if}

  <div class="body">
    {#if view.browse === "folders"}
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
    {:else if view.rowStyle === "list"}
      <AssetList
        items={result.items}
        selected={view.selected}
        onselect={(id) => (view.selected = id)}
        onopen={(id) => (quickLook = id)}
        onmenu={(e, id) => (menu = { x: e.clientX, y: e.clientY, id })}
        {trashedOnly}
        onrestore={(id) => onrestore?.(id)}
      />
    {:else}
      <div class="grid" style="--card: {cardPx}px">
        {#each result.items as item (item.asset_id)}
          <!-- The multi-select box (B9d) is a sibling overlaid on the card's artwork top-left —
               the Card root is a <button>, so nesting a real control inside it would be invalid
               HTML. Hover reveals it; a picked id keeps it lit. Clicks hit the box, never the card,
               so single-click / Quick Look flows below are untouched. -->
          <div class="cell">
            <Card
              {item}
              selected={view.selected === item.asset_id}
              onselect={(id) => (view.selected = id)}
              onopen={(id) => (quickLook = id)}
              onmenu={(e, id) => (menu = { x: e.clientX, y: e.clientY, id })}
              onidentified={() => (view.dataVersion += 1)}
            />
            <span
              class="sel"
              class:on={view.selectedIds.includes(item.asset_id)}
              role="checkbox"
              tabindex="0"
              aria-checked={view.selectedIds.includes(item.asset_id)}
              aria-label={`Select ${item.name}`}
              data-testid="card-select"
              onclick={(e) => {
                e.stopPropagation();
                view.toggleSelect(item.asset_id);
              }}
              onkeydown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  e.stopPropagation();
                  view.toggleSelect(item.asset_id);
                }
              }}
            >✓</span>
          </div>
        {/each}
      </div>
    {/if}
  </div>

  {#if result && result.pages > 1 && view.browse !== "folders"}
    <footer class="pager">
      <button disabled={page === 0} onclick={() => (page = Math.max(0, page - 1))}>‹ Prev</button>
      <span class="num" data-testid="pager">Page {result.page + 1} of {result.pages}</span>
      <button disabled={page >= result.pages - 1} onclick={() => (page = page + 1)}>Next ›</button>
    </footer>
  {/if}

  {#if quickLook !== null}
    <QuickLook assetId={quickLook} onclose={() => (quickLook = null)} onstep={step} />

    {#if menu}
      <!-- A full-screen transparent layer: one click closes; the menu sits above it at the
           cursor, clamped to the viewport. -->
      <div
        class="menu-backdrop"
        role="presentation"
        onclick={closeMenu}
        oncontextmenu={(e) => {
          e.preventDefault();
          closeMenu();
        }}
        onkeydown={(e) => e.key === "Escape" && closeMenu()}
      ></div>
      <div
        class="ctx-menu"
        role="menu"
        aria-label="Asset actions"
        style="left: {Math.min(menu.x, window.innerWidth - 180)}px; top: {Math.min(menu.y, window.innerHeight - 140)}px;"
      >
        <button role="menuitem" onclick={() => menu && ((quickLook = menu.id), closeMenu())}>
          Quick Look
        </button>
        <button role="menuitem" onclick={() => menuAction("favorite")}>Toggle Favorite</button>
        <button role="menuitem" onclick={() => menuAction("identify")}>Identify now</button>
        <button role="menuitem" class="danger" onclick={() => menuAction("trash")}>
          Move to Trash
        </button>
      </div>
    {/if}
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
  /* One grid cell wraps one card; the multi-select box positions against it, not the grid. */
  .cell {
    position: relative;
  }
  /* The multi-select box (B9d): a hover-revealed sibling pinned to the card artwork's top-left,
     which Card.svelte deliberately leaves clear. Mirrors the card's own hover-reveal rhythm. */
  .sel {
    position: absolute;
    top: var(--sp-2);
    left: var(--sp-2);
    z-index: 2;
    display: grid;
    place-items: center;
    width: 18px;
    height: 18px;
    font-size: var(--fs-xs);
    line-height: 1;
    color: var(--ls-gold);
    background: color-mix(in srgb, var(--ls-bg) 85%, transparent);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-sm);
    cursor: pointer;
    opacity: 0;
    transition: opacity 120ms ease;
  }
  .cell:hover .sel,
  .sel:focus-visible,
  .sel.on {
    opacity: 1;
  }
  .sel.on {
    border-color: var(--ls-gold);
    background: var(--ls-overlay);
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

  /* Unidentified nudge (D8/B10d): slim amber banner above the grid, flat browse only. */
  .nudge {
    flex: none;
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    flex-wrap: wrap;
    margin: 0 var(--sp-5) var(--sp-2);
    padding: var(--sp-1) var(--sp-3);
    background: color-mix(in srgb, var(--ls-surface-raised) 85%, var(--ls-warning) 15%);
    border: 1px solid color-mix(in srgb, var(--ls-warning) 45%, transparent);
    border-radius: var(--r-md);
    color: var(--ls-warning);
  }
  .nudge .text {
    flex: 1;
    min-width: 0;
    font-size: var(--fs-sm);
  }
  .nudge .num {
    color: var(--ls-gold-bright);
  }
  .nudge button {
    padding: 2px var(--sp-3);
    border: 1px solid transparent;
    border-radius: var(--r-md);
    font-size: var(--fs-sm);
  }
  .nudge button.go {
    border-color: color-mix(in srgb, var(--ls-warning) 55%, transparent);
    background: color-mix(in srgb, var(--ls-warning) 12%, transparent);
    color: var(--ls-warning);
  }
  .nudge button.go:hover {
    border-color: var(--ls-warning);
    color: var(--ls-gold-bright);
  }
  .nudge button.x {
    color: var(--ls-text-dim);
  }
  .nudge button.x:hover {
    color: var(--ls-warning);
  }

  .msg {
    color: var(--ls-text-dim);
  }
  .msg.danger {
    color: var(--ls-danger);
  }

  /* Bulk bar (B9d): a flex-none row above the scroll area, so it stays put while the grid
     underneath scrolls. Gold-bordered like the selection it reflects. */
  .bulkbar {
    flex: none;
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    flex-wrap: wrap;
    margin: 0 var(--sp-5) var(--sp-2);
    padding: var(--sp-2) var(--sp-3);
    background: var(--ls-surface-raised);
    border: 1px solid var(--ls-gold);
    border-radius: var(--r-md);
  }
  .bulkbar .count {
    font-size: var(--fs-sm);
    color: var(--ls-gold);
  }
  .bulkbar input.tagval {
    flex: 1;
    min-width: 140px;
    max-width: 240px;
    padding: 3px var(--sp-2);
    background: var(--ls-bg);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-sm);
    color: var(--ls-text);
    font: inherit;
    font-size: var(--fs-sm);
  }
  .bulkbar button {
    padding: 3px var(--sp-3);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-md);
    font: inherit;
    font-size: var(--fs-sm);
    color: var(--ls-text);
  }
  .bulkbar button:hover:not(:disabled) {
    border-color: var(--ls-gold);
    color: var(--ls-gold-bright);
  }
  .bulkbar button.gold {
    border-color: var(--ls-gold);
    color: var(--ls-gold-bright);
  }
  .bulkbar button:disabled {
    opacity: 0.45;
    cursor: default;
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
  .menu-backdrop {
    position: fixed;
    inset: 0;
    z-index: 90;
  }
  .ctx-menu {
    position: fixed;
    z-index: 91;
    min-width: 168px;
    padding: 4px;
    border: 1px solid var(--ls-border);
    border-radius: 8px;
    background: var(--ls-surface);
    box-shadow: 0 8px 28px rgb(0 0 0 / 0.35);
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .ctx-menu button {
    all: unset;
    cursor: default;
    padding: 7px 10px;
    border-radius: 5px;
    font: inherit;
    font-size: var(--fs-sm);
    color: var(--ls-text);
  }
  .ctx-menu button:hover {
    background: var(--ls-hover);
  }
  .ctx-menu button.danger {
    color: var(--ls-danger);
  }
</style>
