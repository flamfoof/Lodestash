<script lang="ts">
  import type { HTMLButtonAttributes } from "svelte/elements";

  /**
   * An on/off setting (`role="switch"`, `aria-checked`), drawn as the prototype's settings toggle:
   * a 34 × 18 track, gold when on, with a 14 px knob. `label` is the accessible name; pass
   * `labelledby` instead when the visible row title names it.
   */
  let {
    checked = $bindable(false),
    label,
    labelledby,
    onchange,
    ...rest
  }: Omit<HTMLButtonAttributes, "onchange"> & {
    checked?: boolean;
    label?: string;
    labelledby?: string;
    onchange?: (checked: boolean) => void;
  } = $props();

  function toggle() {
    checked = !checked;
    onchange?.(checked);
  }
</script>

<button
  type="button"
  role="switch"
  class="switch"
  class:on={checked}
  aria-checked={checked}
  aria-label={labelledby ? undefined : label}
  aria-labelledby={labelledby}
  onclick={toggle}
  {...rest}
>
  <span class="knob"></span>
</button>

<style>
  .switch {
    display: flex;
    align-items: center;
    flex: none;
    width: calc(var(--space-30) + var(--space-4));
    height: var(--space-18);
    padding: 0 var(--space-2);
    border-radius: var(--r-pill);
    background: var(--ls-border);
    transition: background var(--dur-150) ease;
  }

  .knob {
    width: var(--space-14);
    height: var(--space-14);
    border-radius: var(--r-pill);
    background: var(--ls-text-dim);
    transition:
      transform var(--dur-150) ease,
      background var(--dur-150) ease;
  }

  .on {
    background: var(--ls-gold-a50);
  }

  .on .knob {
    background: var(--ls-gold);
    transform: translateX(var(--space-16));
  }

  .switch:disabled {
    opacity: var(--op-35);
  }
</style>
