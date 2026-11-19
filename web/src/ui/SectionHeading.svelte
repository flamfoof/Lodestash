<script lang="ts">
  import type { HTMLAttributes } from "svelte/elements";

  /**
   * A section's heading inside a panel ("TRAITS", "LINKED FILES"): 10 px, letter-spaced, dim,
   * written in capitals as the prototype writes it. `sm` is a group's heading inside a popover
   * ("TRAIT", "FILE SIZE"): 9 px. `note` continues the heading in regular weight ("· models
   * only"); `caption` is a quiet mono note on the same baseline (where LINKED FILES were parsed
   * from). `flush` drops the space under it, for a heading that shares its row with a control.
   * `textAttrs` reaches the heading text, `noteAttrs` the note.
   */
  let {
    text,
    size = "md",
    note,
    caption,
    flush = false,
    textAttrs = {},
    noteAttrs = {},
    ...rest
  }: HTMLAttributes<HTMLDivElement> & {
    text: string;
    size?: "md" | "sm";
    note?: string;
    caption?: string;
    flush?: boolean;
    textAttrs?: Record<string, string | undefined>;
    noteAttrs?: Record<string, string | undefined>;
  } = $props();
</script>

<div class="head" class:flush {...rest}>
  <h3 class="text {size}" {...textAttrs}>{text}{#if note}{" "}<span class="note" {...noteAttrs}>{note}</span>{/if}</h3>
  {#if caption}<span class="caption">{caption}</span>{/if}
</div>

<style>
  .head {
    display: flex;
    align-items: baseline;
    gap: var(--space-6);
    margin-bottom: var(--space-6);
  }

  .flush {
    margin-bottom: 0;
  }

  .text {
    margin: 0;
    font-size: var(--fs-10);
    font-weight: var(--fw-600);
    letter-spacing: var(--track-1_2);
    color: var(--ls-text-dim);
  }

  .text.sm {
    font-size: var(--fs-9);
    letter-spacing: var(--track-1);
  }

  .note {
    font-weight: var(--fw-400);
  }

  .caption {
    font-family: var(--ff-mono);
    font-size: var(--fs-10);
    color: var(--ls-text-dim);
  }
</style>
