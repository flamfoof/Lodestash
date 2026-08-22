<script lang="ts">
  import EmptyState from "./EmptyState.svelte";
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
  import { duplicateGroups, nearDuplicateGroups, fmtSize, type DupGroup } from "./api";
  import { view } from "./nav.svelte";

  let { ontrash }: { ontrash: (assetIds: number[]) => void } = $props();

  let groups = $state<DupGroup[] | null>(null);
  let similar = $state<DupGroup[] | null>(null);
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
    nearDuplicateGroups()
      .then((g) => (similar = g))
      .catch(() => {});
  });

  let shown = $derived((groups ?? []).filter((g) => !dismissed.has(`exact:${g.hash}`)));
  let shownSimilar = $derived((similar ?? []).filter((g) => !dismissed.has(`similar:${g.hash}`)));

  function keepBoth(hash: string, kind: "exact" | "similar") {
    dismissed = new Set([...dismissed, `${kind}:${hash}`]);
  }

  /** Trash every member except the keeper. */
  function trashOthers(g: DupGroup) {
    ontrash(g.items.filter((i) => i.asset_id !== g.keeper).map((i) => i.asset_id));
  }
</script>

{#snippet groupCard(g: DupGroup, reason: string, kind: "exact" | "similar")}
  <article class="group">
    <div class="reason">
      <span class="mono">{reason}</span>
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
      <button class="keep" onclick={() => keepBoth(g.hash, kind)} data-testid="keep-both">
        Keep both
      </button>
      <button class="danger" onclick={() => trashOthers(g)} data-testid="trash-others">
        Trash {g.items.length - 1} others
      </button>
    </div>
  </article>
{/snippet}

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
    {:else if shown.length === 0 && shownSimilar.length === 0}
      <EmptyState
        testid="empty"
        icon="ls-nav-duplicates"
        title="No duplicates left to review"
        line="Lodestash flags byte-identical and near-identical assets across packs. Nothing is ever deleted for you."
      />
      <p class="msg dismissed">
        {#if (groups?.length ?? 0) + (similar?.length ?? 0) > 0}
          ({(groups?.length ?? 0) + (similar?.length ?? 0)} dismissed this session.)
        {/if}
      </p>
    {:else}
      {#if shown.length > 0}
        <h2 class="tier">Exact duplicates — identical bytes</h2>
        <div class="groups" data-testid="dup-groups">
          {#each shown as g (g.hash)}
            {@render groupCard(
              g,
              `identical bytes · blake3 ${g.hash.slice(0, 12)} · ${g.items.length} copies · ${fmtSize(g.size_bytes * (g.items.length - 1))} reclaimable`,
              "exact",
            )}
          {/each}
        </div>
      {/if}
      {#if shownSimilar.length > 0}
        <h2 class="tier">Similar — same picture, different bytes</h2>
        <p class="tier-hint">
          dHash within 6 bits: re-encoded, re-sized or re-compressed copies. Verify before
          trashing — this is a visual signal, not a byte identity.
        </p>
        <div class="groups" data-testid="near-dup-groups">
          {#each shownSimilar as g (g.hash)}
            {@render groupCard(
              g,
              `similar picture · dhash ${g.hash.slice(0, 12)} · ${g.items.length} copies · ${fmtSize(g.size_bytes * (g.items.length - 1))} reclaimable`,
              "similar",
            )}
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
  .tier {
    margin: var(--sp-4) var(--sp-1) var(--sp-1);
    font-size: var(--fs-sm);
    letter-spacing: 0.06em;
    color: var(--ls-text-muted);
  }
  .tier-hint {
    margin: 0 var(--sp-1) var(--sp-2);
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
  }
  .reason {    display: flex;
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
