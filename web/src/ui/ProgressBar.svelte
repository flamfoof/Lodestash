<script lang="ts">
  import type { HTMLAttributes } from "svelte/elements";

  /**
   * A thin progress meter: the top bar's library-health meter ("Identified 14 / 19" over a gold
   * bar) and job progress. `role="progressbar"` with its value; `label` names it and is shown
   * above the bar with `valueText` right-aligned in mono, unless `hideLabel`.
   * `labelAttrs` reaches the visible label (the contract measures it).
   */
  let {
    value,
    max = 100,
    label,
    valueText,
    hideLabel = false,
    tone = "gold",
    labelAttrs = {},
    ...rest
  }: HTMLAttributes<HTMLDivElement> & {
    value: number;
    max?: number;
    label: string;
    valueText?: string;
    hideLabel?: boolean;
    tone?: "gold" | "arcane";
    labelAttrs?: Record<string, string | undefined>;
  } = $props();

  const pct = $derived(max > 0 ? Math.round((Math.max(0, Math.min(value, max)) / max) * 100) : 0);
</script>

<div class="meter" {...rest}>
  {#if !hideLabel}
    <div class="row" aria-hidden="true">
      <span {...labelAttrs}>{label}</span>
      {#if valueText}<span class="value">{valueText}</span>{/if}
    </div>
  {/if}
  <div
    class="track"
    role="progressbar"
    aria-label={label}
    aria-valuenow={value}
    aria-valuemin={0}
    aria-valuemax={max}
    aria-valuetext={valueText}
  >
    <div class="fill {tone}" style:width="{pct}%"></div>
  </div>
</div>

<style>
  .meter {
    min-width: var(--meter-w);
    flex-shrink: 0;
  }

  .row {
    display: flex;
    justify-content: space-between;
    margin-bottom: var(--space-3);
    font-size: var(--fs-11);
    color: var(--ls-text-muted);
  }

  .value {
    font-family: var(--ff-mono);
  }

  .track {
    box-sizing: content-box;
    height: var(--space-6);
    border-radius: var(--r-pill);
    background: var(--ls-overlay);
    border: 1px solid var(--ls-border);
    overflow: hidden;
  }

  .fill {
    height: 100%;
    transition: width var(--dur-400) ease;
  }

  .gold {
    background: var(--grad-gold-bar);
    box-shadow: var(--glow-gold-8-a60);
  }

  .arcane {
    background: var(--grad-arcane-bar);
    box-shadow: var(--glow-arcane-8-a50);
  }
</style>
