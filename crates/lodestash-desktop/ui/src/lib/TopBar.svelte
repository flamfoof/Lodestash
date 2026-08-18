<script lang="ts">
  import Icon from "./Icon.svelte";
  import { TABS } from "./routes";
  import { view } from "./nav.svelte";
  import { push } from "./toasts.svelte";
  import {
    projects,
    setActiveProject,
    recentExports,
    undoImportBatch,
    type LibraryStatus,
    type ProjectRow,
    type ImportBatch,
  } from "./api";

  let { engineReady, status = null }: { engineReady: boolean; status?: LibraryStatus | null } = $props();

  /** Tab id → live count (UI-SPEC §0.3). Falls back to 0 so a missing payload renders counts of 0. */
  let tabCounts = $derived(new Map((status?.tab_counts ?? []).map((t) => [t.id, t.count])));

  /**
   * Health meter (UI-SPEC §0.1.5): the two-word status vocabulary maps to the same buckets as the
   * STATUS facet — `identified` + `confirmed` read as Identified, everything else as Unidentified
   * (see `STATUSES` / `statusLabel` in api.ts). The bar's denominator is the sum of those two
   * buckets, not `total_assets`, so trashed/removed assets never skew the split.
   */
  let identified = $derived((status?.identified ?? 0) + (status?.confirmed ?? 0));
  let unidentified = $derived(status?.unidentified ?? 0);
  let identifiedPct = $derived((identified + unidentified) > 0 ? (identified / (identified + unidentified)) * 100 : 0);

  /** Compact counts like the tab counters, but 12,400 → "12,4k" so the bar stays narrow. */
  const fmtCount = (n: number) =>
    n >= 1000
      ? `${(n / 1000).toLocaleString(undefined, { maximumFractionDigits: 1 })}k`
      : n.toLocaleString();

  // ── Project switcher (UI-SPEC §16.1) ──────────────────────────────────────────────────────
  // Replaces the static No project chip: export targets (switch via setActiveProject), recent
  // import batches (per-batch undo), and the Connect wizard entry (routes to Settings → Unity
  // Editors). The shell staging shortlist is in-memory — switching projects never mixes picks.
  let open = $state(false);
  let projectsList = $state<ProjectRow[]>([]);
  let batches = $state<ImportBatch[]>([]);
  let undoing = $state<string | null>(null);
  let confirming = $state<string | null>(null);

  let activeProject = $derived(projectsList.find((p) => p.is_active) ?? null);

  async function loadSwitcher() {
    try {
      const [ps, bs] = await Promise.all([projects(), recentExports(4)]);
      projectsList = ps;
      batches = bs;
    } catch {
      // Open anyway — the empty sections read honestly when the catalog is not there yet.
      projectsList = [];
      batches = [];
    }
  }

  function toggle() {
    open = !open;
    if (open) {
      confirming = null;
      loadSwitcher();
    }
  }

  async function switchProject(p: ProjectRow) {
    try {
      await setActiveProject(p.id);
      push(`Switched to ${p.name}`, "ok");
      view.dataVersion += 1; // re-read the grid — staging/imports follow the active project
    } catch (e) {
      push(String(e), "err");
    }
    await loadSwitcher();
  }

  async function undoBatch(batchId: string) {
    // Confirm-gated (UI-SPEC §16.1): first click arms, the second within 3s confirms.
    if (confirming !== batchId) {
      confirming = batchId;
      setTimeout(() => {
        if (confirming === batchId) confirming = null;
      }, 3000);
      return;
    }
    confirming = null;
    undoing = batchId;
    try {
      const r = await undoImportBatch(batchId);
      push(r.changed === 1 ? "Undid 1 file" : `Undid ${r.changed} files`, "ok");
      view.dataVersion += 1; // re-read the grid after the batch is gone
    } catch (e) {
      push(String(e), "err");
    }
    undoing = null;
    await loadSwitcher();
  }

  function connectProject() {
    open = false;
    // The wizard app half lives in Settings → Unity Editors; the entry routes there.
    view.go("settings");
  }

  /** Ns / Nm / Nh / Nd ago — the relative time the recent-exports rows show. */
  function relTime(atUnix: number): string {
    const s = Math.max(0, Math.floor(Date.now() / 1000 - atUnix));
    if (s < 60) return `${s}s ago`;
    if (s < 3600) return `${Math.floor(s / 60)}m ago`;
    if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
    return `${Math.floor(s / 86400)}d ago`;
  }

  // Hydrate once so the chip shows the active project name (not just after the popover opens).
  loadSwitcher();
</script>

<header class="topbar">
  <div class="brand">
    <Icon name="ls-logo" size={22} />
    <span class="wordmark">LODESTASH</span>
  </div>

  <!-- UI-SPEC §0.1 orders the top bar brand → **mode toggle** → search box. Nested inside the
       input's right edge the toggle read as a suffix of whatever you had typed; outside and
       ahead of it, it reads as what it is — the engine the box will query. -->
  <div class="mode" role="group" aria-label="Search mode">
    <button class:on={view.searchMode === "tags"} onclick={() => (view.searchMode = "tags")}>
      <Icon name="ls-search-tags" size={14} /> Tags
    </button>
    <button class:on={view.searchMode === "ai"} onclick={() => (view.searchMode = "ai")}>
      <Icon name="ls-search-ai" size={14} /> AI
    </button>
  </div>

  <div class="search" class:ai={view.searchMode === "ai"}>
    <Icon name={view.searchMode === "ai" ? "ls-search-ai" : "ls-search-tags"} size={16} />
    <input
      type="text"
      placeholder={view.searchMode === "ai"
        ? "describe what you need — e.g. »pickup sound for rocks« — then Enter"
        : "Search assets, tags, packs…"}
      value={view.search}
      oninput={(e) => view.setSearch(e.currentTarget.value)}
      spellcheck="false"
    />
  </div>

  <nav class="tabs" aria-label="Category">
    {#each TABS as tab (tab.id)}
      {@const count = tabCounts.get(tab.id) ?? 0}
      <!-- UI-SPEC §0.3: `All` never hides; a bucket with a zero count hides — unless it is the
           active tab, which must stay visible or the grid silently filters by an invisible tab. -->
      {#if tab.id === "all" || count > 0 || view.tab === tab.id}
        <button
          class:on={view.tab === tab.id}
          style="--tab: var({tab.token})"
          onclick={() => (view.tab = tab.id)}
          data-testid="tab-{tab.id}"
        >
          <Icon name={tab.icon} size={14} />
          {tab.label}
          <span class="count num">{count.toLocaleString()}</span>
        </button>
      {/if}
    {/each}
  </nav>

  {#if status}
    <div class="health" role="group" title="Identified vs. unidentified assets" aria-label="Identified vs. unidentified" data-testid="health-meter">
      <!-- UI-SPEC §0.1.5: the label `Identified n / total` sits **over** a 6px gold bar. A
           fraction states the goal (get to total); two side-by-side counts only state the
           split, and read as two unrelated statistics. -->
      <span class="health-text">
        <span class="health-label">Identified</span>
        <span class="health-frac num">
          <span class="health-id">{fmtCount(identified)}</span> / {fmtCount(identified + unidentified)}
        </span>
      </span>
      <div class="health-bar" aria-hidden="true">
        <span class="health-seg" style="--pct: {identifiedPct}%"></span>
      </div>
    </div>
  {/if}

  <div class="chips">
    <button
      class="chip project-chip"
      class:has-project={activeProject !== null}
      data-testid="project-chip"
      aria-haspopup="menu"
      aria-expanded={open}
      onclick={toggle}
    >
      <span class="dot" class:on={activeProject !== null}></span>
      <Icon name="ls-project" size={14} />
      <!-- UI-SPEC §0.1.6: the chip is `Name · Engine`. Which editor an export lands in is half
           the fact; a bare project name leaves the Unity version to be guessed. -->
      <span class="label">
        {activeProject?.name ?? "No project"}{activeProject?.unity_version
          ? ` · ${activeProject.unity_version}`
          : ""}
      </span>
      <span class="caret" aria-hidden="true">▾</span>
    </button>
    <span class="chip" class:warn={!engineReady}>
      {engineReady ? "✦ local" : "✦ shell only"}
    </span>
  </div>

  {#if open}
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="switcher-scrim" onclick={() => (open = false)}>
      <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
      <div
        class="switcher"
        role="menu"
        tabindex="-1"
        onclick={(e) => e.stopPropagation()}
        data-testid="project-switcher"
      >
        <div class="section-title">Export targets</div>
        {#if projectsList.length === 0}
          <p class="empty">No Unity project connected.</p>
        {:else}
          {#each projectsList as p (p.id)}
            <button
              class="project-row"
              class:active={p.is_active}
              onclick={() => switchProject(p)}
              data-testid="project-row"
            >
              <span class="dot" class:on={p.is_active}></span>
              <span class="pinfo">
                <span class="pname">
                  {p.name}
                  {#if p.is_active}<span class="tick" aria-hidden="true">✓</span>{/if}
                </span>
                <span class="pmeta">{p.unity_version ?? "Unity"} · {p.project_path}</span>
              </span>
            </button>
          {/each}
        {/if}

        <div class="section-title">Recent exports</div>
        {#if batches.length === 0}
          <p class="empty">Nothing exported yet.</p>
        {:else}
          {#each batches as b (b.batch_id)}
            <div class="batch-row" data-testid="export-batch">
              <span class="bmeta">
                ✓ {b.file_count} {b.file_count === 1 ? "file" : "files"} → {b.dest || "/"}
                <span class="rel">{relTime(b.imported_at)}</span>
              </span>
              <span class="bproj">{b.project_name}</span>
              <button
                class="undo"
                class:armed={confirming === b.batch_id}
                disabled={undoing === b.batch_id}
                onclick={() => undoBatch(b.batch_id)}
                data-testid="undo-batch"
              >
                {undoing === b.batch_id ? "…" : confirming === b.batch_id ? "confirm?" : "↩ undo"}
              </button>
            </div>
          {/each}
        {/if}

        <button class="connect" onclick={connectProject} data-testid="connect-project">
          + Connect Unity project…
        </button>
        <p class="footnote">Recent exports undo per batch — catalog assets are never touched.</p>
      </div>
    </div>
  {/if}
</header>

<style>
  .topbar {
    height: var(--topbar-h);
    flex: none;
    display: flex;
    align-items: center;
    gap: var(--sp-4);
    padding: 0 var(--sp-3);
    background: var(--ls-surface-raised);
    border-bottom: 1px solid var(--ls-border);
    position: relative;
    z-index: 2;
  }

  .brand {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding-right: var(--sp-2);
  }
  .wordmark {
    font-family: var(--ff-display);
    font-weight: 700;
    font-size: var(--fs-lg);
    letter-spacing: 0.08em;
    color: var(--ls-gold);
  }

  .search {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    flex: 1 1 auto;
    max-width: 400px;
    height: var(--control-h);
    padding: 0 var(--sp-2);
    background: var(--ls-bg);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-md);
    color: var(--ls-text-dim);
  }
  .search:focus-within {
    border-color: var(--ls-gold);
  }
  /* DESIGN-GUIDELINES §7: the active mode colours the input's focus ring — gold for Tags,
     arcane-teal for AI — so you always know which engine you're querying. */
  .search.ai:focus-within {
    border-color: var(--ls-arcane);
  }
  .search input {
    flex: 1;
    min-width: 0;
    background: none;
    border: none;
    outline: none;
    color: var(--ls-text);
    font: inherit;
    user-select: text;
  }
  .search input::placeholder {
    color: var(--ls-text-dim);
  }

  /* Standing on its own now (it used to sit inside the search box), the toggle needs its own
     edge or it floats between the wordmark and the input belonging to neither. */
  .mode {
    display: flex;
    gap: 2px;
    background: var(--ls-bg);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-md);
    padding: 2px;
    flex: none;
  }
  .mode button,
  .tabs button {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
    padding: 3px var(--sp-2);
    border-radius: var(--r-sm);
    font-size: var(--fs-sm);
    color: var(--ls-text-dim);
    opacity: 0.75;
  }
  .mode button.on,
  .tabs button.on {
    background: var(--ls-overlay);
    color: var(--ls-text);
    opacity: 1;
  }
  /* UI-SPEC §0.3: the active tab carries a 2px underline in its bucket token colour. */
  .tabs button.on {
    box-shadow: inset 0 -2px 0 var(--tab, var(--ls-gold));
  }
  .tabs .count {
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
  }
  .tabs button.on .count {
    color: var(--ls-text-muted);
  }
  .mode button:hover,
  .tabs button:hover {
    color: var(--ls-text);
  }

  .tabs {
    display: flex;
    gap: var(--sp-1);
  }

  /* Health meter (UI-SPEC §0.1.5): min-width 180px, label over a 6px gold-gradient bar. */
  .health {
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: 3px;
    min-width: 180px;
    padding: 0 var(--sp-2);
    white-space: nowrap;
    flex: none;
  }
  .health-text {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: var(--sp-2);
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
  }
  .health-id {
    color: var(--ls-gold);
  }
  .health-bar {
    width: 100%;
    height: 6px;
    border-radius: var(--r-pill);
    background: var(--ls-border-strong);
    overflow: hidden;
  }
  .health-seg {
    display: block;
    height: 100%;
    width: var(--pct, 0%);
    background: linear-gradient(90deg, var(--ls-gold), var(--ls-gold-bright));
    border-radius: var(--r-pill);
    box-shadow: 0 0 6px color-mix(in srgb, var(--ls-gold) 55%, transparent);
    transition: width 240ms ease;
  }

  .chips {
    margin-left: auto;
    display: flex;
    gap: var(--sp-2);
    align-items: center;
  }
  .chip {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
    font-size: var(--fs-sm);
    color: var(--ls-text-muted);
    background: var(--ls-surface);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-pill);
    padding: 3px var(--sp-3);
  }
  .chip.warn {
    color: var(--ls-warning);
    border-color: color-mix(in srgb, var(--ls-warning) 40%, transparent);
  }

  /* Project switcher popover (UI-SPEC §16.1): a dropdown under the chip. */
  .project-chip {
    cursor: pointer;
    font: inherit;
  }
  .project-chip.has-project {
    color: var(--ls-text);
  }
  .project-chip .dot,
  .project-row .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex: none;
    background: var(--ls-text-muted);
  }
  .project-chip .dot.on,
  .project-row .dot.on {
    background: var(--ls-success);
    box-shadow: 0 0 6px color-mix(in srgb, var(--ls-success) 60%, transparent);
  }
  .project-chip .label {
    max-width: 160px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .project-chip .caret {
    font-size: 10px;
    color: var(--ls-text-dim);
  }

  .switcher-scrim {
    position: fixed;
    inset: 0;
    z-index: 40;
  }
  .switcher {
    position: absolute;
    top: calc(var(--topbar-h) + 4px);
    right: var(--sp-3);
    width: 310px;
    display: flex;
    flex-direction: column;
    gap: var(--sp-1);
    padding: var(--sp-2);
    background: var(--ls-overlay);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-md);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.4);
    color: var(--ls-text);
  }
  .section-title {
    font-size: var(--fs-xs);
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--ls-text-dim);
    padding: var(--sp-1);
  }
  .empty {
    margin: 0;
    padding: var(--sp-1);
    font-size: var(--fs-sm);
    color: var(--ls-text-muted);
  }
  .project-row {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-1) var(--sp-2);
    border-radius: var(--r-sm);
    text-align: left;
    font: inherit;
  }
  .project-row:hover {
    background: var(--ls-surface);
  }
  .project-row.active {
    background: color-mix(in srgb, var(--ls-gold) 12%, transparent);
  }
  .project-row .pinfo {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .project-row .pname {
    font-size: var(--fs-sm);
    color: var(--ls-text);
    display: flex;
    align-items: center;
    gap: var(--sp-1);
  }
  .project-row .tick {
    color: var(--ls-gold);
  }
  .project-row .pmeta {
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .batch-row {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-1) var(--sp-2);
    border-radius: var(--r-sm);
  }
  .batch-row:hover {
    background: var(--ls-surface);
  }
  .batch-row .bmeta {
    flex: 1;
    min-width: 0;
    font-size: var(--fs-sm);
    color: var(--ls-text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .batch-row .rel {
    color: var(--ls-text-dim);
  }
  .batch-row .bproj {
    flex: none;
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
    max-width: 90px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .batch-row .undo {
    flex: none;
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
    text-decoration: underline;
    font: inherit;
  }
  .batch-row .undo:hover:not(:disabled) {
    color: var(--ls-gold);
  }
  .batch-row .undo.armed {
    color: var(--ls-danger);
  }
  .batch-row .undo:disabled {
    opacity: 0.5;
    cursor: wait;
  }
  .connect {
    margin-top: var(--sp-1);
    padding: var(--sp-2);
    border: 1px dashed var(--ls-border-strong);
    border-radius: var(--r-sm);
    font-size: var(--fs-sm);
    color: var(--ls-text-dim);
    text-align: left;
    font: inherit;
  }
  .connect:hover {
    color: var(--ls-gold);
    border-color: var(--ls-gold);
  }
  .footnote {
    margin: var(--sp-1) 0 0;
    padding: 0 var(--sp-1);
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
  }
</style>
