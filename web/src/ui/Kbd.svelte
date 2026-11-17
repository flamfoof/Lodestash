<script lang="ts">
  import type { Snippet } from "svelte";

  /** A key, as in "Space quick-looks": mono, on a small outlined cap. */
  let { children }: { children: Snippet } = $props();
</script>

<kbd class="kbd">{@render children()}</kbd>

<style>
  .kbd {
    display: inline-block;
    padding: 0 var(--space-5);
    font-family: var(--ff-mono);
    font-size: var(--fs-10);
    color: var(--ls-text-muted);
    background: var(--ls-overlay);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-4);
  }
</style>
