<script lang="ts">
  import TopBar from "./lib/TopBar.svelte";
  import Sidebar from "./lib/Sidebar.svelte";
  import Screen from "./lib/Screen.svelte";
  import Inspector from "./lib/Inspector.svelte";
  import LibraryScreen from "./lib/LibraryScreen.svelte";
  import { SCREENS } from "./lib/routes";
  import { view } from "./lib/nav.svelte";
  import ConfirmDelete from "./lib/ConfirmDelete.svelte";
  import Palette from "./lib/Palette.svelte";
  import StatsScreen from "./lib/StatsScreen.svelte";
  import DuplicatesScreen from "./lib/DuplicatesScreen.svelte";
  import {
    libraryStatus,
    reloadLibrary,
    assetDetail,
    setFavorite,
    setTrashed,
    removeFromLibrary,
    deleteFromDisk,
    type AssetDetail,
    type LibraryStatus,
  } from "./lib/api";

  let status = $state<LibraryStatus | null>(null);
  let reloading = $state(false);

  libraryStatus()
    .then((s) => (status = s))
    .catch(() => {});

  async function reload() {
    reloading = true;
    try {
      status = await reloadLibrary();
    } finally {
      reloading = false;
    }
  }

  let screen = $derived(SCREENS[view.route]);
  // Favorites shares the library body; it is a *filter over the library*, not its own data.
  // Routing it here without passing the flag renders the whole library under a "Favorites"
  // heading, which reads as though everything is starred.
  let onLibrary = $derived(
    view.route === "library" || view.route === "favorites" || view.route === "trash",
  );
  let favoritesOnly = $derived(view.route === "favorites");
  let trashedOnly = $derived(view.route === "trash");

  // The Inspector's actions need the selected asset's own flags, and the delete dialog needs its
  // absolute path — the user is entitled to see exactly which file is about to stop existing.
  let selectedDetail = $state<AssetDetail | null>(null);
  $effect(() => {
    const id = view.selected;
    view.dataVersion;
    if (id == null) {
      selectedDetail = null;
      return;
    }
    let stale = false;
    assetDetail(id).then((d) => {
      if (!stale) selectedDetail = d;
    });
    return () => {
      stale = true;
    };
  });

  let confirming = $state(false);
  let actionError = $state<string | null>(null);

  /** Run a write, surface any failure, and make every screen re-ask its question. */
  async function write(run: () => Promise<unknown>) {
    actionError = null;
    try {
      await run();
      view.dataVersion += 1;
      status = await libraryStatus();
    } catch (e) {
      actionError = String(e);
    }
  }

  let paletteOpen = $state(false);

  /**
   * Ctrl/Cmd-K opens the palette from anywhere — including from inside the search box, which is
   * the one place a global shortcut usually stops working and the one place people press it.
   */
  function onkeydown(e: KeyboardEvent) {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
      paletteOpen = !paletteOpen;
      e.preventDefault();
    }
  }

  function afterRemoval() {
    // The asset just left this view; keeping it selected would leave the Inspector describing
    // something the grid no longer shows.
    view.selected = null;
    confirming = false;
  }
</script>

<svelte:window on:keydown={onkeydown} />

<div class="frame">
  <TopBar engineReady={status?.open ?? false} />
  <div class="body">
    <Sidebar />
    <main>
      {#if view.route === "stats"}
        <StatsScreen />
      {:else if view.route === "dups"}
        <DuplicatesScreen ontrash={(ids) => write(() => setTrashed(ids, true))} />
      {:else if onLibrary}
        <LibraryScreen
          {status}
          {favoritesOnly}
          {trashedOnly}
          title={favoritesOnly ? "Favorites" : trashedOnly ? "Trash" : "Library"}
        />
      {:else}
        <Screen title={screen.title} milestone={screen.milestone} />
      {/if}
    </main>
    {#if onLibrary && view.inspectorOpen}
      <Inspector
        assetId={view.selected}
        favorite={selectedDetail?.favorite ?? false}
        trashed={selectedDetail?.trashed ?? false}
        onfavorite={(on) => write(() => setFavorite([view.selected!], on))}
        ontrash={() => write(() => setTrashed([view.selected!], true)).then(afterRemoval)}
        onrestore={() => write(() => setTrashed([view.selected!], false)).then(afterRemoval)}
        onremove={() => write(() => removeFromLibrary([view.selected!])).then(afterRemoval)}
        ondelete={() => (confirming = true)}
      />
    {/if}
  </div>
  {#if confirming && selectedDetail}
    <ConfirmDelete
      names={[selectedDetail.name]}
      paths={[selectedDetail.abs_path]}
      onconfirm={() => write(() => deleteFromDisk([selectedDetail!.asset_id])).then(afterRemoval)}
      onremove={() => write(() => removeFromLibrary([selectedDetail!.asset_id])).then(afterRemoval)}
      oncancel={() => (confirming = false)}
    />
  {/if}

  {#if paletteOpen}
    <Palette onclose={() => (paletteOpen = false)} />
  {/if}

  {#if actionError}
    <p class="action-error" data-testid="action-error">{actionError}</p>
  {/if}

  <footer class="statusbar">
    {#if status?.open}
      <span class="num">{status.total_assets.toLocaleString()} assets</span>
      <span>·</span>
      <span class="num">{status.with_preview.toLocaleString()} vector previews</span>
      <span>·</span>
      <span>{status.sources.length} source{status.sources.length === 1 ? "" : "s"}</span>
      <button class="reload" onclick={reload} disabled={reloading}>
        {reloading ? "reloading…" : "reload"}
      </button>
      <span class="right path" title={status.catalog_path ?? ""}>{status.catalog_path}</span>
    {:else}
      <span>no catalog — shell scaffold</span>
    {/if}
  </footer>
</div>

<style>
  .frame {
    display: flex;
    flex-direction: column;
    height: 100%;
  }
  .body {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  main {
    flex: 1;
    min-width: 0;
    background: var(--ls-bg);
  }
  /* A failed write is never silent: the frontend cannot know whether the catalog changed, so
     saying nothing would leave the user believing an action happened. */
  .action-error {
    flex: none;
    margin: 0;
    padding: var(--sp-2) var(--sp-3);
    background: var(--ls-surface);
    border-top: 1px solid var(--ls-danger);
    color: var(--ls-danger);
    font-size: var(--fs-sm);
  }

  .statusbar {
    flex: none;
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    height: 24px;
    padding: 0 var(--sp-3);
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
    background: var(--ls-surface);
    border-top: 1px solid var(--ls-border);
  }
  .reload {
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
    text-decoration: underline;
  }
  .reload:hover:not(:disabled) {
    color: var(--ls-gold);
  }
  .right {
    margin-left: auto;
  }
  .path {
    max-width: 46ch;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    direction: rtl;
  }
</style>
