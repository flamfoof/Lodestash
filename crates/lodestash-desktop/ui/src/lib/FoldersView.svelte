<script lang="ts">
  /**
   * The Folders browse view (DESIGN-GUIDELINES §7 "Browse views").
   *
   * A directory browser over the sources: breadcrumb, folder tiles for the subdirectories at this
   * level, then the assets living directly in the current folder. The top level lists the sources
   * — "assets from many sources must never blur into one anonymous pile" — and a single-source
   * library descends into it on its own rather than showing a screen with one tile on it.
   *
   * The toolbar keeps working here: tab, status, search and the ⚑ filter all narrow the tiles'
   * counts as well as the assets, so a tile never promises forty assets and opens onto two.
   */
  import Card from "./Card.svelte";
  import Icon from "./Icon.svelte";
  import { folderPage, type FolderPage } from "./api";
  import { view } from "./nav.svelte";

  let { onopen }: { onopen: (assetId: number) => void } = $props();

  let result = $state<FolderPage | null>(null);
  let error = $state<string | null>(null);

  let segments = $derived(view.dir === "" ? [] : view.dir.split("/"));

  function enter(name: string) {
    if (view.source === null) view.source = name;
    else view.dir = view.dir === "" ? name : `${view.dir}/${name}`;
  }

  /** Up one level: out of the deepest folder, then out of the source itself. */
  function up() {
    if (segments.length > 0) view.dir = segments.slice(0, -1).join("/");
    else if (view.source !== null) view.source = null;
  }

  /** Jump to a breadcrumb segment; `-1` is the source root, `-2` is "All libraries". */
  function jump(i: number) {
    if (i === -2) {
      view.source = null;
      view.dir = "";
    } else {
      view.dir = segments.slice(0, i + 1).join("/");
    }
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.target instanceof HTMLInputElement) return;
    if (e.key === "Backspace") {
      up();
      e.preventDefault();
    }
  }

  $effect(() => {
    view.dataVersion;
    folderPage({
      tab: view.tab,
      search: view.search,
      status: view.status,
      filter: view.filter,
      source: view.source,
      dir: view.dir,
    })
      .then((r) => {
        error = null;
        // A library with exactly one source descends automatically — a level whose only choice is
        // "the one library you have" is a click that tells you nothing.
        if (view.source === null && r.tiles.length === 1) {
          view.source = r.tiles[0].name;
          return;
        }
        result = r;
      })
      .catch((e) => (error = String(e)));
  });
</script>

<svelte:window on:keydown={onkeydown} />

<nav class="crumbs" data-testid="folder-crumbs">
  <button class:on={view.source === null} onclick={() => jump(-2)}>All libraries</button>
  {#if view.source !== null}
    <span class="sep">/</span>
    <button class:on={segments.length === 0} onclick={() => (view.dir = "")}>{view.source}</button>
    {#each segments as s, i (i)}
      <span class="sep">/</span>
      <button class:on={i === segments.length - 1} onclick={() => jump(i)}>{s}</button>
    {/each}
  {/if}
  {#if result}
    <span class="total num">{result.total.toLocaleString()} assets</span>
  {/if}
</nav>

{#if error}
  <p class="msg danger">{error}</p>
{:else if !result}
  <p class="msg">Loading…</p>
{:else if result.tiles.length === 0 && result.items.length === 0}
  <p class="msg" data-testid="empty">
    {#if view.search}
      Nothing in this folder matches “{view.search}”.
    {:else}
      This folder is empty.
    {/if}
  </p>
{:else}
  {#if result.tiles.length > 0}
    <div class="tiles" data-testid="folder-tiles">
      {#each result.tiles as t (t.name)}
        <button class="tile" ondblclick={() => enter(t.name)} onclick={() => enter(t.name)}>
          <Icon name="ls-nav-packages" size={22} />
          <span class="name">{t.name}</span>
          <span class="count num">
            {t.recursive.toLocaleString()}
            {#if t.direct !== t.recursive}
              <span class="direct">({t.direct.toLocaleString()} here)</span>
            {/if}
          </span>
        </button>
      {/each}
    </div>
  {/if}

  {#if result.items.length > 0}
    <div class="grid">
      {#each result.items as item (item.asset_id)}
        <Card
          {item}
          selected={view.selected === item.asset_id}
          onselect={(id) => (view.selected = id)}
          {onopen}
        />
      {/each}
    </div>
  {/if}
{/if}

<style>
  .crumbs {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
    flex-wrap: wrap;
    margin-bottom: var(--sp-3);
    font-size: var(--fs-sm);
  }
  .crumbs button {
    padding: 2px var(--sp-2);
    border-radius: var(--r-sm);
    color: var(--ls-text-dim);
    font: inherit;
    font-size: var(--fs-sm);
    cursor: pointer;
  }
  .crumbs button:hover {
    color: var(--ls-text);
  }
  .crumbs button.on {
    color: var(--ls-gold);
  }
  .sep {
    color: var(--ls-text-muted);
  }
  .total {
    margin-left: auto;
    color: var(--ls-text-dim);
  }

  .tiles {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
    gap: var(--sp-2);
    margin-bottom: var(--sp-4);
  }
  .tile {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-2) var(--sp-3);
    background: var(--ls-surface);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-md);
    color: var(--ls-text);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .tile:hover {
    border-color: var(--ls-gold);
  }
  .tile .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tile .count {
    margin-left: auto;
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
    white-space: nowrap;
  }
  .direct {
    color: var(--ls-text-muted);
  }

  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(var(--card, 148px), 1fr));
    gap: var(--sp-3);
  }
  .msg {
    color: var(--ls-text-dim);
  }
  .msg.danger {
    color: var(--ls-danger);
  }
</style>
