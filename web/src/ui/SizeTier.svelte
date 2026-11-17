<script lang="ts" module>
  export type Tier = "S" | "M" | "L" | "XL";
</script>

<script lang="ts">
  import type { HTMLAttributes } from "svelte/elements";

  /**
   * A file's size tier and size ("L · 4.1 MB"), coloured by tier — S success, M sound-aqua,
   * L warning, XL danger (DESIGN-GUIDELINES §7 Filters). `badge` is the outlined tag on a thumbnail
   * corner; `text` the bare mono label of a list row. The tier letter is the shape that keeps it
   * from being colour alone.
   */
  let {
    tier,
    size,
    variant = "badge",
    ...rest
  }: HTMLAttributes<HTMLSpanElement> & { tier: Tier; size: string; variant?: "badge" | "text" } = $props();
</script>

<span class="tier {variant} t{tier}" title="File size on disk" {...rest}>{tier} · {size}</span>

<style>
  .tier {
    font-family: var(--ff-mono);
    white-space: nowrap;
  }

  .badge {
    font-size: var(--fs-9);
    padding: var(--space-2) var(--space-6);
    background: var(--ls-bg-a78);
    border: 1px solid;
    border-radius: var(--r-4);
  }

  .text {
    font-size: var(--fs-10);
  }

  .tS {
    color: var(--ls-success);
    border-color: var(--ls-success-a40);
  }

  .tM {
    color: var(--ls-type-sound);
    border-color: var(--ls-type-sound-a40);
  }

  .tL {
    color: var(--ls-warning);
    border-color: var(--ls-warning-a40);
  }

  .tXL {
    color: var(--ls-danger);
    border-color: var(--ls-danger-a40);
  }
</style>
