<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLAttributes } from "svelte/elements";

  /**
   * The application's top bar (`<header>`, the banner landmark): one 48 px row on the raised
   * surface with a hairline under it, children laid out left to right 14 px apart.
   */
  let { children, ...rest }: HTMLAttributes<HTMLElement> & { children: Snippet } = $props();
</script>

<header class="appbar" {...rest}>
  {@render children()}
</header>

<style>
  .appbar {
    box-sizing: content-box;
    height: var(--topbar-h);
    display: flex;
    align-items: center;
    gap: var(--space-14);
    padding: 0 var(--space-16);
    background: var(--ls-surface-raised);
    border-bottom: 1px solid var(--ls-border);
    z-index: var(--z-sticky);
    min-width: 0;
  }
</style>
