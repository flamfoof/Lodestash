<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLButtonAttributes } from "svelte/elements";
  import Icon from "./Icon.svelte";
  import type { IconName } from "./icons.generated";

  /**
   * A row in a hierarchy (packages by publisher, a pack's folders): indented by `level`, with a
   * disclosure chevron when it has children (`expanded` defined → `aria-expanded`), an icon, the
   * label, and a dim count. The chevron toggles (`ontoggle`); the row selects (`onclick`).
   */
  let {
    label,
    level = 0,
    expanded,
    icon,
    count,
    current = false,
    ontoggle,
    trailing,
    ...rest
  }: HTMLButtonAttributes & {
    label: string;
    level?: number;
    expanded?: boolean;
    icon?: IconName;
    count?: number | string;
    current?: boolean;
    ontoggle?: () => void;
    trailing?: Snippet;
  } = $props();

  function onkeydown(event: KeyboardEvent) {
    if (expanded === undefined || !ontoggle) return;
    if ((event.key === "ArrowRight" && !expanded) || (event.key === "ArrowLeft" && expanded)) {
      event.preventDefault();
      ontoggle();
    }
  }
</script>

<button
  type="button"
  class="tree-row"
  class:current
  style:--level={level}
  aria-expanded={expanded}
  aria-current={current ? "true" : undefined}
  {onkeydown}
  {...rest}
>
  <!-- The chevron is the pointer's toggle; → and ← on the row are the keyboard's. -->
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <span
    class="chevron"
    aria-hidden="true"
    onclick={(e) => {
      if (expanded === undefined) return;
      e.stopPropagation();
      ontoggle?.();
    }}
  >
    {#if expanded !== undefined}<Icon name={expanded ? "action-expand-open" : "action-expand-closed"} size="12" />{/if}
  </span>
  {#if icon}<Icon name={icon} size="16" />{/if}
  <span class="label">{label}</span>
  {#if count !== undefined}<span class="count">{count}</span>{/if}
  {@render trailing?.()}
</button>

<style>
  .tree-row {
    display: flex;
    align-items: center;
    gap: var(--space-8);
    width: 100%;
    padding: var(--space-5) var(--space-14) var(--space-5) calc(var(--space-14) + var(--level) * var(--space-14));
    border-left: var(--bw-2) solid transparent;
    font-size: var(--fs-12);
    color: var(--ls-text-muted);
    text-align: left;
  }

  .tree-row:hover {
    color: var(--ls-text);
    background: var(--ls-surface-raised);
  }

  .current {
    color: var(--ls-text);
    border-left-color: var(--ls-gold);
    background: var(--ls-surface-raised);
  }

  .chevron {
    display: flex;
    width: var(--space-12);
    flex: none;
    color: var(--ls-text-dim);
  }

  .label {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .count {
    font-family: var(--ff-mono);
    font-size: var(--fs-11);
    color: var(--ls-text-dim);
  }
</style>
