<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLButtonAttributes } from "svelte/elements";

  /**
   * An action that reads as part of a line of text: a pack's name in a card's meta line or the
   * inspector's provenance. It takes the line's type and colour, and turns gold and underlined
   * under the pointer — the underline is what says "this one is a link", not the colour.
   */
  let { children, ...rest }: HTMLButtonAttributes & { children: Snippet } = $props();
</script>

<button type="button" class="link" {...rest}>{@render children()}</button>

<style>
  .link {
    display: inline;
    transition: color var(--dur-120) ease;
  }

  .link:hover {
    color: var(--ls-gold);
    text-decoration: underline;
  }
</style>
