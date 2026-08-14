<script lang="ts">
  import Icon from "./Icon.svelte";
  import { NAV } from "./routes";
  import { view } from "./nav.svelte";

  import { packages, addSourceDialog, type LibraryStatus, type PackageRow } from "./api";

  /**
   * The rail reads its badges *and* its facet counts off one status object rather than a separate
   * `counts` map: two sources for the same number is how a badge comes to disagree with the facet
   * directly under it.
   */
  let { status = null, onopensource }: {
    status?: LibraryStatus | null;
    /** Open the "Add a source" chooser (App owns it — watch folder vs import packages). */
    onopensource?: () => void;
  } = $props();

  let counts = $derived<Partial<Record<string, number>>>({
    unidentified: status?.unidentified ?? 0,
    dups: status?.duplicate_groups ?? 0,
    trashed: status?.trashed ?? 0,
  });

  /**
   * Pinned Unity packages (UI-SPEC §15), listed at the top of the LIBRARIES facet as shortcuts
   * into the Unity Assets screen. Empty until `packages()` says otherwise — an empty pin set
   * renders no section at all.
   */
  let pinned = $state<PackageRow[]>([]);

  async function loadPinned() {
    try {
      pinned = (await packages()).filter((p) => p.pinned);
    } catch {
      pinned = [];
    }
  }

  $effect(() => {
    status;
    view.route;
    view.dataVersion;
    loadPinned();
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

  // "+ Add source…" (B2b): native folder dialog; on a pick, the same import path as a drop.
  // The status re-read happens in App.svelte (it re-asks on dataVersion bumps), so the new
  // source appears in the facet list as soon as the write lands.
  let addError = $state<string | null>(null);
  async function pickSourceDialog() {
    addError = null;
    try {
      const r = await addSourceDialog();
      if (r === null) return; // cancelled
      view.dataVersion += 1;
    } catch (e) {
      addError = String(e);
    }
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
    {#if pinned.length > 0}
      <h2 class="sub">Pinned</h2>
      {#each pinned as p (p.id)}
        <button
          class="facet pinned"
          onclick={() => view.go("unity")}
          title="Open Unity Assets"
          data-testid="pinned-{p.id}"
        >
          <span class="pin" aria-hidden="true">⬢</span>
          <span class="label">{p.name}</span>
        </button>
      {/each}
    {/if}
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
      <!-- B2b: the "+ Add source…" chooser is real now — a native folder dialog (Rust-side),
           then the same import path as dropping a folder on the window. -->
      <button
        class="add"
        onclick={() => (onopensource ? onopensource() : pickSourceDialog())}
        data-testid="add-source"
      >
        + Add source…
      </button>
      {#if addError}
        <p class="add-error">{addError}</p>
      {/if}
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
  /* The Pinned sub-heading sits between the Libraries heading and the facet rows. */
  .group h2.sub {
    margin-top: var(--sp-3);
  }
  .pin {
    flex: none;
    color: var(--ls-gold-bright);
  }
  .add {
    margin-top: var(--sp-1);
    padding: var(--sp-1) var(--sp-2);
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
    border: 1px dashed var(--ls-border-strong);
    border-radius: var(--r-md);
    text-align: center;
    background: none;
    cursor: pointer;
    width: 100%;
  }
  .add:hover {
    color: var(--ls-text);
    border-color: var(--ls-gold);
  }
  .add-error {
    margin-top: var(--sp-1);
    font-size: var(--fs-xs);
    color: var(--ls-danger);
  }
</style>
