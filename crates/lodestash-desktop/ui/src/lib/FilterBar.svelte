<script lang="ts">
  /**
   * Active-filter bar (UI-SPEC §2 item 7, DESIGN-GUIDELINES §7 "Active-filter bar", board D2).
   *
   * The one place that answers "why am I seeing these?". Eight filter axes live in four regions —
   * the search box, the type tabs, two sidebar facet groups, the ⚑ popover and the ★ toggle — and
   * before this only pack and collection were ever visible, with the reset hidden behind "click the
   * sidebar's Library row". Every constraint now renders as a removable chip in the order §2
   * composes them, with `n of N shown` and a Clear all that has a name and a place.
   *
   * Removing a chip drives the control it came from, never a parallel copy of the state: one source
   * of truth per axis. When nothing is filtered the bar does not render at all — an empty rail
   * would cost a card row on every screen to say nothing.
   */
  import { view } from "./nav.svelte";
  import { TABS } from "./routes";
  import { filterFacets, STATUSES, type Facets } from "./api";

  let { shown = 0, total = 0 }: { shown?: number; total?: number } = $props();

  /**
   * Labels for the ⚑ chips. The ids are opaque (they come from the Rust enums' serde names), so the
   * label has to come from the same `filter_facets` call the popover uses — spelling one out here
   * is how a chip comes to read as its own id.
   */
  let facets = $state<Facets | null>(null);
  $effect(() => {
    filterFacets()
      .then((f) => (facets = f))
      .catch(() => (facets = { traits: [], tri_buckets: [], sizes: [] }));
  });

  const facetLabel = (group: "tri_buckets" | "sizes", id: string) =>
    (facets?.[group] ?? []).find((o) => o.id === id)?.label ?? id;

  /**
   * Size chips keep their §11.4 tier colour — the same four the card's badge uses, keyed by the
   * serde names of `SizeTier` (`config.rs:239`). An id we don't know falls back to a neutral chip
   * rather than resolving to a `var()` that does not exist.
   */
  const TIER_TINT: Record<string, string> = {
    s: "var(--ls-success)",
    m: "var(--ls-info)",
    l: "var(--ls-gold)",
    xl: "var(--ls-danger)",
  };

  type Chip = {
    key: string;
    label: string;
    /** Mono axis prefix, for the axes with no colour of their own (library / pack / collection). */
    prefix?: string;
    tint?: string;
    /** Leading mark: the gem shape (type + status), the trait dash, or the query magnifier. */
    mark?: "gem" | "dash" | "query";
    remove: () => void;
  };

  let chips = $derived.by<Chip[]>(() => {
    const out: Chip[] = [];

    if (view.search.trim()) {
      out.push({
        key: "q",
        label: view.search.trim(),
        tint: "var(--ls-gold)",
        mark: "query",
        remove: () => view.setSearch(""),
      });
    }

    if (view.tab !== "all") {
      const tab = TABS.find((t) => t.id === view.tab);
      if (tab) {
        out.push({
          key: "tab",
          label: tab.label,
          tint: `var(${tab.token})`,
          mark: "gem",
          remove: () => (view.tab = "all"),
        });
      }
    }

    if (view.status !== "all") {
      const st = STATUSES.find((s) => s.id === view.status);
      if (st) {
        out.push({
          key: "status",
          label: st.label,
          tint: `var(--ls-status-${st.id})`,
          mark: "gem",
          remove: () => (view.status = "all"),
        });
      }
    }

    if (view.source) {
      out.push({
        key: "lib",
        label: view.source,
        prefix: "lib",
        remove: () => (view.source = null),
      });
    }

    if (view.packageFilter) {
      out.push({
        key: "pack",
        label: view.packageFilter,
        prefix: "pack",
        remove: () => (view.packageFilter = null),
      });
    }

    if (view.collectionFilter) {
      out.push({
        key: "coll",
        label: view.collectionFilter.name,
        prefix: "set",
        remove: () => (view.collectionFilter = null),
      });
    }

    for (const name of view.filter.traits) {
      out.push({
        key: `trait:${name}`,
        label: name,
        mark: "dash",
        remove: () => view.toggleFacet("traits", name),
      });
    }
    for (const id of view.filter.tri_buckets) {
      out.push({
        key: `tri:${id}`,
        label: `${facetLabel("tri_buckets", id)} tris`,
        tint: "var(--ls-violet)",
        remove: () => view.toggleFacet("tri_buckets", id),
      });
    }
    for (const id of view.filter.sizes) {
      out.push({
        key: `size:${id}`,
        label: facetLabel("sizes", id),
        tint: TIER_TINT[id],
        remove: () => view.toggleFacet("sizes", id),
      });
    }

    if (view.route === "favorites") {
      out.push({
        key: "fav",
        label: "★ Favorites",
        tint: "var(--ls-gold)",
        remove: () => view.go("library"),
      });
    }

    return out;
  });

  /**
   * §2 item 7 collapses the tail past one row into a popover. Measuring a wrapping flex row to find
   * where "one row" ends costs a ResizeObserver and a reflow per keystroke; a fixed head reaches the
   * same outcome for every filter set anyone actually builds.
   * ponytail: fixed head of 6, swap for real overflow measurement if a narrow window makes it wrap.
   */
  const HEAD = 6;
  let expanded = $state(false);
  let visible = $derived(expanded ? chips : chips.slice(0, HEAD));
  let hidden = $derived(Math.max(0, chips.length - HEAD));

  // A filter set that shrinks back under the cap must not leave the popover stuck open.
  $effect(() => {
    if (hidden === 0 && expanded) expanded = false;
  });
</script>

{#if chips.length > 0}
  <div class="bar" data-testid="filter-bar">
    <span class="kicker">Filtering by</span>

    {#each visible as chip (chip.key)}
      <span class="chip" class:tinted={!!chip.tint} style={chip.tint ? `--tint: ${chip.tint}` : ""}>
        {#if chip.mark === "query"}
          <svg class="mk" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true">
            <circle cx="7" cy="7" r="4.5" />
            <path d="M10.5 10.5 14 14" />
          </svg>
        {:else if chip.mark === "gem"}
          <span class="mk gem"></span>
        {:else if chip.mark === "dash"}
          <span class="mk dash"></span>
        {:else if chip.prefix}
          <span class="mk px">{chip.prefix}</span>
        {/if}
        <span class="txt">{chip.label}</span>
        <button class="x" onclick={chip.remove} aria-label="Remove filter {chip.label}" data-testid="chip-x-{chip.key}">✕</button>
      </span>
    {/each}

    {#if hidden > 0 && !expanded}
      <button class="more" onclick={() => (expanded = true)} data-testid="filter-more">+{hidden} more ▾</button>
    {/if}

    <span class="right">
      <span class="shown num" data-testid="filter-shown">
        {shown.toLocaleString()} of {total.toLocaleString()} shown
      </span>
      <button class="clear" onclick={() => view.goLibrary()} data-testid="filter-clear">Clear all</button>
    </span>
  </div>
{/if}

<style>
  .bar {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--sp-2);
    padding: var(--sp-2) var(--sp-5);
    background: var(--ls-bg);
    border-bottom: 1px solid var(--ls-border);
  }
  .kicker {
    font-size: var(--fs-xs);
    font-weight: 600;
    letter-spacing: 0.09em;
    text-transform: uppercase;
    color: var(--ls-text-dim);
    margin-right: var(--sp-1);
  }

  /* Neutral by default; a chip with an axis colour tints its text and border and keeps a 10%
     wash so it reads as "on" rather than as a label. Colour is never the only cue — every chip
     carries a mark or a mono prefix (DESIGN-GUIDELINES §9). */
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 22px;
    padding: 0 4px 0 8px;
    max-width: 260px;
    border-radius: var(--r-pill);
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
    background: var(--ls-overlay);
    border: 1px solid var(--ls-border-strong);
  }
  .chip.tinted {
    color: var(--tint);
    background: color-mix(in srgb, var(--tint) 10%, transparent);
    border-color: color-mix(in srgb, var(--tint) 45%, transparent);
  }
  .txt {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .mk {
    flex: none;
  }
  svg.mk {
    width: 10px;
    height: 10px;
  }
  .gem {
    width: 6px;
    height: 6px;
    background: currentColor;
    transform: rotate(45deg);
  }
  .dash {
    width: 10px;
    height: 2px;
    border-radius: var(--r-pill);
    background: var(--ls-text-muted);
  }
  .px {
    font-size: 9px;
    letter-spacing: 0.12em;
    text-transform: uppercase;
    color: var(--ls-text-dim);
  }

  .x {
    flex: none;
    display: grid;
    place-items: center;
    width: 14px;
    height: 14px;
    border-radius: var(--r-pill);
    font-size: 10px;
    line-height: 1;
    color: var(--ls-text-dim);
    background: none;
    border: 0;
    cursor: pointer;
  }
  .x:hover {
    color: var(--ls-text);
    background: color-mix(in srgb, var(--ls-text) 12%, transparent);
  }

  .more {
    height: 22px;
    padding: 0 8px;
    border-radius: var(--r-pill);
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
    background: none;
    border: 1px dashed var(--ls-border-strong);
    cursor: pointer;
  }
  .more:hover {
    color: var(--ls-text);
    border-color: var(--ls-gold);
  }

  .right {
    margin-left: auto;
    display: inline-flex;
    align-items: center;
    gap: var(--sp-2);
  }
  .shown {
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
  }
  .clear {
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
    background: none;
    border: 0;
    border-bottom: 1px dotted var(--ls-border-strong);
    padding: 0;
    cursor: pointer;
  }
  .clear:hover {
    color: var(--ls-gold);
    border-bottom-color: var(--ls-gold);
  }

  .chip:focus-within,
  .more:focus-visible,
  .clear:focus-visible {
    box-shadow: var(--el-focus-ring);
    outline: none;
  }
</style>
