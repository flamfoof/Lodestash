<script lang="ts" module>
  /**
   * - `trait` — a human tag (gold-bright, or the trait's own colour when it has one);
   * - `ai` — a machine tag: arcane outline, with its confidence as a small bar;
   * - `neutral` — the "+N" expander and other counters;
   * - `unknown` — the `???` placeholder of an unidentified asset.
   */
  export type ChipTone = "trait" | "ai" | "neutral" | "unknown";
</script>

<script lang="ts">
  import type { HTMLAttributes } from "svelte/elements";

  /**
   * A pill-shaped tag. Clickable (a `<button>`) when `onclick` is given — on a card a chip filters
   * the library by that tag — otherwise a plain `<span>`.
   * `confidence` (0–1) draws the bar machine tags carry. `color` is a trait's own colour (data,
   * not a design value): the chip tints itself from it the way the prototype does.
   * `size`: `sm` is the card chip (1/8 px), `md` the filter-popover chip (2/9 px).
   */
  let {
    label,
    tone = "trait",
    confidence,
    color,
    size = "sm",
    selected,
    onclick,
    ...rest
  }: HTMLAttributes<HTMLElement> & {
    label: string;
    tone?: ChipTone;
    confidence?: number;
    color?: string;
    size?: "sm" | "md";
    /** A filter chip that is on (`aria-pressed`). */
    selected?: boolean;
    onclick?: (event: MouseEvent) => void;
  } = $props();

  const pct = $derived(confidence === undefined ? undefined : Math.round(Math.max(0, Math.min(1, confidence)) * 100));
</script>

<svelte:element
  this={onclick ? "button" : "span"}
  type={onclick ? "button" : undefined}
  class="chip {tone} {size}"
  class:custom={!!color}
  class:selected
  style:--chip-color={color}
  aria-pressed={onclick ? selected : undefined}
  {onclick}
  {...rest}
>
  {label}{#if pct !== undefined}<span class="conf" role="meter" aria-label="confidence" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100}><span class="conf-fill" style:width="{pct}%"></span></span>{/if}
</svelte:element>

<style>
  .chip {
    display: inline-flex;
    align-items: center;
    gap: var(--space-5);
    border: 1px solid transparent;
    border-radius: var(--r-pill);
    font-size: var(--fs-11);
    white-space: nowrap;
    transition: border-color var(--dur-120) ease;
  }

  .sm {
    padding: var(--space-1) var(--space-8);
  }

  .md {
    padding: var(--space-2) var(--space-9);
  }

  .trait {
    color: var(--ls-gold-bright);
    background: var(--ls-gold-bright-a15);
    border-color: var(--ls-gold-bright-a30);
  }

  /* A trait's own colour, at the alphas the prototype derives from it (26 ≈ 15 %, 4d ≈ 30 %). */
  .trait.custom {
    color: var(--chip-color);
    background: color-mix(in srgb, var(--chip-color) 15%, transparent);
    border-color: color-mix(in srgb, var(--chip-color) 30%, transparent);
  }

  .ai {
    color: var(--ls-arcane);
    border-color: var(--ls-arcane-a45);
  }

  .neutral {
    color: var(--ls-text-muted);
    background: var(--ls-overlay);
    border-color: var(--ls-border-strong);
  }

  .unknown {
    color: var(--ls-status-unidentified);
    border-color: var(--ls-border-strong);
  }

  button.chip:hover {
    border-color: var(--ls-arcane);
  }

  button.neutral:hover {
    border-color: var(--ls-text-dim);
  }

  .ai.selected {
    color: var(--ls-arcane-bright);
    border-color: var(--ls-arcane);
    background: var(--ls-arcane-a15);
  }

  .conf {
    display: inline-block;
    width: var(--space-18);
    height: var(--space-3);
    border-radius: var(--r-pill);
    background: var(--ls-border-strong);
    overflow: hidden;
  }

  .conf-fill {
    display: block;
    height: 100%;
    background: var(--ls-arcane);
  }
</style>
