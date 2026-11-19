<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLAttributes } from "svelte/elements";
  import type { TypeTone } from "./TypeMark.svelte";

  /**
   * The recess an asset's picture sits in: the card's 104 px thumbnail, the inspector's 160 px
   * preview, a 26 px row thumb, Quick Look's 440 × 420 stage, a 34 px tile in the staging tray.
   * The image covers it — except in Quick Look, which shows the whole picture; with none (a sound)
   * the children are centred in it instead. `unknown` is the Unidentified treatment — the picture
   * fades and loses its colour behind a large `?` (UI-SPEC §2.5), so the state reads without the
   * status colour. `tone` edges a tile in its asset's type colour.
   * The image is decoration here: the name beside it is the text alternative.
   */
  let {
    src,
    size = "card",
    unknown = false,
    tone,
    children,
    ...rest
  }: HTMLAttributes<HTMLDivElement> & {
    src?: string | null;
    size?: "card" | "preview" | "row" | "quicklook" | "tile";
    unknown?: boolean;
    tone?: TypeTone;
    children?: Snippet;
  } = $props();
</script>

<div class="thumb {size} {tone ?? ''}" class:unknown {...rest}>
  {#if src}<img {src} alt="" loading={size === "quicklook" ? "eager" : "lazy"} decoding="async" draggable="false" />{/if}
  {@render children?.()}
  {#if unknown}<span class="mark" aria-hidden="true">?</span>{/if}
</div>

<style>
  .thumb {
    position: relative;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: var(--space-10);
    flex-shrink: 0;
    background: var(--ls-thumb);
    overflow: hidden;
  }

  .card {
    height: var(--thumb-h);
  }

  .preview {
    height: var(--preview-h);
  }

  /* With no picture a row's thumb holds one glyph — the sound mark — in the sound's colour. */
  .row {
    width: var(--space-26);
    height: var(--space-26);
    border-radius: var(--r-4);
    color: var(--ls-type-sound);
  }

  /* Quick Look's stage: darker than a thumbnail's recess, and the whole picture fits inside it. */
  .quicklook {
    width: var(--quicklook-stage-w);
    height: var(--quicklook-h);
    background: var(--ls-viewer);
  }

  /* A staged pick: the tile is 34 px inside a hairline in the asset's type colour. */
  .tile {
    box-sizing: content-box;
    width: var(--tile-size);
    height: var(--tile-size);
    background: var(--ls-overlay);
    border: 1px solid var(--tile-tone, var(--ls-border-strong));
    border-radius: var(--r-6);
  }

  .model {
    --tile-tone: var(--ls-type-model);
  }

  .sound {
    --tile-tone: var(--ls-type-sound);
  }

  .image {
    --tile-tone: var(--ls-type-image);
  }

  .texture {
    --tile-tone: var(--ls-type-texture);
  }

  .material {
    --tile-tone: var(--ls-type-material);
  }

  .sprite {
    --tile-tone: var(--ls-type-sprite);
  }

  .font {
    --tile-tone: var(--ls-type-font);
  }

  img {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  .quicklook img {
    object-fit: contain;
  }

  .unknown img {
    opacity: var(--op-30);
    filter: grayscale(var(--gray-80));
  }

  .mark {
    position: absolute;
    font-family: var(--ff-mono);
    font-size: var(--fs-28);
    color: var(--ls-status-unidentified);
    text-shadow: 0 0 var(--space-10) var(--ls-bg);
  }

  .preview .mark {
    font-size: var(--fs-44);
  }

  .quicklook .mark {
    font-size: var(--fs-64);
    text-shadow: 0 0 var(--space-16) var(--ls-bg);
  }

  .row .mark,
  .tile .mark {
    font-size: var(--fs-12);
  }
</style>
