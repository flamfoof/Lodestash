<script lang="ts" module>
  export type ButtonVariant =
    | "primary"
    | "secondary"
    | "ghost"
    | "danger"
    | "link"
    | "dashed"
    | "outline-gold"
    | "outline-arcane"
    | "arcane"
    | "quiet-gold";
  export type ButtonSize = "xs" | "sm" | "md" | "lg" | "pill" | "block" | "block-sm" | "bar";
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
   * - `dashed` — the dashed, full-width "+ Add source…" row at the foot of a rail;
   * - `outline-gold` / `outline-arcane` / `arcane` — the inspector's actions: ⟡ Stage (gold
   *   outline), ✦ Re-identify (arcane outline), ✦ Identify now (arcane fill);
   * - `quiet-gold` — an action in the bulk bar ("+ Add tag", "⟡ Stage all"): gold text on a faint
   *   gold hairline, regular weight — one of several in a row, none of them the call to action.
   * `bar` is the size of a button in that bar (2/10 px, the bar's 12 px type, a 4 px corner).
   * `md` is the toolbar size (5/11 px padding, 12 px type); `xs` the on-thumbnail size (2/8, 11 px);
   * `sm` the settings-row size (2/9, 10 px); `lg` the screen-header size (7/16, 13 px); `pill` the
   * rounded "⤢ full size" on a preview (4/12, 11 px); `block` / `block-sm` fill their column (8 px
   * and 7 px tall padding, 13 and 12 px type).
   * `onMedia` puts the translucent backdrop behind it that keeps it legible over a thumbnail;
   * `accent` is what it turns under the pointer there (gold: stage; arcane: looking closer).
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
    accent = "gold",
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
    accent?: "gold" | "arcane";
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
  class:arcane-accent={accent === "arcane"}
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

  .pill {
    padding: var(--space-4) var(--space-12);
    font-size: var(--fs-11);
    border-radius: var(--r-pill);
    gap: var(--space-6);
  }

  .block,
  .block-sm {
    display: flex;
    justify-content: center;
    width: 100%;
    min-width: 0;
  }

  .bar {
    padding: var(--space-2) var(--space-10);
    font-size: var(--fs-12);
    border-radius: var(--r-4);
  }

  .block {
    padding: var(--space-8) 0;
    font-size: var(--fs-13);
  }

  .block-sm {
    padding: var(--space-7) 0;
    font-size: var(--fs-12);
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

  .outline-gold {
    color: var(--ls-gold);
    border: 1px solid var(--ls-gold-a45);
    font-weight: var(--fw-600);
  }

  .outline-gold:hover:not(:disabled) {
    border-color: var(--ls-gold);
    box-shadow: var(--glow-gold-10-a25);
  }

  .quiet-gold {
    color: var(--ls-gold);
    border: 1px solid var(--ls-gold-a40);
  }

  .quiet-gold:hover:not(:disabled) {
    border-color: var(--ls-gold);
  }

  .outline-arcane {
    color: var(--ls-arcane);
    border: 1px solid var(--ls-arcane-a35);
  }

  .outline-arcane:hover:not(:disabled) {
    border-color: var(--ls-arcane);
  }

  .arcane {
    color: var(--ls-arcane-bright);
    background: var(--ls-arcane-a14);
    border: 1px solid var(--ls-arcane);
    font-weight: var(--fw-600);
  }

  .arcane:hover:not(:disabled) {
    box-shadow: var(--glow-arcane-14-a40);
  }

  .on-media {
    background: var(--ls-bg-a78);
  }

  .pill.on-media {
    background: var(--ls-bg-a80);
    color: var(--ls-text);
  }

  .secondary.on-media:hover:not(:disabled) {
    border-color: var(--ls-gold);
    color: var(--ls-gold-bright);
  }

  .secondary.on-media.arcane-accent:hover:not(:disabled) {
    border-color: var(--ls-arcane);
    color: var(--ls-arcane-bright);
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

  /* On a thumbnail the backdrop stays: "✓ staged" is the brighter text on a gold hairline. */
  .on-media.active {
    background: var(--ls-bg-a78);
    border-color: var(--ls-gold-a50);
    color: var(--ls-gold-bright);
  }
</style>
