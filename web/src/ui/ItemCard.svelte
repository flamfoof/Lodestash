<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLAttributes, HTMLButtonAttributes } from "svelte/elements";
  import type { Status } from "./StatusGem.svelte";

  /**
   * The item card (DESIGN-GUIDELINES §7 "Asset card"): a container — never a button — with **one
   * primary action**, the item's name, a real `<button>` stretched over the whole card, and real
   * buttons above it in four corner trays, the meta line and the chip row.
   *
   * It is a cell of a grid (`role="gridcell"`): `tabStop` makes it the grid's one tab stop, and
   * every control inside it is `tabindex="-1"` — the grid owns how focus reaches them.
   *
   * Looks: the surface, a hairline, a 3 px edge in the status colour (colour + the status gem its
   * owner puts in a tray: never colour alone), a 2 px gold ring when `selected`, a faint one at rest
   * when confirmed. The `topStart` and `bottomEnd` trays (pick box; quick actions) show on hover and
   * while focus is inside; `pin` keeps one showing (something is picked; the item is staged).
   */
  let {
    status,
    title,
    selected = false,
    tabStop = false,
    pin = {},
    onopen,
    titleAttrs = {},
    bodyAttrs = {},
    metaAttrs = {},
    mark,
    media,
    topStart,
    topEnd,
    bottomStart,
    bottomEnd,
    meta,
    chips,
    ...rest
  }: HTMLAttributes<HTMLDivElement> & {
    status: Status;
    title: string;
    selected?: boolean;
    tabStop?: boolean;
    pin?: { topStart?: boolean; bottomEnd?: boolean };
    /** The primary action: a click anywhere on the card that is not another control. */
    onopen?: (event: MouseEvent) => void;
    titleAttrs?: HTMLButtonAttributes & Record<`data-${string}`, string | undefined>;
    bodyAttrs?: Record<`data-${string}`, string | undefined>;
    metaAttrs?: Record<`data-${string}`, string | undefined>;
    /** Before the title: the type mark. */
    mark?: Snippet;
    media: Snippet;
    topStart?: Snippet;
    topEnd?: Snippet;
    bottomStart?: Snippet;
    bottomEnd?: Snippet;
    meta?: Snippet;
    chips?: Snippet;
  } = $props();
</script>

<div class="card {status}" class:selected role="gridcell" tabindex={tabStop ? 0 : -1} aria-selected={selected} {...rest}>
  <div class="media">
    {@render media()}
    {#if topStart}<div class="tray top start quiet" class:pin={pin.topStart}>{@render topStart()}</div>{/if}
    {#if topEnd}<div class="tray top end">{@render topEnd()}</div>{/if}
    {#if bottomStart}<div class="tray bottom start">{@render bottomStart()}</div>{/if}
    {#if bottomEnd}<div class="tray bottom end quiet" class:pin={pin.bottomEnd}>{@render bottomEnd()}</div>{/if}
  </div>
  <div class="body" {...bodyAttrs}>
    <div class="title">
      {@render mark?.()}
      <button type="button" class="open" tabindex="-1" onclick={onopen} {...titleAttrs}>{title}</button>
    </div>
    {#if meta}<div class="meta" {...metaAttrs}>{@render meta()}</div>{/if}
    <div class="chips">{@render chips?.()}</div>
  </div>
</div>

<style>
  .card {
    position: relative;
    min-width: 0;
    background: var(--ls-surface);
    border: 1px solid var(--ls-border);
    border-left: var(--bw-3) solid var(--card-status);
    border-radius: var(--r-10);
    overflow: hidden;
    cursor: pointer;
    transition:
      transform var(--dur-120) ease,
      background var(--dur-120) ease;
  }

  .card:hover {
    transform: translateY(calc(var(--space-2) * -1));
    background: var(--ls-surface-raised);
  }

  .unidentified {
    --card-status: var(--ls-status-unidentified);
    opacity: var(--op-92);
  }

  .identified {
    --card-status: var(--ls-status-identified);
  }

  .confirmed {
    --card-status: var(--ls-status-confirmed);
    box-shadow: var(--ring-gold-1-a18);
  }

  .selected {
    box-shadow: var(--ring-gold-2);
  }

  /* Focus stacks outside the selection ring instead of replacing it. */
  .selected:focus-visible {
    box-shadow: var(--ring-focus-selected) !important;
  }

  .media {
    position: relative;
  }

  /* The corner trays hold the card's other controls, above the stretched primary action. */
  .tray {
    position: absolute;
    z-index: var(--z-raised);
    display: flex;
    align-items: center;
    gap: var(--space-5);
  }

  .top {
    top: var(--space-8);
    gap: var(--space-6);
  }

  .bottom {
    bottom: var(--space-8);
  }

  .start {
    left: var(--space-8);
  }

  .end {
    right: var(--space-8);
  }

  .quiet {
    opacity: 0;
    pointer-events: none;
    transition: opacity var(--dur-120) ease;
  }

  .card:hover .quiet,
  .card:focus-within .quiet,
  .card:focus-visible .quiet,
  .quiet.pin {
    opacity: 1;
    pointer-events: auto;
  }

  .body {
    padding: var(--space-8) var(--space-12) var(--space-12);
  }

  .title {
    display: flex;
    align-items: center;
    gap: var(--space-6);
    font-size: var(--fs-12);
    font-weight: var(--fw-600);
  }

  .open {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--ls-text);
  }

  .unidentified .open {
    color: var(--ls-status-unidentified);
    font-style: italic;
  }

  /* The primary action covers the card: a click on the artwork or the body opens it. */
  .open::after {
    content: "";
    position: absolute;
    inset: 0;
  }

  .meta,
  .chips {
    position: relative;
    z-index: var(--z-raised);
    /* Only the controls inside take the pointer; plain text lets the click through to the card. */
    pointer-events: none;
  }

  .meta :global(button),
  .chips :global(button) {
    pointer-events: auto;
  }

  .meta {
    margin: var(--space-3) 0 var(--space-6);
    font-family: var(--ff-mono);
    font-size: var(--fs-11);
    color: var(--ls-text-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-5);
    min-height: var(--space-18);
  }
</style>
