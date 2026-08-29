<script lang="ts">
  /**
   * Pack stats (UI-SPEC §15) — "Opened from **any** pack-name link (cards, list rows, Packages,
   * Stats)".
   *
   * It lived inside `UnityAssetsScreen`, which is why "any pack-name link" meant one screen: the
   * grid had nowhere to send a click even once the link existed. Lifted here so the four entry
   * points the spec names share one modal rather than growing copies that drift.
   *
   * Addressed by **name**, because a name is all a card has (`GridItem.package` is a string, and
   * putting a `package_id` on 8,000 grid rows to open a modal would be a field per card for a
   * click). It resolves the row itself, so callers hand over a string and nothing else.
   *
   * ponytail: first match wins if two packs share a name. Checked against the real catalog — 87
   * packs, no collisions. Carry `package_id` on `GridItem` if that ever stops being true.
   */
  import { packages, fmtSize, setPackKeepCached, type PackageRow } from "./api";
  import Icon from "./Icon.svelte";

  let { name, onclose }: { name: string | null; onclose: () => void } = $props();

  let row = $state<PackageRow | null>(null);
  let error = $state<string | null>(null);
  let sheetRef = $state<HTMLDivElement | null>(null);
  let keepBusy = $state(false);

  /**
   * UI-SPEC §15's keep-cached control, beside the size it governs.
   *
   * Deliberately **not** `pinned`, which already means "Add to Library" (M7): whether a pack
   * shows in the grid and whether its extracted bytes are worth hundreds of megabytes are
   * different questions, and one switch answering both would make freeing space cost visibility.
   *
   * Marking a pack that is not extracted starts no work — it is a promise about the future, not
   * a command to unpack a gigabyte on a click.
   */
  async function toggleKeep() {
    const r = row;
    if (!r || keepBusy) return;
    keepBusy = true;
    try {
      await setPackKeepCached(r.id, !r.keep_cached);
      row = { ...r, keep_cached: !r.keep_cached };
    } catch (e) {
      error = String(e);
    } finally {
      keepBusy = false;
    }
  }

  $effect(() => {
    const want = name;
    row = null;
    error = null;
    if (!want) return;
    let stale = false;
    packages()
      .then((rows) => {
        if (stale) return;
        const hit = rows.find((p) => p.name === want);
        if (hit) row = hit;
        else error = `No pack named “${want}” is in the catalog.`;
      })
      .catch((e) => {
        if (!stale) error = String(e);
      });
    return () => {
      stale = true;
    };
  });

  // Focus the sheet once it exists, so Escape and screen readers land inside the dialog rather
  // than on whatever card was clicked behind it.
  $effect(() => {
    if (row || error) sheetRef?.focus();
  });

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      onclose();
    }
  }
</script>

<svelte:window on:keydown={onkeydown} />

{#if name}
  <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
  <div class="scrim" onclick={onclose} data-testid="pack-stats">
    <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
    <div
      class="sheet"
      onclick={(e) => e.stopPropagation()}
      role="alertdialog"
      tabindex="-1"
      bind:this={sheetRef}
      aria-modal="true"
      aria-labelledby="pack-stats-title"
    >
      <div class="sheet-head">
        <div class="sheet-title">
          <h3 id="pack-stats-title">{row?.name ?? name}</h3>
          {#if row}
            <p class="meta">
              {row.publisher}
              {#if row.category}<span class="sep">·</span> {row.category}{/if}
            </p>
          {/if}
        </div>
        <button
          class="close"
          aria-label="Close pack stats"
          onclick={onclose}
          data-testid="pack-stats-close"
        >
          ✕
        </button>
      </div>

      {#if error}
        <p class="note">{error}</p>
      {:else if !row}
        <p class="note">Loading…</p>
      {:else}
        <div class="grid" data-testid="pack-stats-grid">
          <div class="tile">
            <span class="num value">{row.count.toLocaleString()}</span>
            <span class="label">indexed assets</span>
          </div>
          <div class="tile" class:warn={row.unidentified > 0}>
            <span class="num value">{row.unidentified.toLocaleString()}</span>
            <span class="label">unidentified</span>
          </div>
          <div class="tile">
            <span class="num value">{fmtSize(row.bytes)}</span>
            <span class="label">size</span>
          </div>
          <div class="tile">
            <span class="num value">{row.triangles ? row.triangles.toLocaleString() : "—"}</span>
            <span class="label">triangles</span>
          </div>
        </div>

        <div class="keeprow" data-testid="pack-keep-cached">
          <span class="keepicon" class:on={row.keep_cached} aria-hidden="true">
            <Icon name="ls-nav-packages" size={16} />
          </span>
          <div class="keeptext">
            <span>Keep cached</span>
            <span class="sub">
              {#if !row.extracted}
                Not extracted yet — marking it only promises the bytes survive if you do.
              {:else if row.keep_cached}
                Kept, so its assets open from disk instead of streaming from the archive.
              {:else}
                Evictable — reclaimed once nothing references it. It stays searchable either way.
              {/if}
            </span>
          </div>
          <button
            class="switch"
            class:on={row.keep_cached}
            onclick={toggleKeep}
            disabled={keepBusy}
            aria-pressed={row.keep_cached}
            aria-label="Keep cached"
            data-testid="pack-keep-toggle"
          >
            <span class="knob"></span>
          </button>
        </div>

        {#if row.by_type.length > 0}
          <div class="types" data-testid="pack-stats-types">
            {#each row.by_type as t (t.media_type)}
              <div class="type-row">
                <span class="type-name">{t.media_type}</span>
                <span class="type-count mono">{t.count.toLocaleString()}</span>
                <span class="type-bytes mono dim">{fmtSize(t.bytes)}</span>
              </div>
            {/each}
          </div>
        {:else}
          <p class="note">
            No assets indexed from this pack yet — add its folder as a source to pull the contents
            in.
          </p>
        {/if}
      {/if}
    </div>
  </div>
{/if}

<style>
  .num,
  .mono {
    font-variant-numeric: tabular-nums;
  }
  .dim {
    color: var(--ls-text-dim);
  }
  .scrim {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 0.6);
    display: grid;
    place-items: center;
    z-index: 100;
  }
  .sheet {
    width: min(420px, 92vw);
    background: var(--ls-surface);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-lg);
    padding: var(--sp-4);
    display: flex;
    flex-direction: column;
    gap: var(--sp-3);
  }
  .sheet-head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: var(--sp-2);
    padding: 0;
    flex-wrap: nowrap;
  }
  .sheet h3 {
    margin: 0;
    font-size: var(--fs-lg);
  }
  .meta {
    margin: var(--sp-1) 0 0;
    font-size: var(--fs-sm);
    color: var(--ls-text-muted);
  }
  .sep {
    color: var(--ls-text-dim);
  }
  .close {
    flex: none;
    width: var(--control-h);
    height: var(--control-h);
    background: none;
    border: 1px solid var(--ls-border);
    border-radius: var(--r-sm);
    color: var(--ls-text-dim);
    font: inherit;
    cursor: pointer;
  }
  .close:hover {
    color: var(--ls-gold-bright);
    border-color: var(--ls-gold);
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: var(--sp-2);
  }
  .tile {
    display: flex;
    flex-direction: column;
    gap: var(--sp-1);
    padding: var(--sp-3);
    background: var(--ls-bg);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-md);
  }
  .tile .value {
    font-size: var(--fs-xl);
    font-weight: 700;
  }
  .tile.warn .value {
    color: var(--ls-gold-bright);
  }
  .tile .label {
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  /* §15's keep-cached row — the control sits next to the size it governs. */
  .keeprow {
    display: flex;
    align-items: flex-start;
    gap: var(--sp-3);
    padding: var(--sp-3);
    background: var(--ls-surface-raised);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-md);
  }
  /* The sprite bakes its own stroke colour (Icon.svelte), so state reads as opacity. */
  .keepicon {
    flex: none;
    margin-top: 1px;
    opacity: 0.3;
  }
  .keepicon.on {
    opacity: 1;
  }
  .keeptext {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 2px;
    font-size: var(--fs-sm);
  }
  .keeptext .sub {
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
  }
  .switch {
    flex: none;
    position: relative;
    width: 34px;
    height: 18px;
    margin-top: 2px;
    padding: 0;
    background: none;
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-pill);
    cursor: pointer;
  }
  .switch.on {
    background: color-mix(in srgb, var(--ls-gold) 18%, transparent);
    border-color: var(--ls-gold);
  }
  .switch:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .switch .knob {
    position: absolute;
    top: 2px;
    left: 2px;
    width: 12px;
    height: 12px;
    border-radius: var(--r-pill);
    background: var(--ls-text-dim);
    transition: left 120ms ease, background 120ms ease;
  }
  .switch.on .knob {
    left: 18px;
    background: var(--ls-gold);
  }
  @media (prefers-reduced-motion: reduce) {
    .switch .knob {
      transition: none;
    }
  }

  .types {
    margin-top: 12px;
    border-top: 1px solid var(--ls-border);
    padding-top: 8px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    max-height: 220px;
    overflow: auto;
  }
  .type-row {
    display: grid;
    grid-template-columns: 1fr auto auto;
    gap: 12px;
    align-items: baseline;
    font-size: var(--fs-sm);
  }
  .type-name {
    color: var(--ls-text);
  }
  .note {
    margin: 0;
    font-size: var(--fs-sm);
    color: var(--ls-text-dim);
  }
</style>
