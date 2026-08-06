<script lang="ts">
  /**
   * Traits (UI-SPEC §9) — the vocabulary, grouped by namespace, most-used first. **A working
   * screen (B8a):** the NEW TRAIT card, the ✦ promote-existing-tags strip, the teaching toggle,
   * per-trait style (6-colour palette), and demote — all through the same writer thread every
   * other catalog write goes through. Chips across the app (⚑ cloud, cards) pick up a trait's
   * colour from the shared `traitColor` map.
   *
   * The Mapping tab is not here — taxonomy/category/rule authoring needs a core slice first
   * (only `list_rules`/`apply_mapping` exist), so it stays out of this card.
   */
  import { vocabulary, upsertTrait, unpromoteTrait, setTraitTeach, setTraitStyle, type VocabRow } from "./api";
  import { refreshTraitColors } from "./traits.svelte";
  import { view } from "./nav.svelte";

  let rows = $state<VocabRow[] | null>(null);
  let error = $state<string | null>(null);
  let actionError = $state<string | null>(null);
  let q = $state("");
  let ntName = $state("");
  let ntNamespace = $state("trait");

  const STYLE_SWATCHES = [
    "--ls-type-model",
    "--ls-type-sound",
    "--ls-type-image",
    "--ls-type-material",
    "--ls-type-sprite",
    "--ls-type-font",
  ] as const;

  async function load() {
    try {
      rows = await vocabulary();
      error = null;
    } catch (e) {
      error = String(e);
    }
  }

  $effect(() => {
    view.dataVersion;
    load();
  });

  /** Run a write, then re-ask everything the write could have changed. */
  async function write(run: () => Promise<unknown>) {
    actionError = null;
    try {
      await run();
      view.dataVersion += 1;
      await refreshTraitColors();
    } catch (e) {
      actionError = String(e);
    }
  }

  let shown = $derived(
    (rows ?? []).filter((t) => t.value.toLowerCase().includes(q.trim().toLowerCase())),
  );

  let byNamespace = $derived.by(() => {
    const map = new Map<string, VocabRow[]>();
    for (const t of shown) {
      const list = map.get(t.namespace) ?? [];
      list.push(t);
      map.set(t.namespace, list);
    }
    return [...map.entries()]
      .map(([namespace, traits]) => ({
        namespace,
        traits,
        total: traits.reduce((a, t) => a + t.usage_count, 0),
      }))
      .sort((a, b) => b.total - a.total || a.namespace.localeCompare(b.namespace));
  });

  /** Plain tags — the ✦ promote strip (B8a: `is_trait = false` rows are already listed). */
  let promotable = $derived(shown.filter((t) => !t.is_trait));

  function browse(value: string) {
    view.clearFilter();
    view.filter = { traits: [value], tri_buckets: [], sizes: [] };
    view.go("library");
  }

  async function createTrait() {
    const name = ntName.trim();
    const namespace = ntNamespace.trim() || "trait";
    if (!name) return;
    await write(() => upsertTrait({ namespace, value: name, teach: false }));
    ntName = "";
  }
</script>

<section class="screen">
  <header>
    <h1>Traits</h1>
    <input class="search" placeholder="Filter traits" bind:value={q} />
  </header>

  <div class="body">
    {#if error}
      <p class="msg danger">{error}</p>
    {:else if !rows}
      <p class="msg">Loading…</p>
    {:else}
      {#if actionError}
        <p class="msg danger">{actionError}</p>
      {/if}

      <section class="card new-trait" data-testid="new-trait">
        <h2>NEW TRAIT</h2>
        <p class="hint">
          A trait is a first-class vocabulary term. Creating one with a name that already exists as
          a tag promotes that tag — its assignments stay.
        </p>
        <div class="fields">
          <input
            placeholder="Name"
            bind:value={ntName}
            data-testid="new-trait-name"
            onkeydown={(e) => e.key === "Enter" && createTrait()}
          />
          <input placeholder="Namespace" bind:value={ntNamespace} aria-label="Namespace" />
          <button class="gold" onclick={createTrait} disabled={!ntName.trim()} data-testid="new-trait-go">
            + Create
          </button>
        </div>
      </section>

      {#if promotable.length > 0}
        <section class="card promote" data-testid="promote-strip">
          <h2>✦ PROMOTE EXISTING TAGS</h2>
          <p class="hint">
            Tags the identifier or you have applied, but that are not yet vocabulary terms.
            Promoting one makes it a trait — the term survives even when no asset carries it.
          </p>
          <div class="chips">
            {#each promotable as t (t.id)}
              <button
                class="chip"
                title="Promote {t.namespace}:{t.value}"
                onclick={() =>
                  write(() => upsertTrait({ namespace: t.namespace, value: t.value, teach: false }))}
                data-testid="promote-{t.value}"
              >
                {t.namespace}:{t.value}
                <span class="count num">{t.usage_count}</span>
              </button>
            {/each}
          </div>
        </section>
      {/if}

      {#if rows.length === 0}
        <p class="msg" data-testid="empty">
          Nothing is tagged yet. Traits appear here once identification has run over some assets —
          they are what AI search matches on, so an empty vocabulary means an unsearchable library.
        </p>
      {:else if byNamespace.length === 0}
        <p class="msg" data-testid="empty">No trait matches “{q}”.</p>
      {:else}
        <div class="namespaces" data-testid="vocab">
          {#each byNamespace as group (group.namespace)}
            <section>
              <h2>{group.namespace}<span class="count num">{group.traits.length}</span></h2>
              <ul>
                {#each group.traits as t (t.id)}
                  {@const tinted = t.color ? `var(${t.color})` : undefined}
                  <li class="row" style="--tint: {tinted ?? 'var(--ls-text-dim)'}">
                    <button class="trait" onclick={() => browse(t.value)} data-testid="trait-row">
                      <i class="dot" aria-hidden="true"></i>
                      <span class="name">{t.value}</span>
                      {#if t.scope}
                        <span class="pill scope">{t.scope}</span>
                      {/if}
                      <span class="stat num">
                        {t.usage_count.toLocaleString()}
                        <span class="split">
                          ({t.manual_count.toLocaleString()} manual · {t.ai_count.toLocaleString()} AI)
                        </span>
                      </span>
                    </button>
                    <div class="ops">
                      <button
                        class="op teach"
                        class:on={t.teach}
                        onclick={() => write(() => setTraitTeach(t.id, !t.teach))}
                        title="Teach the identifier to recognise this trait"
                        data-testid="teach-{t.value}"
                      >
                        {t.teach ? "✦ teaching" : "✦ teach"}
                      </button>
                      <div class="swatches" role="group" aria-label="Chip colour">
                        {#each STYLE_SWATCHES as sw (sw)}
                          <button
                            class="swatch"
                            class:on={t.color === sw}
                            style="--sw: var({sw})"
                            aria-label={sw}
                            onclick={() => write(() => setTraitStyle(t.id, t.color === sw ? null : sw))}
                          ></button>
                        {/each}
                      </div>
                      <button
                        class="op demote"
                        onclick={() => write(() => unpromoteTrait(t.id))}
                        title="Demote to a plain tag — assignments stay"
                        data-testid="demote-{t.value}"
                      >
                        ✕
                      </button>
                    </div>
                  </li>
                {/each}
              </ul>
            </section>
          {/each}
        </div>
      {/if}
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
    align-items: center;
    gap: var(--sp-3);
    padding: var(--sp-4) var(--sp-5) var(--sp-3);
  }
  h1 {
    font-size: var(--fs-2xl);
  }
  .search {
    margin-left: auto;
    width: 240px;
    height: var(--control-h);
    padding: 0 var(--sp-2);
    background: var(--ls-surface);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-md);
    color: var(--ls-text);
    font: inherit;
  }
  .body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 0 var(--sp-5) var(--sp-5);
    display: flex;
    flex-direction: column;
    gap: var(--sp-4);
    max-width: 900px;
  }
  .card {
    background: var(--ls-surface);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-lg);
    padding: var(--sp-4);
  }
  .card h2 {
    margin: 0 0 var(--sp-1);
    font-family: var(--ff-mono);
    font-size: var(--fs-xs);
    font-weight: 600;
    letter-spacing: 0.1em;
    color: var(--ls-text-dim);
  }
  .hint {
    margin: 0 0 var(--sp-3);
    font-size: var(--fs-sm);
    color: var(--ls-text-muted);
  }
  .fields {
    display: flex;
    gap: var(--sp-2);
  }
  .fields input {
    flex: 1;
    height: var(--control-h);
    padding: 0 var(--sp-2);
    background: var(--ls-bg);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-md);
    color: var(--ls-text);
    font: inherit;
  }
  .fields input:focus {
    outline: none;
    border-color: var(--ls-gold);
  }
  .gold {
    height: var(--control-h);
    padding: 0 var(--sp-3);
    background: none;
    border: 1px solid var(--ls-gold);
    border-radius: var(--r-md);
    color: var(--ls-gold-bright);
    font: inherit;
    cursor: pointer;
    white-space: nowrap;
  }
  .gold:hover:not(:disabled) {
    background: color-mix(in srgb, var(--ls-gold) 15%, transparent);
  }
  .gold:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: var(--sp-1);
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-1);
    padding: 3px var(--sp-2);
    background: none;
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-pill);
    color: var(--ls-text-muted);
    font: inherit;
    font-size: var(--fs-xs);
    cursor: pointer;
  }
  .chip:hover {
    border-color: var(--ls-gold);
    color: var(--ls-text);
  }
  .count {
    color: var(--ls-text-dim);
  }
  .namespaces {
    display: flex;
    flex-direction: column;
    gap: var(--sp-4);
  }
  .namespaces h2 {
    display: flex;
    align-items: baseline;
    gap: var(--sp-2);
    margin: 0 0 var(--sp-2);
    font-size: var(--fs-lg);
  }
  .namespaces h2 .count {
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
  .row {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: 0 var(--sp-2);
    border-bottom: 1px solid color-mix(in srgb, var(--ls-border) 50%, transparent);
    background: var(--ls-surface);
  }
  .row:last-child {
    border-bottom: none;
  }
  .trait {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: 0;
    background: none;
    border: none;
    color: var(--ls-text);
    font: inherit;
    cursor: pointer;
    text-align: left;
    height: var(--row-h);
  }
  .trait:hover .name {
    color: var(--ls-gold);
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 2px;
    background: var(--tint, var(--ls-text-dim));
    flex: none;
    transform: rotate(45deg);
  }
  .name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .pill {
    padding: 1px var(--sp-2);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-pill);
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
    white-space: nowrap;
  }
  .pill.scope {
    color: var(--ls-arcane-bright);
    border-color: color-mix(in srgb, var(--ls-arcane) 45%, transparent);
  }
  .stat {
    margin-left: auto;
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
    white-space: nowrap;
  }
  .split {
    color: var(--ls-text-muted);
    opacity: 0.7;
  }
  .ops {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
    flex: none;
  }
  .op {
    background: none;
    border: 1px solid var(--ls-border);
    border-radius: var(--r-sm);
    color: var(--ls-text-dim);
    font: inherit;
    font-size: var(--fs-xs);
    cursor: pointer;
    padding: 2px var(--sp-2);
  }
  .op:hover {
    border-color: var(--ls-text-muted);
    color: var(--ls-text);
  }
  .op.teach.on {
    color: var(--ls-arcane-bright);
    border-color: var(--ls-arcane);
    background: color-mix(in srgb, var(--ls-arcane) 14%, transparent);
  }
  .op.demote {
    color: var(--ls-danger);
    border-color: transparent;
  }
  .op.demote:hover {
    border-color: var(--ls-danger);
  }
  .swatches {
    display: flex;
    gap: 3px;
    margin-left: var(--sp-1);
  }
  .swatch {
    width: 14px;
    height: 14px;
    border-radius: 3px;
    border: 1px solid var(--ls-border-strong);
    background: var(--sw);
    cursor: pointer;
    padding: 0;
  }
  .swatch:hover {
    transform: scale(1.15);
  }
  .swatch.on {
    outline: 2px solid var(--ls-gold);
    outline-offset: 1px;
  }
  .msg {
    color: var(--ls-text-dim);
    margin: 0;
  }
  .msg.danger {
    color: var(--ls-danger);
  }
</style>
