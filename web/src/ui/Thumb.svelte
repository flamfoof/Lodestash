<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLAttributes } from "svelte/elements";

  /**
   * The recess an asset's picture sits in: the card's 104 px thumbnail, the inspector's 160 px
   * preview, a 26 px row thumb. The image covers it; with none (a sound) the children are centred
   * in it instead. `unknown` is the Unidentified treatment — the picture fades and loses its colour
   * behind a large `?` (UI-SPEC §2.5), so the state reads without the status colour.
   * The image is decoration here: the name beside it is the text alternative.
   */
  let {
    src,
    size = "card",
    unknown = false,
    children,
    ...rest
  }: HTMLAttributes<HTMLDivElement> & {
    src?: string | null;
    size?: "card" | "preview" | "row";
    unknown?: boolean;
    children?: Snippet;
  } = $props();
</script>

<div class="thumb {size}" class:unknown {...rest}>
  {#if src}<img {src} alt="" loading="lazy" decoding="async" draggable="false" />{/if}
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

  .row {
    width: var(--space-26);
    height: var(--space-26);
    border-radius: var(--r-4);
  }

  img {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
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

  .row .mark {
    font-size: var(--fs-12);
  }
</style>
