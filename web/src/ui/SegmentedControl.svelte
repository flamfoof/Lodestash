<script lang="ts" module>
  import type { IconBox, IconSize } from "./Icon.svelte";
  import type { IconName } from "./icons.generated";

  export interface Segment<V extends string = string> {
    value: V;
    /** Visible text; omit for an icon-only segment (then `title` is its accessible name). */
    label?: string;
    title?: string;
    icon?: IconName;
    iconSize?: IconSize;
    iconBox?: IconBox;
    /** Accent when selected: gold (default) or arcane teal (the AI search mode). */
    tone?: "gold" | "arcane";
  }
</script>

<script lang="ts" generics="V extends string">
  import type { HTMLAttributes } from "svelte/elements";
  import Icon from "./Icon.svelte";

  /**
   * One choice out of a few, side by side in a well: the search mode (⟡ Tags / ✦ AI), the grid
   * density (▪ ▦ ▣), the view (⊞ ☰). Each segment is a `<button aria-pressed>`.
   * `md` is the mode toggle (5/12 px, weight 500, an inset ring when on); `sm` the toolbar size
   * (3/9 px). `anchors` marks the selected and unselected segments for the contract gate.
   */
  let {
    options,
    value = $bindable(),
    onchange,
    label,
    size = "md",
    anchors = {},
    ...rest
  }: Omit<HTMLAttributes<HTMLDivElement>, "onchange"> & {
    options: Segment<V>[];
    value: V;
    onchange?: (value: V) => void;
    /** Accessible name of the group. */
    label: string;
    size?: "md" | "sm";
    anchors?: { on?: string; off?: string };
  } = $props();

  function pick(v: V) {
    if (v === value) return;
    value = v;
    onchange?.(v);
  }
</script>

<div class="seg {size}" role="group" aria-label={label} {...rest}>
  {#each options as option (option.value)}
    {@const on = option.value === value}
    <button
      type="button"
      class="item {option.tone ?? 'gold'}"
      class:on
      aria-pressed={on}
      aria-label={option.label ? undefined : option.title}
      title={option.title}
      data-anchor={on ? anchors.on : anchors.off}
      onclick={() => pick(option.value)}
    >
      <span class="label">
        {#if option.icon}<Icon name={option.icon} size={option.iconSize ?? "12"} box={option.iconBox} />{#if option.label}{" "}{/if}{/if}{option.label ?? ""}
      </span>
    </button>
  {/each}
</div>

<style>
  .seg {
    display: inline-flex;
    flex-shrink: 0;
    padding: var(--space-2);
    background: var(--ls-overlay);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-6);
  }

  .item {
    display: flex;
    align-items: center;
    border-radius: var(--r-4);
    font-size: var(--fs-12);
    color: var(--ls-text-muted);
    transition:
      color var(--dur-120) ease,
      background var(--dur-120) ease;
  }

  .item:hover:not(.on) {
    color: var(--ls-text);
  }

  .md .item {
    gap: var(--space-5);
    padding: var(--space-5) var(--space-12);
    font-weight: var(--fw-500);
  }

  .sm .item {
    padding: var(--space-3) var(--space-9);
  }

  .on.gold {
    color: var(--ls-gold);
    background: var(--ls-gold-a15);
  }

  .on.arcane {
    color: var(--ls-arcane);
    background: var(--ls-arcane-a15);
  }

  .md .on.gold {
    box-shadow: var(--ring-inset-gold-1-a40);
  }

  .md .on.arcane {
    box-shadow: var(--ring-inset-arcane-1-a40);
  }
</style>
