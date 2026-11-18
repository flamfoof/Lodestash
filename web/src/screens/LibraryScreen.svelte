<script lang="ts">
  import { untrack } from "svelte";
  import { keymap } from "../app/keymap.svelte";
  import { bindAssetKeys, type AssetIntent } from "../features/asset/actions";
  import AssetContextMenu from "../features/asset/AssetContextMenu.svelte";
  import AssetGrid from "../features/asset/AssetGrid.svelte";
  import type { AssetSummary, PackageRef, ProjectRef, SizeThresholds, TraitDef } from "../features/asset/model";
  import LibraryToolbar from "../features/library/LibraryToolbar.svelte";
  import UnidentifiedBanner from "../features/library/UnidentifiedBanner.svelte";
  import type { LibraryView } from "../features/library/view";
  import type { Selection } from "../state/selection.svelte";
  import { Button, EmptyState } from "../ui";

  /**
   * The Library: the Unidentified nudge, the toolbar, and one page of the result as a grid
   * (UI-SPEC §2). A thin composition — it places features and wires them; every look is a
   * primitive's and nothing here fetches. Data comes in as props (the page, already filtered and
   * sorted by whoever owns the query); what the person asks for goes out as intents and view
   * changes. The one state it shares is the `selection`, which it also clears on Escape.
   *
   * Keys (UI-SPEC §0.4): Space, F and Delete are bound in the keymap's global scope for as long as
   * the screen is mounted, so they act on the selection from anywhere in the window and never behind
   * an open overlay. The grid's own keys are the grid's (AssetGrid).
   */
  let {
    assets,
    count = assets.length,
    scope = "all libraries",
    unidentified = 0,
    nudgeDismissed = false,
    view,
    filterCount = 0,
    favoriteCount = 0,
    selection,
    project,
    traits = [],
    thresholds,
    onintent,
    onview,
    onfilters,
    onidentify,
    ondismissnudge,
    onpack,
    onsearchtag,
  }: {
    /** The current page of the result. */
    assets: readonly AssetSummary[];
    /** Assets in the whole result (the page may be a part of it). */
    count?: number;
    /** What the result is drawn from: "all libraries", or a library's name. */
    scope?: string;
    /** Unidentified assets in the library — the nudge. */
    unidentified?: number;
    nudgeDismissed?: boolean;
    view: LibraryView;
    filterCount?: number;
    favoriteCount?: number;
    selection: Selection;
    project: ProjectRef | null;
    traits?: readonly TraitDef[];
    thresholds: SizeThresholds;
    onintent?: (intent: AssetIntent) => void;
    onview?: (patch: Partial<LibraryView>) => void;
    onfilters?: (anchor: HTMLElement) => void;
    onidentify?: () => void;
    ondismissnudge?: () => void;
    onpack?: (pack: PackageRef) => void;
    onsearchtag?: (value: string) => void;
  } = $props();

  // The context menu: which asset it was opened on (an id — never a copy of the asset) and where.
  let menuId = $state<number | null>(null);
  let menuAt = $state<{ event?: MouseEvent; anchor?: HTMLElement } | null>(null);

  const order = $derived(assets.map((a) => a.id));
  const resolve = (ids: readonly number[]) => assets.filter((a) => ids.includes(a.id));
  const menuTargets = $derived(menuId === null ? [] : resolve(selection.targets(menuId)));

  /** "Open" is the selection's business; everything else is the owner's. */
  function dispatch(intent: AssetIntent) {
    if (intent.action === "open") selection.moveTo(intent.ids[0]);
    else onintent?.(intent);
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
  <LibraryToolbar label="{count} {count === 1 ? 'asset' : 'assets'} · {scope}" {view} {favoriteCount} {filterCount} {onview} {onfilters} />
  <div class="body">
    {#if assets.length === 0}
      <EmptyState
        icon="search"
        title="Nothing matches"
        line={unidentified > 0 ? `${unidentified} assets are still Unidentified and cannot be found this way.` : "Try a broader term or clear a filter."}
      >
        {#snippet action()}
          {#if unidentified > 0}<Button onclick={() => onidentify?.()}>Open Identify mode</Button>{/if}
        {/snippet}
      </EmptyState>
    {:else if view.layout === "list"}
      <EmptyState icon="view-list" title="The list view is not built yet" line="Rows arrive with the list view; the grid shows the same result.">
        {#snippet action()}<Button onclick={() => onview?.({ layout: "grid" })}>Back to the grid</Button>{/snippet}
      </EmptyState>
    {:else}
      <AssetGrid
        {assets}
        {selection}
        {project}
        {traits}
        {thresholds}
        size={view.size}
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
</style>
