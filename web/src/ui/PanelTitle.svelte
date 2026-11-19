<script lang="ts">
  import type { HTMLAttributes } from "svelte/elements";

  /**
   * The name at the head of a panel — the inspected asset's: 14 px, semibold, one line, cut with
   * an ellipsis (the full name is its tooltip). `lg` is Quick Look's, a size up (15 px). `unknown`
   * is the Unidentified treatment the card's name has too: slate and italic. A heading, so the
   * panel is reachable by heading navigation.
   */
  let {
    text,
    size = "md",
    unknown = false,
    ...rest
  }: HTMLAttributes<HTMLHeadingElement> & { text: string; size?: "md" | "lg"; unknown?: boolean } = $props();
</script>

<h2 class="title {size}" class:unknown title={text} {...rest}>{text}</h2>

<style>
  .title {
    min-width: 0;
    margin: 0;
    font-size: var(--fs-14);
    font-weight: var(--fw-600);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    user-select: text;
  }

  .lg {
    font-size: var(--fs-15);
  }

  .unknown {
    color: var(--ls-status-unidentified);
    font-style: italic;
  }
</style>
