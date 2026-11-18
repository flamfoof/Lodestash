<script lang="ts">
  import type { HTMLInputAttributes } from "svelte/elements";

  /**
   * A native checkbox (so Space, labels and forms all work) drawn as the prototype's pick box: a
   * small dark tile that turns gold with a check. `label` is visible unless `hideLabel`.
   */
  let {
    checked = $bindable(false),
    label,
    hideLabel = false,
    boxAttrs = {},
    onchange,
    ...rest
  }: Omit<HTMLInputAttributes, "onchange"> & {
    checked?: boolean;
    label: string;
    hideLabel?: boolean;
    /** Reaches the drawn box (the contract gate marks it). */
    boxAttrs?: Record<string, string | undefined>;
    onchange?: (checked: boolean) => void;
  } = $props();
</script>

<label class="check">
  <span class="box" class:on={checked} {...boxAttrs}>
    <input type="checkbox" bind:checked onchange={() => onchange?.(checked)} {...rest} />
    {#if checked}<span class="mark" aria-hidden="true"></span>{/if}
  </span>
  <span class:visually-hidden={hideLabel}>{label}</span>
</label>

<style>
  .check {
    display: inline-flex;
    align-items: center;
    gap: var(--space-8);
    font-size: var(--fs-12);
    color: var(--ls-text-muted);
    cursor: pointer;
  }

  .box {
    position: relative;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    box-sizing: content-box;
    width: var(--space-18);
    height: var(--space-18);
    flex: none;
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-4);
    background: var(--ls-bg-a80);
    color: var(--ls-gold-bright);
    transition: border-color var(--dur-120) ease;
  }

  .box:hover,
  .on {
    border-color: var(--ls-gold);
  }

  .box:has(input:focus-visible) {
    box-shadow: var(--ring-focus);
  }

  /* The real input covers the box: it takes the click, the focus and the keyboard. */
  input {
    position: absolute;
    inset: 0;
    margin: 0;
    opacity: 0;
    cursor: pointer;
  }

  .mark {
    width: var(--space-5);
    height: var(--space-9);
    margin-top: calc(var(--space-2) * -1);
    border: solid currentColor;
    border-width: 0 var(--bw-2) var(--bw-2) 0;
    transform: rotate(45deg);
    pointer-events: none;
  }
</style>
