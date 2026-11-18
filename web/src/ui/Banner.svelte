<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLAttributes } from "svelte/elements";
  import Icon from "./Icon.svelte";

  /**
   * A full-width strip across the top of a screen: the library's "N assets are Unidentified" nudge.
   * A tinted band with a hairline under it, a leading mark, the message, and at most one action
   * (a text button in the band's colour). `actionAttrs` reaches the action button. `figure` is a
   * count that opens the message, in mono bold ("**5** assets are Unidentified…"). With `ondismiss`
   * a small ✕ closes the band (the nudge is dismissed for the session — UI-SPEC §2.1); it is no
   * taller than the text, so the band keeps its height.
   */
  let {
    tone = "warning",
    mark = "?",
    figure,
    action,
    actionAttrs = {},
    ondismiss,
    children,
    ...rest
  }: HTMLAttributes<HTMLDivElement> & {
    tone?: "warning" | "info" | "danger";
    /** A one-character mark before the message. */
    mark?: string;
    figure?: string | number;
    action?: { label: string; onclick: () => void };
    actionAttrs?: Record<string, string | undefined>;
    ondismiss?: () => void;
    children: Snippet;
  } = $props();
</script>

<div class="banner {tone}" role="status" {...rest}>
  <span class="mark" aria-hidden="true">{mark}</span>
  <span class="msg">{#if figure !== undefined}<b class="figure">{figure}</b>{" "}{/if}{@render children()}</span>
  {#if action}
    <button type="button" class="action" onclick={action.onclick} {...actionAttrs}>{action.label}</button>
  {/if}
  {#if ondismiss}
    <button type="button" class="dismiss" aria-label="Dismiss" title="Dismiss" onclick={ondismiss}><Icon name="action-clear" size="10" /></button>
  {/if}
</div>

<style>
  .banner {
    display: flex;
    align-items: center;
    gap: var(--space-10);
    flex-shrink: 0;
    padding: var(--space-7) var(--space-16);
    font-size: var(--fs-12);
  }

  .warning {
    --banner-accent: var(--ls-warning);
    background: var(--ls-warning-a7);
    border-bottom: 1px solid var(--ls-warning-a30);
  }

  .info {
    --banner-accent: var(--ls-arcane);
    background: var(--ls-arcane-a14);
    border-bottom: 1px solid var(--ls-arcane-a30);
  }

  .danger {
    --banner-accent: var(--ls-danger);
    background: var(--ls-danger-a18);
    border-bottom: 1px solid var(--ls-danger-a35);
  }

  .mark {
    color: var(--banner-accent);
  }

  .msg {
    flex: 1;
    color: var(--ls-text);
  }

  .figure {
    font-family: var(--ff-mono);
  }

  .action {
    color: var(--banner-accent);
    font-weight: var(--fw-600);
  }

  .action:hover {
    color: var(--ls-gold-bright);
  }

  .dismiss {
    display: flex;
    color: var(--ls-text-dim);
    transition: color var(--dur-120) ease;
  }

  .dismiss:hover {
    color: var(--ls-text);
  }
</style>
