<script lang="ts" module>
  /** One tone per media type (the `--ls-type-*` tokens). */
  export type TypeTone = "model" | "sound" | "image" | "texture" | "material" | "sprite" | "font";
</script>

<script lang="ts">
  import type { HTMLAttributes } from "svelte/elements";

  /**
   * The small diamond before an asset's name, in its type's colour. `label` names the type for
   * assistive technology ("Model") — the colour never carries it alone.
   * `sm` is the card's 8 px mark, `md` the inspector's 9 px one.
   */
  let {
    tone,
    size = "sm",
    label,
    ...rest
  }: HTMLAttributes<HTMLSpanElement> & { tone: TypeTone; size?: "sm" | "md"; label?: string } = $props();
</script>

<span class="mark {tone} {size}" role={label ? "img" : undefined} aria-label={label} title={label} {...rest}></span>

<style>
  .mark {
    flex: none;
    transform: rotate(45deg);
    border-radius: var(--r-2);
    background: var(--type);
  }

  .sm {
    width: var(--space-8);
    height: var(--space-8);
  }

  .md {
    width: var(--space-9);
    height: var(--space-9);
  }

  .model {
    --type: var(--ls-type-model);
  }

  .sound {
    --type: var(--ls-type-sound);
  }

  .image {
    --type: var(--ls-type-image);
  }

  .texture {
    --type: var(--ls-type-texture);
  }

  .material {
    --type: var(--ls-type-material);
  }

  .sprite {
    --type: var(--ls-type-sprite);
  }

  .font {
    --type: var(--ls-type-font);
  }
</style>
