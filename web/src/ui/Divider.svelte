<script lang="ts">
  /** A hairline between groups: inset 14 px with 12 px above and below (the rail), or flush. */
  let { inset = true }: { inset?: boolean } = $props();
</script>

<hr class="divider" class:inset />

<style>
  .divider {
    height: 1px;
    margin: 0;
    flex-shrink: 0;
    border: 0;
    background: var(--ls-border);
  }

  .inset {
    margin: var(--space-12) var(--space-14);
  }
</style>
