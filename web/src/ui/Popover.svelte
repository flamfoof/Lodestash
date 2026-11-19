<script lang="ts">
  import { untrack, type Snippet } from "svelte";
  import { keymap } from "../app/keymap.svelte";
  import { overlayHost } from "../app/overlay";
  import { focusables, restoreFocus } from "./focus";
  import { boxOf, place, viewportBox } from "./position";

  /**
   * A floating panel anchored to the control that opened it (the ⚑ Filters panel, the project
   * switcher): placed below it by measurement and flipped above when it would not fit, clamped to
   * the viewport. Non-modal — the page stays interactive — but it pushes a blocking keymap scope
   * while open, closes on Escape (focus back to the anchor), on a click outside, and when focus
   * leaves it. The anchor should carry `aria-expanded` and `aria-controls` = `id`. `data-*` attributes
   * reach the panel (the contract gate marks it).
   */
  let {
    open = $bindable(false),
    anchor,
    label,
    id,
    align = "end",
    flush = false,
    onclose,
    children,
    ...rest
  }: Record<`data-${string}`, string | undefined> & {
    open?: boolean;
    anchor: HTMLElement | null | undefined;
    label: string;
    id?: string;
    align?: "start" | "end" | "center";
    /** No inner padding (a list that draws its own). */
    flush?: boolean;
    onclose?: () => void;
    children: Snippet;
  } = $props();

  let pos = $state<{ x: number; y: number; above: boolean } | null>(null);

  function close() {
    if (!open) return;
    open = false;
    onclose?.();
  }

  function float(layer: HTMLElement) {
    const { anchorEl, side } = untrack(() => ({ anchorEl: anchor, side: align }));
    overlayHost().appendChild(layer);
    const scope = keymap.push("popover");
    scope.bind("Escape", () => untrack(close), { inInputs: true });

    const reposition = () => {
      if (!anchorEl) return;
      const r = layer.getBoundingClientRect();
      pos = place(boxOf(anchorEl), { width: r.width, height: r.height }, viewportBox(), { align: side });
    };
    reposition();
    // Content can change size while open (a filtered chip list): re-place when it does.
    const resize = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(reposition);
    resize?.observe(layer);
    window.addEventListener("resize", reposition);

    const onPointerDown = (e: PointerEvent) => {
      const t = e.target as Node;
      if (!layer.contains(t) && !anchorEl?.contains(t)) untrack(close);
    };
    const onFocusOut = (e: FocusEvent) => {
      const next = e.relatedTarget as Node | null;
      if (next && !layer.contains(next) && !anchorEl?.contains(next)) untrack(close);
    };
    document.addEventListener("pointerdown", onPointerDown, true);
    layer.addEventListener("focusout", onFocusOut);

    (focusables(layer)[0] ?? layer).focus();

    return () => {
      scope.pop();
      resize?.disconnect();
      window.removeEventListener("resize", reposition);
      document.removeEventListener("pointerdown", onPointerDown, true);
      const hadFocus = layer.contains(document.activeElement);
      layer.remove();
      pos = null;
      // Back to the control that opened it: the anchor, or the button inside an anchor that is a
      // wrapper (a span cannot take focus, and focus must not be left on the document).
      const opener = anchorEl ? (focusables(anchorEl)[0] ?? anchorEl) : null;
      if (hadFocus || document.activeElement === document.body) restoreFocus(opener);
    };
  }
</script>

{#if open}
  <div
    {id}
    class="popover"
    class:flush
    role="dialog"
    aria-label={label}
    tabindex="-1"
    data-side={pos ? (pos.above ? "above" : "below") : undefined}
    style:left="{pos?.x ?? 0}px"
    style:top="{pos?.y ?? 0}px"
    style:visibility={pos ? "visible" : "hidden"}
    {...rest}
    {@attach float}
  >
    {@render children()}
  </div>
{/if}

<style>
  .popover {
    position: fixed;
    z-index: var(--z-popover);
    max-height: 64vh;
    overflow-y: auto;
    padding: var(--space-13) var(--space-14);
    background: var(--ls-surface-raised);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-10);
    box-shadow: var(--shadow-12-32-a55);
    animation: rise var(--dur-150) ease;
  }

  .flush {
    padding: var(--space-6);
  }

  @keyframes rise {
    from {
      transform: translateY(var(--space-4));
      opacity: 0;
    }
  }
</style>
