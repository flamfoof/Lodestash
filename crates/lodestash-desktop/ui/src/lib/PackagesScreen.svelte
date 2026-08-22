<script lang="ts">
  import EmptyState from "./EmptyState.svelte";
  /**
   * Packages (UI-SPEC §4): everything you own, publisher → pack.
   *
   * Counts come from the **indexed rows**, not `package.asset_count` — that column is populated
   * for 1 of 87 packs on a real Asset Store cache, so printing it would render a screen of blanks
   * next to packs that plainly contain things. The number shown is the number Browse → lands on.
   *
   * Packs with nothing indexed are kept, showing `0`: most of a download cache is archives nobody
   * has opened, and hiding them would claim the library holds six packs when it holds eighty-seven.
   * Extraction is not offered here — it needs the ingest path this shell does not have yet (B2b).
   */
  import Icon from "./Icon.svelte";
  import { packages, fmtSize, type PackageRow } from "./api";
  import { view } from "./nav.svelte";

  let rows = $state<PackageRow[] | null>(null);
  let error = $state<string | null>(null);
  let q = $state("");

  $effect(() => {
    view.dataVersion;
    packages()
      .then((r) => {
        rows = r;
        error = null;
      })
      .catch((e) => (error = String(e)));
  });

  let shown = $derived(
    (rows ?? []).filter((p) => {
      const needle = q.trim().toLowerCase();
      return (
        needle === "" ||
        p.name.toLowerCase().includes(needle) ||
        p.publisher.toLowerCase().includes(needle)
      );
    }),
  );

  /** Publisher → its packs, indexed packs first so the ones you can actually browse lead. */
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
        total: packs.reduce((a, p) => a + p.count, 0),
      }))
      .sort((a, b) => b.total - a.total || a.publisher.localeCompare(b.publisher));
  });

  function browse(pack: PackageRow) {
    view.packageFilter = pack.name;
    view.go("library");
  }
</script>

<section class="screen">
  <header>
    <h1>Packages</h1>
    <input class="search" placeholder="Filter packs and publishers" bind:value={q} />
  </header>

  <div class="body">
    {#if error}
      <p class="msg danger">{error}</p>
    {:else if !rows}
      <p class="msg">Loading…</p>
    {:else if byPublisher.length === 0}
      {#if q}
        <!-- Filtered-empty is not empty: it needs a way out, not a lesson about what a pack
             is (UI-SPEC §2.6). -->
        <p class="msg" data-testid="empty">No pack matches “{q}”.</p>
      {:else}
        <EmptyState
          testid="empty"
          icon="ls-nav-packages"
          title="No packs indexed"
          line="A pack is one publisher's release — Synty's Dungeon Realms, say. They appear here once a source is scanned."
        />
      {/if}
    {:else}
      <div class="publishers" data-testid="publishers">
        {#each byPublisher as group (group.publisher)}
          <section class="publisher">
            <h2>
              <Icon name="ls-nav-packages" size={16} />
              {group.publisher}
              <span class="count num">{group.total.toLocaleString()} assets</span>
            </h2>
            <ul>
              {#each group.packs as p (p.id)}
                <li class="pack" class:empty={p.count === 0}>
                  <span class="name" title={p.category ?? ""}>{p.name}</span>
                  {#if p.unidentified > 0}
                    <span class="pill unid num">{p.unidentified.toLocaleString()} unid</span>
                  {/if}
                  {#if p.extracted}
                    <span class="pill">extracted</span>
                  {/if}
                  <span class="stat num">
                    {p.count.toLocaleString()}
                    {#if p.bytes > 0}· {fmtSize(p.bytes)}{/if}
                  </span>
                  {#if p.count > 0}
                    <button class="browse" onclick={() => browse(p)} data-testid="pack-browse">
                      Browse →
                    </button>
                  {:else}
                    <span class="not-indexed">not indexed</span>
                  {/if}
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
    color: var(--ls-text-dim);
  }
  .msg.danger {
    color: var(--ls-danger);
  }

  .publishers {
    display: flex;
    flex-direction: column;
    gap: var(--sp-4);
  }
  h2 {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    margin: 0 0 var(--sp-1);
    font-size: var(--fs-md);
    color: var(--ls-gold);
  }
  h2 .count {
    margin-left: auto;
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
  }
  ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .pack {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-1) var(--sp-2);
    border-top: 1px solid var(--ls-border);
    font-size: var(--fs-sm);
  }
  .pack.empty {
    color: var(--ls-text-muted);
  }
  .pack .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .pill {
    padding: 0 var(--sp-1);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-pill);
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
  }
  .pill.unid {
    border-color: var(--ls-warning);
    color: var(--ls-warning);
  }
  .stat {
    margin-left: auto;
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
    white-space: nowrap;
  }
  .browse {
    background: none;
    border: none;
    color: var(--ls-gold);
    font: inherit;
    font-size: var(--fs-sm);
    cursor: pointer;
  }
  .browse:hover {
    text-decoration: underline;
  }
  .not-indexed {
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
  }
</style>
