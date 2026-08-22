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
  import CompareRows from "./CompareRows.svelte";
  import {
    duplicateGroups,
    nearDuplicateGroups,
    fmtSize,
    statusLabel,
    type CompareRow,
    type DupGroup,
    type GridItem,
  } from "./api";
  import { view } from "./nav.svelte";

  let { ontrash }: { ontrash: (assetIds: number[]) => void } = $props();

  let groups = $state<DupGroup[] | null>(null);
  let similar = $state<DupGroup[] | null>(null);
  let error = $state<string | null>(null);
  /** What each group was decided as. Session-only — a decision to keep both is not a catalog fact. */
  let outcome = $state<Record<string, "kept" | "trashed">>({});

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

  /**
   * One queue, not two stacked lists (UI-SPEC §5, D11).
   *
   * Exact groups first, then the visual ones — a byte-identical copy is unambiguously safe to trash
   * and a dHash match is not, so the ordering puts the easy decisions first and never mixes them up
   * with the ones that want a look.
   */
  type Row = { key: string; kind: "exact" | "similar"; group: DupGroup; reason: string };
  let queue = $derived.by<Row[]>(() => [
    ...(groups ?? []).map((g) => ({
      key: `exact:${g.hash}`,
      kind: "exact" as const,
      group: g,
      reason: `identical bytes · blake3 ${g.hash.slice(0, 12)} · ${g.items.length} copies · ${fmtSize(g.size_bytes * (g.items.length - 1))} reclaimable`,
    })),
    ...(similar ?? []).map((g) => ({
      key: `similar:${g.hash}`,
      kind: "similar" as const,
      group: g,
      reason: `similar picture · dhash ${g.hash.slice(0, 12)} · ${g.items.length} copies · ${fmtSize(g.size_bytes * (g.items.length - 1))} reclaimable`,
    })),
  ]);

  let cursor = $state(0);
  /**
   * Which member of a group of three or more is being compared against the keeper. The keeper holds
   * the left pane throughout: always one comparison against the thing you are keeping, never a
   * three-way stare.
   */
  let other = $state(1);
  let current = $derived(queue[Math.min(cursor, Math.max(0, queue.length - 1))] ?? null);
  let reviewed = $derived(Object.keys(outcome).length);

  // Moving to another group starts its comparison at the top again.
  $effect(() => {
    cursor;
    other = 1;
  });

  let keeperItem = $derived(
    current ? (current.group.items.find((i) => i.asset_id === current.group.keeper) ?? current.group.items[0]) : null,
  );
  let others = $derived(current ? current.group.items.filter((i) => i.asset_id !== keeperItem?.asset_id) : []);
  let otherItem = $derived(others[Math.min(other - 1, Math.max(0, others.length - 1))] ?? null);

  /** The fields two copies can actually be judged on. Agreement dims; difference goes gold. */
  let rows = $derived.by<CompareRow[]>(() => {
    const a = keeperItem;
    const b = otherItem;
    if (!a || !b) return [];
    const row = (label: string, f: (i: GridItem) => string): CompareRow => ({
      label,
      values: [f(a), f(b)],
    });
    const out: CompareRow[] = [
      row("Status", (i) => statusLabel(i.status)),
      row("Size", (i) => fmtSize(i.size_bytes)),
      row("Where", (i) => i.package ?? "on disk"),
      row("Tags", (i) => String(i.traits.length)),
    ];
    if (a.triangles != null || b.triangles != null) {
      out.push(row("Tris", (i) => (i.triangles != null ? i.triangles.toLocaleString() : "—")));
    }
    return out;
  });

  function decide(kind: "kept" | "trashed") {
    const row = current;
    if (!row) return;
    if (kind === "trashed") {
      ontrash(row.group.items.filter((i) => i.asset_id !== row.group.keeper).map((i) => i.asset_id));
    }
    outcome = { ...outcome, [row.key]: kind };
    // Move to the next *undecided* group, so working the queue never lands you back on one you
    // just resolved — and stop at the end rather than wrapping, which would hide that you finished.
    const next = queue.findIndex((r, i) => i > cursor && !outcome[r.key] && r.key !== row.key);
    cursor = next >= 0 ? next : Math.min(cursor + 1, queue.length - 1);
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) return;
    if (e.key === "ArrowDown") {
      e.preventDefault();
      cursor = Math.min(queue.length - 1, cursor + 1);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      cursor = Math.max(0, cursor - 1);
    }
  }
</script>

<svelte:window on:keydown={onkeydown} />

<section class="screen">
  <header>
    <div class="htext">
      <h1>Duplicates</h1>
      <p class="subtitle">
        Near-duplicates found across packs. Nothing is ever auto-deleted — you decide, and “Move to
        Trash” is a catalog-only soft delete that leaves the files on disk.
      </p>
    </div>
    {#if queue.length > 0}
      <div class="progress" data-testid="dup-progress">
        <span class="num big">{reviewed} / {queue.length}</span>
        <span class="plabel">groups reviewed</span>
      </div>
      <span class="pbar" aria-hidden="true">
        <span class="pfill" style="width: {Math.round((reviewed / queue.length) * 100)}%"></span>
      </span>
    {/if}
  </header>

  {#if error}
    <p class="msg danger">{error}</p>
  {:else if !groups}
    <p class="msg">Loading…</p>
  {:else if queue.length === 0}
    <EmptyState
      testid="empty"
      icon="ls-nav-duplicates"
      title="No duplicates left to review"
      line="Lodestash flags byte-identical and near-identical assets across packs. Nothing is ever deleted for you."
    />
  {:else}
    <div class="queue">
      <!-- The list keeps your place: a decision moves to the next *undecided* group, and resolved
           rows stay where they are with their outcome, so the queue reads as work done rather than
           work vanishing. -->
      <aside class="list" data-testid="dup-list">
        <div class="lhead"><span class="kicker">Groups</span></div>
        {#each queue as row, i (row.key)}
          <button
            class="lrow"
            class:on={i === cursor}
            class:done={!!outcome[row.key]}
            onclick={() => (cursor = i)}
            data-testid="dup-row-{i}"
          >
            {#if outcome[row.key]}
              <span class="tick">✓</span>
            {:else}
              <span class="bullet" style="--tint: var(--ls-type-model)"></span>
            {/if}
            <span class="lname">
              {row.group.items.find((it) => it.asset_id === row.group.keeper)?.name ??
                row.group.items[0]?.name}
            </span>
            {#if outcome[row.key]}
              <span class="lnote">{outcome[row.key] === "kept" ? "kept both" : "trashed"}</span>
            {:else}
              <span class="lcount num">{row.group.items.length}</span>
            {/if}
          </button>
        {/each}
        <p class="lfoot">↑ ↓ moves · your place survives every decision</p>
      </aside>

      {#if current && keeperItem && otherItem}
        <div class="pane" data-testid="dup-pane">
          <div class="reason">
            <span class="mono">{current.reason}</span>
            {#if current.group.cross_pack}<span class="pill">different packs</span>{/if}
            {#if current.kind === "similar"}
              <span class="pill warn">visual match — verify before trashing</span>
            {/if}
          </div>

          <div class="two">
            <div class="side">
              <span class="ribbon">★ keeper</span>
              <Card
                item={keeperItem}
                selected={view.selected === keeperItem.asset_id}
                onselect={(id) => (view.selected = id)}
                onopen={(id) => (view.selected = id)}
              />
            </div>
            <div class="side">
              {#if others.length > 1}
                <span class="cycle">
                  <button
                    onclick={() => (other = other > 1 ? other - 1 : others.length)}
                    aria-label="Previous copy">‹</button
                  >
                  <span class="num">{other} of {others.length}</span>
                  <button
                    onclick={() => (other = other < others.length ? other + 1 : 1)}
                    aria-label="Next copy"
                    data-testid="dup-cycle">›</button
                  >
                </span>
              {:else}
                <span class="ribbon dim">duplicate</span>
              {/if}
              <Card
                item={otherItem}
                selected={view.selected === otherItem.asset_id}
                onselect={(id) => (view.selected = id)}
                onopen={(id) => (view.selected = id)}
              />
            </div>
          </div>

          <div class="table">
            <CompareRows {rows} testid="dup-compare-rows" />
          </div>

          <div class="actions">
            <button class="keep" onclick={() => decide("kept")} data-testid="keep-both">
              Keep both
            </button>
            <button class="danger" onclick={() => decide("trashed")} data-testid="trash-others">
              Move {current.group.items.length - 1} to Trash
            </button>
          </div>
        </div>
      {/if}
    </div>
  {/if}
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
  .msg {
    color: var(--ls-text-dim);
  }
  .msg.danger {
    color: var(--ls-danger);
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

  /* D11: a queue you work, not a page you scroll. The list keeps your place; the pane is the §13
     compare view, so a duplicate group and a two-card comparison look and read the same. */
  header {
    display: grid;
    grid-template-columns: 1fr auto;
    grid-template-areas: "text progress" "bar bar";
    gap: var(--sp-2) var(--sp-4);
    align-items: end;
  }
  .htext {
    grid-area: text;
  }
  .progress {
    grid-area: progress;
    text-align: right;
    white-space: nowrap;
  }
  .progress .big {
    font-size: var(--fs-xl);
    color: var(--ls-text);
  }
  .plabel {
    display: block;
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
  }
  .pbar {
    grid-area: bar;
    height: 6px;
    border-radius: var(--r-pill);
    background: var(--ls-border-strong);
    overflow: hidden;
  }
  .pfill {
    display: block;
    height: 100%;
    background: linear-gradient(90deg, var(--ls-gold), var(--ls-gold-bright));
  }

  .queue {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: 280px minmax(0, 1fr);
    border-top: 1px solid var(--ls-border);
  }
  .list {
    border-right: 1px solid var(--ls-border);
    overflow-y: auto;
    display: flex;
    flex-direction: column;
  }
  .lhead {
    padding: var(--sp-2) var(--sp-3);
    border-bottom: 1px solid var(--ls-border);
  }
  .kicker {
    font-size: var(--fs-xs);
    font-weight: 600;
    letter-spacing: 0.09em;
    text-transform: uppercase;
    color: var(--ls-text-dim);
  }
  .lrow {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    width: 100%;
    padding: var(--sp-2) var(--sp-3);
    border: 0;
    border-bottom: 1px solid color-mix(in srgb, var(--ls-border) 50%, transparent);
    border-left: 2px solid transparent;
    background: none;
    font: inherit;
    font-size: var(--fs-sm);
    color: var(--ls-text-muted);
    text-align: left;
    cursor: pointer;
  }
  .lrow:hover {
    background: var(--ls-surface);
  }
  .lrow.on {
    background: var(--ls-surface-raised);
    border-left-color: var(--ls-gold);
    color: var(--ls-text);
  }
  .lrow.done {
    opacity: 0.5;
  }
  .tick {
    color: var(--ls-success);
    font-size: var(--fs-xs);
  }
  .bullet {
    width: 7px;
    height: 7px;
    background: var(--tint);
    transform: rotate(45deg);
    flex: none;
  }
  .lname {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .lcount,
  .lnote {
    flex: none;
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
  }
  .lfoot {
    margin: auto 0 0;
    padding: var(--sp-2) var(--sp-3);
    border-top: 1px solid var(--ls-border);
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
  }

  .pane {
    min-width: 0;
    overflow-y: auto;
    padding: var(--sp-4) var(--sp-5);
    display: flex;
    flex-direction: column;
    gap: var(--sp-3);
  }
  .two {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: var(--sp-4);
    max-width: 620px;
  }
  .side {
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
  }
  .cycle {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-2);
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
  }
  .cycle button {
    background: none;
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-sm);
    color: var(--ls-text-muted);
    font: inherit;
    width: 18px;
    height: 18px;
    cursor: pointer;
  }
  .cycle button:hover {
    border-color: var(--ls-gold);
    color: var(--ls-text);
  }
  .ribbon.dim {
    color: var(--ls-text-dim);
    border-color: var(--ls-border-strong);
  }
  .table {
    max-width: 620px;
    padding-top: var(--sp-3);
    border-top: 1px solid var(--ls-border);
  }
  .pill.warn {
    color: var(--ls-warning);
    border-color: color-mix(in srgb, var(--ls-warning) 45%, transparent);
  }
</style>
