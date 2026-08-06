<script lang="ts">
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
  import { gridPage, type GridItem } from "./api";
  import { view } from "./nav.svelte";

  let items = $state<GridItem[]>([]);
  let total = $state(0);
  let error = $state<string | null>(null);

  $effect(() => {
    view.dataVersion;
    gridPage({
      tab: "all",
      search: "",
      status: "unidentified",
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
      <span class="count mono">{total.toLocaleString()}</span>
      <span class="count-label">unidentified</span>
      <button
        class="identify"
        disabled
        title="The appraiser engine binds with the B7 cutover — this shell shows the backlog, it does not appraise."
        data-testid="identify-all"
      >
        ✦ Identify all ({total.toLocaleString()})
      </button>
      <p class="offline">
        The appraiser (local AI tagger + embeddings) ships with the engine build — this shell
        shows the backlog honestly until then.
      </p>
    </div>
  </header>

  <div class="body">
    {#if error}
      <p class="msg danger">{error}</p>
    {:else if items.length === 0}
      <div class="empty" data-testid="backlog-clear">
        <span class="gem" aria-hidden="true">✓</span>
        <h2>Backlog clear</h2>
        <p>Every asset is appraised and searchable.</p>
        <button class="go" onclick={() => view.go("library")}>Go to Library →</button>
      </div>
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
  .empty {
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--sp-2);
    color: var(--ls-text-muted);
  }
  .gem {
    width: 48px;
    height: 48px;
    display: grid;
    place-items: center;
    border: 2px solid var(--ls-success);
    border-radius: 12px;
    transform: rotate(45deg);
    color: var(--ls-success);
    font-size: var(--fs-2xl);
    margin-bottom: var(--sp-2);
  }
  .gem::first-line {
    transform: rotate(-45deg);
  }
  .empty h2 {
    margin: 0;
    font-family: var(--ff-display);
    font-size: var(--fs-xl);
  }
  .empty p {
    margin: 0;
    font-size: var(--fs-sm);
  }
  .go {
    margin-top: var(--sp-2);
    background: none;
    border: 1px solid var(--ls-gold);
    border-radius: var(--r-md);
    color: var(--ls-gold-bright);
    font: inherit;
    font-size: var(--fs-sm);
    padding: 6px var(--sp-4);
    cursor: pointer;
  }
  .go:hover {
    background: color-mix(in srgb, var(--ls-gold) 14%, transparent);
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
