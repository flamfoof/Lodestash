<script lang="ts" module>
  export type ButtonVariant = "primary" | "secondary" | "ghost" | "danger" | "link" | "dashed";
  export type ButtonSize = "xs" | "sm" | "md" | "lg";
</script>

<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLButtonAttributes } from "svelte/elements";
  import Icon, { type IconBox, type IconSize } from "./Icon.svelte";
  import type { IconName } from "./icons.generated";

  /**
   * The one button. Variants and sizes are the prototype's:
   * - `primary` — the gold gradient call to action ("+ Create", "Open library →");
   * - `secondary` — outlined, muted ("★ Favorites", "⚑ Filters", "+ stage");
   * - `ghost` — text only until hovered; `danger` — outlined red; `link` — inline text action;
   * - `dashed` — the dashed, full-width "+ Add source…" row at the foot of a rail.
   * `md` is the toolbar size (5/11 px padding, 12 px type); `xs` the on-thumbnail size (2/8, 11 px);
   * `sm` the settings-row size (2/9, 10 px); `lg` the screen-header size (7/16, 13 px).
   * `onMedia` puts the translucent backdrop behind it that keeps it legible over a thumbnail.
   * `active` is the gold "this is filtering the view" state: `true` is the Filters button with
   * filters set, `"pressed"` the brighter on-state of a toggle (★ Favorites on).
   * An icon replaces the prototype's leading Unicode glyph at the glyph's advance (`iconBox`).
   */
  let {
    variant = "secondary",
    size = "md",
    icon,
    iconSize = "12",
    iconBox,
    onMedia = false,
    active = false,
    type = "button",
    children,
    leading,
    trailing,
    ...rest
  }: HTMLButtonAttributes & {
    variant?: ButtonVariant;
    size?: ButtonSize;
    icon?: IconName;
    iconSize?: IconSize;
    iconBox?: IconBox;
    onMedia?: boolean;
    active?: boolean | "pressed";
    children?: Snippet;
    /** Before the label, separated by the button's gap (the "+" of "+ Add source…"). */
    leading?: Snippet;
    /** After the label, separated by the button's gap (a count pill, a caret). */
    trailing?: Snippet;
  } = $props();
</script>

<button
  {type}
  class="btn {variant} {size}"
  class:on-media={onMedia}
  class:active={!!active}
  class:pressed={active === "pressed"}
  {...rest}
>
  {@render leading?.()}
  <span class="label">
    {#if icon}<Icon name={icon} size={iconSize} box={iconBox} />{" "}{/if}{@render children?.()}
  </span>
  {@render trailing?.()}
</button>

<style>
  .btn {
    display: inline-flex;
    align-items: center;
    flex-shrink: 0;
    white-space: nowrap;
    color: var(--ls-text-muted);
    border-radius: var(--r-6);
    transition:
      border-color var(--dur-120) ease,
      color var(--dur-120) ease,
      box-shadow var(--dur-120) ease;
  }

  .btn:disabled {
    opacity: var(--op-35);
  }

  /* ---- sizes ---- */
  .xs {
    padding: var(--space-2) var(--space-8);
    font-size: var(--fs-11);
    border-radius: var(--r-4);
  }

  .sm {
    padding: var(--space-2) var(--space-9);
    font-size: var(--fs-10);
    border-radius: var(--r-4);
    gap: var(--space-5);
  }

  .md {
    padding: var(--space-5) var(--space-11);
    font-size: var(--fs-12);
    gap: var(--space-6);
  }

  .lg {
    padding: var(--space-7) var(--space-16);
    gap: var(--space-6);
  }

  /* ---- variants ---- */
  .primary {
    background: var(--grad-gold);
    color: var(--ls-on-gold);
    font-weight: var(--fw-600);
  }

  .primary:hover:not(:disabled) {
    box-shadow: var(--glow-gold-14-a50);
  }

  .secondary {
    border: 1px solid var(--ls-border-strong);
  }

  .secondary:hover:not(:disabled) {
    border-color: var(--ls-text-dim);
    color: var(--ls-text);
  }

  .ghost:hover:not(:disabled) {
    color: var(--ls-text);
    background: var(--ls-surface-raised);
  }

  .danger {
    color: var(--ls-danger);
    border: 1px solid var(--ls-danger-a40);
  }

  .danger:hover:not(:disabled) {
    border-color: var(--ls-danger);
    box-shadow: var(--glow-danger-10-a20);
  }

  .link {
    padding: 0;
    color: var(--ls-gold);
    font-weight: var(--fw-600);
  }

  .link:hover:not(:disabled) {
    color: var(--ls-gold-bright);
  }

  .dashed {
    display: flex;
    width: 100%;
    gap: var(--space-8);
    padding: var(--space-6) var(--space-14);
    border: 1px dashed var(--ls-border-strong);
    color: var(--ls-text-dim);
    font-size: var(--fs-12);
  }

  .dashed:hover:not(:disabled) {
    color: var(--ls-gold);
    border-color: var(--ls-gold-a50);
  }

  .on-media {
    background: var(--ls-bg-a78);
  }

  .secondary.on-media:hover:not(:disabled) {
    border-color: var(--ls-gold);
    color: var(--ls-gold-bright);
  }

  .active {
    border-color: var(--ls-gold-a45);
    color: var(--ls-gold);
    background: var(--ls-gold-a15);
  }

  .active:hover:not(:disabled),
  .active.pressed {
    border-color: var(--ls-gold);
    color: var(--ls-gold-bright);
  }
</style>
