<script lang="ts">
  import { Badge, Button, Icon, Menu, SegmentedControl, Select, Text, ToggleButton, Toolbar, type MenuEntry } from "../../ui";
  import { SORT_OPTIONS, type GridSize, type Layout, type LibraryView, type SortKey } from "./view";

  /**
   * The strip above the grid (FEATURES §1.3, DESIGN-GUIDELINES §7 "View / sort toolbar"): the
   * result label, a keyboard hint, sort, ★ Favorites, ⚑ Filters, grid size and view.
   *
   * The responsive ladder (ADR-0009) applies here: below a 1400 px window the hint hides; below
   * 960 px sort and grid size fold into one ⋯ menu. ★, ⚑ and the view toggle never fold.
   * Layout only — and it changes nothing itself: every control reports the change (`onview`).
   */
  let {
    label,
    view,
    favoriteCount = 0,
    filterCount = 0,
    filtersOpen = false,
    sortDisabled = false,
    onview,
    onfilters,
  }: {
    /** "19 assets · all libraries". */
    label: string;
    view: LibraryView;
    favoriteCount?: number;
    /** Active ⚑ filters. */
    filterCount?: number;
    filtersOpen?: boolean;
    /** AI-ranked results have their own order (UI-SPEC §2.2). */
    sortDisabled?: boolean;
    onview?: (patch: Partial<LibraryView>) => void;
    /** Open the ⚑ Filters popover at this element. */
    onfilters?: (anchor: HTMLElement) => void;
  } = $props();

  const SIZES: { value: GridSize; title: string }[] = [
    { value: "compact", title: "Compact" },
    { value: "cozy", title: "Cozy" },
    { value: "large", title: "Large" },
  ];

  let filtersAnchor = $state<HTMLElement>();
  let moreAnchor = $state<HTMLElement>();
  let moreOpen = $state(false);

  // The ⋯ menu repeats sort and grid size; the current choice of each is checked.
  const check = (on: boolean, text: string) => (on ? `✓ ${text}` : text);
  const more = $derived<MenuEntry[]>([
    ...SORT_OPTIONS.map((o) => ({ id: `sort:${o.value}`, label: check(o.value === view.sort, o.label), disabled: sortDisabled })),
    ...(view.layout === "grid"
      ? [{ separator: true as const }, ...SIZES.map((s) => ({ id: `size:${s.value}`, label: check(s.value === view.size, `grid · ${s.title.toLowerCase()}`) }))]
      : []),
  ]);

  function pickMore(id: string) {
    const [kind, value] = id.split(":");
    if (kind === "sort") onview?.({ sort: value as SortKey });
    else onview?.({ size: value as GridSize });
  }
</script>

<Toolbar label="Library view" data-anchor="library.toolbar">
  <Text data-anchor="library.toolbar.count">{label}</Text>
  <span class="grow"></span>
  <span class="hint">
    <Text variant="hint" data-anchor="library.hint">
      Space quick-looks · <Icon name="action-fullsize" size="11" box="10" /> full size · <Icon name="action-stage" size="11" box="10" /> stage
    </Text>
  </span>
  <span class="fold">
    <Select
      label="Sort"
      value={view.sort}
      options={[...SORT_OPTIONS]}
      disabled={sortDisabled}
      onchange={(sort) => onview?.({ sort })}
      data-anchor="library.sort"
    />
  </span>
  <ToggleButton
    pressed={view.favoritesOnly}
    onchange={(favoritesOnly) => onview?.({ favoritesOnly })}
    icon="nav-favorites"
    iconBox="13"
    title="Show only favorites"
    data-anchor="library.favorites"
  >
    Favorites
    {#snippet trailing()}
      {#if favoriteCount > 0}<Badge tone="gold" label="{favoriteCount} favorites">{favoriteCount}</Badge>{/if}
    {/snippet}
  </ToggleButton>
  <span class="anchor" bind:this={filtersAnchor}>
    <Button
      icon="action-filters"
      iconBox="9"
      active={filterCount > 0 || filtersOpen}
      title="Filter by trait, triangles, or file size"
      aria-expanded={filtersOpen}
      aria-haspopup="dialog"
      onclick={() => filtersAnchor && onfilters?.(filtersAnchor)}
      data-anchor="library.filters"
    >
      Filters
      {#snippet trailing()}
        {#if filterCount > 0}<Badge tone="gold" label="{filterCount} active filters">{filterCount}</Badge>{/if}
      {/snippet}
    </Button>
  </span>
  {#if view.layout === "grid"}
    <span class="fold">
      <SegmentedControl
        label="Grid size"
        size="sm"
        value={view.size}
        onchange={(size) => onview?.({ size })}
        data-anchor="library.density"
        anchors={{ on: "library.density.on" }}
        options={[
          { value: "compact", title: "Compact", icon: "grid-compact", iconSize: "10", iconBox: "7" },
          { value: "cozy", title: "Cozy", icon: "grid-cozy", iconSize: "10", iconBox: "10" },
          { value: "large", title: "Large", icon: "grid-large", iconSize: "10", iconBox: "10" },
        ]}
      />
    </span>
  {/if}
  <span class="more" bind:this={moreAnchor}>
    <Button aria-label="Sort and grid size" title="Sort and grid size" aria-haspopup="menu" aria-expanded={moreOpen} onclick={() => (moreOpen = true)}>⋯</Button>
  </span>
  <SegmentedControl
    label="View"
    size="sm-wide"
    value={view.layout}
    onchange={(layout: Layout) => onview?.({ layout })}
    data-anchor="library.view"
    anchors={{ on: "library.view.on" }}
    options={[
      { value: "grid", title: "Grid view", icon: "view-grid", iconBox: "12" },
      { value: "list", title: "List view", icon: "view-list", iconBox: "11" },
    ]}
  />
</Toolbar>
<Menu bind:open={moreOpen} anchor={moreAnchor} align="end" label="Sort and grid size" items={more} onselect={pickMore} />

<style>
  /* Layout only: what grows, and what the responsive ladder folds (ADR-0009). */
  .grow {
    flex: 1;
  }

  .hint,
  .fold,
  .anchor,
  .more {
    display: flex;
    flex-shrink: 0;
  }

  .more {
    display: none;
  }

  @media (max-width: 1399.98px) {
    .hint {
      display: none;
    }
  }

  @media (max-width: 959.98px) {
    .fold {
      display: none;
    }

    .more {
      display: flex;
    }
  }
</style>
