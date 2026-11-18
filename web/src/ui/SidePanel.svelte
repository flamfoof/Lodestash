<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLAttributes } from "svelte/elements";

  /**
   * The panel on the right edge of the frame — the Inspector: the surface with a hairline on its
   * left, as tall as its column, scrolling on its own. It rises in when it appears. `label` is its
   * accessible name; it fills the column its owner gives it.
   */
  let { label, children, ...rest }: HTMLAttributes<HTMLElement> & { label: string; children: Snippet } = $props();
</script>

<aside class="panel" aria-label={label} {...rest}>
  {@render children()}
</aside>

<style>
  .panel {
    display: flex;
    flex-direction: column;
    width: 100%;
    height: 100%;
    min-height: 0;
    background: var(--ls-surface);
    border-left: 1px solid var(--ls-border);
    overflow-y: auto;
    animation: rise var(--dur-160) ease;
  }

  @keyframes rise {
    from {
      transform: translateY(var(--space-4));
      opacity: 0;
    }
  }
</style>
