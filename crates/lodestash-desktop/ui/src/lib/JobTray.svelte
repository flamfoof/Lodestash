<script lang="ts">
  /**
   * Job tray (B2b, UI-SPEC §5 / §16.3): a bottom-left bar for long-running work — pack
   * extraction (with byte progress + per-job cancel), appraise kicks (spinner), imports.
   * Active rows keep the tray open; the session ring of finished jobs stays under a toggle.
   */
  import { jobs, cancelJob, onChanged, type JobsView } from "./api";

  let view = $state<JobsView>({ active: [], recent: [] });
  let open = $state(false);
  let error = $state<string | null>(null);

  // The writer pushes `ls:changed` events when the tray changes (pump events, B2b) — no
  // polling. The initial fetch covers whatever happened before this component mounted.
  $effect(() => {
    jobs()
      .then((jv) => (view = jv))
      .catch((e) => (error = String(e)));
    const off = onChanged("jobs", async () => {
      try {
        view = await jobs();
        if (view.active.length > 0) open = true;
      } catch (e) {
        error = String(e);
      }
    });
    return off;
  });

  async function cancel(jobId: number) {
    try {
      await cancelJob(jobId);
    } catch (e) {
      error = String(e);
    }
  }

  function pct(p: [number, number] | null): number {
    if (!p || p[1] <= 0) return 0;
    return Math.min(100, Math.round((p[0] / p[1]) * 100));
  }

  function ago(atUnix: number): string {
    const s = Math.max(0, Math.floor(Date.now() / 1000) - atUnix);
    if (s < 60) return `${s}s ago`;
    if (s < 3600) return `${Math.floor(s / 60)}m ago`;
    return `${Math.floor(s / 3600)}h ago`;
  }

  const activeCount = $derived(view.active.length);
</script>

{#if error}
  <p class="err">{error}</p>
{/if}

{#if open || activeCount > 0}
  <aside class="tray" data-testid="job-tray" class:has-active={activeCount > 0}>
    <header>
      <span class="label">{activeCount > 0 ? `Jobs · ${activeCount} active` : "Recent activity"}</span>
      <div class="spacer"></div>
      <button
        class="mini"
        onclick={() => (open = !open)}
        title={open ? "Hide recent activity" : "Show recent activity"}
        aria-expanded={open}
      >
        {open ? "▾" : "▸"}
      </button>
    </header>

    {#if activeCount > 0}
      <ul class="active">
        {#each view.active as j (j.id)}
          <li class="job" data-testid="job-{j.id}">
            <span class="kind" aria-hidden="true">{j.kind === "extract" ? "⇩" : j.kind === "appraise" ? "✦" : "⇧"}</span>
            <span class="label">{j.label}</span>
            {#if j.progress}
              <span class="bar" aria-hidden="true"><i style="width: {pct(j.progress)}%"></i></span>
              <span class="num">{pct(j.progress)}%</span>
            {:else}
              <span class="spin" aria-hidden="true"></span>
            {/if}
            {#if j.cancellable}
              <button
                class="mini cancel"
                title="Cancel"
                aria-label={`Cancel ${j.label}`}
                onclick={() => cancel(j.id)}
              >
                ✕
              </button>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}

    {#if open}
      <ul class="recent">
        {#each view.recent.slice(0, 8) as r (r.id)}
          <li class="job recent" data-testid="recent-{r.id}">
            <span
              class="outcome"
              class:bad={!r.ok}
              aria-hidden="true"
              title={r.cancelled ? "Cancelled" : r.ok ? "Done" : "Failed"}
            >
              {r.cancelled ? "⊘" : r.ok ? "✓" : "✕"}
            </span>
            <span class="label">{r.label}</span>
            {#if r.changed > 0}
              <span class="num">{r.changed.toLocaleString()}</span>
            {/if}
            <span class="when num">{ago(r.at_unix)}</span>
          </li>
        {/each}
        {#if view.recent.length === 0}
          <li class="none">Nothing finished this session yet.</li>
        {/if}
      </ul>
    {/if}
  </aside>
{/if}

<style>
  .tray {
    position: fixed;
    left: var(--sp-2);
    bottom: var(--sp-2);
    z-index: 60; /* below toasts (70) and the staging tray, above the grid */
    width: 320px;
    max-height: 40vh;
    overflow-y: auto;
    background: var(--ls-overlay);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-md);
    box-shadow: 0 4px 16px rgb(0 0 0 / 0.35);
    padding: var(--sp-1) var(--sp-2);
    font-size: var(--fs-xs);
  }
  header {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
    padding: var(--sp-1) 0;
    color: var(--ls-text-dim);
  }
  .spacer {
    flex: 1;
  }
  .mini {
    background: none;
    border: none;
    color: var(--ls-text-dim);
    cursor: pointer;
    padding: 0 var(--sp-1);
    font-size: var(--fs-sm);
  }
  .mini:hover {
    color: var(--ls-text);
  }
  .mini.cancel {
    color: var(--ls-text-dim);
  }
  .mini.cancel:hover {
    color: var(--ls-danger);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .job {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
    padding: var(--sp-1) 0;
    border-top: 1px solid var(--ls-border);
  }
  .kind,
  .outcome {
    width: 14px;
    text-align: center;
    flex: none;
    color: var(--ls-gold);
  }
  .outcome.bad {
    color: var(--ls-danger);
  }
  .label {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--ls-text);
  }
  .num {
    color: var(--ls-text-dim);
    font-variant-numeric: tabular-nums;
  }
  .bar {
    flex: none;
    width: 72px;
    height: 6px;
    background: var(--ls-border);
    border-radius: 3px;
    overflow: hidden;
  }
  .bar i {
    display: block;
    height: 100%;
    background: var(--ls-gold);
    transition: width 200ms ease;
  }
  .spin {
    flex: none;
    width: 12px;
    height: 12px;
    border: 2px solid var(--ls-border-strong);
    border-top-color: var(--ls-gold);
    border-radius: 50%;
    animation: spin 800ms linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  .when {
    font-size: var(--fs-xs);
  }
  .none {
    padding: var(--sp-1) 0;
    color: var(--ls-text-dim);
    border-top: 1px solid var(--ls-border);
  }
  .err {
    position: fixed;
    left: var(--sp-2);
    bottom: var(--sp-2);
    z-index: 65;
    background: var(--ls-danger);
    color: white;
    padding: var(--sp-1) var(--sp-2);
    border-radius: var(--r-sm);
    font-size: var(--fs-xs);
  }
</style>
