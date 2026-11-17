<script lang="ts">
  import type { IconName } from "./icons.generated";

  /**
   * One icon from the sprite, tinted by `currentColor`. `size` is a token name suffix so icon
   * sizes stay on the design's ladder (ADR-0012): the box is `var(--fs-<size>)` square.
   * Decorative by default; pass `label` when the icon alone carries meaning.
   */
  let {
    name,
    size = "16",
    label,
  }: { name: IconName; size?: "9" | "10" | "11" | "12" | "13" | "14" | "15" | "16" | "18" | "20" | "24"; label?: string } =
    $props();
</script>

<svg
  class="icon"
  style:--icon-size="var(--fs-{size})"
  role={label ? "img" : undefined}
  aria-label={label}
  aria-hidden={label ? undefined : "true"}
  focusable="false"
>
  <use href="/icons.svg#ls-{name}" />
</svg>

<style>
  .icon {
    width: var(--icon-size);
    height: var(--icon-size);
    flex: none;
    vertical-align: middle;
  }
</style>
