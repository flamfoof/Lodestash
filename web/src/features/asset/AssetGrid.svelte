<script lang="ts">
  import { untrack } from "svelte";
  import type { Selection } from "../../state/selection.svelte";
  import { intentFor, type AssetActionId, type AssetIntent } from "./actions";
  import { RowFrame } from "../../ui";
  import AssetCard from "./AssetCard.svelte";
  import AssetRow from "./AssetRow.svelte";
  import { columnsFor, moveIndex } from "./grid";
  import type { AssetSummary, PackageRef, ProjectRef, SizeThresholds, TraitDef } from "./model";
  import { userStatus } from "./present";

  /**
   * The asset grid (UI-SPEC §2.5, DESIGN-GUIDELINES §9): one page of cards in an ARIA grid —
   * `grid` → `row` → `gridcell` (the card).
   *
   * **The cards never change parent.** There is one `row` element in the DOM (`display: contents`)
   * and every card states its own place with `aria-rowindex` / `aria-colindex`, as a virtualised
   * grid does. A DOM row per drawn row would re-parent cards whenever the column count changes —
   * and it changes when the inspector opens, which a single arrow key does: the focused card would
   * be destroyed under the keyboard, and every card's state with it. The cost: assistive technology
   * that ignores a cell's own indices hears one long row.
   *
   * **One tab stop.** Exactly one card has `tabindex="0"` — the lead, else the card focus was last
   * on, else the first — and every control inside every card has `tabindex="-1"`. Tab enters the
   * grid at that card and leaves it again; it never walks the cards.
   *
   * **Keys, with focus on a card:** ← → ↑ ↓ Home End move the lead, and focus follows it (Shift
   * extends the pick set instead); Enter opens the card; Space quick-looks it; Ctrl/Cmd-A picks the
   * page; the Menu key or Shift+F10 opens the context menu at the card. F and Delete are global
   * keys (`bindAssetKeys`) and act on the selection.
   *
   * **A card's own buttons — the one pattern** (the APG grid's "cell that contains widgets"):
   * `F2` moves focus from the card to its first control; inside, ← → (and ↑ ↓) move among that
   * card's controls and Home/End jump to its ends; `Escape` or `F2` returns to the card. The
   * controls never become tab stops, so Tab from inside still leaves the grid. Every control's
   * action is also a key or a context-menu item, so F2 is a convenience, not the only road.
   *
   * **Columns** are counted once per resize, from the grid's width, the card's minimum width for
   * this `size` and the gap (`columnsFor`) — the arithmetic the browser's `auto-fill` does — and
   * the count is what the layout uses, so the keyboard's rows cannot differ from the drawn ones.
   * The two lengths are read from the grid's own style when the size changes, never per key press.
   * `columns` fixes the count (tests, where nothing is laid out).
   *
   * **Two layouts, one model.** `layout="list"` draws each asset as a row (`AssetRow`) in a single
   * column instead of a card. Nothing else changes: the same `grid` → `row` → `gridcell`, the same
   * selection, the one tab stop, every key above (← → then step by one, as ↑ ↓ do), F2 into a
   * row's buttons, and the focus hand-over — it is this code, not a copy of it.
   *
   * One page is rendered (UI-SPEC §2 pagination bounds it), so the grid is not virtualised; the
   * list renders the rows it is handed, and windowing joins it with paged data (plan P6b).
   */
  let {
    assets,
    selection,
    project,
    traits = [],
    thresholds,
    size = "cozy",
    layout = "grid",
    columns,
    label = "Assets",
    anchored = false,
    onintent,
    onmenu,
    onpack,
    ontag,
  }: {
    assets: readonly AssetSummary[];
    selection: Selection;
    project: ProjectRef | null;
    traits?: readonly TraitDef[];
    thresholds: SizeThresholds;
    size?: "compact" | "cozy" | "large";
    /** Cards in a grid, or rows in one column. */
    layout?: "grid" | "list";
    columns?: number;
    label?: string;
    /** Mark the grid (or the list) and its first cells for the contract gate. */
    anchored?: boolean;
    onintent?: (intent: AssetIntent) => void;
    /** Open the context menu for `asset`: at the pointer (`event`) or at the card (`anchor`). */
    onmenu?: (asset: AssetSummary, at: { event?: MouseEvent; anchor?: HTMLElement }) => void;
    onpack?: (pack: PackageRef) => void;
    ontag?: (value: string) => void;
  } = $props();

  let grid = $state<HTMLElement>();
  let measured = $state<number | null>(null);
  /** The card focus was last on — the tab stop when nothing is selected. */
  let cursor = $state<number | null>(null);
  /** The card whose controls the keyboard is moving among (after F2). */
  let interacting = $state<number | null>(null);

  const order = $derived(assets.map((a) => a.id));
  const list = $derived(layout === "list");
  /** A list is one column; a grid's count is given, or measured. */
  const fixed = $derived(list ? 1 : columns);
  const cols = $derived(fixed ?? measured ?? undefined);
  /** The first Unidentified asset: the gate marks one unknown name in the list. */
  const firstUnknown = $derived(assets.find((a) => userStatus(a.status) === "unidentified")?.id);
  /** Until the width is measured the grid is one row as far as it can tell. */
  const perRow = $derived(cols ?? Math.max(1, assets.length));
  const tabStop = $derived.by(() => {
    if (selection.lead !== null && order.includes(selection.lead)) return selection.lead;
    if (cursor !== null && order.includes(cursor)) return cursor;
    return order[0] ?? null;
  });

  $effect(() => {
    void size; // the minimum width depends on it: measure again when it changes
    if (fixed !== undefined || !grid || typeof ResizeObserver === "undefined") return;
    const el = grid;
    const style = getComputedStyle(el);
    const min = parseFloat(style.getPropertyValue("--grid-min"));
    const gap = parseFloat(style.columnGap) || 0;
    const observer = new ResizeObserver(([entry]) => {
      measured = columnsFor(entry.contentRect.width, min, gap);
    });
    observer.observe(el);
    return () => observer.disconnect();
  });

  // A card that had focus can leave the page (trashed, filtered out, another page). Focus must not
  // fall to the document: it goes to the card now in that place, so the keyboard carries on there.
  let hadFocus = false;
  let lastOrder: readonly number[] = [];
  let lostAt = -1;
  $effect.pre(() => {
    void order;
    hadFocus = !!grid && grid.contains(document.activeElement);
    // Read where focus was without depending on it: only the page changing re-runs this.
    const at = untrack(() => cursor);
    lostAt = hadFocus && at !== null ? lastOrder.indexOf(at) : -1;
  });
  $effect(() => {
    const shown = order;
    if (hadFocus && grid && !grid.contains(document.activeElement) && shown.length > 0) {
      // Only the page changing is a reason to move focus: nothing else is read reactively here.
      const next = lostAt >= 0 ? shown[Math.min(lostAt, shown.length - 1)] : untrack(() => tabStop);
      if (next !== null) cellOf(next)?.focus();
    }
    hadFocus = false;
    lastOrder = shown;
  });

  const cellOf = (id: number) => grid?.querySelector<HTMLElement>(`[role="gridcell"][data-id="${id}"]`) ?? null;
  const controlsOf = (cell: HTMLElement) => [...cell.querySelectorAll<HTMLElement>("button:not(:disabled), input:not(:disabled), a[href]")];

  function emit(id: AssetActionId, asset: AssetSummary) {
    const intent = intentFor(id, { targets: [asset], project });
    if (intent) onintent?.(intent);
  }

  function open(asset: AssetSummary, event: MouseEvent) {
    if (event.shiftKey) selection.extendTo(asset.id, order);
    else if (event.ctrlKey || event.metaKey) selection.toggle(asset.id);
    else selection.open(asset.id);
    // The click landed on the card's name; focus belongs on the card, where the arrows work.
    cellOf(asset.id)?.focus();
  }

  function onkeydown(event: KeyboardEvent) {
    const target = event.target as HTMLElement;
    const cell = target.closest<HTMLElement>('[role="gridcell"]');
    if (!cell || !grid?.contains(cell)) return;
    const id = Number(cell.dataset.id);
    const index = order.indexOf(id);
    if (index < 0) return;
    const inside = target !== cell;
    const done = () => {
      event.preventDefault();
      event.stopPropagation();
    };

    // Among a card's own controls (after F2).
    if (inside && interacting === id) {
      if (event.key === "Escape" || event.key === "F2") {
        interacting = null;
        cell.focus();
        done();
        return;
      }
      // The controls are a list: all four arrows step through it.
      const controls = controlsOf(cell);
      const to = moveIndex(controls.indexOf(target), event.key, 1, controls.length);
      if (to !== null) {
        controls[to].focus();
        done();
      }
      return;
    }

    if (event.key === "F2") {
      const first = controlsOf(cell)[0];
      if (first) {
        interacting = id;
        first.focus();
      }
      done();
      return;
    }

    const to = event.altKey ? null : moveIndex(index, event.key, cols ?? 1, order.length);
    if (to !== null) {
      const next = order[to];
      if (event.shiftKey) selection.extendTo(next, order);
      else selection.moveTo(next);
      cellOf(next)?.focus();
      done();
      return;
    }

    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "a") {
      selection.pickAll(order);
      done();
      return;
    }

    if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) {
      onmenu?.(assets[index], { anchor: cell });
      done();
      return;
    }

    // Enter and Space belong to a focused button; on the card itself they open and quick-look it.
    if (inside) return;
    if (event.key === "Enter") {
      selection.moveTo(id);
      done();
    } else if (event.key === " ") {
      emit("quick-look", assets[index]);
      done();
    }
  }

  function onfocusin(event: FocusEvent) {
    const cell = (event.target as HTMLElement).closest<HTMLElement>('[role="gridcell"]');
    if (!cell) return;
    const id = Number(cell.dataset.id);
    cursor = id;
    if (interacting !== null && interacting !== id) interacting = null;
  }

  function onfocusout(event: FocusEvent) {
    const next = event.relatedTarget as Node | null;
    if (!next || !grid?.contains(next)) interacting = null;
  }
</script>

{#snippet cells()}
  {#each assets as asset, i (asset.id)}
    {#if list}
      <AssetRow
        {asset}
        row={i + 1}
        col={1}
        alt={i % 2 === 1}
        selected={selection.lead === asset.id}
        picked={selection.picks.has(asset.id)}
        picking={selection.picking}
        tabStop={tabStop === asset.id}
        {thresholds}
        anchors={!anchored ? undefined : i === 0 ? "first" : i === 1 ? "alt" : asset.id === firstUnknown ? "unknown" : undefined}
        onopen={(event) => open(asset, event)}
        onpick={() => selection.toggle(asset.id)}
        onaction={(id) => emit(id, asset)}
        onmenu={(event) => onmenu?.(asset, { event })}
        {onpack}
      />
    {:else}
      <AssetCard
        {asset}
        row={Math.floor(i / perRow) + 1}
        col={(i % perRow) + 1}
        selected={selection.lead === asset.id}
        picked={selection.picks.has(asset.id)}
        picking={selection.picking}
        tabStop={tabStop === asset.id}
        {project}
        {traits}
        {thresholds}
        anchored={anchored && i === 0}
        onopen={(event) => open(asset, event)}
        onpick={() => selection.toggle(asset.id)}
        onaction={(id) => emit(id, asset)}
        onmenu={(event) => onmenu?.(asset, { event })}
        {onpack}
        {ontag}
      />
    {/if}
  {/each}
{/snippet}

<div
  class={list ? "list" : "grid"}
  role="grid"
  tabindex={-1}
  aria-label={label}
  aria-multiselectable="true"
  aria-rowcount={Math.ceil(assets.length / perRow)}
  aria-colcount={cols}
  data-layout={layout}
  data-size={list ? undefined : size}
  data-anchor={anchored && !list ? "library.grid" : undefined}
  style:--cols={list ? undefined : cols}
  bind:this={grid}
  {onkeydown}
  {onfocusin}
  {onfocusout}
>
  {#if list}
    <RowFrame role="row" data-anchor={anchored ? "list" : undefined}>{@render cells()}</RowFrame>
  {:else}
    <div class="row" role="row">{@render cells()}</div>
  {/if}
</div>

<style>
  /* Layout only. Until the width is measured the browser's own auto-fill decides the columns. */
  .grid {
    display: grid;
    grid-template-columns: repeat(var(--cols, auto-fill), minmax(var(--grid-min), 1fr));
    gap: var(--space-12);
  }

  .grid[data-size="compact"] {
    --grid-min: var(--card-min-compact);
  }

  .grid[data-size="cozy"] {
    --grid-min: var(--card-min-cozy);
  }

  .grid[data-size="large"] {
    --grid-min: var(--card-min-large);
  }

  /* The row exists for assistive technology; its cards are the grid's items. */
  .row {
    display: contents;
  }
</style>
