<script lang="ts" module>
  /** Drawn sizes: suffixes of the `--fs-*` tokens, so icon sizes stay on the design's scale. */
  export type IconSize = "8" | "9" | "10" | "11" | "12" | "13" | "14" | "15" | "16" | "18" | "20" | "24" | "32";
  /** Layout widths: suffixes of the `--space-*` tokens. */
  export type IconBox = "4" | "5" | "6" | "7" | "8" | "9" | "10" | "11" | "12" | "13" | "14" | "15" | "16" | "18" | "20";
</script>

<script lang="ts">
  import type { IconName } from "./icons.generated";

  /**
   * One icon from the sprite, tinted by `currentColor`.
   *
   * `size` is the drawn size. `box` is the width the icon occupies in layout, when that must differ:
   * the prototype draws nav and action marks as Unicode glyphs, and the app replaces each with the
   * designed SVG at the glyph's measured advance (docs/design/DEVIATIONS.md), so a 10 px glyph keeps
   * its 10 px slot even where the icon is drawn larger and overflows it symmetrically.
   * Decorative by default; pass `label` when the icon alone carries meaning.
   */
  let {
    name,
    size = "16",
    box,
    label,
  }: { name: IconName; size?: IconSize; box?: IconBox; label?: string } = $props();
</script>

<svg
  class="icon"
  style:--icon-size="var(--fs-{size})"
  style:--icon-box={box ? `var(--space-${box})` : undefined}
  preserveAspectRatio="xMidYMid slice"
  role={label ? "img" : undefined}
  aria-label={label}
  aria-hidden={label ? undefined : "true"}
  focusable="false"
>
  <use href="/icons.svg#ls-{name}" />
</svg>

<style>
  .icon {
    width: var(--icon-box, var(--icon-size));
    height: var(--icon-size);
    flex: none;
    overflow: visible;
    vertical-align: middle;
  }
</style>
