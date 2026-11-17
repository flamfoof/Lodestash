<script lang="ts" module>
  import type { IconBox, IconSize } from "./Icon.svelte";
  import type { IconName } from "./icons.generated";

  export interface Tab<V extends string = string> {
    value: V;
    label: string;
    icon?: IconName;
    iconSize?: IconSize;
    iconBox?: IconBox;
    /** Accent when selected: the category's own colour. */
    tone?: "gold" | "model" | "material" | "sound" | "image" | "arcane";
  }
</script>

<script lang="ts" generics="V extends string">
  import type { HTMLAttributes } from "svelte/elements";
  import Icon from "./Icon.svelte";

  /**
   * A row of tabs (`role="tablist"`, the selected one `aria-selected`), one tab stop with arrow
   * keys, Home and End moving between them. Drawn as the top bar's bordered category tabs: the
   * selected tab takes its category colour for text and border.
   * `controls` is the id of the region the tabs switch, when there is one.
   * `anchors` marks the selected and unselected tabs for the contract gate.
   */
  let {
    tabs,
    value = $bindable(),
    onchange,
    label,
    controls,
    anchors = {},
    collapse,
    ...rest
  }: Omit<HTMLAttributes<HTMLDivElement>, "onchange"> & {
    tabs: Tab<V>[];
    value: V;
    onchange?: (value: V) => void;
    label: string;
    controls?: string;
    anchors?: { on?: string; off?: string };
    /** In the app bar, show only the icons below this rung (AppBar); labels stay the tabs' names. */
    collapse?: "xl" | "lg" | "md";
  } = $props();

  let list: HTMLDivElement;

  function pick(v: V) {
    if (v !== value) {
      value = v;
      onchange?.(v);
    }
  }

  function onkeydown(event: KeyboardEvent) {
    const at = tabs.findIndex((t) => t.value === value);
    const to =
      event.key === "ArrowRight" ? (at + 1) % tabs.length
      : event.key === "ArrowLeft" ? (at - 1 + tabs.length) % tabs.length
      : event.key === "Home" ? 0
      : event.key === "End" ? tabs.length - 1
      : -1;
    if (to < 0) return;
    event.preventDefault();
    pick(tabs[to].value);
    list.querySelectorAll<HTMLButtonElement>('[role="tab"]')[to]?.focus();
  }
</script>

<div class="tabs {collapse ? `collapse-${collapse}` : ''}" role="tablist" aria-label={label} bind:this={list} {onkeydown} {...rest}>
  {#each tabs as tab (tab.value)}
    {@const on = tab.value === value}
    <button
      type="button"
      role="tab"
      class="tab {tab.tone ?? 'gold'}"
      class:on
      aria-selected={on}
      aria-controls={controls}
      tabindex={on ? 0 : -1}
      title={collapse ? tab.label : undefined}
      data-anchor={on ? anchors.on : anchors.off}
      onclick={() => pick(tab.value)}
    >
      <span class="label">
        {#if tab.icon}<Icon name={tab.icon} size={tab.iconSize ?? "12"} box={tab.iconBox} />{/if}<span class="text">{tab.icon ? " " : ""}{tab.label}</span>
      </span>
    </button>
  {/each}
</div>

<style>
  .tabs {
    display: inline-flex;
    flex-shrink: 0;
    gap: var(--space-5);
  }

  .tab {
    display: flex;
    align-items: center;
    gap: var(--space-5);
    padding: var(--space-5) var(--space-10);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-6);
    font-size: var(--fs-12);
    color: var(--ls-text-muted);
    white-space: nowrap;
    transition: border-color var(--dur-120) ease;
  }

  .tab:hover:not(.on) {
    border-color: var(--ls-text-dim);
  }

  @container appbar (width < 1200px) {
    .collapse-lg .text {
      position: absolute;
      width: 1px;
      height: 1px;
      overflow: hidden;
      clip-path: inset(50%);
      white-space: nowrap;
    }
  }

  .on.gold {
    color: var(--ls-gold);
    border-color: var(--ls-gold);
  }

  .on.model {
    color: var(--ls-type-model);
    border-color: var(--ls-type-model);
  }

  .on.material {
    color: var(--ls-type-material);
    border-color: var(--ls-type-material);
  }

  .on.sound {
    color: var(--ls-type-sound);
    border-color: var(--ls-type-sound);
  }

  .on.image {
    color: var(--ls-type-image);
    border-color: var(--ls-type-image);
  }

  .on.arcane {
    color: var(--ls-arcane);
    border-color: var(--ls-arcane);
  }
</style>
