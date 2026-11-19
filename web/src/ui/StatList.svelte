<script lang="ts">
  import type { HTMLAttributes } from "svelte/elements";

  /**
   * The stat block (DESIGN-GUIDELINES §7): key/value rows in tabular mono, each on a dashed
   * hairline — the key dim on the left, the value on the right. A description list, so a screen
   * reader pairs each value with its key. `anchors` marks the first row for the contract gate.
   */
  let {
    rows,
    anchors = {},
    ...rest
  }: HTMLAttributes<HTMLDListElement> & {
    rows: readonly (readonly [key: string, value: string])[];
    anchors?: { row?: string; key?: string; value?: string };
  } = $props();
</script>

<dl class="stats" {...rest}>
  {#each rows as [key, value], i (key)}
    <div class="row" data-anchor={i === 0 ? anchors.row : undefined}>
      <dt data-anchor={i === 0 ? anchors.key : undefined}>{key}</dt>
      <dd data-anchor={i === 0 ? anchors.value : undefined}>{value}</dd>
    </div>
  {/each}
</dl>

<style>
  .stats {
    margin: 0;
    font-family: var(--ff-mono);
    font-size: var(--fs-12);
    user-select: text;
  }

  .row {
    display: flex;
    justify-content: space-between;
    padding: var(--space-4) 0;
    border-bottom: 1px dashed var(--ls-border);
  }

  dt {
    color: var(--ls-text-dim);
  }

  dd {
    margin: 0;
    font-weight: var(--fw-500);
    text-align: right;
  }
</style>
