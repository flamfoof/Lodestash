<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLAttributes } from "svelte/elements";

  /**
   * The application's top bar (`<header>`, the banner landmark): one 48 px row on the raised
   * surface with a hairline under it, children laid out left to right 14 px apart.
   *
   * It is the `appbar` size container. As it narrows, controls in it fold to their icon in a
   * stated order — decoration first (ADR-0009's ordering principle) — each keeping its label as
   * its accessible name and tooltip. A primitive opts in with `collapse="xl" | "lg" | "md"`:
   *   xl — below 1400 px of bar (window < 1432): the local-inference chip;
   *   lg — below 1200 px (window < 1232): the category tabs;
   *   md — below 1000 px (window < 1032): the wordmark's name and the search-mode labels.
   * At the 900 px window floor the search well keeps ~150 px.
   */
  let { children, ...rest }: HTMLAttributes<HTMLElement> & { children: Snippet } = $props();
</script>

<header class="appbar" {...rest}>
  {@render children()}
</header>

<style>
  .appbar {
    container: appbar / inline-size;
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
