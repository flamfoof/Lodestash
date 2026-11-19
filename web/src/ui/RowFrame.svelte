<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLAttributes } from "svelte/elements";

  /**
   * The hairline frame a list of rows sits in: rows stacked edge to edge, the corners rounded, and
   * whatever a row draws past the frame cut off. It has no role of its own — its owner says what
   * the rows are (`role="row"` in the asset grid's list layout).
   */
  let { children, ...rest }: HTMLAttributes<HTMLDivElement> & { children: Snippet } = $props();
</script>

<div class="frame" {...rest}>{@render children()}</div>

<style>
  .frame {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--ls-border);
    border-radius: var(--r-8);
    overflow: hidden;
  }
</style>
