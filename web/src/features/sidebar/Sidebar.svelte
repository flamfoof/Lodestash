<script lang="ts">
  import { NAV, type BadgeCounts, type NavId, type StatusFacet } from "../../app/shell";
  import { Badge, Button, Divider, IconButton, ListRow, Popover, Rail, RailHeading } from "../../ui";

  /**
   * The left rail (FEATURES §0.3): the eleven destinations, then the STATUS and LIBRARIES facet
   * groups and "+ Add source…". Facet groups show on every screen (ADR-0009 narrows the old
   * Library-only rule). Below a 1120 px window the rail is icons only and a toggle at its top opens
   * the full list — labels, counts and both facet groups — as a flyout. Layout only; looks are the
   * primitives'.
   */
  let {
    route,
    badges,
    status,
    libraries,
    onnavigate,
    onstatus,
    onlibrary,
    onaddsource,
  }: {
    route: NavId;
    badges: BadgeCounts;
    status: { value: StatusFacet; counts: Record<StatusFacet, number> };
    libraries: { value: string; options: { id: string; label: string; count: number }[] };
    onnavigate?: (id: NavId) => void;
    onstatus?: (value: StatusFacet) => void;
    onlibrary?: (id: string) => void;
    onaddsource?: () => void;
  } = $props();

  const STATUS_ROWS: { value: StatusFacet; label: string; dot: string }[] = [
    { value: "all", label: "All", dot: "var(--ls-text-dim)" },
    { value: "unidentified", label: "Unidentified", dot: "var(--ls-status-unidentified)" },
    { value: "identified", label: "Identified", dot: "var(--ls-status-identified)" },
    { value: "confirmed", label: "Confirmed", dot: "var(--ls-status-confirmed)" },
  ];

  const uid = $props.id();
  let toggle = $state<HTMLElement>();
  let flyout = $state(false);
  const badgeOf = (id: NavId) => (id === "identify" || id === "duplicates" || id === "trash" ? (badges[id] ?? 0) : 0);
</script>

{#snippet list(anchored: boolean)}
  <nav class="group" aria-label="Main">
    {#each NAV as item (item.id)}
      {@const current = item.id === route}
      {@const count = badgeOf(item.id)}
      <ListRow
        label={item.label}
        icon={item.icon}
        {current}
        onclick={() => onnavigate?.(item.id)}
        data-anchor={anchored ? (current ? "sidebar.nav.on" : "sidebar.nav") : undefined}
      >
        {#snippet trailing()}
          {#if count > 0}
            <Badge label="{count} {item.label.toLowerCase()}" data-anchor={anchored ? "sidebar.nav.badge" : undefined}>{count}</Badge>
          {/if}
        {/snippet}
      </ListRow>
    {/each}
  </nav>
  <div class="facets group">
    <Divider />
    <section class="group" aria-labelledby="{uid}-{anchored}-status">
      <RailHeading id="{uid}-{anchored}-status" text="STATUS" data-anchor={anchored ? "sidebar.heading" : undefined} />
      {#each STATUS_ROWS as row (row.value)}
        {@const current = row.value === status.value}
        <ListRow
          variant="facet"
          label={row.label}
          dot={row.dot}
          count={status.counts[row.value]}
          {current}
          onclick={() => onstatus?.(row.value)}
          data-anchor={anchored && !current ? "sidebar.facet" : undefined}
        />
      {/each}
    </section>
    <Divider />
    <section class="group" aria-labelledby="{uid}-{anchored}-libraries">
      <RailHeading id="{uid}-{anchored}-libraries" text="LIBRARIES" />
      {#each libraries.options as lib (lib.id)}
        <ListRow
          variant="facet"
          label={lib.label}
          count={lib.count}
          current={lib.id === libraries.value}
          onclick={() => onlibrary?.(lib.id)}
        />
      {/each}
    </section>
    <div class="add">
      <Button variant="dashed" onclick={() => onaddsource?.()} data-anchor={anchored ? "sidebar.addSource" : undefined}>
        {#snippet leading()}<span aria-hidden="true">+</span>{/snippet}
        Add source…
      </Button>
    </div>
  </div>
{/snippet}

<Rail aria-label="Sidebar" data-anchor="sidebar">
  <span class="toggle" bind:this={toggle}>
    <IconButton
      icon="view-list"
      iconSize="16"
      label="Show labels and filters"
      aria-expanded={flyout}
      aria-controls="{uid}-flyout"
      onclick={() => (flyout = !flyout)}
    />
  </span>
  {@render list(true)}
</Rail>

<Popover bind:open={flyout} anchor={toggle} id="{uid}-flyout" label="Sidebar" align="start" flush>
  <div class="flyout">{@render list(false)}</div>
</Popover>

<style>
  .group {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }

  section {
    margin: 0;
  }

  .add {
    padding: var(--space-4) var(--space-12) 0;
  }

  .toggle {
    display: none;
    justify-content: center;
    padding-bottom: var(--space-6);
  }

  .flyout {
    width: var(--sidebar-w);
  }

  /* The 48 px icon rail (ADR-0009): the facet groups live in the flyout. */
  @container rail (width < 100px) {
    .toggle {
      display: flex;
    }

    .facets {
      display: none;
    }
  }
</style>
