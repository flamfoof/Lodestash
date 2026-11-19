<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLAttributes } from "svelte/elements";

  /**
   * Running text at the sizes the prototype uses beside controls — so a feature never sets a font
   * or a colour itself:
   * - `label` — a result count beside a toolbar ("19 assets · all libraries"): 12 px, muted;
   * - `selection` — how many are picked, in the bulk bar ("3 selected"): 12 px, bright gold, bold;
   * - `hint` — a keyboard hint in a toolbar: 11 px, dim;
   * - `note` — a line under a group ("quick-tag from your trait library"): 10 px, dim;
   * - `empty` — what a group says when it has nothing ("No traits yet — run Identify."): italic;
   * - `nothing` — the same for a list a search narrowed to nothing ("No traits match."): 11 px;
   * - `warning` — a note that asks for attention (missing links are skipped on export).
   * `center` centres a block variant in its container (a caption under a full-width button).
   */
  let {
    variant = "label",
    center = false,
    children,
    ...rest
  }: HTMLAttributes<HTMLSpanElement> & {
    variant?: "label" | "selection" | "hint" | "note" | "empty" | "nothing" | "warning";
    center?: boolean;
    children: Snippet;
  } = $props();
</script>

<span class="text {variant}" class:center {...rest}>{@render children()}</span>

<style>
  .label {
    font-size: var(--fs-12);
    color: var(--ls-text-muted);
  }

  .selection {
    font-size: var(--fs-12);
    font-weight: var(--fw-600);
    color: var(--ls-gold-bright);
    white-space: nowrap;
  }

  .hint {
    font-size: var(--fs-11);
    color: var(--ls-text-dim);
    white-space: nowrap;
  }

  .note,
  .empty,
  .nothing,
  .warning {
    display: block;
  }

  .center {
    text-align: center;
  }

  .nothing {
    font-size: var(--fs-11);
    color: var(--ls-text-dim);
    font-style: italic;
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
