<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLAttributes } from "svelte/elements";

  /**
   * The left rail: a 210 px column on the surface with a hairline on its right, its rows stacked
   * 2 px apart, scrolling on its own when it outgrows the window. Holds the nav list, facet groups
   * (RailHeading, ListRow) and Dividers.
   */
  let { children, ...rest }: HTMLAttributes<HTMLElement> & { children: Snippet } = $props();
</script>

<aside class="rail" {...rest}>
  {@render children()}
</aside>

<style>
  .rail {
    box-sizing: content-box;
    width: var(--sidebar-w);
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: var(--space-12) 0;
    background: var(--ls-surface);
    border-right: 1px solid var(--ls-border);
    overflow-y: auto;
    min-height: 0;
  }
</style>
