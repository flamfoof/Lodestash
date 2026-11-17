<script lang="ts">
  /**
   * Indeterminate activity, for the rare wait with no measurable progress (prefer ProgressBar).
   * Under reduced motion it stops turning but stays: it still says "working".
   */
  let { label = "Working", size = "16" }: { label?: string; size?: "12" | "16" | "24" } = $props();
</script>

<span class="spinner" style:--spin-size="var(--fs-{size})" role="status" aria-label={label}></span>

<style>
  .spinner {
    display: inline-block;
    width: var(--spin-size);
    height: var(--spin-size);
    border: var(--bw-2) solid var(--ls-border-strong);
    border-top-color: var(--ls-gold);
    border-radius: var(--r-pill);
    animation: spin var(--dur-1000) linear infinite;
    vertical-align: middle;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
