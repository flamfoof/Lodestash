<script lang="ts">
  import type { Snippet } from "svelte";
  import Icon from "./Icon.svelte";
  import type { IconName } from "./icons.generated";

  /**
   * What a screen shows when it has nothing (DESIGN-GUIDELINES §7 Feedback): a 32 px mark, a title
   * that states the fact, one line that teaches the concept, and at most one action — only when
   * there is something to do here. Drawn as the prototype's library no-results block.
   */
  let {
    icon,
    title,
    line,
    action,
  }: {
    icon: IconName;
    title: string;
    line?: string;
    action?: Snippet;
  } = $props();
</script>

<div class="empty">
  <Icon name={icon} size="32" />
  <div class="title">{title}</div>
  {#if line}<div class="line">{line}</div>{/if}
  {#if action}<div class="action">{@render action()}</div>{/if}
</div>

<style>
  .empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-10);
    padding: var(--space-80) var(--space-20);
    color: var(--ls-text-dim);
    text-align: center;
  }

  .title {
    color: var(--ls-text-muted);
  }

  .line {
    max-width: 40ch;
    font-size: var(--fs-12);
  }

  .action {
    margin-top: var(--space-6);
  }
</style>
