<script lang="ts">
  import type { HTMLButtonAttributes } from "svelte/elements";
  import type { Tier } from "./SizeTier.svelte";

  /**
   * A value in the ⚑ Filters popover: a toggle (`aria-pressed`), outlined and muted until chosen.
   * Each group has its own on-state, so a glance says which axis a chosen chip is on — and the
   * shape says it too, never the colour alone (DESIGN-GUIDELINES §9):
   * - `trait` — a pill; on, the arcane outline of a machine tag;
   * - `tris` — a square-cornered mono figure; on, the model type's violet;
   * - `size` — a tier letter with its range in mono beside it; on, the tier's own colour.
   * `detail` is the size chip's range ("1.0 MB–4.0 MB"); `tierAttrs` / `detailAttrs` reach its two
   * parts (the contract gate marks them).
   */
  let {
    label,
    kind = "trait",
    selected = false,
    tier,
    detail,
    tierAttrs = {},
    detailAttrs = {},
    ...rest
  }: HTMLButtonAttributes & {
    label: string;
    kind?: "trait" | "tris" | "size";
    selected?: boolean;
    /** `size` only: which tier's colour the on-state takes. */
    tier?: Tier;
    detail?: string;
    tierAttrs?: Record<`data-${string}`, string | undefined>;
    detailAttrs?: Record<`data-${string}`, string | undefined>;
  } = $props();
</script>

<button type="button" class="chip {kind} {tier ? `t${tier}` : ''}" class:selected aria-pressed={selected} {...rest}>
  {#if kind === "size"}
    <b class="tier" {...tierAttrs}>{label}</b>{#if detail}<span class="detail" {...detailAttrs}>{detail}</span>{/if}
  {:else}
    {label}
  {/if}
</button>

<style>
  .chip {
    flex: none;
    padding: var(--space-2) var(--space-9);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-pill);
    font-size: var(--fs-11);
    color: var(--ls-text-muted);
    white-space: nowrap;
    transition: border-color var(--dur-120) ease;
  }

  .tris,
  .size {
    border-radius: var(--r-6);
  }

  .tris {
    font-family: var(--ff-mono);
  }

  .size {
    display: flex;
    align-items: center;
    gap: var(--space-5);
  }

  .tier {
    font-weight: var(--fw-700);
  }

  .detail {
    font-family: var(--ff-mono);
    font-size: var(--fs-9);
    opacity: var(--op-85);
  }

  .trait:hover {
    border-color: var(--ls-arcane);
  }

  .tris:hover {
    border-color: var(--ls-violet);
  }

  .size:hover {
    border-color: var(--ls-text-dim);
  }

  .trait.selected {
    color: var(--ls-arcane-bright);
    border-color: var(--ls-arcane);
    background: var(--ls-arcane-a16);
  }

  .tris.selected {
    color: var(--ls-violet-bright);
    border-color: var(--ls-violet);
    background: var(--ls-violet-a16);
  }

  /* A chosen size chip takes its tier's colour, at the tint the prototype derives from it. */
  .size.selected.tS {
    color: var(--ls-success);
    border-color: var(--ls-success);
    background: var(--ls-success-a15);
  }

  .size.selected.tM {
    color: var(--ls-type-sound);
    border-color: var(--ls-type-sound);
    background: var(--ls-type-sound-a15);
  }

  .size.selected.tL {
    color: var(--ls-warning);
    border-color: var(--ls-warning);
    background: var(--ls-warning-a15);
  }

  .size.selected.tXL {
    color: var(--ls-danger);
    border-color: var(--ls-danger);
    background: var(--ls-danger-a15);
  }
</style>
