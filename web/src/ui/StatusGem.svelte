<script lang="ts" module>
  export type Status = "unidentified" | "identified" | "confirmed";

  export const STATUS_LABEL: Record<Status, string> = {
    unidentified: "Unidentified",
    identified: "Identified",
    confirmed: "Confirmed",
  };
</script>

<script lang="ts">
  import type { HTMLAttributes } from "svelte/elements";
  import Icon from "./Icon.svelte";

  /**
   * An asset's identification status as its gem, in the status colour (DESIGN-GUIDELINES §6: colour
   * + gem + label, never colour alone — the label is the gem's accessible name).
   * `tile` is the 20 px tile on a thumbnail's corner; `inline` sits in a line of text.
   */
  let {
    status,
    variant = "tile",
    ...rest
  }: HTMLAttributes<HTMLSpanElement> & { status: Status; variant?: "tile" | "inline" } = $props();
</script>

<span class="gem {variant} {status}" role="img" aria-label={STATUS_LABEL[status]} title={STATUS_LABEL[status]} {...rest}>
  <Icon name="status-{status}" size={variant === "tile" ? "10" : "12"} />
</span>

<style>
  .gem {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex: none;
  }

  .tile {
    width: var(--space-20);
    height: var(--space-20);
    border-radius: var(--r-4);
    background: var(--ls-bg-a75);
    font-size: var(--fs-11);
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
