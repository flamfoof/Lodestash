<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLAttributes } from "svelte/elements";

  /**
   * A full-width strip across the top of a screen: the library's "N assets are Unidentified" nudge.
   * A tinted band with a hairline under it, a leading mark, the message, and at most one action
   * (a text button in the band's colour). `actionAttrs` reaches the action button.
   */
  let {
    tone = "warning",
    mark = "?",
    action,
    actionAttrs = {},
    children,
    ...rest
  }: HTMLAttributes<HTMLDivElement> & {
    tone?: "warning" | "info" | "danger";
    /** A one-character mark before the message. */
    mark?: string;
    action?: { label: string; onclick: () => void };
    actionAttrs?: Record<string, string | undefined>;
    children: Snippet;
  } = $props();
</script>

<div class="banner {tone}" role="status" {...rest}>
  <span class="mark" aria-hidden="true">{mark}</span>
  <span class="msg">{@render children()}</span>
  {#if action}
    <button type="button" class="action" onclick={action.onclick} {...actionAttrs}>{action.label}</button>
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

  .action {
    color: var(--banner-accent);
    font-weight: var(--fw-600);
  }

  .action:hover {
    color: var(--ls-gold-bright);
  }
</style>
