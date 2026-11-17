<script lang="ts">
  import { untrack, type Snippet } from "svelte";
  import { keymap } from "../app/keymap.svelte";
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
   */
  let {
    open = $bindable(false),
    title,
    description,
    size = "sm",
    dismissable = true,
    onclose,
    children,
    footer,
  }: {
    open?: boolean;
    title: string;
    description?: string;
    size?: "sm" | "md";
    dismissable?: boolean;
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

    const panel = layer.querySelector<HTMLElement>('[role="dialog"]')!;
    const body = panel.querySelector<HTMLElement>(".body");
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
    <div class="scrim" onclick={() => dismissable && close()}></div>
    <div
      class="panel {size}"
      role="dialog"
      aria-modal="true"
      aria-labelledby="{uid}-title"
      aria-describedby={description ? `${uid}-desc` : undefined}
      tabindex="-1"
      onkeydown={(e) => trapTab(e, e.currentTarget)}
    >
      <header class="head">
        <h2 id="{uid}-title" class="title">{title}</h2>
        {#if dismissable}<IconButton icon="action-clear" label="Close" onclick={close} />{/if}
      </header>
      <div class="body">
        {#if description}<p id="{uid}-desc" class="desc">{description}</p>{/if}
        {@render children?.()}
        {#if footer}<footer class="foot">{@render footer()}</footer>{/if}
      </div>
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

  .sm {
    width: var(--dialog-w-sm);
  }

  .md {
    width: var(--dialog-w-md);
    padding: var(--space-22);
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
