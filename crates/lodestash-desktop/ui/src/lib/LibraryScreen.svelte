<script lang="ts">
  import Card from "./Card.svelte";
  import Toolbar from "./Toolbar.svelte";
  import FilterBar from "./FilterBar.svelte";
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
  function menuAction(action: "favorite" | "identify" | "import" | "trash") {
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
  <!-- UI-SPEC §2.1 composes the Library top-down as nudge → result header row → grid, with no
       screen title: the rail already says where you are, and the heading costs ~56px of card
       space on every screen. Favorites and Trash keep theirs — FEATURES §4.7 specifies that
       header, and it is the only thing distinguishing them from the Library they filter. -->
  {#if title !== "Library" || (status && !status.open)}
    <header>
      {#if title !== "Library"}<h1>{title}</h1>{/if}
      {#if status && !status.open}
        <span class="warn">no catalog — run a scan in the egui build first</span>
      {/if}
    </header>
  {/if}

  <Toolbar total={result?.total ?? 0} favorites={status?.favorites ?? 0} />
  <!-- UI-SPEC §2 item 7: renders directly under the toolbar, and only when something is filtered. -->
  <FilterBar shown={result?.total ?? 0} total={status?.total_assets ?? 0} />

  {#if view.searchMode === "ai"}
    <!-- §0.1.3: while a ranked result is shown the mode is a read-out, with a way back to the
         substring result for the same query. -->
    <div class="ranked" data-testid="ranked-header">
      <span class="ai">✦ <span class="num">{(result?.total ?? 0).toLocaleString()}</span> ranked matches</span>
      <button class="back" onclick={() => view.backToFilter()} data-testid="back-to-filter">
        ← back to name filter
      </button>
    </div>
  {/if}

  {#if view.browse === "flat" && !nudgeDismissed && (status?.unidentified ?? 0) > 0}
    <!-- Unidentified nudge (D8/B10d): flat browse only, session-dismissable, routes to Identify. -->
    <div class="nudge" role="status" data-testid="unidentified-banner">
      <span class="text">
        <span class="q num" aria-hidden="true">?</span>
        <span class="num">{status!.unidentified.toLocaleString()}</span> assets are Unidentified —
        invisible to AI search until appraised.
      </span>
      <button class="go" onclick={() => view.go("identify")} data-testid="unidentified-go">
        Open Identify mode →
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
      <!-- UI-SPEC §2.6: a filtered-empty result needs a way *out*, not a screen's teaching copy.
           When there is a query, the way out is the engine that can actually answer a description
           (ADR-0007) — this is the state the old "Nothing matches" dead-ended in. -->
      <div class="empty-state" data-testid="empty">
        {#if view.search && view.searchMode !== "ai"}
          <p class="msg">No name or trait contains “{view.search}”.</p>
          <p class="hint">Which is expected, if that is a description rather than a filename.</p>
          <button class="ask-big" onclick={() => view.askAi()} data-testid="empty-ask-ai">
            ✦ Search meaning instead
          </button>
          {#if (status?.unidentified ?? 0) > 0}
            <p class="hint">
              <span class="num">{(status?.unidentified ?? 0).toLocaleString()}</span> assets are still
              Unidentified and cannot be reached this way —
              <button class="link" onclick={() => view.go("identify")}>appraise them</button>.
            </p>
          {/if}
        {:else if view.search}
          <p class="msg">Nothing close in your hoard for “{view.search}”.</p>
          <button class="link" onclick={() => view.backToFilter()}>← back to name filter</button>
        {:else if view.filterCount > 0}
          <p class="msg">No assets match those filters.</p>
        {:else if favoritesOnly}
          <p class="msg">No favourites yet — star an asset to pin it here.</p>
        {:else if trashedOnly}
          <p class="msg">
            The Trash is empty. Nothing here is deleted from disk — trashed assets keep their files.
          </p>
        {:else}
          <p class="msg">Nothing in this category.</p>
        {/if}
      </div>
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
      <div class="grid" style="--card: {cardPx}px" data-testid="grid">
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
              onfavorite={(id, favorite) => onmenuaction?.({ action: "favorite", id, favorite })}
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

    <!-- UI-SPEC §0.1.3: the escalation is pinned under the filtered results and stays there the
         whole time a substring query is showing, so the AI is never a mode you had to predict. It
         is not shown for an empty result — §2.6's empty state already offers it as the primary
         action, and two offers of the same thing on one screen is one too many. -->
    {#if view.search.trim() && view.searchMode !== "ai" && result && result.items.length > 0}
      <button class="escalate" onclick={() => view.askAi()} data-testid="escalate">
        <span class="mark">✦</span>
        <span class="txt">
          Ask AI for <span class="q">»{view.search.trim()}«</span> — searches meaning, not names
        </span>
        <span class="key">⏎</span>
      </button>
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
        <button role="menuitem" onclick={() => menuAction("import")}>⇢ Add to project</button>
        <button role="menuitem" class="danger" onclick={() => menuAction("trash")}>
          Move to Trash
        </button>
      </div>
    {/if}
  {/if}
</section>

<style>
  /* ── D1: one search box (ADR-0007) ────────────────────────────────────────────────────────── */
  .ranked {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    padding: var(--sp-2) var(--sp-5);
    border-bottom: 1px solid var(--ls-border);
  }
  .ranked .ai {
    font-size: var(--fs-sm);
    color: var(--ls-arcane);
  }
  .ranked .back,
  .link {
    background: none;
    border: 0;
    padding: 0;
    font: inherit;
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
    border-bottom: 1px dotted var(--ls-border-strong);
    cursor: pointer;
  }
  .ranked .back {
    margin-left: auto;
  }
  .ranked .back:hover,
  .link:hover {
    color: var(--ls-gold);
    border-bottom-color: var(--ls-gold);
  }

  .escalate {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    width: calc(100% - var(--sp-5) * 2);
    margin: var(--sp-3) var(--sp-5) var(--sp-4);
    padding: var(--sp-2) var(--sp-3);
    text-align: left;
    font: inherit;
    font-size: var(--fs-sm);
    color: var(--ls-text);
    background: color-mix(in srgb, var(--ls-arcane) 7%, transparent);
    border: 1px solid color-mix(in srgb, var(--ls-arcane) 35%, transparent);
    border-radius: var(--r-md);
    cursor: pointer;
  }
  .escalate:hover {
    border-color: var(--ls-arcane);
    background: color-mix(in srgb, var(--ls-arcane) 12%, transparent);
  }
  .escalate .mark {
    color: var(--ls-arcane);
  }
  .escalate .txt {
    flex: 1;
    min-width: 0;
  }
  .escalate .q {
    color: var(--ls-arcane);
  }
  .escalate .key {
    flex: none;
    padding: 1px 6px;
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-sm);
  }

  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-6) var(--sp-5);
    text-align: center;
  }
  .empty-state .hint {
    margin: 0;
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
  }
  .ask-big {
    margin-top: var(--sp-1);
    height: 34px;
    padding: 0 var(--sp-4);
    font: inherit;
    font-size: var(--fs-sm);
    color: var(--ls-arcane);
    background: none;
    border: 1px solid var(--ls-arcane);
    border-radius: var(--r-md);
    cursor: pointer;
  }
  .ask-big:hover {
    color: var(--ls-arcane-bright);
    box-shadow: 0 0 12px color-mix(in srgb, var(--ls-arcane) 25%, transparent);
  }
  .escalate:focus-visible,
  .ask-big:focus-visible,
  .link:focus-visible,
  .ranked .back:focus-visible {
    box-shadow: var(--el-focus-ring);
    outline: none;
  }
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

  /* Unidentified nudge (D8/B10d, UI-SPEC §2.1): a **slim** amber banner above the grid, flat
     browse only. A boxed card here competes with the cards it sits above for the same glance;
     one tinted full-bleed rule reads as chrome and gets out of the way. */
  .nudge {
    flex: none;
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    flex-wrap: wrap;
    padding: var(--sp-1) var(--sp-5);
    background: color-mix(in srgb, var(--ls-surface) 88%, var(--ls-warning) 12%);
    border-bottom: 1px solid var(--ls-border);
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
  .nudge .q {
    margin-right: var(--sp-1);
    color: var(--ls-status-unidentified);
  }
  .nudge button {
    padding: 2px var(--sp-2);
    border: none;
    border-radius: var(--r-sm);
    font-size: var(--fs-sm);
  }
  /* A text link, not a filled button: the nudge is an offer, and a solid button in a warning
     colour reads as "something is broken, press this". */
  .nudge button.go {
    color: var(--ls-gold);
  }
  .nudge button.go:hover {
    color: var(--ls-gold-bright);
    text-decoration: underline;
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
