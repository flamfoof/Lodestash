<script lang="ts">
  import TopBar from "./lib/TopBar.svelte";
  import Sidebar from "./lib/Sidebar.svelte";
  import Screen from "./lib/Screen.svelte";
  import Inspector from "./lib/Inspector.svelte";
  import LibraryScreen from "./lib/LibraryScreen.svelte";
  import { SCREENS } from "./lib/routes";
  import { view } from "./lib/nav.svelte";
  import { libraryStatus, reloadLibrary, type LibraryStatus } from "./lib/api";

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
  // Favorites shares the library body; it filters rather than being its own screen.
  let onLibrary = $derived(view.route === "library" || view.route === "favorites");
</script>

<div class="frame">
  <TopBar engineReady={status?.open ?? false} />
  <div class="body">
    <Sidebar />
    <main>
      {#if onLibrary}
        <LibraryScreen {status} />
      {:else}
        <Screen title={screen.title} milestone={screen.milestone} />
      {/if}
    </main>
    {#if onLibrary && view.inspectorOpen}
      <Inspector assetId={view.selected} />
    {/if}
  </div>
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
