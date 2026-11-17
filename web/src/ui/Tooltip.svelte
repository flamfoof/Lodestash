<script lang="ts">
  /**
   * The bubble a `tooltip(text)` attachment shows (./tooltip.ts) — rendered into the overlay host,
   * placed below its anchor or above when there is no room. `role="tooltip"`; the anchor points at
   * it with `aria-describedby` while it is up.
   */
  let { id, text }: { id: string; text: string } = $props();
</script>

<!-- Positioned by ./tooltip.ts once it has measured itself; hidden until then. -->
<div {id} class="tip" role="tooltip">
  {text}
</div>

<style>
  .tip {
    position: fixed;
    z-index: var(--z-toast);
    max-width: var(--dialog-w-sm);
    padding: var(--space-4) var(--space-8);
    background: var(--ls-overlay);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-6);
    box-shadow: var(--shadow-8-24-a45);
    font-size: var(--fs-11);
    color: var(--ls-text);
    pointer-events: none;
    visibility: hidden;
  }
</style>
