<script lang="ts">
  import Icon from "./Icon.svelte";
  import { type GridItem, previewUrl, typeToken, typeIcon, fmtSize } from "./api";

  let {
    item,
    selected = false,
    onselect,
  }: { item: GridItem; selected?: boolean; onselect?: (id: number) => void } = $props();

  let tint = $derived(`var(${typeToken(item.media_type)})`);
  let unidentified = $derived(item.status === "discovered" || item.status === "indexed");
</script>

<button
  class="card"
  class:selected
  style="--tint: {tint}"
  onclick={() => onselect?.(item.asset_id)}
  title={item.rel_path}
>
  <div class="thumb">
    {#if item.has_preview}
      <!-- Vector preview: the webview renders the stored SVG natively, no rasterisation. -->
      <img src={previewUrl(item.asset_id)} alt="" loading="lazy" />
    {:else}
      <Icon name={typeIcon(item.media_type)} size={28} />
    {/if}
    {#if unidentified}
      <span class="gem" title="Unidentified">???</span>
    {/if}
    {#if item.favorite}
      <span class="star" title="Favourite" aria-label="Favourite">★</span>
    {/if}
  </div>
  <div class="meta">
    <span class="name">{item.name}</span>
    <span class="sub num">
      {fmtSize(item.size_bytes)}{#if item.triangles} · {item.triangles.toLocaleString()} tri{/if}
    </span>
  </div>
</button>

<style>
  .card {
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
    padding: var(--sp-3);
    background: var(--ls-surface);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-lg);
    text-align: left;
    overflow: hidden;
  }
  .card:hover {
    border-color: var(--tint);
    background: var(--ls-surface-raised);
  }
  .card.selected {
    border-color: var(--ls-gold);
    box-shadow: 0 0 0 1px var(--ls-gold);
  }

  .thumb {
    position: relative;
    aspect-ratio: 1;
    display: grid;
    place-items: center;
    background: var(--ls-bg);
    border-radius: var(--r-md);
    /* The type colour reads through the preview as a faint wash, so a card's category is
       legible at a glance without tinting the artwork itself. */
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--tint) 22%, transparent);
    overflow: hidden;
  }
  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: contain;
  }

  .gem {
    position: absolute;
    top: var(--sp-1);
    right: var(--sp-1);
    font-size: var(--fs-xs);
    letter-spacing: 0.06em;
    color: var(--ls-status-unidentified);
    background: color-mix(in srgb, var(--ls-bg) 80%, transparent);
    border-radius: var(--r-pill);
    padding: 0 6px;
  }

  .star {
    position: absolute;
    top: var(--sp-1);
    left: var(--sp-1);
    font-size: var(--fs-sm);
    line-height: 1;
    color: var(--ls-gold);
    text-shadow: 0 1px 2px var(--ls-bg);
  }

  .meta {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .name {
    font-size: var(--fs-sm);
    color: var(--ls-text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sub {
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
  }
</style>
