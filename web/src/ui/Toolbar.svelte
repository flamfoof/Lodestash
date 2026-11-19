<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLAttributes } from "svelte/elements";
  import { focusables } from "./focus";

  /**
   * A strip of controls above a view (`role="toolbar"`): the library's result label, sort, view
   * and filter controls — 8/16 px padding, 10 px gaps, a hairline under it. ← and → move focus
   * between its controls (Tab still visits each); a text field keeps its own arrow keys.
   * `tone="selection"` is the bulk bar: the same strip in the gold wash that says a selection is
   * being acted on (7/16 px, 12 px type).
   */
  let {
    label,
    tone = "plain",
    children,
    ...rest
  }: HTMLAttributes<HTMLDivElement> & { label: string; tone?: "plain" | "selection"; children: Snippet } = $props();

  function onkeydown(event: KeyboardEvent) {
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    const target = event.target as HTMLElement;
    if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement) return;
    const items = focusables(event.currentTarget as HTMLElement);
    const at = items.indexOf(target);
    if (at < 0) return;
    event.preventDefault();
    const to = event.key === "ArrowRight" ? Math.min(items.length - 1, at + 1) : Math.max(0, at - 1);
    items[to].focus();
  }
</script>

<div class="toolbar {tone}" role="toolbar" aria-label={label} {onkeydown} {...rest}>
  {@render children()}
</div>

<style>
  .toolbar {
    display: flex;
    align-items: center;
    gap: var(--space-10);
    flex-shrink: 0;
    padding: var(--space-8) var(--space-16);
    border-bottom: 1px solid var(--ls-border);
  }

  .selection {
    padding: var(--space-7) var(--space-16);
    background: var(--ls-gold-a6);
    border-bottom-color: var(--ls-gold-a35);
    font-size: var(--fs-12);
  }
</style>
