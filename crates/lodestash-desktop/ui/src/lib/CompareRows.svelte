<script lang="ts">
  /**
   * The two-column stat table shared by the inspector's compare pane (UI-SPEC §13) and Duplicates
   * (§5), which reviews groups in that same pane.
   *
   * The rule this component exists to hold in one place: **rows where the two differ go gold on
   * both sides; rows where they agree stay dim.** Agreement is not a finding — the eye should land
   * on the handful of facts that actually decide it, not on the whole table.
   *
   * Callers build their own rows, because they hold different shapes (the inspector has
   * `AssetDetail`, Duplicates has `GridItem`). What must not be duplicated is the rule.
   *
   * The cell class stays `.cmp-v` — the e2e checks assert on it, and renaming a hook that three
   * checks reach for buys nothing.
   */
  import type { CompareRow } from "./api";

  let { rows, testid = "compare-rows" }: { rows: CompareRow[]; testid?: string } = $props();
</script>

<div class="rows" data-testid={testid}>
  {#each rows as r (r.label)}
    <span class="k">{r.label}</span>
    <span class="cmp-v num" class:differs={r.values[0] !== r.values[1]}>{r.values[0]}</span>
    <span class="cmp-v num" class:differs={r.values[0] !== r.values[1]}>{r.values[1]}</span>
  {/each}
</div>

<style>
  .rows {
    display: grid;
    grid-template-columns: 62px 1fr 1fr;
    gap: 4px var(--sp-2);
    align-items: center;
    font-size: var(--fs-xs);
  }
  .k {
    color: var(--ls-text-dim);
  }
  .cmp-v {
    color: var(--ls-text-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .cmp-v.differs {
    color: var(--ls-gold);
  }
</style>
