<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLAttributes } from "svelte/elements";

  /**
   * A small count or status label in mono figures.
   * - `warning` — the amber outlined count on a nav row (Identify 5, Duplicates 1);
   * - `gold` — the solid gold count inside a toggle (★ Favorites 3);
   * - `neutral` — a quiet outlined count.
   * Weight is inherited, as the prototype's nav badge takes its row's.
   */
  let {
    tone = "warning",
    label,
    children,
    ...rest
  }: HTMLAttributes<HTMLSpanElement> & {
    tone?: "warning" | "gold" | "neutral";
    /** Spoken instead of the bare figure ("5 unidentified"). */
    label?: string;
    children: Snippet;
  } = $props();
</script>

<span class="badge {tone}" aria-label={label} {...rest}>{@render children()}</span>

<style>
  .badge {
    font-family: var(--ff-mono);
    border-radius: var(--r-pill);
    white-space: nowrap;
  }

  .warning {
    font-size: var(--fs-10);
    padding: 0 var(--space-7);
    color: var(--ls-warning);
    background: var(--ls-warning-a15);
    border: 1px solid var(--ls-warning-a40);
  }

  .gold {
    font-size: var(--fs-9);
    font-weight: var(--fw-700);
    padding: 0 var(--space-6);
    color: var(--ls-on-gold);
    background: var(--ls-gold);
  }

  .neutral {
    font-size: var(--fs-10);
    padding: 0 var(--space-7);
    color: var(--ls-text-muted);
    border: 1px solid var(--ls-border-strong);
  }
</style>
