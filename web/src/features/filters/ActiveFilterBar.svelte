<script lang="ts">
  import { Chip, FilterToken, Popover, Text, TextLink, Toolbar, type IconName, type TokenTone } from "../../ui";
  import type { ActiveFilter } from "./active";

  /**
   * The active-filter bar (UI-SPEC §2 item 7, DESIGN-GUIDELINES §7): one strip under the toolbar
   * with a removable chip for **every** constraint on the result, `n of N shown`, and Clear all.
   * It answers "why am I seeing these?" — the filters live in the search box, the type tabs, two
   * sidebar groups, the ⚑ popover and the ★ toggle, and this is the one place that lists them.
   *
   * It owns none of them: a ✕ reports which entry to remove (`onremove`) and the owner drives the
   * control that entry came from. With nothing active the bar is not rendered at all.
   *
   * One row, never two: chips that do not fit fold into `+N more ▾`, which opens the rest in a
   * popover, so a long filter list cannot push the grid down. How many fit is counted from where
   * the chips actually wrap, once per resize or change of the list.
   */
  let {
    filters,
    shown,
    total,
    onremove,
    onclear,
  }: {
    filters: readonly ActiveFilter[];
    /** Assets in the result. */
    shown: number;
    /** Assets with no constraint at all. */
    total: number;
    onremove?: (filter: ActiveFilter) => void;
    /** Reset every constraint — the same reset the sidebar's Library row performs. */
    onclear?: () => void;
  } = $props();

  let row = $state<HTMLElement>();
  /** How many chips fit on the row; the rest are in the "more" popover. */
  let fit = $state(Infinity);
  let moreAnchor = $state<HTMLElement>();
  let moreOpen = $state(false);

  const folded = $derived(filters.slice(fit));

  $effect(() => {
    void filters; // a changed list wraps differently
    const el = row;
    if (!el || typeof ResizeObserver === "undefined") return;
    const count = () => {
      const chips = [...el.children] as HTMLElement[];
      const top = chips[0]?.offsetTop ?? 0;
      const wrapped = chips.findIndex((chip) => chip.offsetTop > top);
      fit = wrapped < 0 ? Infinity : wrapped;
    };
    const observer = new ResizeObserver(count);
    observer.observe(el);
    count();
    return () => observer.disconnect();
  });

  $effect(() => {
    if (folded.length === 0) moreOpen = false;
  });

  /** Colour says which axis; the icon, diamond or prefix says it again (never colour alone). */
  function look(filter: ActiveFilter): { tone: TokenTone; icon?: IconName; diamond?: boolean } {
    switch (filter.axis) {
      case "query":
        return { tone: "gold", icon: "search" };
      case "type":
        return { tone: filter.value as TokenTone, diamond: true };
      case "status":
        return { tone: filter.value as TokenTone, icon: `status-${filter.value}` as IconName };
      case "tris":
        return { tone: "violet" };
      case "size":
        return { tone: filter.value as TokenTone };
      case "favorite":
        return { tone: "gold", icon: "nav-favorites" };
      default:
        return { tone: "neutral" };
    }
  }
</script>

{#snippet token(filter: ActiveFilter, inert: boolean)}
  <FilterToken label={filter.label} prefix={filter.prefix} {...look(filter)} {inert} data-axis={filter.axis} onremove={() => onremove?.(filter)} />
{/snippet}

{#if filters.length > 0}
  <Toolbar label="Active filters" data-active-filters>
    <div class="row" bind:this={row}>
      {#each filters as filter, i (filter.key)}
        {@render token(filter, i >= fit)}
      {/each}
    </div>
    {#if folded.length > 0}
      <span class="more" bind:this={moreAnchor}>
        <Chip
          tone="neutral"
          size="md"
          label="+{folded.length} more ▾"
          aria-haspopup="dialog"
          aria-expanded={moreOpen}
          aria-label="{folded.length} more active filters"
          onclick={() => (moreOpen = !moreOpen)}
        />
      </span>
    {/if}
    <span class="tail">
      <Text variant="hint">{shown} of {total} shown</Text>
      <TextLink tone="muted" hover="danger" size="11" onclick={() => onclear?.()}>Clear all</TextLink>
    </span>
  </Toolbar>
  <Popover bind:open={moreOpen} anchor={moreAnchor} label="More active filters" align="start">
    <div class="list">
      {#each folded as filter (filter.key)}
        {@render token(filter, false)}
      {/each}
    </div>
  </Popover>
{/if}

<style>
  /* Layout only. The row is one chip tall and clips what wraps below it. */
  .row {
    display: flex;
    flex: 1;
    flex-wrap: wrap;
    gap: var(--space-6);
    min-width: 0;
    height: var(--space-22);
    overflow: hidden;
  }

  .more,
  .tail {
    display: flex;
    align-items: center;
    flex-shrink: 0;
  }

  .tail {
    gap: var(--space-10);
  }

  .list {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: var(--space-6);
  }
</style>
