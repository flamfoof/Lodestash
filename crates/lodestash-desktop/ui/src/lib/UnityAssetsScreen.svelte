<script lang="ts">
  /**
   * Unity Assets (FEATURES §5, UI-SPEC §15) — the `.unitypackage` archive view, distinct from
   * Packages (already-indexed assets): every pack in the download cache, pin/unpin as a library,
   * and **Open →** into the three-pane pack browser (PACK-DIRECTORY-NAV) rooted at the pack.
   *
   * Per-pack Extract and the §5.2 import dialog ride the B2b ingest path and are documented as
   * blocked; the ◉ Stats modal (UI-SPEC §15) opens per pack with the summary numbers the catalog
   * exposes — the per-type CONTENTS/TAGS/HEAVIEST breakdown isn't in the web API, so the modal
   * says so rather than inventing it.
   */
  import Icon from "./Icon.svelte";
  import { packages, setPackagePinned, extractPackage, jobs, cancelJob, fmtSize, syncUnityCache, type PackageRow } from "./api";
  import { view } from "./nav.svelte";

  let rows = $state<PackageRow[] | null>(null);
  let error = $state<string | null>(null);
  let q = $state("");
  let pinnedOnly = $state(false);

  /** The pack whose stats modal is open, if any (UI-SPEC §15). */
  let statsRow = $state<PackageRow | null>(null);
  let sheetRef = $state<HTMLDivElement | null>(null);

  // Focus the dialog when it opens so Tab/Escape work immediately (review B10e).
  $effect(() => {
    if (statsRow) sheetRef?.focus();
  });

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      statsRow = null;
      e.preventDefault();
    }
  }

  async function load() {
    try {
      rows = await packages();
      error = null;
    } catch (e) {
      error = String(e);
    }
  }

  $effect(() => {
    view.dataVersion;
    load();
  });

  let shown = $derived(
    (rows ?? []).filter((p) => {
      if (pinnedOnly && !p.pinned) return false;
      if (catFilter !== "all" && p.category !== catFilter) return false;
      const needle = q.trim().toLowerCase();
      return (
        needle === "" ||
        p.name.toLowerCase().includes(needle) ||
        p.publisher.toLowerCase().includes(needle)
      );
    }),
  );

  // Content-derived filter tabs (FEATURES §5.3): the categories that actually exist in the
  // cache, with an All tab first.
  let categories = $derived([
    "all",
    ...[...new Set((rows ?? []).map((p) => p.category).filter((c): c is string => !!c))].sort(),
  ]);
  let catFilter = $state<string>("all");

  // `↻ Sync` (FEATURES §5.3): re-crawl the Asset Store cache roots so new packs appear.
  let syncBusy = $state(false);
  let syncNote = $state<string | null>(null);
  async function sync() {
    if (syncBusy) return;
    syncBusy = true;
    syncNote = null;
    try {
      const r = await syncUnityCache();
      syncNote = r.changed > 0 ? `Synced — ${r.changed.toLocaleString()} new assets indexed.` : "Up to date — nothing new to index.";
      await load();
      view.dataVersion += 1;
    } catch (e) {
      syncNote = String(e);
    } finally {
      syncBusy = false;
    }
  }

  let byPublisher = $derived.by(() => {
    const map = new Map<string, PackageRow[]>();
    for (const p of shown) {
      const list = map.get(p.publisher) ?? [];
      list.push(p);
      map.set(p.publisher, list);
    }
    return [...map.entries()]
      .map(([publisher, packs]) => ({
        publisher,
        packs: packs.slice().sort((a, b) => b.count - a.count || a.name.localeCompare(b.name)),
      }))
      .sort((a, b) => a.publisher.localeCompare(b.publisher));
  });

  async function pin(p: PackageRow) {
    try {
      await setPackagePinned(p.id, !p.pinned);
      view.dataVersion += 1;
    } catch (e) {
      error = String(e);
    }
  }

  // Per-pack Extract (B2b slice): unpack into the workspace + reconcile the payloads. Runs on
  // the persist thread; the row's button shows a spinner until the write returns.
  let extracting = $state<Set<number>>(new Set());
  async function extract(p: PackageRow) {
    if (extracting.has(p.id)) return;
    extracting = new Set(extracting).add(p.id);
    try {
      await extractPackage(p.id);
      view.dataVersion += 1;
    } catch (e) {
      error = String(e);
    } finally {
      const next = new Set(extracting);
      next.delete(p.id);
      extracting = next;
    }
  }

  // Batch Extract (UI-SPEC §5): ticked packs unpack **one at a time** — the frontend serializes
  // the kicks, the writer serializes whatever arrives concurrently. Cancel queue stops the
  // not-yet-started packs and asks the active extract job to stop.
  let ticked = $state<Set<number>>(new Set());
  let queued = $state<Set<number>>(new Set());
  let runningNow = $state<Set<number>>(new Set());
  let cancelQueue = $state(false);
  let queueError = $state<string | null>(null);

  function toggleTick(id: number) {
    const next = new Set(ticked);
    if (next.has(id)) {
      next.delete(id);
    } else {
      next.add(id);
    }
    ticked = next;
  }

  async function extractSelected() {
    if (ticked.size === 0) return;
    queueError = null;
    const ids = [...ticked];
    queued = new Set(ids);
    for (const id of ids) {
      if (cancelQueue) break;
      const nextQueue = new Set(queued);
      nextQueue.delete(id);
      queued = nextQueue;
      const run = new Set(runningNow);
      run.add(id);
      runningNow = run;
      try {
        await extractPackage(id);
        view.dataVersion += 1;
      } catch (e) {
        queueError = String(e);
      } finally {
        const run2 = new Set(runningNow);
        run2.delete(id);
        runningNow = run2;
      }
    }
    cancelQueue = false;
    ticked = new Set();
  }

  async function cancelQueueFn() {
    cancelQueue = true;
    // The active extract (if any) cooperates through the tray's job row.
    try {
      const jv = await jobs();
      for (const j of jv.active) {
        if (j.kind === "extract" && j.cancellable) await cancelJob(j.id);
      }
    } catch {
      /* the tray reports errors; the queue stop is the important part */
    }
  }

  const queuedCount = $derived(queued.size + runningNow.size);

  /** Open the pack in the three-pane browser, rooted at the pack's top folder. */
  function open(p: PackageRow) {
    view.packageFilter = p.name;
    view.browse = "folders";
    view.rowStyle = "list";
    view.dir = "";
    view.go("library");
  }
</script>

<svelte:window on:keydown={onkeydown} />
<section class="screen">
  <header>
    <div class="title">
      <h1>Unity Assets</h1>
      <p class="subtitle">
        Your `.unitypackage` archives — browse inside one like a file explorer, or pin it as a
        library for the sidebar. Extraction is an explicit, on-demand action.
      </p>
    </div>
    <div class="tools">
      <input class="search" placeholder="Filter packs and publishers" bind:value={q} />
      <button class="pin-toggle" class:on={pinnedOnly} onclick={() => (pinnedOnly = !pinnedOnly)}>
        ⬢ pinned
      </button>
      <button class="pin-toggle" onclick={sync} disabled={syncBusy} data-testid="sync-unity">
        {syncBusy ? "Syncing…" : "↻ Sync"}
      </button>
    </div>
    {#if syncNote}
      <p class="sync-note mono dim" data-testid="sync-note">{syncNote}</p>
    {/if}
    {#if categories.length > 1}
      <div class="cats" role="tablist" aria-label="Pack categories">
        {#each categories as c (c)}
          <button
            class="tab"
            class:on={catFilter === c}
            role="tab"
            aria-selected={catFilter === c}
            onclick={() => (catFilter = c)}
            data-testid="cat-{c}"
          >
            {c}
          </button>
        {/each}
      </div>
    {/if}
  </header>

  <div class="body">
    {#if error}
      <p class="msg danger">{error}</p>
    {:else if !rows}
      <p class="msg">Loading…</p>
    {:else if byPublisher.length === 0}
      <p class="msg" data-testid="empty">
        {#if pinnedOnly}
          Nothing pinned yet — use ⬢ on a pack to pin it as a library.
        {:else if q}
          No pack matches “{q}”.
        {:else}
          No Unity packages found in the Asset Store cache yet.
        {/if}
      </p>
    {:else}
      {#if ticked.size > 0 || queuedCount > 0}
        <div class="batch" data-testid="extract-batch">
          <span class="batch-label num">
            {ticked.size} selected · {queuedCount} queued/extracting
          </span>
          <button class="open primary" onclick={extractSelected} data-testid="extract-selected">
            ▶ Extract selected ({ticked.size})
          </button>
          {#if queuedCount > 0}
            <button class="open" onclick={cancelQueueFn} data-testid="cancel-queue">
              Cancel queue
            </button>
          {/if}
          {#if queueError}
            <span class="err">{queueError}</span>
          {/if}
        </div>
      {/if}
      <div class="publishers" data-testid="unity-packs">
        {#each byPublisher as group (group.publisher)}
          <section class="publisher">
            <h2>
              <Icon name="ls-nav-packages" size={16} />
              {group.publisher}
              <span class="count num">{group.packs.length}</span>
            </h2>
            <ul>
              {#each group.packs as p (p.id)}
                <li class="pack" class:pinned={p.pinned}>
                  <input
                    type="checkbox"
                    class="tick"
                    checked={ticked.has(p.id) || runningNow.has(p.id)}
                    disabled={runningNow.has(p.id)}
                    aria-label={`Queue ${p.name} for extraction`}
                    data-testid="tick-{p.id}"
                    onchange={() => toggleTick(p.id)}
                  />
                  <span class="name" title={p.category ?? ""}>{p.name}</span>
                  {#if p.pinned}
                    <span class="pill pin">⬢ pinned</span>
                  {/if}
                  <span class="stat num">
                    {p.count.toLocaleString()} assets
                    {#if p.bytes > 0}· {fmtSize(p.bytes)}{/if}
                  </span>
                  <div class="actions">
                    <button
                      class="icon"
                      class:on={p.pinned}
                      title={p.pinned ? "Unpin library" : "Pin as library"}
                      onclick={() => pin(p)}
                      data-testid="pin-{p.id}"
                    >
                      ⬢
                    </button>
                    <button
                      class="open"
                      class:busy={extracting.has(p.id)}
                      disabled={extracting.has(p.id)}
                      onclick={() => extract(p)}
                      data-testid="extract-{p.id}"
                    >
                      {extracting.has(p.id) ? "Extracting…" : "Extract"}
                    </button>
                    <button class="open" onclick={() => (statsRow = p)} data-testid="stats-{p.id}">
                      Stats
                    </button>
                    <button class="open" onclick={() => open(p)} data-testid="open-{p.id}">
                      Open →
                    </button>
                  </div>
                </li>
              {/each}
            </ul>
          </section>
        {/each}
      </div>
    {/if}
  </div>

  {#if statsRow}
    <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
    <div class="scrim" onclick={() => (statsRow = null)} data-testid="pack-stats">
      <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
      <div
        class="sheet"
        onclick={(e) => e.stopPropagation()}
        role="alertdialog"
        tabindex="-1"
        bind:this={sheetRef}
        aria-modal="true"
        aria-labelledby="pack-stats-title"
      >
        <div class="sheet-head">
          <div class="sheet-title">
            <h3 id="pack-stats-title">{statsRow.name}</h3>
            <p class="meta">
              {statsRow.publisher}
              {#if statsRow.category}<span class="sep">·</span> {statsRow.category}{/if}
            </p>
          </div>
          <button
            class="close"
            aria-label="Close pack stats"
            onclick={() => (statsRow = null)}
            data-testid="pack-stats-close"
          >
            ✕
          </button>
        </div>

        <div class="grid" data-testid="pack-stats-grid">
          <div class="tile">
            <span class="num value">{statsRow.count.toLocaleString()}</span>
            <span class="label">indexed assets</span>
          </div>
          <div class="tile" class:warn={statsRow.unidentified > 0}>
            <span class="num value">{statsRow.unidentified.toLocaleString()}</span>
            <span class="label">unidentified</span>
          </div>
          <div class="tile">
            <span class="num value">{fmtSize(statsRow.bytes)}</span>
            <span class="label">size</span>
          </div>
          <div class="tile">
            <span class="num value">{statsRow.triangles ? statsRow.triangles.toLocaleString() : "—"}</span>
            <span class="label">triangles</span>
          </div>
        </div>

        {#if statsRow.by_type.length > 0}
          <div class="types" data-testid="pack-stats-types">
            {#each statsRow.by_type as t (t.media_type)}
              <div class="type-row">
                <span class="type-name">{t.media_type}</span>
                <span class="type-count mono">{t.count.toLocaleString()}</span>
                <span class="type-bytes mono dim">{fmtSize(t.bytes)}</span>
              </div>
            {/each}
          </div>
        {:else}
          <p class="note">No assets indexed from this pack yet — add its folder as a source to pull
            the contents in.</p>
        {/if}
      </div>
    </div>
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
    align-items: flex-start;
    gap: var(--sp-4);
    padding: var(--sp-4) var(--sp-5) var(--sp-3);
    flex-wrap: wrap;
  }
  .title {
    flex: 1;
    min-width: 0;
  }
  h1 {
    margin: 0;
    font-family: var(--ff-display);
    font-size: var(--fs-2xl);
    font-weight: 700;
    letter-spacing: 0.02em;
  }
  .subtitle {
    margin: var(--sp-1) 0 0;
    font-size: var(--fs-sm);
    color: var(--ls-text-muted);
    max-width: 76ch;
  }
  .tools {
    display: flex;
    gap: var(--sp-2);
    align-items: center;
  }
  .cats {
    display: flex;
    gap: 4px;
    margin-top: 10px;
  }
  .cats .tab {
    border: 1px solid var(--ls-border);
    border-radius: 999px;
    background: transparent;
    color: var(--ls-text-dim);
    font: inherit;
    font-size: var(--fs-xs);
    padding: 3px 10px;
    cursor: default;
  }
  .cats .tab.on {
    border-color: var(--ls-gold);
    color: var(--ls-gold-bright);
  }
  .sync-note {
    margin: 8px 0 0;
    font-size: var(--fs-xs);
  }
  .search {
    width: 240px;
    height: var(--control-h);
    padding: 0 var(--sp-2);
    background: var(--ls-surface);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-md);
    color: var(--ls-text);
    font: inherit;
  }
  .pin-toggle {
    height: var(--control-h);
    padding: 0 var(--sp-3);
    background: none;
    border: 1px solid var(--ls-border);
    border-radius: var(--r-md);
    color: var(--ls-text-dim);
    font: inherit;
    cursor: pointer;
  }
  .pin-toggle.on {
    color: var(--ls-gold-bright);
    border-color: var(--ls-gold);
    background: color-mix(in srgb, var(--ls-gold) 12%, transparent);
  }
  .body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: var(--sp-4) var(--sp-5);
  }
  .publishers {
    display: flex;
    flex-direction: column;
    gap: var(--sp-4);
    max-width: 960px;
  }
  .publisher h2 {
    display: flex;
    align-items: baseline;
    gap: var(--sp-2);
    margin: 0 0 var(--sp-2);
    font-size: var(--fs-lg);
  }
  .count {
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    border: 1px solid var(--ls-border);
    border-radius: var(--r-md);
    overflow: hidden;
  }
  .pack {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-2) var(--sp-3);
    border-bottom: 1px solid color-mix(in srgb, var(--ls-border) 50%, transparent);
    background: var(--ls-surface);
  }
  .pack:last-child {
    border-bottom: none;
  }
  .tick {
    accent-color: var(--ls-gold);
    flex: none;
    cursor: pointer;
  }
  .tick:disabled {
    cursor: wait;
  }
  .batch {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-1) var(--sp-3);
    background: var(--ls-overlay);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-md);
    margin-bottom: var(--sp-2);
  }
  .batch-label {
    flex: 1;
    color: var(--ls-text-dim);
  }
  .err {
    color: var(--ls-danger);
  }
  .pack.pinned {
    background: color-mix(in srgb, var(--ls-gold) 6%, var(--ls-surface));
  }
  .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-weight: 600;
  }
  .pill {
    padding: 1px var(--sp-2);
    border: 1px solid var(--ls-gold);
    border-radius: var(--r-pill);
    font-size: var(--fs-xs);
    color: var(--ls-gold-bright);
    white-space: nowrap;
  }
  .stat {
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
    white-space: nowrap;
  }
  .actions {
    display: flex;
    gap: var(--sp-1);
    flex: none;
  }
  .icon {
    width: var(--control-h);
    height: var(--control-h);
    background: none;
    border: 1px solid var(--ls-border);
    border-radius: var(--r-sm);
    color: var(--ls-text-dim);
    cursor: pointer;
  }
  .icon:hover {
    color: var(--ls-gold-bright);
    border-color: var(--ls-gold);
  }
  .icon.on {
    color: var(--ls-gold-bright);
    border-color: var(--ls-gold);
  }
  .open {
    height: var(--control-h);
    padding: 0 var(--sp-3);
    background: none;
    border: 1px solid var(--ls-arcane);
    border-radius: var(--r-sm);
    color: var(--ls-arcane-bright);
    font: inherit;
    font-size: var(--fs-sm);
    cursor: pointer;
  }
  .open:hover {
    background: color-mix(in srgb, var(--ls-arcane) 12%, transparent);
  }
  .msg {
    color: var(--ls-text-dim);
  }
  .msg.danger {
    color: var(--ls-danger);
  }
  .num {
    font-variant-numeric: tabular-nums;
  }
  .scrim {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 0.6);
    display: grid;
    place-items: center;
    z-index: 100;
  }
  .sheet {
    width: min(420px, 92vw);
    background: var(--ls-surface);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-lg);
    padding: var(--sp-4);
    display: flex;
    flex-direction: column;
    gap: var(--sp-3);
  }
  .sheet-head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: var(--sp-2);
    padding: 0;
    flex-wrap: nowrap;
  }
  .sheet h3 {
    margin: 0;
    font-size: var(--fs-lg);
  }
  .meta {
    margin: var(--sp-1) 0 0;
    font-size: var(--fs-sm);
    color: var(--ls-text-muted);
  }
  .sep {
    color: var(--ls-text-dim);
  }
  .close {
    flex: none;
    width: var(--control-h);
    height: var(--control-h);
    background: none;
    border: 1px solid var(--ls-border);
    border-radius: var(--r-sm);
    color: var(--ls-text-dim);
    font: inherit;
    cursor: pointer;
  }
  .close:hover {
    color: var(--ls-gold-bright);
    border-color: var(--ls-gold);
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: var(--sp-2);
  }
  .tile {
    display: flex;
    flex-direction: column;
    gap: var(--sp-1);
    padding: var(--sp-3);
    background: var(--ls-bg);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-md);
  }
  .tile .value {
    font-size: var(--fs-xl);
    font-weight: 700;
  }
  .tile.warn .value {
    color: var(--ls-gold-bright);
  }
  .tile .label {
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .types {
    margin-top: 12px;
    border-top: 1px solid var(--ls-border);
    padding-top: 8px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    max-height: 220px;
    overflow: auto;
  }
  .type-row {
    display: grid;
    grid-template-columns: 1fr auto auto;
    gap: 12px;
    align-items: baseline;
    font-size: var(--fs-sm);
  }
  .type-name {
    color: var(--ls-text);
  }
  .note {
    margin: 0;
    font-size: var(--fs-sm);
    color: var(--ls-text-dim);
  }
</style>
