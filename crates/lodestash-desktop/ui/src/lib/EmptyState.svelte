<script lang="ts">
  /**
   * One empty-state anatomy (DESIGN-GUIDELINES §7 "Feedback", board D14).
   *
   * There were eleven of these in the app and no two were built the same — some a bare `<p>`, some
   * with a hint, one with an action. They are where a new user meets a screen for the first time,
   * so they are the copy that teaches the product; they deserve to be one component rather than
   * eleven improvisations.
   *
   * The rules the component enforces, so callers cannot drift from them:
   *
   * - the icon is the screen's own nav glyph at 32px, dim — never a new drawing;
   * - the title is a statement of fact, not an apology;
   * - one line, ≤ 40ch, teaching the concept behind the screen rather than restating the absence;
   * - **at most one** action, and only when there is something to do here.
   *
   * Not for a filtered-empty result — that is UI-SPEC §2.6 and needs a way *out*, not a lesson.
   */
  import Icon from "./Icon.svelte";

  let {
    icon,
    title,
    line,
    action,
    onaction,
    testid,
  }: {
    /** The screen's own `ls-nav-*` sprite id. */
    icon: string;
    title: string;
    line: string;
    /** Omit for a screen you cannot act on — an empty Trash needs no button. */
    action?: string;
    onaction?: () => void;
    testid?: string;
  } = $props();
</script>

<div class="empty" data-testid={testid ?? "empty-state"}>
  <Icon name={icon} size={32} />
  <p class="title">{title}</p>
  <p class="line">{line}</p>
  {#if action && onaction}
    <button class="act" onclick={onaction}>{action}</button>
  {/if}
</div>

<style>
  .empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-7) var(--sp-5);
    text-align: center;
    color: var(--ls-text-dim);
  }
  .title {
    margin: var(--sp-1) 0 0;
    font-size: var(--fs-lg);
    color: var(--ls-text);
  }
  .line {
    margin: 0;
    max-width: 40ch;
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
    line-height: 1.6;
  }
  .act {
    margin-top: var(--sp-2);
    height: var(--control-h);
    padding: 0 var(--sp-4);
    font: inherit;
    font-size: var(--fs-sm);
    font-weight: 500;
    color: var(--ls-bg);
    background: var(--ls-gold);
    border: 1px solid var(--ls-gold);
    border-radius: var(--r-md);
    cursor: pointer;
  }
  .act:hover {
    background: var(--ls-gold-bright);
    border-color: var(--ls-gold-bright);
  }
</style>
