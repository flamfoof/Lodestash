<script lang="ts">
  import type { HTMLButtonAttributes } from "svelte/elements";
  import Icon, { type IconSize } from "./Icon.svelte";
  import type { IconName } from "./icons.generated";

  /**
   * A square button that is only an icon. `label` is required: it is the accessible name, and the
   * tooltip (`title`) unless one is given.
   * - `tile` — the 20 px tile on a thumbnail (☆ favourite), on the translucent backdrop;
   * - `plain` — the 22 px dialog close / row action, on the overlay surface;
   * - `media` — the 22 px close on a preview, on the translucent backdrop;
   * - `round` — the 30 px arcane play button of a sound.
   * `pressed` makes it a toggle (`aria-pressed`) drawn gold when on.
   */
  let {
    icon,
    label,
    variant = "plain",
    iconSize = "12",
    pressed,
    title,
    ...rest
  }: HTMLButtonAttributes & {
    icon: IconName;
    label: string;
    variant?: "tile" | "plain" | "media" | "round";
    iconSize?: IconSize;
    pressed?: boolean;
  } = $props();
</script>

<button
  type="button"
  class="icon-btn {variant}"
  class:pressed
  aria-label={label}
  aria-pressed={pressed}
  title={title ?? label}
  {...rest}
>
  <Icon name={icon} size={iconSize} />
</button>

<style>
  .icon-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    flex: none;
    border-radius: var(--r-4);
    transition: color var(--dur-120) ease;
  }

  .tile {
    width: var(--space-20);
    height: var(--space-20);
    background: var(--ls-bg-a75);
    color: var(--ls-status-unidentified);
    font-size: var(--fs-12);
  }

  .plain {
    width: var(--space-22);
    height: var(--space-22);
    background: var(--ls-overlay);
    color: var(--ls-text-muted);
    font-size: var(--fs-12);
  }

  .media {
    width: var(--space-22);
    height: var(--space-22);
    background: var(--ls-bg-a75);
    color: var(--ls-text-muted);
    font-size: var(--fs-12);
  }

  .round {
    width: var(--space-30);
    height: var(--space-30);
    border-radius: var(--r-pill);
    background: var(--ls-arcane-a16);
    border: 1px solid var(--ls-arcane);
    color: var(--ls-arcane-bright);
    font-size: var(--fs-11);
    transition: box-shadow var(--dur-120) ease;
  }

  .icon-btn:hover:not(:disabled) {
    color: var(--ls-text);
  }

  .round:hover:not(:disabled) {
    color: var(--ls-arcane-bright);
    box-shadow: var(--glow-arcane-12-a50);
  }

  .tile:hover:not(:disabled) {
    color: var(--ls-gold-bright);
  }

  .pressed {
    color: var(--ls-gold);
  }
</style>
