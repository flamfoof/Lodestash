<script lang="ts" generics="V extends string">
  import type { HTMLSelectAttributes } from "svelte/elements";

  /**
   * A native `<select>` (the OS draws the open list, which keeps it keyboard- and screen-reader-
   * correct for free) in the prototype's toolbar styling. `label` is its accessible name; the
   * options' own text carries the visible context ("sort · default").
   * The prototype never styled its select's font, so it measures Arial there; the app's select
   * uses the UI font (docs/design/DEVIATIONS.md, `library.sort`).
   */
  let {
    options,
    value = $bindable(),
    onchange,
    label,
    ...rest
  }: Omit<HTMLSelectAttributes, "onchange" | "value"> & {
    options: { value: V; label: string }[];
    value: V;
    onchange?: (value: V) => void;
    label: string;
  } = $props();
</script>

<select
  class="select"
  aria-label={label}
  bind:value
  onchange={() => onchange?.(value)}
  {...rest}
>
  {#each options as option (option.value)}
    <option value={option.value}>{option.label}</option>
  {/each}
</select>

<style>
  .select {
    flex-shrink: 0;
    padding: var(--space-4) var(--space-8);
    background: var(--ls-overlay);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-6);
    color: var(--ls-text-muted);
    font-size: var(--fs-11);
    outline: none;
    cursor: pointer;
  }

  .select:hover {
    border-color: var(--ls-text-dim);
  }

  option {
    background: var(--ls-overlay);
    color: var(--ls-text);
  }
</style>
