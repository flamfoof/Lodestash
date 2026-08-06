<script lang="ts">
  /**
   * Unity Assets (FEATURES §5, UI-SPEC §15) — the `.unitypackage` archive view, distinct from
   * Packages (already-indexed assets): every pack in the download cache, pin/unpin as a library,
   * and **Open →** into the three-pane pack browser (PACK-DIRECTORY-NAV) rooted at the pack.
   *
   * Per-pack Extract and the §5.2 import dialog ride the B2b ingest path and are documented as
   * blocked; the ◉ Stats modal is the UI-SPEC §15 slice not yet ported.
   */
  import Icon from "./Icon.svelte";
  import { packages, setPackagePinned, fmtSize, type PackageRow } from "./api";
  import { view } from "./nav.svelte";

  let rows = $state<PackageRow[] | null>(null);
  let error = $state<string | null>(null);
  let q = $state("");
  let pinnedOnly = $state(false);

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
      const needle = q.trim().toLowerCase();
      return (
        needle === "" ||
        p.name.toLowerCase().includes(needle) ||
        p.publisher.toLowerCase().includes(needle)
      );
    }),
  );

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

  /** Open the pack in the three-pane browser, rooted at the pack's top folder. */
  function open(p: PackageRow) {
    view.packageFilter = p.name;
    view.browse = "folders";
    view.rowStyle = "list";
    view.dir = "";
    view.go("library");
  }
</script>

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
    </div>
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
</style>
