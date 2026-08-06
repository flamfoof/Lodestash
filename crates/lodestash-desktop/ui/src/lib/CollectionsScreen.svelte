<script lang="ts">
  /**
   * Collections (UI-SPEC §6): pinned loadouts — "stash tabs for your hoard". Create, browse,
   * delete. Membership is toggled from the asset Inspector (G6); that surface lands with the
   * inspector's tag-editing slice, so today a collection's members arrive only via the backend.
   */
  import { collections, createCollection, deleteCollection, type CollectionRow } from "./api";
  import { view } from "./nav.svelte";

  let rows = $state<CollectionRow[] | null>(null);
  let error = $state<string | null>(null);
  let name = $state("");

  async function load() {
    try {
      rows = await collections();
      error = null;
    } catch (e) {
      error = String(e);
    }
  }

  $effect(() => {
    view.dataVersion;
    load();
  });

  async function create() {
    const n = name.trim();
    if (!n) return;
    try {
      await createCollection(n);
      name = "";
      view.dataVersion += 1;
    } catch (e) {
      error = String(e);
    }
  }

  async function remove(id: number) {
    try {
      await deleteCollection(id);
      if (view.collectionFilter?.id === id) view.collectionFilter = null;
      view.dataVersion += 1;
    } catch (e) {
      error = String(e);
    }
  }

  function browse(c: CollectionRow) {
    view.collectionFilter = { id: c.id, name: c.name };
    view.go("library");
  }

  /** Two-step delete: first click arms the ✕ ("Sure?"), second deletes. Assets are never
   *  touched — only the group row and its memberships go. */
  let armed = $state<number | null>(null);
</script>

<section class="screen">
  <header>
    <div class="title">
      <h1>Collections</h1>
      <p class="subtitle">
        Stash tabs for your hoard — pinned groups you can browse as one. Add assets from any
        asset's inspector.
      </p>
    </div>
    <div class="create">
      <input
        placeholder="Collection name"
        bind:value={name}
        onkeydown={(e) => e.key === "Enter" && create()}
        data-testid="new-collection-name"
      />
      <button class="gold" onclick={create} disabled={!name.trim()} data-testid="new-collection-go">
        + Create
      </button>
    </div>
  </header>

  <div class="body">
    {#if error}
      <p class="msg danger">{error}</p>
    {:else if !rows}
      <p class="msg">Loading…</p>
    {:else if rows.length === 0}
      <p class="msg" data-testid="empty">
        No collections yet. Create one above — they are loadouts you can browse as a single
        group, not folders on disk.
      </p>
    {:else}
      <div class="grid" data-testid="collections">
        {#each rows as c (c.id)}
          <article class="card">
            <header>
              <span class="gem" aria-hidden="true">⟡</span>
              <h2>{c.name}</h2>
              <span class="count num">{c.asset_count.toLocaleString()}</span>
            </header>
            <footer>
              <button class="browse" onclick={() => browse(c)} data-testid="collection-browse-{c.id}">
                Browse →
              </button>
              <button
                class="del"
                class:armed={armed === c.id}
                title={
                  armed === c.id
                    ? "Click again to delete — assets are untouched"
                    : "Delete collection — assets are untouched"
                }
                onclick={() => {
                  if (armed === c.id) {
                    armed = null;
                    remove(c.id);
                  } else {
                    armed = c.id;
                  }
                }}
                data-testid="collection-delete-{c.id}"
              >
                {armed === c.id ? "Sure?" : "✕"}
              </button>
            </footer>
          </article>
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
  }
  .create {
    display: flex;
    gap: var(--sp-2);
  }
  .create input {
    width: 220px;
    height: var(--control-h);
    padding: 0 var(--sp-2);
    background: var(--ls-bg);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-md);
    color: var(--ls-text);
    font: inherit;
  }
  .create input:focus {
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
  }
  .gold:hover:not(:disabled) {
    background: color-mix(in srgb, var(--ls-gold) 15%, transparent);
  }
  .gold:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: var(--sp-4) var(--sp-5);
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
    gap: var(--sp-3);
  }
  .card {
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
    padding: var(--sp-3);
    background: var(--ls-surface);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-lg);
  }
  .card:hover {
    border-color: var(--ls-gold);
  }
  .card header {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: 0;
    flex-wrap: nowrap;
  }
  .gem {
    color: var(--ls-gold);
    font-size: var(--fs-lg);
  }
  .card h2 {
    margin: 0;
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: var(--fs-md);
  }
  .count {
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
  }
  .card footer {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: 0;
  }
  .browse {
    background: none;
    border: none;
    color: var(--ls-gold);
    font: inherit;
    font-size: var(--fs-sm);
    cursor: pointer;
    padding: 0;
  }
  .browse:hover {
    text-decoration: underline;
  }
  .del {
    margin-left: auto;
    background: none;
    border: none;
    color: var(--ls-text-dim);
    font: inherit;
    cursor: pointer;
  }
  .del:hover {
    color: var(--ls-danger);
  }
  .del.armed {
    color: var(--ls-danger);
    border: 1px solid var(--ls-danger);
    border-radius: 6px;
    padding: 1px 8px;
    margin-right: -8px;
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
