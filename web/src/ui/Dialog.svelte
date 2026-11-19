<script lang="ts">
  import { untrack, type Snippet } from "svelte";
  import { keymap, type KeyHandler } from "../app/keymap.svelte";
  import { overlayHost, pushModal } from "../app/overlay";
  import { focusables, restoreFocus, trapTab } from "./focus";
  import IconButton from "./IconButton.svelte";

  /**
   * The one modal (spec §9.2 rule 4). While open it:
   * - renders into the overlay host over a dimming, blurring scrim;
   * - makes the app root (and any dialog beneath it) `inert`;
   * - moves focus in — to the element marked `data-autofocus`, else the first focusable control
   *   after the title bar, else the panel — and keeps Tab and Shift+Tab cycling inside;
   * - pushes a blocking keymap scope, so Escape closes this dialog only and no global shortcut
   *   fires behind it;
   * - on close, returns focus to whatever had it when the dialog opened.
   * `dismissable={false}` keeps Escape and the scrim from closing it (a step that must be answered).
   *
   * `bare` is the dialog as a frame only — Quick Look: no title bar and no padding, the content
   * laid out edge to edge (`wide`: 780 px) and clipped to the rounded corners. The content brings
   * its own heading and close button; `title` is then the dialog's accessible name, and focus
   * starts on the panel itself unless something is marked `data-autofocus`.
   *
   * `keys` are bound in the dialog's own keymap scope for as long as it is open (Quick Look's
   * Space and ← →): a handler that returns `false` leaves the key to whatever has focus.
   * `scrimAttrs` and `panelAttrs` reach the scrim and the panel (the contract gate marks them).
   */
  let {
    open = $bindable(false),
    title,
    description,
    size = "sm",
    bare = false,
    dismissable = true,
    keys = {},
    scrimAttrs = {},
    panelAttrs = {},
    onclose,
    children,
    footer,
  }: {
    open?: boolean;
    title: string;
    description?: string;
    size?: "sm" | "md" | "wide";
    bare?: boolean;
    dismissable?: boolean;
    keys?: Record<string, KeyHandler>;
    scrimAttrs?: Record<`data-${string}`, string | undefined>;
    panelAttrs?: Record<`data-${string}`, string | undefined>;
    onclose?: () => void;
    children?: Snippet;
    footer?: Snippet;
  } = $props();

  const uid = $props.id();

  function close() {
    if (!open) return;
    open = false;
    onclose?.();
  }

  function modal(layer: HTMLElement) {
    const opener = document.activeElement;
    overlayHost().appendChild(layer);
    const release = pushModal(layer);
    const scope = keymap.push("dialog");
    scope.bind("Escape", () => untrack(() => (dismissable ? close() : undefined)), { inInputs: true });
    // The combos are fixed when the dialog opens; each press runs the handler given at that moment.
    for (const combo of untrack(() => Object.keys(keys))) scope.bind(combo, (event) => untrack(() => keys[combo]?.(event)));

    const panel = layer.querySelector<HTMLElement>('[role="dialog"]')!;
    // A bare dialog has no body of its own: its content is the caller's, and focus starts on the panel.
    const body = untrack(() => bare) ? null : panel.querySelector<HTMLElement>(":scope > .body");
    const target =
      panel.querySelector<HTMLElement>("[data-autofocus]") ??
      (body ? focusables(body)[0] : undefined) ??
      panel;
    target.focus();

    return () => {
      scope.pop();
      release();
      layer.remove();
      restoreFocus(opener);
    };
  }
</script>

{#if open}
  <div class="layer" {@attach modal}>
    <!-- The scrim is a pointer target only; Escape is its keyboard equivalent. -->
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="scrim" onclick={() => dismissable && close()} {...scrimAttrs}></div>
    <div
      class="panel {size}"
      class:bare
      role="dialog"
      aria-modal="true"
      aria-label={bare ? title : undefined}
      aria-labelledby={bare ? undefined : `${uid}-title`}
      aria-describedby={description && !bare ? `${uid}-desc` : undefined}
      tabindex="-1"
      onkeydown={(e) => trapTab(e, e.currentTarget)}
      {...panelAttrs}
    >
      {#if bare}
        {@render children?.()}
      {:else}
        <header class="head">
          <h2 id="{uid}-title" class="title">{title}</h2>
          {#if dismissable}<IconButton icon="action-clear" label="Close" onclick={close} />{/if}
        </header>
        <div class="body">
          {#if description}<p id="{uid}-desc" class="desc">{description}</p>{/if}
          {@render children?.()}
          {#if footer}<footer class="foot">{@render footer()}</footer>{/if}
        </div>
      {/if}
    </div>
  </div>
{/if}

<style>
  .layer {
    position: fixed;
    inset: 0;
    z-index: var(--z-dialog);
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .scrim {
    position: absolute;
    inset: 0;
    background: var(--ls-backdrop-a74);
    backdrop-filter: blur(var(--space-4));
  }

  .panel {
    position: relative;
    max-width: 92vw;
    max-height: 88vh;
    overflow-y: auto;
    padding: var(--space-20);
    background: var(--ls-surface-raised);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-12);
    box-shadow: var(--shadow-8-32-a60);
    animation: rise var(--dur-180) ease;
    user-select: text;
  }

  /* Focus on the panel itself means "in this dialog": the panel is the indicator, and it keeps its
     shadow instead of trading it for the ring a focused control draws. */
  .panel:focus-visible {
    box-shadow: var(--shadow-8-32-a60) !important;
  }

  .sm {
    width: var(--dialog-w-sm);
  }

  .md {
    width: var(--dialog-w-md);
    padding: var(--space-22);
  }

  /* The width is the content's: the hairline is outside it, as the prototype's panel measures. */
  .wide {
    box-sizing: content-box;
    width: var(--dialog-w-wide);
    max-width: 94vw;
  }

  .bare {
    display: flex;
    padding: 0;
    overflow: hidden;
  }

  .head {
    display: flex;
    align-items: center;
    gap: var(--space-10);
    margin-bottom: var(--space-14);
  }

  .title {
    margin: 0 auto 0 0;
    font-family: var(--ff-display);
    font-size: var(--fs-17);
    font-weight: var(--fw-700);
    letter-spacing: var(--track-0_5);
    color: var(--ls-text);
  }

  .desc {
    margin: 0 0 var(--space-14);
    font-size: var(--fs-12);
    color: var(--ls-text-muted);
  }

  .foot {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-8);
    margin-top: var(--space-18);
  }

  @keyframes rise {
    from {
      transform: translateY(var(--space-6));
      opacity: 0;
    }
  }
</style>
