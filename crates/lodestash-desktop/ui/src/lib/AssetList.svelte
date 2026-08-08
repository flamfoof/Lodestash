<script lang="ts">
  import Icon from "./Icon.svelte";
  import {
    type GridItem,
    previewUrl,
    thumbUrl,
    typeToken,
    typeIcon,
    fmtSize,
    statusLabel,
  } from "./api";

  let {
    items,
    selected,
    onselect,
    onopen,
    onmenu,
    trashedOnly = false,
    onrestore,
  }: {
    items: GridItem[];
    selected: number | null;
    onselect: (id: number) => void;
    onopen: (id: number) => void;
    onmenu?: (e: MouseEvent, id: number) => void;
    /** Trash view: a per-row Restore (UI-SPEC §10 — the spec'd trash table). */
    trashedOnly?: boolean;
    onrestore?: (id: number) => void;
  } = $props();

  /** A row that cannot be thumbnailed just shows nothing rather than a broken-image glyph. */
  const hideBroken = (e: Event) => ((e.currentTarget as HTMLImageElement).style.display = "none");
</script>

<div class="list" role="table" aria-label="Assets">
  <div class="head" role="row">
    <span role="columnheader">Name</span>
    <span role="columnheader">Type</span>
    <span role="columnheader">Status</span>
    <span role="columnheader" class="right">Size</span>
    <span role="columnheader" class="right">Tris</span>
  </div>
  {#each items as item (item.asset_id)}
    <button
      class="row"
      class:on={selected === item.asset_id}
      style="--tint: var({typeToken(item.media_type)})"
      role="row"
      title={item.rel_path}
      onclick={() => onselect(item.asset_id)}
      ondblclick={() => onopen(item.asset_id)}
      oncontextmenu={(e) => {
        e.preventDefault();
        onmenu?.(e, item.asset_id);
      }}
    >
      <span class="cell name" role="cell">
        <span class="thumb">
          {#if item.has_preview}
            <img src={previewUrl(item.asset_id)} alt="" loading="lazy" />
          {:else if item.media_type !== "model_3d" && !item.in_pack}
            <img src={thumbUrl(item.asset_id)} alt="" loading="lazy" onerror={hideBroken} />
          {:else}
            <Icon name={typeIcon(item.media_type)} size={16} />
          {/if}
        </span>
        {#if item.favorite}<span class="star">★</span>{/if}
        <span class="label">{item.name}</span>
      </span>
      <span class="cell type" role="cell">{item.media_type}</span>
      <span class="cell" role="cell" class:dim={item.status !== "confirmed"}>
        {statusLabel(item.status)}
      </span>
      <span class="cell right num" role="cell">{fmtSize(item.size_bytes)}</span>
      <span class="cell right num" role="cell">
        {item.triangles ? item.triangles.toLocaleString() : "—"}
      </span>
      {#if trashedOnly}
        <span class="cell right" role="cell">
          <!-- The row root is a <button>, so a real control inside it would be invalid HTML —
               same rule as the grid's multi-select box. A role=button span keeps the click. -->
          <span
            class="restore"
            role="button"
            tabindex="0"
            data-testid="row-restore"
            onclick={(e) => {
              e.stopPropagation();
              onrestore?.(item.asset_id);
            }}
            onkeydown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.stopPropagation();
                onrestore?.(item.asset_id);
              }
            }}
          >
            Restore
          </span>
        </span>
      {/if}
    </button>
  {/each}
</div>

<style>
  .list {
    display: flex;
    flex-direction: column;
  }
  /* One grid template shared by the header and every row, so columns stay aligned without a
     real <table> — which would fight the virtualisation this grows into. */
  .head,
  .row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 90px 110px 90px 80px;
    gap: var(--sp-3);
    align-items: center;
    padding: 0 var(--sp-2);
    text-align: left;
  }
  .head {
    height: 26px;
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
    border-bottom: 1px solid var(--ls-border);
    position: sticky;
    top: 0;
    background: var(--ls-bg);
    z-index: 1;
  }
  .row {
    height: var(--row-h);
    border-bottom: 1px solid color-mix(in srgb, var(--ls-border) 50%, transparent);
    color: var(--ls-text-muted);
    font-size: var(--fs-sm);
  }
  .row:hover {
    background: var(--ls-surface);
    color: var(--ls-text);
  }
  .row.on {
    background: var(--ls-surface-raised);
    box-shadow: inset 2px 0 0 var(--ls-gold);
    color: var(--ls-text);
  }

  .cell {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .name {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    min-width: 0;
  }
  .label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .thumb {
    flex: none;
    /* Tracks the density-driven row height (DESIGN-GUIDELINES §4: --row-h 32/36/40px) with
       a fixed 6px breathing margin — a fixed 22px thumb sat oddly in a 40px cozy row and
       shrunk relative to the grid cards when density changed. */
    width: calc(var(--row-h) - 12px);
    height: calc(var(--row-h) - 12px);
    display: grid;
    place-items: center;
    background: var(--ls-bg);
    border-radius: var(--r-sm);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--tint) 25%, transparent);
    overflow: hidden;
  }
  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: contain;
  }
  .star {
    flex: none;
    color: var(--ls-gold);
    font-size: var(--fs-xs);
  }
  .type {
    color: var(--tint);
    font-size: var(--fs-xs);
  }
  .dim {
    color: var(--ls-text-dim);
  }
  .right {
    text-align: right;
  }
  .restore {
    border: 1px solid var(--ls-border);
    border-radius: 5px;
    background: transparent;
    color: var(--ls-text-dim);
    font: inherit;
    font-size: var(--fs-xs);
    padding: 2px 8px;
    cursor: default;
  }
  .restore:hover {
    border-color: var(--ls-border-strong);
    color: var(--ls-text);
    background: var(--ls-hover);
  }
</style>
