<script lang="ts">
  import type { HTMLAttributes } from "svelte/elements";
  import Icon, { type IconBox, type IconSize } from "./Icon.svelte";
  import type { IconName } from "./icons.generated";

  /**
   * A rounded status chip in the top bar: the project chip (`neutral`, with its colour dot and a
   * caret — a `<button>` that opens the project switcher) and the local-inference chip (`arcane`).
   * `dot` is a CSS colour from data (the project's colour), glowing as the prototype draws it.
   * `open` marks the chip whose menu is showing (gold border, `aria-expanded`).
   */
  let {
    label,
    tone = "neutral",
    icon,
    iconSize = "11",
    iconBox,
    dot,
    caret = false,
    open,
    onclick,
    ...rest
  }: HTMLAttributes<HTMLElement> & {
    label: string;
    tone?: "neutral" | "arcane";
    icon?: IconName;
    iconSize?: IconSize;
    iconBox?: IconBox;
    dot?: string;
    caret?: boolean;
    open?: boolean;
    onclick?: (event: MouseEvent) => void;
  } = $props();
</script>

<svelte:element
  this={onclick ? "button" : "span"}
  type={onclick ? "button" : undefined}
  class="pill {tone}"
  class:open
  aria-expanded={onclick && open !== undefined ? open : undefined}
  aria-haspopup={onclick && open !== undefined ? "menu" : undefined}
  {onclick}
  {...rest}
>
  {#if dot}<span class="dot" style:--pill-dot={dot}></span>{/if}
  <span class="label">{#if icon}<Icon name={icon} size={iconSize} box={iconBox} />{" "}{/if}{label}</span>
  {#if caret}<span class="caret"><Icon name="action-expand-closed" size="8" box="8" /></span>{/if}
</svelte:element>

<style>
  .pill {
    display: flex;
    align-items: center;
    gap: var(--space-6);
    flex-shrink: 0;
    padding: var(--space-4) var(--space-10);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-pill);
    font-size: var(--fs-11);
    white-space: nowrap;
    transition: border-color var(--dur-120) ease;
  }

  .neutral {
    color: var(--ls-text);
  }

  button.neutral:hover {
    border-color: var(--ls-text-dim);
  }

  .arcane {
    color: var(--ls-arcane);
    border-color: var(--ls-arcane-a40);
  }

  .open {
    border-color: var(--ls-gold);
  }

  .dot {
    --pill-glow: 0 0 var(--space-6) 0 var(--pill-dot);
    width: var(--space-7);
    height: var(--space-7);
    border-radius: var(--r-pill);
    background: var(--pill-dot);
    box-shadow: var(--pill-glow);
  }

  .caret {
    display: flex;
    color: var(--ls-text-dim);
  }
</style>
