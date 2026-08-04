<script lang="ts">
  /**
   * Traits (UI-SPEC §9) — the vocabulary, grouped by namespace, most-used first.
   *
   * **Read-only.** Authoring (create, rename, merge, teach) writes to `tag`, and the writer thread
   * has no ops for it; that is its own slice with its own undo story. What this screen does buy
   * today is the other half of §9: seeing what the vocabulary actually is, and jumping from a
   * trait to the assets carrying it.
   */
  import { vocabulary, type VocabRow } from "./api";
  import { view } from "./nav.svelte";

  let rows = $state<VocabRow[] | null>(null);
  let error = $state<string | null>(null);
  let q = $state("");

  $effect(() => {
    view.dataVersion;
    vocabulary()
      .then((r) => {
        rows = r;
        error = null;
      })
      .catch((e) => (error = String(e)));
  });

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

  /** Jump to the library filtered to this trait — the ⚑ filter's trait group, not a text search. */
  function browse(value: string) {
    view.clearFilter();
    view.filter = { traits: [value], tri_buckets: [], sizes: [] };
    view.go("library");
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
    {:else if rows.length === 0}
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
              {#each group.traits as t (t.namespace + t.value)}
                <li>
                  <button class="trait" onclick={() => browse(t.value)} data-testid="trait-row">
                    <span class="name">{t.value}</span>
                    {#if t.teach}
                      <span class="pill">teaching</span>
                    {/if}
                    <span class="stat num">
                      {t.usage_count.toLocaleString()}
                      <span class="split">
                        ({t.manual_count.toLocaleString()} manual · {t.ai_count.toLocaleString()} AI)
                      </span>
                    </span>
                  </button>
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
    align-items: center;
    gap: var(--sp-3);
    padding: var(--sp-4) var(--sp-5) var(--sp-3);
  }
  h1 {
    font-size: var(--fs-2xl);
  }
  .search {
    margin-left: auto;
    width: min(280px, 40%);
    padding: var(--sp-1) var(--sp-2);
    background: var(--ls-bg);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-md);
    color: var(--ls-text);
    font: inherit;
    font-size: var(--fs-sm);
  }
  .body {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 0 var(--sp-5) var(--sp-5);
  }
  .msg {
    max-width: 70ch;
    color: var(--ls-text-dim);
  }
  .msg.danger {
    color: var(--ls-danger);
  }

  .namespaces {
    display: flex;
    flex-direction: column;
    gap: var(--sp-4);
  }
  h2 {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    margin: 0 0 var(--sp-1);
    font-size: var(--fs-xs);
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--ls-text-dim);
  }
  h2 .count {
    color: var(--ls-text-muted);
  }
  ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .trait {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    width: 100%;
    padding: var(--sp-1) var(--sp-2);
    border: none;
    border-top: 1px solid var(--ls-border);
    background: none;
    color: var(--ls-text);
    font: inherit;
    font-size: var(--fs-sm);
    text-align: left;
    cursor: pointer;
  }
  .trait:hover {
    background: var(--ls-overlay);
  }
  .pill {
    padding: 0 var(--sp-1);
    border: 1px solid var(--ls-arcane, var(--ls-border-strong));
    border-radius: var(--r-pill);
    font-size: var(--fs-xs);
    color: var(--ls-arcane, var(--ls-text-dim));
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
</style>
