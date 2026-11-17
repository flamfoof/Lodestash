<script lang="ts">
  import type { HTMLInputAttributes } from "svelte/elements";

  /**
   * A single-line text field with its label. The label is visible unless `hideLabel` (then it is
   * still the accessible name). `md` is the settings-row field (5/10 px, 11 px type); `sm` the
   * compact search-in-a-popover field (3/9 px, 10 px).
   */
  let {
    label,
    hideLabel = false,
    value = $bindable(""),
    size = "md",
    mono = false,
    id,
    ...rest
  }: Omit<HTMLInputAttributes, "size"> & {
    label: string;
    hideLabel?: boolean;
    value?: string;
    size?: "sm" | "md";
    mono?: boolean;
  } = $props();

  const uid = $props.id();
  const inputId = $derived(id ?? `input-${uid}`);
</script>

<span class="field">
  <label for={inputId} class:visually-hidden={hideLabel}>{label}</label>
  <input id={inputId} class="input {size}" class:mono type="text" bind:value {...rest} />
</span>

<style>
  .field {
    display: inline-flex;
    flex-direction: column;
    gap: var(--space-4);
    min-width: 0;
  }

  label {
    font-size: var(--fs-9);
    font-weight: var(--fw-600);
    letter-spacing: var(--track-1);
    color: var(--ls-text-dim);
  }

  .input {
    min-width: 0;
    background: var(--ls-surface);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-5);
    color: var(--ls-text);
    outline: none;
  }

  .md {
    padding: var(--space-5) var(--space-10);
    font-size: var(--fs-11);
  }

  .sm {
    padding: var(--space-3) var(--space-9);
    font-size: var(--fs-10);
  }

  .mono {
    font-family: var(--ff-mono);
  }
</style>
