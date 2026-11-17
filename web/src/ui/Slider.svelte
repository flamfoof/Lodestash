<script lang="ts">
  import type { HTMLInputAttributes } from "svelte/elements";

  /**
   * A native range input (arrow keys, Page Up/Down, Home/End for free) in the brand gold — the
   * prototype's cache-cap slider. `label` is its accessible name; `valueText` what a screen reader
   * says for the current value ("8 GB").
   */
  let {
    value = $bindable(0),
    min = 0,
    max = 100,
    step = 1,
    label,
    valueText,
    oninput,
    ...rest
  }: Omit<HTMLInputAttributes, "value" | "min" | "max" | "step" | "oninput"> & {
    value?: number;
    min?: number;
    max?: number;
    step?: number;
    label: string;
    valueText?: string;
    oninput?: (value: number) => void;
  } = $props();
</script>

<input
  class="slider"
  type="range"
  {min}
  {max}
  {step}
  aria-label={label}
  aria-valuetext={valueText}
  bind:value
  oninput={() => oninput?.(value)}
  {...rest}
/>

<style>
  .slider {
    width: 100%;
    height: var(--space-4);
    margin: 0;
    accent-color: var(--ls-gold);
    cursor: pointer;
  }
</style>
