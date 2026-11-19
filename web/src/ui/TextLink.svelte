<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLButtonAttributes } from "svelte/elements";

  /**
   * An action that reads as text. By default it is part of a line — a pack's name in a card's meta
   * line or the inspector's provenance: it takes the line's type and colour, and turns gold and
   * underlined under the pointer (the underline is what says "this one is a link", not the colour).
   *
   * As a quiet action of its own ("Clear all", "✕ Clear", "← prev") it states its rest colour
   * (`tone`) and size (`size`, a `--fs-*` suffix), and `hover` is what it turns: `danger` for an
   * action that throws something away, `text` for one that does not. Disabled, it simply does not
   * react: the caller gives it the `dim` tone, which is how the prototype draws "nothing to clear".
   */
  let {
    tone = "inherit",
    hover = "gold",
    size,
    children,
    ...rest
  }: HTMLButtonAttributes & {
    tone?: "inherit" | "muted" | "dim";
    hover?: "gold" | "danger" | "text";
    size?: "10" | "11" | "12";
    children: Snippet;
  } = $props();
</script>

<button type="button" class="link {tone} to-{hover}" style:--link-size={size ? `var(--fs-${size})` : undefined} {...rest}>{@render children()}</button>

<style>
  .link {
    display: inline;
    font-size: var(--link-size, inherit);
    transition: color var(--dur-120) ease;
  }

  .muted {
    color: var(--ls-text-muted);
  }

  .dim {
    color: var(--ls-text-dim);
  }

  .to-gold:hover:not(:disabled) {
    color: var(--ls-gold);
    text-decoration: underline;
  }

  .to-danger:hover:not(:disabled) {
    color: var(--ls-danger);
  }

  .to-text:hover:not(:disabled) {
    color: var(--ls-text);
  }
</style>
