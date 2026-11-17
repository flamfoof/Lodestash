<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLButtonAttributes } from "svelte/elements";
  import Icon from "./Icon.svelte";
  import type { IconName } from "./icons.generated";

  /**
   * A full-width row in a rail, as a `<button>`:
   * - `nav` — a sidebar destination: 16 px icon slot, label, an optional trailing badge; the current
   *   one is `aria-current="page"` with the raised fill and a gold left rail;
   * - `facet` — a filter value: a diamond dot in its colour, label, a dim mono count; the chosen one
   *   is `aria-pressed` with the gold rail.
   * State is rail + fill + announced state, never colour alone.
   */
  let {
    variant = "nav",
    label,
    icon,
    dot,
    count,
    current = false,
    trailing,
    ...rest
  }: HTMLButtonAttributes & {
    variant?: "nav" | "facet";
    label: string;
    icon?: IconName;
    /** Facet dot colour — a CSS colour (a status or type token). */
    dot?: string;
    count?: number | string;
    current?: boolean;
    trailing?: Snippet;
  } = $props();
</script>

<button
  type="button"
  class="row {variant}"
  class:current
  aria-current={variant === "nav" && current ? "page" : undefined}
  aria-pressed={variant === "facet" ? current : undefined}
  {...rest}
>
  {#if variant === "nav"}
    <span class="slot">{#if icon}<Icon name={icon} size="16" />{/if}</span>
  {:else if dot}
    <span class="dot" style:--row-dot={dot}></span>
  {/if}
  <span class="label">{label}</span>
  {#if count !== undefined}<span class="count">{count}</span>{/if}
  {@render trailing?.()}
</button>

<style>
  .row {
    display: flex;
    align-items: center;
    width: 100%;
    border-left: var(--bw-2) solid transparent;
    color: var(--ls-text-muted);
    text-align: left;
    transition:
      color var(--dur-120) ease,
      background var(--dur-120) ease;
  }

  .row:hover {
    color: var(--ls-text);
    background: var(--ls-surface-raised);
  }

  .nav {
    gap: var(--space-10);
    padding: var(--space-7) var(--space-14);
    font-size: var(--fs-13);
    font-weight: var(--fw-500);
  }

  .facet {
    gap: var(--space-8);
    padding: var(--space-5) var(--space-14);
    font-size: var(--fs-12);
  }

  .current {
    color: var(--ls-text);
    border-left-color: var(--ls-gold);
  }

  .nav.current {
    background: var(--ls-surface-raised);
  }

  .slot {
    display: flex;
    justify-content: center;
    width: var(--space-16);
    flex: none;
  }

  .dot {
    width: var(--space-8);
    height: var(--space-8);
    flex: none;
    border-radius: var(--r-2);
    background: var(--row-dot);
    transform: rotate(45deg);
  }

  .label {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .count {
    font-family: var(--ff-mono);
    font-size: var(--fs-11);
    color: var(--ls-text-dim);
  }
</style>
