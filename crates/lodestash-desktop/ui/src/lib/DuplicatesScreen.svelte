<script lang="ts">
  /**
   * Duplicates (UI-SPEC §5) — a review queue. **Nothing here is ever auto-resolved.**
   *
   * Exact duplicates only: the sole similarity signal in the catalog today is the BLAKE3 content
   * hash. The spec describes perceptual near-duplicate review, which arrives with the embeddings;
   * the screen's shape does not change when it does, only the reason line. Saying "identical
   * bytes" is the honest version of that, and it is also the case where trashing a copy is
   * unambiguously safe.
   *
   * The only action that touches the catalog is **Move to Trash**, which is a soft delete — the
   * file stays on disk, and the Trash screen restores it. Deleting from disk is deliberately not
   * offered here: a screen showing six near-identical thumbnails is exactly where a mis-click is
   * most likely and least recoverable.
   */
  import Card from "./Card.svelte";
  import { duplicateGroups, fmtSize, type DupGroup } from "./api";
  import { view } from "./nav.svelte";

  let { ontrash }: { ontrash: (assetIds: number[]) => void } = $props();

  let groups = $state<DupGroup[] | null>(null);
  let error = $state<string | null>(null);
  /** Groups the user has said are fine. Session-only — dismissing is not a catalog fact. */
  let dismissed = $state<Set<string>>(new Set());

  $effect(() => {
    view.dataVersion;
    duplicateGroups()
      .then((g) => {
        groups = g;
        error = null;
      })
      .catch((e) => (error = String(e)));
  });

  let shown = $derived((groups ?? []).filter((g) => !dismissed.has(g.hash)));

  function keepBoth(hash: string) {
    dismissed = new Set([...dismissed, hash]);
  }

  /** Trash every member except the keeper. */
  function trashOthers(g: DupGroup) {
    ontrash(g.items.filter((i) => i.asset_id !== g.keeper).map((i) => i.asset_id));
  }
</script>

<section class="screen">
  <header>
    <h1>Duplicates</h1>
    <p class="subtitle">
      Assets with byte-identical copies across your library. Nothing is ever auto-deleted — you
      decide, and “Move to Trash” is a catalog-only soft delete that leaves the files on disk.
    </p>
  </header>

  <div class="body">
    {#if error}
      <p class="msg danger">{error}</p>
    {:else if !groups}
      <p class="msg">Loading…</p>
    {:else if shown.length === 0}
      <p class="msg" data-testid="empty">
        ⧉ No duplicates left to review.
        {#if (groups?.length ?? 0) > 0}
          ({groups.length} dismissed this session.)
        {/if}
      </p>
    {:else}
      <div class="groups" data-testid="dup-groups">
        {#each shown as g (g.hash)}
          <article class="group">
            <div class="reason">
              <span class="mono">
                identical bytes · blake3 {g.hash.slice(0, 12)} · {g.items.length} copies ·
                {fmtSize(g.size_bytes * (g.items.length - 1))} reclaimable
              </span>
              {#if g.cross_pack}
                <span class="pill">different packs</span>
              {/if}
            </div>

            <div class="members">
              {#each g.items as item (item.asset_id)}
                <div class="member" class:keeper={item.asset_id === g.keeper}>
                  {#if item.asset_id === g.keeper}
                    <span class="ribbon">★ keeper</span>
                  {/if}
                  <Card
                    {item}
                    selected={view.selected === item.asset_id}
                    onselect={(id) => (view.selected = id)}
                    onopen={(id) => (view.selected = id)}
                  />
                  <span class="where">{item.package ?? "on disk"}</span>
                </div>
              {/each}
            </div>

            <div class="actions">
              <button onclick={() => keepBoth(g.hash)} data-testid="dup-keep">Keep all</button>
              <button class="danger" onclick={() => trashOthers(g)} data-testid="dup-trash">
                Move {g.items.length - 1} duplicate{g.items.length > 2 ? "s" : ""} to Trash
              </button>
            </div>
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
    padding: var(--sp-4) var(--sp-5) var(--sp-3);
  }
  h1 {
    font-size: var(--fs-2xl);
  }
  .subtitle {
    margin: var(--sp-1) 0 0;
    max-width: 70ch;
    font-size: var(--fs-sm);
    color: var(--ls-text-dim);
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

  .groups {
    display: flex;
    flex-direction: column;
    gap: var(--sp-3);
  }
  .group {
    padding: var(--sp-3);
    background: var(--ls-surface);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-lg);
  }
  .reason {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    margin-bottom: var(--sp-2);
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
  }
  .mono {
    font-family: var(--font-mono, monospace);
  }
  .pill {
    padding: 0 var(--sp-2);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-pill);
    color: var(--ls-text-dim);
  }

  .members {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(148px, 1fr));
    gap: var(--sp-3);
  }
  .member {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: var(--sp-1);
  }
  .ribbon {
    position: absolute;
    top: -8px;
    left: 4px;
    z-index: 2;
    padding: 0 var(--sp-1);
    border-radius: var(--r-sm);
    background: var(--ls-gold);
    color: var(--ls-bg);
    font-size: var(--fs-xs);
  }
  .member.keeper :global(.card) {
    border-color: var(--ls-gold);
  }
  .where {
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .actions {
    display: flex;
    gap: var(--sp-2);
    margin-top: var(--sp-3);
  }
  .actions button {
    padding: var(--sp-1) var(--sp-2);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-sm);
    background: var(--ls-surface);
    color: var(--ls-text);
    font: inherit;
    font-size: var(--fs-sm);
    cursor: pointer;
  }
  .actions button:hover {
    border-color: var(--ls-gold);
  }
  .actions .danger {
    margin-left: auto;
    color: var(--ls-danger);
    border-color: var(--ls-danger);
  }
</style>
