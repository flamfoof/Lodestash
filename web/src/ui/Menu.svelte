<script lang="ts" module>
  import type { IconName } from "./icons.generated";

  export type MenuEntry =
    | {
        id: string;
        label: string;
        icon?: IconName;
        /** Shown right-aligned ("Space", "Del"). */
        shortcut?: string;
        disabled?: boolean;
        /** Why it is disabled — the item's tooltip (engine actions without a project). */
        hint?: string;
        danger?: boolean;
      }
    | { separator: true };
</script>

<script lang="ts">
  import { untrack } from "svelte";
  import { keymap } from "../app/keymap.svelte";
  import { overlayHost } from "../app/overlay";
  import { restoreFocus } from "./focus";
  import Icon from "./Icon.svelte";
  import { boxOf, place, viewportBox, type Box } from "./position";

  /**
   * A menu of actions (`role="menu"`, items `role="menuitem"`), floating in the overlay host.
   *
   * Placed by measurement: below its `anchor` (or at `point`, for a context menu), flipped above
   * when its measured height would cross the bottom of the viewport, clamped inside it — never a
   * hard-coded height. Focus moves to the first enabled item; ↑ ↓ Home End move, Enter and Space
   * activate, Escape and Tab close and give focus back to whatever opened it. A blocking keymap
   * scope is pushed while open, so no global shortcut fires behind it.
   */
  let {
    open = $bindable(false),
    items,
    label,
    anchor,
    point,
    align = "start",
    onselect,
    onclose,
  }: {
    open?: boolean;
    items: MenuEntry[];
    label: string;
    anchor?: HTMLElement | null;
    point?: { x: number; y: number } | null;
    align?: "start" | "end";
    onselect: (id: string) => void;
    onclose?: () => void;
  } = $props();

  let pos = $state<{ x: number; y: number; above: boolean } | null>(null);

  function close() {
    if (!open) return;
    open = false;
    onclose?.();
  }

  function enabledItems(menu: HTMLElement): HTMLElement[] {
    return [...menu.querySelectorAll<HTMLElement>('[role="menuitem"]:not([aria-disabled="true"])')];
  }

  function float(layer: HTMLElement) {
    const opener = document.activeElement;
    const { anchorEl, at, side } = untrack(() => ({ anchorEl: anchor, at: point, side: align }));
    overlayHost().appendChild(layer);
    const scope = keymap.push("menu");
    scope.bind("Escape", () => untrack(close), { inInputs: true });

    const reposition = () => {
      const ref: Box = anchorEl ? boxOf(anchorEl) : { x: at?.x ?? 0, y: at?.y ?? 0, width: 0, height: 0 };
      const r = layer.getBoundingClientRect();
      pos = place(ref, { width: r.width, height: r.height }, viewportBox(), { align: side, gap: anchorEl ? 4 : 0 });
    };
    reposition();
    window.addEventListener("resize", reposition);

    const onPointerDown = (e: PointerEvent) => {
      if (!layer.contains(e.target as Node) && !anchorEl?.contains(e.target as Node)) untrack(close);
    };
    document.addEventListener("pointerdown", onPointerDown, true);

    enabledItems(layer)[0]?.focus();

    return () => {
      scope.pop();
      window.removeEventListener("resize", reposition);
      document.removeEventListener("pointerdown", onPointerDown, true);
      const hadFocus = layer.contains(document.activeElement);
      layer.remove();
      pos = null;
      if (hadFocus || document.activeElement === document.body) restoreFocus(opener);
    };
  }

  function activate(entry: MenuEntry) {
    if ("separator" in entry || entry.disabled) return;
    close();
    onselect(entry.id);
  }

  function onkeydown(event: KeyboardEvent) {
    const menu = event.currentTarget as HTMLElement;
    const list = enabledItems(menu);
    const at = list.indexOf(document.activeElement as HTMLElement);
    let to = -1;
    if (event.key === "ArrowDown") to = (at + 1) % list.length;
    else if (event.key === "ArrowUp") to = at <= 0 ? list.length - 1 : at - 1;
    else if (event.key === "Home") to = 0;
    else if (event.key === "End") to = list.length - 1;
    else if (event.key === "Tab") {
      event.preventDefault();
      close();
      return;
    } else if (event.key === "Enter" || event.key === " ") {
      const id = (document.activeElement as HTMLElement | null)?.dataset.id;
      const entry = items.find((e) => !("separator" in e) && e.id === id);
      if (entry) {
        event.preventDefault();
        activate(entry);
      }
      return;
    }
    if (to >= 0 && list.length) {
      event.preventDefault();
      list[to].focus();
    }
  }
</script>

{#if open}
  <div
    class="menu"
    role="menu"
    aria-label={label}
    tabindex="-1"
    data-side={pos ? (pos.above ? "above" : "below") : undefined}
    style:left="{pos?.x ?? 0}px"
    style:top="{pos?.y ?? 0}px"
    style:visibility={pos ? "visible" : "hidden"}
    {onkeydown}
    {@attach float}
  >
    {#each items as entry, i ("separator" in entry ? `sep-${i}` : entry.id)}
      {#if "separator" in entry}
        <div class="sep" role="separator"></div>
      {:else}
        <button
          type="button"
          role="menuitem"
          class="item"
          class:danger={entry.danger}
          tabindex="-1"
          data-id={entry.id}
          aria-disabled={entry.disabled ? "true" : undefined}
          title={entry.disabled ? entry.hint : undefined}
          onclick={() => activate(entry)}
        >
          {#if entry.icon}<Icon name={entry.icon} size="14" />{/if}
          <span class="text">{entry.label}</span>
          {#if entry.shortcut}<kbd class="shortcut">{entry.shortcut}</kbd>{/if}
        </button>
      {/if}
    {/each}
  </div>
{/if}

<style>
  .menu {
    position: fixed;
    z-index: var(--z-popover);
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    min-width: max-content;
    max-height: 88vh;
    overflow-y: auto;
    padding: var(--space-6);
    background: var(--ls-surface-raised);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-10);
    box-shadow: var(--shadow-12-32-a55);
  }

  .menu:focus-visible {
    outline: none;
  }

  .item {
    display: flex;
    align-items: center;
    gap: var(--space-9);
    padding: var(--space-7) var(--space-10);
    border-radius: var(--r-7);
    font-size: var(--fs-12);
    color: var(--ls-text);
    white-space: nowrap;
  }

  .item:hover,
  .item:focus {
    background: var(--ls-overlay);
  }

  .item[aria-disabled="true"] {
    color: var(--ls-text-faint);
    cursor: default;
  }

  .item[aria-disabled="true"]:hover {
    background: none;
  }

  .danger {
    color: var(--ls-danger);
  }

  .text {
    flex: 1;
  }

  .shortcut {
    font-family: var(--ff-mono);
    font-size: var(--fs-10);
    color: var(--ls-text-dim);
  }

  .sep {
    height: 1px;
    margin: var(--space-6) var(--space-4);
    background: var(--ls-border);
  }
</style>
