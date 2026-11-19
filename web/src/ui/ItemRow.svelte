<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLAttributes, HTMLButtonAttributes } from "svelte/elements";
  import type { Status } from "./StatusGem.svelte";

  /**
   * The item card's list form (DESIGN-GUIDELINES §7 "Asset list row"): one 38 px line in fixed
   * columns — thumb · mark · name · pack · size · stats · status · actions — so a column reads
   * straight down the list.
   *
   * Like the card it is a container, never a button: a cell of a grid (`role="gridcell"`, the
   * grid's one tab stop when `tabStop`) with **one primary action**, the item's name, a real
   * `<button>` stretched over the whole row, and real buttons above it (`tabindex="-1"` — the grid
   * owns how focus reaches them).
   *
   * Looks: the surface or, on every second row, its zebra tone (`alt`); a 3 px edge in the status
   * colour; the raised tone under the pointer; a 2 px gold ring when `selected`. The rings are drawn
   * inside the row — the frame clips what is outside it. The `pick` control sits over the thumb
   * and shows on hover, while focus is inside, and while `pin` is set (something is picked).
   */
  let {
    status,
    title,
    selected = false,
    alt = false,
    tabStop = false,
    pin = false,
    onopen,
    titleAttrs = {},
    packAttrs = {},
    statsAttrs = {},
    media,
    pick,
    mark,
    pack,
    size,
    stats,
    state,
    actions,
    ...rest
  }: HTMLAttributes<HTMLDivElement> & {
    status: Status;
    title: string;
    selected?: boolean;
    /** Every second row. */
    alt?: boolean;
    tabStop?: boolean;
    /** Keep the pick control showing. */
    pin?: boolean;
    /** The primary action: a click anywhere on the row that is not another control. */
    onopen?: (event: MouseEvent) => void;
    titleAttrs?: HTMLButtonAttributes & Record<`data-${string}`, string | undefined>;
    packAttrs?: Record<`data-${string}`, string | undefined>;
    statsAttrs?: Record<`data-${string}`, string | undefined>;
    /** The 26 px thumb. */
    media: Snippet;
    /** Over the thumb: the pick box. */
    pick?: Snippet;
    /** Before the name: the type mark. */
    mark?: Snippet;
    pack?: Snippet;
    size?: Snippet;
    stats?: Snippet;
    /** The status, in words. */
    state?: Snippet;
    actions?: Snippet;
  } = $props();
</script>

<div class="row {status}" class:selected class:alt role="gridcell" tabindex={tabStop ? 0 : -1} aria-selected={selected} {...rest}>
  <span class="media">
    {@render media()}
    {#if pick}<span class="pick" class:pin>{@render pick()}</span>{/if}
  </span>
  {@render mark?.()}
  <button type="button" class="open" tabindex="-1" title={title} onclick={onopen} {...titleAttrs}>{title}</button>
  <span class="pack" {...packAttrs}>{@render pack?.()}</span>
  <span class="size">{@render size?.()}</span>
  <span class="stats" {...statsAttrs}>{@render stats?.()}</span>
  <span class="state">{@render state?.()}</span>
  <span class="actions">{@render actions?.()}</span>
</div>

<style>
  .row {
    position: relative;
    display: flex;
    align-items: center;
    gap: var(--space-10);
    flex-shrink: 0;
    height: var(--list-row-h);
    padding: 0 var(--space-12);
    background: var(--ls-surface);
    border-left: var(--bw-3) solid var(--row-status);
    cursor: pointer;
    transition: background var(--dur-120) ease;
  }

  .alt {
    background: var(--ls-row-alt);
  }

  .row:hover {
    background: var(--ls-overlay);
  }

  .unidentified {
    --row-status: var(--ls-status-unidentified);
  }

  .identified {
    --row-status: var(--ls-status-identified);
  }

  .confirmed {
    --row-status: var(--ls-status-confirmed);
  }

  /* Inside the row: the frame's overflow would cut an outer ring off at both ends. */
  .selected {
    box-shadow: var(--ring-inset-gold-2);
  }

  .row:focus-visible {
    box-shadow: var(--ring-inset-focus) !important;
  }

  .selected:focus-visible {
    box-shadow: var(--ring-inset-focus-selected) !important;
  }

  .media {
    position: relative;
    display: flex;
    flex: none;
  }

  /* The pick box, centred over the thumb, above the stretched primary action. */
  .pick {
    position: absolute;
    inset: 0;
    z-index: var(--z-raised);
    display: flex;
    align-items: center;
    justify-content: center;
    opacity: 0;
    pointer-events: none;
    transition: opacity var(--dur-120) ease;
  }

  .row:hover .pick,
  .row:focus-within .pick,
  .row:focus-visible .pick,
  .pick.pin {
    opacity: 1;
    pointer-events: auto;
  }

  .open {
    width: var(--list-name-w);
    flex: none;
    font-size: var(--fs-12);
    font-weight: var(--fw-600);
    color: var(--ls-text);
    text-align: left;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .unidentified .open {
    color: var(--ls-status-unidentified);
    font-style: italic;
  }

  /* The primary action covers the row: a click that is not on another control opens it. */
  .open::after {
    content: "";
    position: absolute;
    inset: 0;
  }

  .pack,
  .stats {
    font-family: var(--ff-mono);
    font-size: var(--fs-11);
    color: var(--ls-text-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .pack {
    width: var(--list-pack-w);
    flex: none;
  }

  .stats {
    flex: 1;
    min-width: 0;
  }

  .size,
  .state {
    display: flex;
    flex: none;
  }

  .size {
    width: var(--list-size-w);
  }

  .state {
    width: var(--list-status-w);
  }

  /* What sits in a fixed column fills it. */
  .size > :global(*),
  .state > :global(*) {
    flex: 1;
    min-width: 0;
  }

  /* The row's buttons are its flex items, at the row's own gap. */
  .actions {
    display: contents;
  }

  /* Only controls take the pointer above the stretched action; plain text lets the click through. */
  .pack :global(button),
  .actions > :global(*) {
    position: relative;
    z-index: var(--z-raised);
  }
</style>
