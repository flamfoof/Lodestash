<script lang="ts">
  import type { HTMLInputAttributes } from "svelte/elements";

  /**
   * A single-line text field with its label. The label is visible unless `hideLabel` (then it is
   * still the accessible name). Sizes are the prototype's fields:
   * - `md` — a settings row's (5/10 px, 11 px type);
   * - `sm` — the search inside a popover (3/9 px, 10 px);
   * - `bar` — a field in a strip of actions, the bulk bar's tag name (3/9 px, 12 px, the stronger
   *   edge its neighbours have);
   * - `dense` — a short value beside its label, the staging tray's destination (4/8 px, 11 px).
   * The last three have a stated height: the prototype drew them as unstyled inputs, whose height
   * came from the browser's default font rather than from the type the app sets.
   * `fill` makes the field take its container's width (the container is the layout's to size).
   */
  let {
    label,
    hideLabel = false,
    value = $bindable(""),
    size = "md",
    mono = false,
    fill = false,
    id,
    ...rest
  }: Omit<HTMLInputAttributes, "size"> & {
    label: string;
    hideLabel?: boolean;
    value?: string;
    size?: "sm" | "md" | "bar" | "dense";
    mono?: boolean;
    fill?: boolean;
  } = $props();

  const uid = $props.id();
  const inputId = $derived(id ?? `input-${uid}`);
</script>

<span class="field" class:fill>
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

  .fill {
    display: flex;
    flex: 1;
  }

  .fill .input {
    width: 100%;
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
    height: var(--field-h-sm);
    padding: var(--space-3) var(--space-9);
    font-size: var(--fs-10);
  }

  .bar {
    height: var(--field-h-bar);
    padding: var(--space-3) var(--space-9);
    border-color: var(--ls-border-strong);
    border-radius: var(--r-4);
    font-size: var(--fs-12);
  }

  .dense {
    height: var(--field-h-dense);
    padding: var(--space-4) var(--space-8);
    border-color: var(--ls-border-strong);
    border-radius: var(--r-4);
    font-size: var(--fs-11);
  }

  .mono {
    font-family: var(--ff-mono);
  }
</style>
