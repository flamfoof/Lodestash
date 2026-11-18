<script lang="ts">
  import type { HTMLAttributes } from "svelte/elements";
  import Icon from "./Icon.svelte";

  /**
   * "Already imported" on a thumbnail (DESIGN-GUIDELINES §7): a check and where — `active` is the
   * open project (success), `others` only other projects (dim: history, not a warning), `stale` an
   * older copy (warning — the one that asks for action). The check and the words carry it; the
   * colour only ranks it.
   */
  let {
    label,
    tone = "active",
    ...rest
  }: HTMLAttributes<HTMLSpanElement> & { label: string; tone?: "active" | "others" | "stale" } = $props();
</script>

<span class="imported {tone}" {...rest}><span><Icon name="status-identified" size="10" box="9" /> {label}</span></span>

<style>
  .imported {
    display: flex;
    align-items: center;
    gap: var(--space-4);
    padding: var(--space-2) var(--space-7);
    border-radius: var(--r-4);
    background: var(--ls-bg-a78);
    font-size: var(--fs-10);
    white-space: nowrap;
  }

  .active {
    color: var(--ls-success);
  }

  .others {
    color: var(--ls-text-dim);
  }

  .stale {
    color: var(--ls-warning);
  }
</style>
