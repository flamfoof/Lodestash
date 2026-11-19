<script lang="ts">
  import { untrack } from "svelte";
  import { keymap } from "../app/keymap.svelte";
  import { bindAssetKeys, type AssetIntent } from "../features/asset/actions";
  import AssetContextMenu from "../features/asset/AssetContextMenu.svelte";
  import AssetGrid from "../features/asset/AssetGrid.svelte";
  import type { AssetSummary, PackageRef, ProjectRef, SizeThresholds, TraitDef } from "../features/asset/model";
  import { activeFilters, isShellAxis, type ActiveFilter, type Constraints, type ShellAxis } from "../features/filters/active";
  import ActiveFilterBar from "../features/filters/ActiveFilterBar.svelte";
  import FilterPanel from "../features/filters/FilterPanel.svelte";
  import { filterCount, NO_FILTERS, toggleFilter, type LibraryFilters, type TraitCount, type TriBucket } from "../features/filters/filters";
  import BulkBar from "../features/library/BulkBar.svelte";
  import LibraryToolbar from "../features/library/LibraryToolbar.svelte";
  import UnidentifiedBanner from "../features/library/UnidentifiedBanner.svelte";
  import type { LibraryView } from "../features/library/view";
  import QuickLook from "../features/quicklook/QuickLook.svelte";
  import { stepId } from "../features/quicklook/step";
  import type { Playback } from "../state/playback";
  import type { Selection } from "../state/selection.svelte";
  import { Button, EmptyState, Popover, type Tier } from "../ui";

  /**
   * The Library: the Unidentified nudge, the toolbar, the active-filter bar, and one page of the
   * result as a grid of cards or a list of rows — the view toggle's two stops (UI-SPEC §2). A thin composition — it places features and wires them; every
   * look is a primitive's and nothing here fetches. Data comes in as props (the page, already
   * filtered and sorted by whoever owns the query); what the person asks for goes out as intents
   * and as changes to the view and the filters. The one state it shares is the `selection`, which
   * it also clears on Escape.
   *
   * **Filters.** The ⚑ set (`filters`) and the shell's constraints (`constraints`: the search box,
   * the type tabs, the sidebar facets) are handed in. The screen opens the ⚑ popover, and the
   * active-filter bar lists every constraint; removing one reports it to whoever owns that axis —
   * `onfilterschange` for ⚑, `onview` for ★, `onunconstrain` for the shell's.
   *
   * **Picks.** While anything is picked the bulk bar sits under the toolbar and acts on the pick
   * set; its intents go the way every intent goes.
   *
   * **Quick Look.** Which asset it shows is the owner's (`quickLook`) — the inspector opens it too.
   * The screen knows the page, so it resolves the asset, steps through the page in the order
   * shown, moves the lead along, and puts focus back on that asset's cell when it closes.
   *
   * Keys (UI-SPEC §0.4): Space, F and Delete are bound in the keymap's global scope for as long as
   * the screen is mounted, so they act on the selection from anywhere in the window and never behind
   * an open overlay. The grid's own keys are the grid's (AssetGrid).
   */
  let {
    assets,
    count = assets.length,
    total = count,
    scope = "all libraries",
    unidentified = 0,
    nudgeDismissed = false,
    view,
    filters = NO_FILTERS,
    traitOptions = [],
    constraints = {},
    favoriteCount = 0,
    selection,
    project,
    traits = [],
    thresholds,
    quickLook = null,
    playback = null,
    peaks = null,
    onintent,
    onview,
    onfilterschange,
    onunconstrain,
    onclearfilters,
    onquicklook,
    onseek,
    onidentify,
    ondismissnudge,
    onpack,
    onsearchtag,
  }: {
    /** The current page of the result. */
    assets: readonly AssetSummary[];
    /** Assets in the whole result (the page may be a part of it). */
    count?: number;
    /** Assets with no constraint at all — the "N" of the bar's `n of N shown`. */
    total?: number;
    /** What the result is drawn from: "all libraries", or a library's name. */
    scope?: string;
    /** Unidentified assets in the library — the nudge. */
    unidentified?: number;
    nudgeDismissed?: boolean;
    view: LibraryView;
    /** The ⚑ content filter in effect. */
    filters?: LibraryFilters;
    /** The tags the ⚑ popover offers, in the order to offer them (most used first). */
    traitOptions?: readonly TraitCount[];
    /** What the shell constrains the result by — for the active-filter bar. */
    constraints?: Constraints;
    favoriteCount?: number;
    selection: Selection;
    project: ProjectRef | null;
    traits?: readonly TraitDef[];
    thresholds: SizeThresholds;
    /** The asset Quick Look is showing; null when it is closed. */
    quickLook?: number | null;
    /** What is playing, for Quick Look's transport. */
    playback?: Playback | null;
    /** The waveform of the sound Quick Look is showing. */
    peaks?: readonly number[] | null;
    onintent?: (intent: AssetIntent) => void;
    onview?: (patch: Partial<LibraryView>) => void;
    onfilterschange?: (next: LibraryFilters) => void;
    /** Remove one of the shell's constraints (a chip's ✕ in the active-filter bar). */
    onunconstrain?: (axis: ShellAxis) => void;
    /** Reset every constraint: the shell's, ⚑ and ★ (the bar's Clear all). */
    onclearfilters?: () => void;
    /** Quick Look stepped to another asset of the page, or closed (null). Opening is an intent. */
    onquicklook?: (id: number | null) => void;
    onseek?: (seconds: number) => void;
    onidentify?: () => void;
    ondismissnudge?: () => void;
    onpack?: (pack: PackageRef) => void;
    onsearchtag?: (value: string) => void;
  } = $props();

  const uid = $props.id();
  const filtersId = `${uid}-filters`;

  // The context menu: which asset it was opened on (an id — never a copy of the asset) and where.
  let menuId = $state<number | null>(null);
  let menuAt = $state<{ event?: MouseEvent; anchor?: HTMLElement } | null>(null);
  // The ⚑ popover: open or not, and the button it hangs from.
  let filtersOpen = $state(false);
  let filtersAnchor = $state<HTMLElement>();

  const order = $derived(assets.map((a) => a.id));
  const resolve = (ids: readonly number[]) => assets.filter((a) => ids.includes(a.id));
  const menuTargets = $derived(menuId === null ? [] : resolve(selection.targets(menuId)));
  /** The pick set as assets, in the page's order: what the bulk bar acts on. */
  const picked = $derived(assets.filter((a) => selection.picks.has(a.id)));
  const active = $derived(activeFilters(constraints, filters, view.favoritesOnly, thresholds));
  /** The asset Quick Look shows — looked up on the page every time, never kept. */
  const peeked = $derived(quickLook === null ? null : (assets.find((a) => a.id === quickLook) ?? null));

  let grid = $state<{ focusCell(id: number): boolean; holds(node: Node | null): boolean }>();

  /**
   * "Open" is the selection's business; everything else is the owner's. Quick Look is both: the
   * asset looked at becomes the lead (UI-SPEC §14 "syncing selection"), and the owner — who also
   * hears the inspector ask for it — opens it.
   */
  function dispatch(intent: AssetIntent) {
    if (intent.action === "open") {
      selection.moveTo(intent.ids[0]);
      return;
    }
    if (intent.action === "quick-look") selection.moveTo(intent.ids[0]);
    onintent?.(intent);
  }

  /** ← → in Quick Look: the next asset of this page, in the order shown; the lead goes with it. */
  function stepQuickLook(dir: 1 | -1) {
    const next = stepId(order, quickLook, dir);
    if (next === null) return;
    selection.moveTo(next);
    onquicklook?.(next);
  }

  // Quick Look shows an asset of this page: if the page loses it, there is nothing to show.
  $effect(() => {
    if (quickLook !== null && !order.includes(quickLook)) untrack(() => onquicklook?.(null));
  });

  // When Quick Look closes the dialog hands focus back to what opened it. If that was in the grid
  // (or is gone), focus belongs on the cell of the asset last shown — stepping moved the lead
  // there, and focus follows the lead. Opened from elsewhere (the inspector), it stays there.
  let lastPeeked: number | null = null;
  $effect(() => {
    if (peeked) {
      lastPeeked = peeked.id;
      return;
    }
    if (lastPeeked === null) return;
    const last = lastPeeked;
    lastPeeked = null;
    const at = document.activeElement;
    if (at === null || at === document.body || grid?.holds(at)) grid?.focusCell(last);
  });

  /** A chip's ✕: each axis has one owner, and it is told. */
  function removeFilter(filter: ActiveFilter) {
    if (filter.axis === "trait") onfilterschange?.(toggleFilter(filters, "traits", filter.value!));
    else if (filter.axis === "tris") onfilterschange?.(toggleFilter(filters, "tris", filter.value as TriBucket));
    else if (filter.axis === "size") onfilterschange?.(toggleFilter(filters, "sizes", filter.value as Tier));
    else if (filter.axis === "favorite") onview?.({ favoritesOnly: false });
    else if (isShellAxis(filter.axis)) onunconstrain?.(filter.axis);
  }

  // A selection follows the result: what is no longer shown is no longer selected.
  $effect(() => {
    const shown = order;
    untrack(() => selection.prune(shown));
  });

  $effect(() => {
    const unbindKeys = bindAssetKeys(
      keymap,
      {
        lead: () => assets.find((a) => a.id === selection.lead) ?? null,
        selection: () => resolve(selection.targets()),
        project: () => project,
      },
      dispatch,
    );
    const unbindEscape = keymap.bind("Escape", () => selection.clear());
    return () => {
      unbindKeys();
      unbindEscape();
    };
  });
</script>

<section class="screen" aria-label="Library" data-screen="library">
  {#if !nudgeDismissed}
    <UnidentifiedBanner count={unidentified} {onidentify} ondismiss={ondismissnudge} />
  {/if}
  <LibraryToolbar
    label="{count} {count === 1 ? 'asset' : 'assets'} · {scope}"
    {view}
    {favoriteCount}
    filterCount={filterCount(filters)}
    {filtersOpen}
    {filtersId}
    {onview}
    onfilters={(anchor) => {
      filtersAnchor = anchor;
      filtersOpen = !filtersOpen;
    }}
  />
  <ActiveFilterBar filters={active} shown={count} {total} onremove={removeFilter} onclear={() => onclearfilters?.()} />
  <BulkBar targets={picked} {project} onintent={dispatch} onclear={() => selection.picks.clear()} />
  <div class="body" class:list={view.layout === "list" && assets.length > 0}>
    {#if assets.length === 0}
      <EmptyState
        icon="search"
        title="Nothing matches"
        line={unidentified > 0 ? `${unidentified} assets are still Unidentified and cannot be found this way.` : "Try a broader term or clear a filter."}
      >
        {#snippet action()}
          {#if active.length > 0}<Button onclick={() => onclearfilters?.()}>Clear all filters</Button>{/if}
          {#if unidentified > 0}<Button onclick={() => onidentify?.()}>Open Identify mode</Button>{/if}
        {/snippet}
      </EmptyState>
    {:else}
      <AssetGrid
        bind:this={grid}
        {assets}
        {selection}
        {project}
        {traits}
        {thresholds}
        size={view.size}
        layout={view.layout}
        anchored
        onintent={dispatch}
        onmenu={(asset: AssetSummary, at) => {
          menuId = asset.id;
          menuAt = at;
        }}
        {onpack}
        ontag={onsearchtag}
      />
    {/if}
  </div>
</section>

<AssetContextMenu bind:at={menuAt} targets={menuTargets} {project} onintent={dispatch} />

<QuickLook asset={peeked} {project} {traits} {playback} {peaks} onintent={dispatch} onstep={stepQuickLook} {onseek} onclose={() => onquicklook?.(null)} />

<Popover bind:open={filtersOpen} anchor={filtersAnchor} id={filtersId} label="Filter library" data-anchor="filters">
  <FilterPanel {filters} traits={traitOptions} {thresholds} anchored onchange={(next) => onfilterschange?.(next)} />
</Popover>

<style>
  /* Placement only. */
  .screen {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-width: 0;
    min-height: 0;
  }

  .body {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: var(--space-16);
  }

  /* The list sits a little closer to the toolbar than the grid does. */
  .list {
    padding: var(--space-12) var(--space-16);
  }
</style>
