<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLAttributes } from "svelte/elements";

  /**
   * Running text at the sizes the prototype uses beside controls — so a feature never sets a font
   * or a colour itself:
   * - `label` — a result count beside a toolbar ("19 assets · all libraries"): 12 px, muted;
   * - `hint` — a keyboard hint in a toolbar: 11 px, dim;
   * - `note` — a line under a group ("quick-tag from your trait library"): 10 px, dim;
   * - `empty` — what a group says when it has nothing ("No traits yet — run Identify."): italic;
   * - `warning` — a note that asks for attention (missing links are skipped on export).
   */
  let {
    variant = "label",
    children,
    ...rest
  }: HTMLAttributes<HTMLSpanElement> & {
    variant?: "label" | "hint" | "note" | "empty" | "warning";
    children: Snippet;
  } = $props();
</script>

<span class="text {variant}" {...rest}>{@render children()}</span>

<style>
  .label {
    font-size: var(--fs-12);
    color: var(--ls-text-muted);
  }

  .hint {
    font-size: var(--fs-11);
    color: var(--ls-text-dim);
    white-space: nowrap;
  }

  .note,
  .empty,
  .warning {
    display: block;
  }

  .note {
    font-size: var(--fs-10);
    color: var(--ls-text-dim);
  }

  .empty {
    font-size: var(--fs-12);
    color: var(--ls-text-dim);
    font-style: italic;
  }

  .warning {
    font-size: var(--fs-10);
    color: var(--ls-warning);
  }
</style>
