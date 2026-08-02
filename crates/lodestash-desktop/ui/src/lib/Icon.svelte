<script lang="ts">
  /**
   * The whole icon abstraction: one <use> into the checked-in sprite
   * (docs/design/icons/icons.svg, copied to public/).
   *
   * Each symbol bakes its own stroke colour from the palette rather than using
   * currentColor, so a parent `color` does not tint it — dim inactive states with
   * opacity instead. That is the sprite's design, not an oversight to work around.
   */
  let { name, size = 20 }: { name: string; size?: number } = $props();
</script>

<svg class="icon" width={size} height={size} aria-hidden="true" focusable="false">
  <use href="/icons.svg#{name}" />
</svg>

<style>
  .icon {
    display: block;
    flex: none;
    color: inherit;
  }
</style>
