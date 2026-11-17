<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLInputAttributes } from "svelte/elements";
  import Icon from "./Icon.svelte";

  /**
   * The top bar's search box: a field in a gold-edged well that grows to fill the bar up to
   * `--search-max-w`. `tone="arcane"` is the AI-search look (teal edge and glow), so the engine
   * that answers stays legible. `status` renders after the input ("✦ sieving…").
   * The well is what the contract measures; `rest` goes to the `<input>` (name, onkeydown, …),
   * `wellAttrs` to the well.
   */
  let {
    value = $bindable(""),
    label = "Search",
    tone = "gold",
    status,
    wellAttrs = {},
    ...rest
  }: HTMLInputAttributes & {
    value?: string;
    label?: string;
    tone?: "gold" | "arcane";
    status?: Snippet;
    wellAttrs?: Record<string, string | undefined>;
  } = $props();
</script>

<div class="search {tone}" {...wellAttrs}>
  <span class="glyph"><Icon name={tone === "arcane" ? "search-ai" : "search"} size="13" box="8" /></span>
  <input type="search" aria-label={label} bind:value {...rest} />
  {@render status?.()}
</div>

<style>
  .search {
    box-sizing: content-box;
    flex: 1;
    min-width: 0;
    max-width: var(--search-max-w);
    height: var(--space-32);
    display: flex;
    align-items: center;
    gap: var(--space-8);
    padding: 0 var(--space-12);
    background: var(--ls-surface);
    border: 1px solid var(--ls-gold);
    border-radius: var(--r-6);
    box-shadow: var(--ring-gold-2-a18);
    transition:
      border-color var(--dur-150) ease,
      box-shadow var(--dur-150) ease;
  }

  .arcane {
    border-color: var(--ls-arcane);
    box-shadow: var(--ring-arcane-2-a22);
  }

  .search:has(input:focus-visible) {
    box-shadow: var(--ring-focus);
  }

  .glyph {
    display: flex;
    color: var(--ls-text-dim);
  }

  input {
    flex: 1;
    min-width: 0;
    padding: 0;
    background: transparent;
    border: none;
    outline: none;
    color: var(--ls-text);
    font-size: var(--fs-13);
  }

  /* The well shows focus; the field inside it does not draw a second ring. */
  input:focus-visible {
    box-shadow: none !important; /* style-lint-allow: overrides the global focus ring, the well draws it */
  }

  input::-webkit-search-cancel-button {
    display: none;
  }
</style>
