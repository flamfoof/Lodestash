<script lang="ts">
  /**
   * Stats → Overview (UI-SPEC §8.1): stat tiles, size by type, disk weight by pack, heaviest
   * assets, insights.
   *
   * The Tag Graph tab (§8.2) is not here. It is a force-directed canvas over the identified
   * library, and 24 of 8,237 assets here carry a tag — it would render an empty field and read as
   * broken. It lands when identification has run.
   */
  import Icon from "./Icon.svelte";
  import { libraryStats, fmtSize, fmtDuration, typeToken, typeIcon, type Stats } from "./api";
  import { view } from "./nav.svelte";

  let stats = $state<Stats | null>(null);
  let error = $state<string | null>(null);

  $effect(() => {
    view.dataVersion;
    libraryStats()
      .then((s) => {
        stats = s;
        error = null;
      })
      .catch((e) => (error = String(e)));
  });

  let identified = $derived(
    stats && stats.total_assets > 0
      ? Math.round(((stats.total_assets - stats.unidentified) / stats.total_assets) * 100)
      : 0,
  );
  let heaviestPackShare = $derived(
    stats && stats.bytes_total > 0 && stats.by_pack.length > 0
      ? Math.round((stats.by_pack[0].bytes / stats.bytes_total) * 100)
      : 0,
  );

  /** Reveal an asset in the library, the same move the palette makes. */
  function reveal(id: number, name: string) {
    view.go("library");
    view.setSearch(name);
    view.selected = id;
  }
</script>

<section class="screen">
  <header><h1>Stats</h1></header>

  <div class="body">
    {#if error}
      <p class="msg danger">{error}</p>
    {:else if !stats}
      <p class="msg">Loading…</p>
    {:else}
      <div class="tiles" data-testid="stat-tiles">
        <div class="tile">
          <span class="value num">{stats.total_assets.toLocaleString()}</span>
          <span class="label">assets indexed</span>
          <span class="sub">{stats.sources} libraries · {stats.packs.toLocaleString()} packs</span>
        </div>
        <div class="tile gold">
          <span class="value num">{fmtSize(stats.bytes_total)}</span>
          <span class="label">on disk</span>
          <span class="sub">
            avg {stats.total_assets > 0 ? fmtSize(stats.bytes_total / stats.total_assets) : "—"}
          </span>
        </div>
        <div class="tile violet">
          <span class="value num">{stats.triangles_total.toLocaleString()}</span>
          <span class="label">triangles</span>
          <span class="sub">{stats.models.toLocaleString()} models</span>
        </div>
        <div class="tile" class:amber={identified < 100} class:verdant={identified === 100}>
          <span class="value num">{identified}%</span>
          <span class="label">identified</span>
          <span class="sub">{stats.unidentified.toLocaleString()} unidentified</span>
        </div>
      </div>

      <h2>Size by type</h2>
      <div class="bars">
        {#each stats.by_type as t (t.media_type)}
          <div class="bar-row">
            <span class="bar-label" style="--tint: var({typeToken(t.media_type)})">
              <Icon name={typeIcon(t.media_type)} size={14} />
              {t.media_type}
            </span>
            <div class="track">
              <div
                class="fill"
                style="--tint: var({typeToken(t.media_type)}); width: {stats.bytes_total > 0
                  ? (t.bytes / stats.bytes_total) * 100
                  : 0}%"
              ></div>
            </div>
            <span class="bar-meta num">
              {fmtSize(t.bytes)} · {t.count.toLocaleString()}
              {#if t.triangles > 0}· {t.triangles.toLocaleString()} tris{/if}
              {#if t.duration_ms > 0}· {fmtDuration(t.duration_ms)}{/if}
            </span>
          </div>
        {/each}
      </div>

      {#if stats.by_pack.length > 0}
        <h2>Disk weight by pack</h2>
        <div class="bars">
          {#each stats.by_pack as p (p.name)}
            <div class="bar-row">
              <span class="bar-label pack" title={p.name}>{p.name}</span>
              <div class="track">
                <div
                  class="fill gold"
                  style="width: {(p.bytes / stats.by_pack[0].bytes) * 100}%"
                ></div>
              </div>
              <span class="bar-meta num">{fmtSize(p.bytes)} · {p.count.toLocaleString()}</span>
            </div>
          {/each}
        </div>
      {/if}

      <h2>Heaviest assets</h2>
      <ul class="heaviest">
        {#each stats.heaviest as a (a.asset_id)}
          <li>
            <button onclick={() => reveal(a.asset_id, a.name)}>
              <span class="name">{a.name}</span>
              <span class="meta num">{a.package ?? "on disk"} · {fmtSize(a.size_bytes)}</span>
            </button>
          </li>
        {/each}
      </ul>

      <h2>Insights</h2>
      <ul class="insights" data-testid="insights">
        {#if stats.unidentified > 0}
          <li>
            <span class="glyph amber">?</span>
            {stats.unidentified.toLocaleString()} assets are Unidentified — invisible to AI search
            until appraised.
            <button class="link" onclick={() => view.go("identify")}>Open Identify</button>
          </li>
        {/if}
        {#if stats.duplicate_groups > 0}
          <li>
            <span class="glyph">⧉</span>
            {stats.duplicate_groups.toLocaleString()} exact-duplicate groups are waiting for review.
            <button class="link" onclick={() => view.go("dups")}>Open Duplicates</button>
          </li>
        {/if}
        {#if heaviestPackShare > 0}
          <li>
            <span class="glyph gold">▤</span>
            {stats.by_pack[0].name} is {heaviestPackShare}% of your library's disk.
          </li>
        {/if}
      </ul>
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
  h2 {
    margin: var(--sp-5) 0 var(--sp-2);
    font-size: var(--fs-xs);
    letter-spacing: 0.08em;
    text-transform: uppercase;
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

  .tiles {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(170px, 1fr));
    gap: var(--sp-3);
  }
  .tile {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: var(--sp-3);
    background: var(--ls-surface);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-md);
  }
  .tile .value {
    font-size: var(--fs-2xl);
    color: var(--ls-text);
  }
  .tile.gold .value {
    color: var(--ls-gold);
  }
  .tile.violet .value {
    color: var(--ls-type-model);
  }
  .tile.amber .value {
    color: var(--ls-warning);
  }
  .tile.verdant .value {
    color: var(--ls-success, var(--ls-status-confirmed));
  }
  .tile .label {
    font-size: var(--fs-sm);
    color: var(--ls-text-dim);
  }
  .tile .sub {
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
  }

  .bars {
    display: flex;
    flex-direction: column;
    gap: var(--sp-1);
  }
  .bar-row {
    display: grid;
    grid-template-columns: 10rem minmax(0, 1fr) max-content;
    align-items: center;
    gap: var(--sp-2);
    font-size: var(--fs-sm);
  }
  .bar-label {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
    color: var(--tint, var(--ls-text-dim));
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .bar-label.pack {
    color: var(--ls-text-dim);
  }
  .track {
    height: 8px;
    background: var(--ls-bg);
    border-radius: var(--r-pill);
    overflow: hidden;
  }
  .fill {
    height: 100%;
    background: var(--tint, var(--ls-gold));
    border-radius: var(--r-pill);
  }
  .fill.gold {
    background: var(--ls-gold);
  }
  .bar-meta {
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
    white-space: nowrap;
  }

  .heaviest,
  .insights {
    margin: 0;
    padding: 0;
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: var(--sp-1);
  }
  .heaviest button {
    display: flex;
    width: 100%;
    gap: var(--sp-2);
    padding: var(--sp-1) var(--sp-2);
    border-radius: var(--r-sm);
    background: none;
    border: none;
    color: var(--ls-text);
    font: inherit;
    font-size: var(--fs-sm);
    text-align: left;
    cursor: pointer;
  }
  .heaviest button:hover {
    background: var(--ls-overlay);
  }
  .heaviest .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .heaviest .meta {
    margin-left: auto;
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
    white-space: nowrap;
  }

  .insights li {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-2);
    background: var(--ls-surface);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-md);
    font-size: var(--fs-sm);
    color: var(--ls-text-dim);
  }
  .glyph {
    color: var(--ls-text-muted);
  }
  .glyph.amber {
    color: var(--ls-warning);
  }
  .glyph.gold {
    color: var(--ls-gold);
  }
  .link {
    margin-left: auto;
    background: none;
    border: none;
    color: var(--ls-gold);
    font: inherit;
    font-size: var(--fs-sm);
    cursor: pointer;
    text-decoration: underline;
  }
</style>
