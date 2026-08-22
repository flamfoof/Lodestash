<script lang="ts">
  import EmptyState from "./EmptyState.svelte";
  /**
   * Identify — the appraisal workspace (UI-SPEC §3, FEATURES §3).
   *
   * Shows the backlog: everything still Unidentified. The **session set** per §3 is the
   * unidentified backlog today — nothing can be appraised yet, because the identify engine
   * (tagger + embeddings + the job pump) is the B7 engine build, not this shell. So the screen
   * is the honest workbench: the backlog, the count, the empty state — and the Identify-all
   * action, present but off, saying exactly why it is off instead of pretending to run.
   */
  import Card from "./Card.svelte";
  import {
    gridPage,
    identifyAll,
    identifyIds,
    pumpProgress,
    getSettings,
    setSettings,
    libraryStatus,
    type GridItem,
    type PumpProgress,
    type Settings,
    type SourceSummary,
  } from "./api";
  import { view } from "./nav.svelte";

  /**
   * ADR-0008: appraisal is a background job, not a screen you sit on. The writer's idle pass does
   * the work whenever `auto_identify` is on, so this screen is a dashboard — what is running, how
   * far it has got, and where the backlog actually lives — rather than a place you wait.
   *
   * Pause and Resume write that same setting, because it *is* the switch. Inventing a second flag
   * beside it would give the app two answers to "is appraisal running?".
   */
  let settings = $state<Settings | null>(null);
  let togglingPause = $state(false);
  let backlog = $state<{ source: SourceSummary; remaining: number }[]>([]);

  $effect(() => {
    view.dataVersion;
    getSettings()
      .then((s) => (settings = s))
      .catch(() => {});
  });

  /** Per-source remaining counts — the unit people think in, not "all 3,912". */
  $effect(() => {
    view.dataVersion;
    let live = true;
    (async () => {
      try {
        const st = await libraryStatus();
        const rows = await Promise.all(
          st.sources.map(async (source) => ({
            source,
            remaining: (await unidentifiedIn(source.name)).total,
          })),
        );
        if (live) backlog = rows.filter((r) => r.remaining > 0);
      } catch {
        if (live) backlog = [];
      }
    })();
    return () => {
      live = false;
    };
  });

  const unidentifiedIn = (source: string) =>
    gridPage({
      tab: "all",
      search: "",
      search_mode: "tags",
      ids: [],
      status: "unidentified",
      sort_by: "name",
      sort_asc: true,
      favorites_only: false,
      trashed_only: false,
      filter: { traits: [], tri_buckets: [], sizes: [] },
      source,
      page: 0,
      page_size: 500,
    });

  async function togglePause() {
    const s = settings;
    if (!s || togglingPause) return;
    togglingPause = true;
    try {
      const next = { ...s, auto_identify: !s.auto_identify };
      await setSettings(next);
      settings = next;
    } catch (e) {
      error = String(e);
    } finally {
      togglingPause = false;
    }
  }

  /** Appraise one source's backlog in a single pass. */
  async function appraiseSource(name: string) {
    if (running) return;
    running = true;
    try {
      const page = await unidentifiedIn(name);
      const ids = page.items.map((i) => i.asset_id);
      if (ids.length === 0) return;
      await identifyIds(ids);
      view.dataVersion += 1;
    } catch (e) {
      error = String(e);
    } finally {
      running = false;
    }
  }

  let items = $state<GridItem[]>([]);
  let total = $state(0);
  let error = $state<string | null>(null);
  let running = $state(false);
  let outcome = $state<string | null>(null);
  // B11d: the persist thread may be appraising on its own (auto-identify idle pass) — the
  // header reflects it instead of claiming idle while a pass is running.
  let pumpBusy = $state(false);
  // B11d surface: the shared pass progress (done/total) — a real bar instead of a spinner.
  let pump = $state<PumpProgress>({ active: false, done: 0, total: 0, failed: 0 });
  let pumpPoll: ReturnType<typeof setInterval> | null = null;
  $effect(() => {
    view.route;
    if (view.route !== "identify") return;
    pumpPoll ??= setInterval(async () => {
      pump = await pumpProgress();
      pumpBusy = pump.active;
    }, 1000);
    return () => {
      clearInterval(pumpPoll!);
      pumpPoll = null;
      pumpBusy = false;
      pump = { active: false, done: 0, total: 0, failed: 0 };
    };
  });

  // ✦ Identify all (B11): one bounded appraise pass on the persist thread; the backlog count
  // shrinks as assets get embeddings + tags, and the grid re-reads on completion.
  async function appraise() {
    if (running) return;
    running = true;
    outcome = null;
    try {
      const r = await identifyAll();
      // Session set (UI-SPEC §3): the ids that just became identified — show them instead of
      // letting them vanish into the library.
      session = r.promoted;
      sessionOn = r.promoted.length > 0;
      outcome = r.changed > 1
        ? `Appraised ${r.changed.toLocaleString()} assets — ${r.promoted.length} newly identified.`
        : "No local embedder models — install SigLIP/CLAP (Settings → AI) to appraise.";
    } catch (e) {
      outcome = `Appraise failed: ${String(e)}`;
    } finally {
      running = false;
      view.dataVersion += 1; // re-read the backlog
    }
  }

  // The just-appraised session set: `sessionOn` swaps the grid to exactly these ids.
  let session = $state<number[]>([]);
  let sessionOn = $state(false);
  $effect(() => {
    view.dataVersion;
    gridPage({
      tab: "all",
      search: "",
      search_mode: "tags",
      ids: sessionOn ? session : [],
      status: sessionOn ? "all" : "unidentified",
      sort_by: "name",
      sort_asc: true,
      favorites_only: false,
      trashed_only: false,
      filter: { traits: [], tri_buckets: [], sizes: [] },
      page: 0,
      page_size: 200,
    })
      .then((r) => {
        items = r.items;
        // The page is capped at 200 — the header must count the whole backlog, not the page.
        total = r.total;
        error = null;
      })
      .catch((e) => (error = String(e)));
  });
</script>

<section class="screen">
  <header>
    <div class="title">
      <h1>Appraise the Backlog</h1>
      <p class="subtitle">
        Fresh loot is Unidentified — no tags, no embeddings, invisible to AI search. Appraisal
        runs once per asset, locally.
      </p>
    </div>
    <div class="right">
      {#if sessionOn}
        <span class="count mono" data-testid="session-count">{session.length.toLocaleString()}</span>
        <span class="count-label">just appraised</span>
        <button class="link" onclick={() => (sessionOn = false)}>back to backlog</button>
      {:else}
        <span class="count mono">{total.toLocaleString()}</span>
        <span class="count-label">unidentified</span>
      {/if}
      <button
        class="identify"
        disabled={running}
        onclick={appraise}
        title={
          running || pumpBusy
            ? "Appraising… this pass runs up to ~20s on the persist thread."
            : "Run one appraise pass — local AI tags + embeddings for the unidentified backlog."
        }
        data-testid="identify-all"
      >
        {running || pumpBusy
          ? "✦ Appraising…"
          : `✦ Identify all (${total.toLocaleString()})`}
      </button>
      {#if pump.active && pump.total > 0}
        <div class="pass-progress" data-testid="pass-progress">
          <div class="bar">
            <div
              class="fill"
              style="width: {Math.round((pump.done / pump.total) * 100)}%"
            ></div>
          </div>
          <span class="mono"
            >{pump.done.toLocaleString()} / {pump.total.toLocaleString()}{#if pump.failed > 0}
              · {pump.failed} failed{/if}</span
          >
        </div>
      {/if}
      {#if outcome}
        <p class="offline" data-testid="identify-outcome">{outcome}</p>
      {:else}
        <p class="offline">
          Appraises with the local AI tagger + embeddings (SigLIP/CLAP) when those models are
          installed — with none, this pass still reconciles statuses and previews.
        </p>
      {/if}
    </div>
  </header>

  <!-- ADR-0008: the state of the background job, and where the backlog is. This is what makes the
       screen a dashboard you can walk away from rather than a wall you sit at. -->
  {#if settings}
    <div class="bg-row" class:paused={!settings.auto_identify} data-testid="appraisal-state">
      <span class="mark">✦</span>
      <div class="what">
        {#if settings.auto_identify}
          <div class="line">Running in the background</div>
          <div class="sub">You can browse, stage and import while it works.</div>
        {:else}
          <div class="line">Appraisal paused</div>
          <div class="sub">
            <span class="num">{total.toLocaleString()}</span> assets stay Unidentified — and
            invisible to AI search — until it resumes. Nothing is lost.
          </div>
        {/if}
      </div>
      <button
        class="pause"
        disabled={togglingPause}
        onclick={togglePause}
        data-testid="appraisal-pause"
      >
        {settings.auto_identify ? "⏸ Pause" : "▶ Resume"}
      </button>
    </div>
  {/if}

  {#if backlog.length > 0}
    <div class="backlog" data-testid="backlog-by-source">
      <span class="kicker">Where the backlog is</span>
      {#each backlog as row (row.source.id)}
        <div class="bl-row">
          <span class="bl-name">{row.source.name}</span>
          <span class="bl-bar" aria-hidden="true">
            <span
              class="bl-fill"
              style="width: {Math.min(100, Math.round((row.remaining / Math.max(1, row.source.asset_count)) * 100))}%"
            ></span>
          </span>
          <span class="bl-count num">{row.remaining.toLocaleString()}</span>
          <button
            class="bl-go"
            disabled={running}
            onclick={() => appraiseSource(row.source.name)}
            data-testid="appraise-source-{row.source.id}"
          >
            ✦ Appraise
          </button>
        </div>
      {/each}
      <p class="bl-note">
        A source or a pack is the unit you think in — appraise one without waiting for all
        <span class="num">{total.toLocaleString()}</span>.
      </p>
    </div>
  {/if}

  <div class="body">
    {#if error}
      <p class="msg danger">{error}</p>
    {:else if items.length === 0}
      <EmptyState
        testid="backlog-clear"
        icon="ls-nav-identify"
        title="Nothing left to appraise"
        line="Every asset has been identified. New files are appraised as they arrive."
        action="Go to Library →"
        onaction={() => view.go("library")}
      />
    {:else}
      <div class="grid" data-testid="backlog">
        {#each items as item (item.asset_id)}
          <Card
            {item}
            selected={view.selected === item.asset_id}
            onselect={(id) => (view.selected = id)}
            onopen={(id) => (view.selected = id)}
          />
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
    border-bottom: 1px solid var(--ls-border);
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
    max-width: 72ch;
  }
  .right {
    flex: none;
    text-align: right;
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: var(--sp-1);
    max-width: 300px;
  }
  .count {
    font-size: var(--fs-2xl);
    color: var(--ls-arcane-bright);
    line-height: 1;
  }
  .count-label {
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
  }
  .identify {
    margin-top: var(--sp-2);
    padding: 6px var(--sp-4);
    background: none;
    border: 1px solid var(--ls-arcane);
    border-radius: var(--r-md);
    color: var(--ls-arcane-bright);
    font: inherit;
    font-size: var(--fs-sm);
    font-weight: 600;
    cursor: not-allowed;
    opacity: 0.55;
  }
  .offline {
    margin: var(--sp-1) 0 0;
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
    text-align: right;
  }
  .pass-progress {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    margin: var(--sp-2) 0 0;
    justify-content: flex-end;
  }
  .pass-progress .bar {
    width: 160px;
    height: 6px;
    border-radius: 3px;
    background: var(--ls-border);
    overflow: hidden;
  }
  .pass-progress .fill {
    height: 100%;
    background: var(--ls-gold);
    transition: width 0.3s ease;
  }
  .pass-progress span {
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
  }
  .body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: var(--sp-4) var(--sp-5);
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(var(--card, 148px), 1fr));
    gap: var(--sp-3);
  }
  .bg-row {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    margin: 0 var(--sp-5) var(--sp-3);
    padding: var(--sp-3);
    border-radius: var(--r-md);
    background: color-mix(in srgb, var(--ls-arcane) 6%, transparent);
    border: 1px solid color-mix(in srgb, var(--ls-arcane) 35%, transparent);
  }
  .bg-row.paused {
    background: none;
    border-color: var(--ls-border-strong);
  }
  .bg-row .mark {
    color: var(--ls-arcane);
    font-size: var(--fs-lg);
  }
  .bg-row.paused .mark {
    color: var(--ls-text-dim);
  }
  .bg-row .what {
    flex: 1;
    min-width: 0;
  }
  .bg-row .line {
    font-size: var(--fs-sm);
    color: var(--ls-text);
  }
  .bg-row .sub {
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
  }
  .pause {
    flex: none;
    height: var(--control-h);
    padding: 0 var(--sp-3);
    font: inherit;
    font-size: var(--fs-sm);
    color: var(--ls-text-muted);
    background: none;
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-md);
    cursor: pointer;
  }
  .pause:hover:not(:disabled) {
    border-color: var(--ls-gold);
    color: var(--ls-text);
  }
  .pause:disabled {
    opacity: 0.5;
    cursor: wait;
  }

  .backlog {
    margin: 0 var(--sp-5) var(--sp-4);
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
  }
  .kicker {
    font-size: var(--fs-xs);
    font-weight: 600;
    letter-spacing: 0.09em;
    text-transform: uppercase;
    color: var(--ls-text-dim);
  }
  .bl-row {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    font-size: var(--fs-sm);
  }
  .bl-name {
    width: 170px;
    flex: none;
    color: var(--ls-text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .bl-bar {
    flex: 1;
    height: 8px;
    border-radius: var(--r-pill);
    background: var(--ls-border-strong);
    overflow: hidden;
  }
  .bl-fill {
    display: block;
    height: 100%;
    background: var(--ls-status-unidentified);
  }
  .bl-count {
    width: 62px;
    flex: none;
    text-align: right;
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
  }
  .bl-go {
    flex: none;
    height: 24px;
    padding: 0 var(--sp-2);
    font: inherit;
    font-size: var(--fs-xs);
    color: var(--ls-arcane);
    background: none;
    border: 1px solid color-mix(in srgb, var(--ls-arcane) 45%, transparent);
    border-radius: var(--r-md);
    cursor: pointer;
  }
  .bl-go:hover:not(:disabled) {
    border-color: var(--ls-arcane);
    color: var(--ls-arcane-bright);
  }
  .bl-go:disabled {
    opacity: 0.5;
    cursor: wait;
  }
  .bl-note {
    margin: 0;
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
  }

  .msg {
    color: var(--ls-text-dim);
  }
  .msg.danger {
    color: var(--ls-danger);
  }
  .mono {
    font-family: var(--ff-mono);
    font-variant-numeric: tabular-nums;
  }
</style>
