<script lang="ts">
  import type { HTMLAttributes } from "svelte/elements";
  import Icon from "./Icon.svelte";
  import { STATUS_LABEL, type Status } from "./StatusGem.svelte";

  /**
   * An asset's status in words: gem, label and an optional dim suffix ("◆ Confirmed · Model"), in
   * the status colour — colour, gem and label together (DESIGN-GUIDELINES §6). With `rule`, the
   * 2 px rule in the same colour, fading to nothing, is drawn under it: the stat block's header.
   */
  let {
    status,
    suffix,
    rule = false,
    ruleAttrs = {},
    ...rest
  }: HTMLAttributes<HTMLDivElement> & {
    status: Status;
    suffix?: string;
    rule?: boolean;
    ruleAttrs?: Record<`data-${string}`, string | undefined>;
  } = $props();
</script>

<div class="line {status}" {...rest}>
  <span><Icon name="status-{status}" size="11" box="11" /></span>
  <span>{STATUS_LABEL[status]}</span>
  {#if suffix}<span class="suffix">· {suffix}</span>{/if}
</div>
{#if rule}<div class="rule {status}" {...ruleAttrs}></div>{/if}

<style>
  .line {
    display: flex;
    align-items: center;
    gap: var(--space-6);
    margin-top: var(--space-4);
    font-size: var(--fs-11);
  }

  .suffix {
    color: var(--ls-text-dim);
  }

  .rule {
    height: var(--bw-2);
    margin-top: var(--space-8);
    border-radius: var(--r-2);
    background: linear-gradient(90deg, currentColor, transparent);
  }

  .unidentified {
    color: var(--ls-status-unidentified);
  }

  .identified {
    color: var(--ls-status-identified);
  }

  .confirmed {
    color: var(--ls-status-confirmed);
  }
</style>
