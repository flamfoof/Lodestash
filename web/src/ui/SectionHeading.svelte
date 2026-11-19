<script lang="ts">
  import type { HTMLAttributes } from "svelte/elements";

  /**
   * A section's heading inside a panel ("TRAITS", "LINKED FILES"): 10 px, letter-spaced, dim,
   * written in capitals as the prototype writes it. `caption` is a quiet mono note on the same
   * baseline (where LINKED FILES were parsed from). `textAttrs` reaches the heading text.
   */
  let {
    text,
    caption,
    textAttrs = {},
    ...rest
  }: HTMLAttributes<HTMLDivElement> & {
    text: string;
    caption?: string;
    textAttrs?: Record<string, string | undefined>;
  } = $props();
</script>

<div class="head" {...rest}>
  <h3 class="text" {...textAttrs}>{text}</h3>
  {#if caption}<span class="caption">{caption}</span>{/if}
</div>

<style>
  .head {
    display: flex;
    align-items: baseline;
    gap: var(--space-6);
    margin-bottom: var(--space-6);
  }

  .text {
    margin: 0;
    font-size: var(--fs-10);
    font-weight: var(--fw-600);
    letter-spacing: var(--track-1_2);
    color: var(--ls-text-dim);
  }

  .caption {
    font-family: var(--ff-mono);
    font-size: var(--fs-10);
    color: var(--ls-text-dim);
  }
</style>
