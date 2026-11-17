<script lang="ts">
  import type { HTMLButtonAttributes } from "svelte/elements";
  import Icon, { type IconSize } from "./Icon.svelte";
  import type { IconName } from "./icons.generated";

  /**
   * A square button that is only an icon. `label` is required: it is the accessible name, and the
   * tooltip (`title`) unless one is given.
   * - `tile` — the 20 px tile on a thumbnail (☆ favourite), on the translucent backdrop;
   * - `plain` — the 22 px dialog close / row action, on the overlay surface.
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
    variant?: "tile" | "plain";
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

  .icon-btn:hover:not(:disabled) {
    color: var(--ls-text);
  }

  .tile:hover:not(:disabled) {
    color: var(--ls-gold-bright);
  }

  .pressed {
    color: var(--ls-gold);
  }
</style>
