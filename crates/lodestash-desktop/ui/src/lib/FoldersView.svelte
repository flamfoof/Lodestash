<script lang="ts">
  /**
   * The Folders browse view (DESIGN-GUIDELINES §7 "Browse views").
   *
   * A directory browser over the sources: breadcrumb, folder tiles for the subdirectories at this
   * level, then the assets living directly in the current folder. The top level lists the sources
   * — "assets from many sources must never blur into one anonymous pile" — and a single-source
   * library descends into it on its own rather than showing a screen with one tile on it.
   *
   * When a pack filter is active (`view.packageFilter`) this becomes the **pack browser**
   * (PACK-DIRECTORY-NAV): a three-pane layout — persistent left DIRECTORY tree (244px), the
   * contents pane, and the app's global inspector on the right — with ← → ↑ history, a
   * pack-rooted breadcrumb (the pack's name is a gold pill, never a link), and a folders-first
   * `Name · Size · Items` list. Grid mode keeps the tiles + cards.
   *
   * The toolbar keeps working here: tab, status, search and the ⚑ filter all narrow the tiles'
   * counts as well as the assets, so a tile never promises forty assets and opens onto two.
   */
  import Card from "./Card.svelte";
  import Icon from "./Icon.svelte";
  import {
    folderPage,
    packDirTree,
    DENSITIES,
    fmtSize,
    typeToken,
    typeIcon,
    type FolderPage,
    type PackDirNode,
  } from "./api";
  import { view } from "./nav.svelte";

  let { onopen }: { onopen: (assetId: number) => void } = $props();

  let result = $state<FolderPage | null>(null);
  let tree = $state<PackDirNode[] | null>(null);
  let error = $state<string | null>(null);

  let isPack = $derived(view.packageFilter !== null);
  let segments = $derived(view.dir === "" ? [] : view.dir.split("/"));
  let cardPx = $derived(DENSITIES.find((d) => d.id === view.density)?.px ?? 148);

  /** A node is "expanded" when it is the current folder or an ancestor of it. */
  let expanded = $derived((p: string) => view.dir === p || view.dir.startsWith(p + "/"));

  function enter(name: string) {
    if (isPack) view.navDir(view.dir === "" ? name : `${view.dir}/${name}`);
    else if (view.source === null) view.source = name;
    else view.dir = view.dir === "" ? name : `${view.dir}/${name}`;
  }

  /** Up one level: out of the deepest folder, then out of the source itself. */
  function up() {
    if (segments.length > 0) view.navDir(segments.slice(0, -1).join("/"));
    else if (!isPack && view.source !== null) view.source = null;
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

  // Entering or leaving a pack resets the browser's history stacks — the ← → stacks are scoped
  // to one pack's tree, and a stale position from another pack would navigate to nowhere.
  let lastPack: string | null = null;
  $effect(() => {
    if (view.packageFilter !== lastPack) {
      lastPack = view.packageFilter;
      view.histBack = [];
      view.histFwd = [];
      view.dir = "";
    }
  });

  $effect(() => {
    view.dataVersion;
    folderPage({
      tab: view.tab,
      search: view.search,
      status: view.status,
      filter: view.filter,
      source: isPack ? null : view.source,
      dir: view.dir,
      package: isPack ? view.packageFilter : null,
    })
      .then((r) => {
        error = null;
        // A library with exactly one source descends automatically — a level whose only choice is
        // "the one library you have" is a click that tells you nothing. Never inside a pack: the
        // pack's top folders are the root, and auto-descending past them skips the tree.
        if (!isPack && view.source === null && r.tiles.length === 1) {
          view.source = r.tiles[0].name;
          return;
        }
        result = r;
      })
      .catch((e) => (error = String(e)));
  });

  // The pack tree is whole-library, fetched once per pack (and re-asked after a write, like
  // every screen — a new asset can add folders the tree should show).
  $effect(() => {
    view.dataVersion;
    if (!isPack || !view.packageFilter) return;
    packDirTree(view.packageFilter)
      .then((t) => (tree = t))
      .catch(() => (tree = []));
  });
</script>

<svelte:window on:keydown={onkeydown} />

<nav class="crumbs" data-testid="folder-crumbs">
  {#if isPack}
    <button
      class="navbtn"
      disabled={view.histBack.length === 0}
      onclick={() => view.back()}
      aria-label="Back"
      title="Back"
    >←</button>
    <button
      class="navbtn"
      disabled={view.histFwd.length === 0}
      onclick={() => view.fwd()}
      aria-label="Forward"
      title="Forward"
    >→</button>
    <button class="navbtn" onclick={up} aria-label="Up one level" title="Up one level">↑</button>
    <!-- Pack-rooted breadcrumb (PACK-DIRECTORY-NAV §4): the pack is a gold pill, never a link. -->
    <span class="packpill"><Icon name="ls-unitypackage" size={16} /> {view.packageFilter}</span>
    {#each segments as s, i (i)}
      <span class="sep">/</span>
      <button class:on={i === segments.length - 1} onclick={() => view.navDir(segments.slice(0, i + 1).join("/"))}>
        {s}
      </button>
    {/each}
  {:else}
    <button class:on={view.source === null} onclick={() => jump(-2)}>All libraries</button>
    {#if view.source !== null}
      <span class="sep">/</span>
      <button class:on={segments.length === 0} onclick={() => (view.dir = "")}>{view.source}</button>
      {#each segments as s, i (i)}
        <span class="sep">/</span>
        <button class:on={i === segments.length - 1} onclick={() => jump(i)}>{s}</button>
      {/each}
    {/if}
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
  <div class="browse">
    {#if isPack}
      <aside class="tree" data-testid="pack-tree">
        <h3>DIRECTORY</h3>
        {#if tree === null}
          <p class="msg">Loading…</p>
        {:else if tree.length === 0}
          <p class="msg">No folders yet.</p>
        {:else}
          {#each tree as node (node.path)}
            <button
              class="trow"
              class:on={view.dir === node.path}
              style="--depth: {node.depth}"
              onclick={() => view.navDir(node.path)}
              title={node.path}
            >
              <span class="chev" aria-hidden="true">
                {#if node.has_children}
                  <Icon name={expanded(node.path) ? "ls-action-expand-open" : "ls-action-expand-closed"} size={16} />
                {/if}
              </span>
              <Icon name={expanded(node.path) ? "ls-folder-open" : "ls-folder"} size={16} />
              <span class="tname">{node.name}</span>
              <span class="tcount num">
                {node.direct} · {node.recursive}
              </span>
            </button>
          {/each}
        {/if}
      </aside>
    {/if}

    <div class="contents">
      {#if view.rowStyle === "list"}
        <!-- Folders-first list (PACK-DIRECTORY-NAV §6): `Name · Size · Items`, folders first. -->
        <div class="flist" data-testid="folder-list" role="table">
          <div class="lhead" role="row">
            <span role="columnheader">Name</span>
            <span role="columnheader" class="right">Size</span>
            <span role="columnheader" class="right">Items</span>
          </div>
          {#each result.tiles as t (t.name)}
            <button class="frow folder" role="row" onclick={() => enter(t.name)}>
              <span class="gdot" aria-hidden="true"></span>
              <Icon name="ls-folder" size={16} />
              <span class="fname">{t.name}</span>
              <span class="num dim" role="cell">—</span>
              <span class="num dim" role="cell">{t.direct} ({t.recursive})</span>
            </button>
          {/each}
          {#each result.items as item, i (item.asset_id)}
            <button
              class="frow file"
              role="row"
              style="--tint: var({typeToken(item.media_type)})"
              onclick={() => onopen(item.asset_id)}
              ondblclick={() => onopen(item.asset_id)}
            >
              <span class="swatch" aria-hidden="true"></span>
              <Icon name={typeIcon(item.media_type)} size={16} />
              <span class="fname">{item.name}</span>
              <span class="num" role="cell">{fmtSize(item.size_bytes)}</span>
              <span class="num dim" role="cell">{result.third[i] ?? "—"}</span>
            </button>
          {/each}
        </div>
      {:else}
        {#if result.tiles.length > 0}
          <div class="tiles" data-testid="folder-tiles">
            {#each result.tiles as t (t.name)}
              <button class="tile" ondblclick={() => enter(t.name)} onclick={() => enter(t.name)}>
                <Icon name="ls-folder" size={24} />
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
          <div class="grid" data-testid="folder-grid" style="--card: {cardPx}px">
            {#each result.items as item (item.asset_id)}
              <Card
                {item}
                selected={view.selected === item.asset_id}
                onselect={(id) => (view.selected = id)}
                onopen={onopen}
              />
            {/each}
          </div>
        {/if}
      {/if}
    </div>
  </div>
{/if}

<style>
  .crumbs {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-2) var(--sp-3);
    border-bottom: 1px solid var(--ls-border);
    min-width: 0;
    flex: none;
    position: relative;
    z-index: 1;
    background: var(--ls-bg);
  }
  .crumbs button {
    background: none;
    border: none;
    color: var(--ls-text-muted);
    font: inherit;
    font-size: var(--fs-sm);
    cursor: pointer;
    padding: 2px 4px;
    border-radius: var(--r-sm);
    white-space: nowrap;
  }
  .crumbs button.on {
    color: var(--ls-text);
  }
  .crumbs button:hover:not(:disabled) {
    color: var(--ls-gold);
    background: var(--ls-surface-raised);
  }
  .crumbs button:disabled {
    opacity: 0.35;
    cursor: default;
  }
  .crumbs .navbtn {
    width: var(--control-h);
    height: var(--control-h);
    border: 1px solid var(--ls-border);
    display: grid;
    place-items: center;
  }
  .sep {
    color: var(--ls-text-dim);
  }
  .total {
    margin-left: auto;
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
  }
  .packpill {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-1);
    padding: 2px var(--sp-2);
    border: 1px solid var(--ls-gold);
    border-radius: var(--r-pill);
    background: color-mix(in srgb, var(--ls-gold) 14%, transparent);
    color: var(--ls-gold-bright);
    font-size: var(--fs-xs);
    font-weight: 600;
    white-space: nowrap;
    max-width: 34ch;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .browse {
    display: flex;
    flex: 1;
    min-height: 0;
  }

  /* Left pane — the directory tree (PACK-DIRECTORY-NAV §5). Recess background, right border. */
  .tree {
    width: 244px;
    flex: none;
    background: var(--ls-bg);
    border-right: 1px solid var(--ls-border);
    overflow: auto;
    padding: var(--sp-2) 0 var(--sp-3);
  }
  .tree h3 {
    margin: 0;
    padding: 0 var(--sp-3) var(--sp-1);
    font-family: var(--ff-mono);
    font-size: var(--fs-xs);
    font-weight: 600;
    letter-spacing: 0.1em;
    color: var(--ls-text-dim);
  }
  .trow {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
    width: 100%;
    padding: 3px var(--sp-2) 3px calc(var(--sp-2) + var(--depth, 1) * 14px);
    background: none;
    border: none;
    border-left: 2px solid transparent;
    color: var(--ls-text-muted);
    font: inherit;
    font-size: var(--fs-sm);
    cursor: pointer;
    text-align: left;
    min-width: 0;
  }
  .trow:hover {
    background: var(--ls-surface);
    color: var(--ls-text);
  }
  .trow.on {
    background: color-mix(in srgb, var(--ls-gold) 12%, var(--ls-surface));
    border-left-color: var(--ls-gold);
    color: var(--ls-text);
  }
  .chev {
    width: 12px;
    flex: none;
    color: var(--ls-text-dim);
    font-size: 10px;
  }
  .tname {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tcount {
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
    white-space: nowrap;
  }
  .trow.on .tcount {
    color: var(--ls-gold);
  }

  /* Center pane — folders first (PACK-DIRECTORY-NAV §6). */
  .contents {
    flex: 1;
    min-width: 0;
    overflow: auto;
    display: flex;
    flex-direction: column;
    gap: var(--sp-3);
    padding: var(--sp-3);
  }
  .flist {
    display: flex;
    flex-direction: column;
    align-self: start;
    width: 100%;
    max-width: 860px;
  }
  .lhead,
  .frow {
    display: grid;
    grid-template-columns: 1fr 92px 128px;
    gap: var(--sp-3);
    align-items: center;
    padding: 0 var(--sp-2);
  }
  .lhead {
    height: 26px;
    font-family: var(--ff-mono);
    font-size: var(--fs-xs);
    letter-spacing: 0.06em;
    color: var(--ls-text-dim);
    border-bottom: 1px solid var(--ls-border);
    position: sticky;
    top: 0;
    background: var(--ls-bg);
    z-index: 1;
  }
  .frow {
    border: none;
    background: none;
    border-bottom: 1px solid color-mix(in srgb, var(--ls-border) 50%, transparent);
    color: var(--ls-text-muted);
    font: inherit;
    font-size: var(--fs-sm);
    cursor: pointer;
    text-align: left;
    height: var(--row-h);
  }
  .frow:hover {
    background: var(--ls-surface);
    color: var(--ls-text);
  }
  .gdot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--ls-gold);
    flex: none;
  }
  .swatch {
    width: 8px;
    height: 8px;
    border-radius: 2px;
    background: var(--tint, var(--ls-text-dim));
    flex: none;
  }
  .fname {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    display: flex;
    align-items: center;
    gap: var(--sp-1);
  }
  .frow.folder .fname {
    font-weight: 600;
    color: var(--ls-text);
  }
  .dim {
    color: var(--ls-text-dim);
  }
  .right {
    text-align: right;
  }
  .tiles {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
    gap: var(--sp-3);
  }
  .tile {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: var(--sp-1);
    padding: var(--sp-3);
    background: var(--ls-surface);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-md);
    color: var(--ls-text);
    font: inherit;
    cursor: pointer;
    text-align: left;
  }
  .tile:hover {
    border-color: var(--ls-gold);
    background: var(--ls-surface-raised);
  }
  .tile .name {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    width: 100%;
  }
  .tile .count {
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
  }
  .tile .direct {
    color: var(--ls-text-dim);
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(var(--card, 148px), 1fr));
    gap: var(--sp-3);
  }
  .msg {
    color: var(--ls-text-dim);
    padding: var(--sp-2) var(--sp-3);
  }
  .msg.danger {
    color: var(--ls-danger);
  }
</style>
