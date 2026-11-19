<script lang="ts" module>
  export interface ProvenanceLine {
    /** "Source", "License" — written before the value with a middle dot. */
    label?: string;
    value: string;
    /** `success` is the imported-here record, with its check. */
    tone?: "success";
    /** Makes the value a link (a pack's name opens its stats). */
    onclick?: () => void;
    title?: string;
  }
</script>

<script lang="ts">
  import type { HTMLAttributes } from "svelte/elements";
  import Icon from "./Icon.svelte";
  import TextLink from "./TextLink.svelte";

  /**
   * The quiet footer of the stat block (DESIGN-GUIDELINES §7 "Provenance & license"): where an
   * asset came from, its licence, and where it was imported — 11 px, dim, under a hairline; never
   * shouty. Selectable, because a source or a path is something you copy.
   */
  let { lines, ...rest }: HTMLAttributes<HTMLDivElement> & { lines: readonly ProvenanceLine[] } = $props();
</script>

<div class="prov" {...rest}>
  {#each lines as line (line.label ?? line.value)}
    <div class="line {line.tone ?? ''}">
      {#if line.tone === "success"}<Icon name="status-identified" size="11" box="9" />{" "}{/if}{#if line.label}{line.label} ·{" "}{/if}{#if line.onclick}<TextLink onclick={line.onclick} title={line.title}>{line.value}</TextLink>{:else}<span class="value">{line.value}</span>{/if}
    </div>
  {/each}
</div>

<style>
  .prov {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    padding-top: var(--space-10);
    border-top: 1px solid var(--ls-border);
    font-size: var(--fs-11);
    color: var(--ls-text-dim);
    user-select: text;
  }

  .line :global(.link),
  .value {
    color: var(--ls-text-muted);
  }

  .success,
  .success .value {
    color: var(--ls-success);
  }
</style>
