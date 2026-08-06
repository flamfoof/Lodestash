<script lang="ts">
  import Icon from "./Icon.svelte";
  import { NAV } from "./routes";
  import { view } from "./nav.svelte";

  import type { LibraryStatus } from "./api";

  /**
   * The rail reads its badges *and* its facet counts off one status object rather than a separate
   * `counts` map: two sources for the same number is how a badge comes to disagree with the facet
   * directly under it.
   */
  let { status = null }: { status?: LibraryStatus | null } = $props();

  let counts = $derived<Partial<Record<string, number>>>({
    unidentified: status?.unidentified ?? 0,
    dups: status?.duplicate_groups ?? 0,
    trashed: status?.trashed ?? 0,
  });

  /**
   * UI-SPEC §0.2: the facet groups are Library filters, and per the 2026-08-06 owner call
   * they stay expanded on **every** route — the sidebar's navigation must not vanish when a
   * screen other than Library/Identify is active. Headers and rows render always; clicking a
   * facet still routes to the Library with that filter applied.
   */

  const STATUSES = [
    { id: "all", label: "All", tint: "var(--ls-gold)" },
    { id: "unidentified", label: "Unidentified", tint: "var(--ls-status-unidentified)" },
    { id: "identified", label: "Identified", tint: "var(--ls-status-identified)" },
    { id: "confirmed", label: "Confirmed", tint: "var(--ls-status-confirmed)" },
  ] as const;

  let statusCounts = $derived<Record<string, number>>({
    all: status?.total_assets ?? 0,
    unidentified: status?.unidentified ?? 0,
    identified: status?.identified ?? 0,
    confirmed: status?.confirmed ?? 0,
  });

  function pickStatus(id: string) {
    view.route = "library";
    view.status = id;
  }

  function pickSource(name: string | null) {
    view.route = "library";
    view.source = name;
    view.dir = "";
  }
</script>

<aside class="sidebar">
  <nav aria-label="Main">
    {#each NAV as item (item.route)}
      {@const badge = item.badge ? (counts[item.badge] ?? 0) : 0}
      <button
        class="row"
        class:on={view.route === item.route}
        aria-current={view.route === item.route ? "page" : undefined}
        onclick={() => (item.route === "library" ? view.goLibrary() : view.go(item.route))}
      >
        <Icon name={item.icon} size={18} />
        <span class="label">{item.label}</span>
        {#if badge > 0}
          <span class="badge num">{badge}</span>
        {/if}
      </button>
    {/each}
  </nav>

  <div class="group">
    <h2>Status</h2>
    {#each STATUSES as s (s.id)}
        <button
          class="facet"
          class:on={view.status === s.id}
          aria-pressed={view.status === s.id}
          onclick={() => pickStatus(s.id)}
        >
          <i class="dot" style="--dot: {s.tint}" aria-hidden="true"></i>
          <span class="label">{s.label}</span>
          <span class="count num">{statusCounts[s.id]}</span>
        </button>
      {/each}
  </div>

  <div class="group">
    <h2>Libraries</h2>
    <button
        class="facet"
        class:on={view.source === null}
        aria-pressed={view.source === null}
        onclick={() => pickSource(null)}
      >
        <span class="label">All libraries</span>
        <span class="count num">{status?.total_assets ?? 0}</span>
      </button>
      {#each status?.sources ?? [] as src (src.id)}
        <button
          class="facet"
          class:on={view.source === src.name}
          aria-pressed={view.source === src.name}
          onclick={() => pickSource(src.name)}
          title={src.root_path}
        >
          <span class="label">{src.name}</span>
          <span class="count num">{src.asset_count}</span>
        </button>
      {/each}
      <!-- ponytail: the chooser (FEATURES §7.4) is B2b's; until then the drop target is the only
           way in, and saying so beats a button that opens nothing. -->
      <div class="add" title="Drop a folder on the window to add it">+ Add source…</div>
  </div>
</aside>

<style>
  .sidebar {
    width: var(--sidebar-w);
    flex: none;
    background: var(--ls-surface);
    border-right: 1px solid var(--ls-border);
    padding: var(--sp-2) var(--sp-2);
    overflow-y: auto;
    position: relative;
    z-index: 1;
  }

  .row {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    width: 100%;
    height: var(--row-h);
    padding: 0 var(--sp-2);
    border-radius: var(--r-md);
    /* Active rows get a gold left border (FEATURES §0.3); reserve the space on every
       row so switching rows never shifts the label by 2px. */
    border-left: 2px solid transparent;
    color: var(--ls-text-muted);
    text-align: left;
  }
  .row :global(.icon) {
    opacity: 0.7;
  }
  .row:hover {
    background: var(--ls-overlay);
    color: var(--ls-text);
  }
  .row.on {
    background: var(--ls-surface-raised);
    border-left-color: var(--ls-gold);
    color: var(--ls-text);
  }
  .row.on :global(.icon) {
    opacity: 1;
  }

  .label {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .badge {
    font-size: var(--fs-xs);
    color: var(--ls-bg);
    background: var(--ls-gold);
    border-radius: var(--r-pill);
    padding: 1px 7px;
  }

  .group {
    margin-top: var(--sp-4);
  }
  .group h2 {
    margin: 0 0 var(--sp-1);
    padding: 0 var(--sp-2);
    font-size: var(--fs-xs);
    font-weight: 600;
    letter-spacing: 0.09em;
    text-transform: uppercase;
    color: var(--ls-text-dim);
  }
  .facet {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    width: 100%;
    height: var(--row-h);
    padding: 0 var(--sp-2);
    border-radius: var(--r-md);
    /* Same reserved 2px as .row, for the same reason: selecting must not nudge the label. */
    border-left: 2px solid transparent;
    color: var(--ls-text-muted);
    text-align: left;
  }
  .facet:hover {
    background: var(--ls-overlay);
    color: var(--ls-text);
  }
  .facet.on {
    background: var(--ls-surface-raised);
    border-left-color: var(--ls-gold);
    color: var(--ls-text);
  }
  .dot {
    flex: none;
    width: 7px;
    height: 7px;
    background: var(--dot);
    transform: rotate(45deg);
  }
  .count {
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
  }
  .add {
    margin-top: var(--sp-1);
    padding: var(--sp-1) var(--sp-2);
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
    border: 1px dashed var(--ls-border-strong);
    border-radius: var(--r-md);
    text-align: center;
  }
</style>
