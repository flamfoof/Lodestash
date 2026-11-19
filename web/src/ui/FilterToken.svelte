<script lang="ts" module>
  /**
   * Which axis a constraint is on, as a colour: `gold` the search query and ★; a media type's or a
   * status's own token; `violet` triangles; a size tier's (`S` `M` `L` `XL`); `neutral` a library,
   * a pack, a collection or a trait.
   */
  export type TokenTone =
    | "gold"
    | "neutral"
    | "violet"
    | "model"
    | "material"
    | "sound"
    | "image"
    | "unidentified"
    | "identified"
    | "confirmed"
    | "S"
    | "M"
    | "L"
    | "XL";
</script>

<script lang="ts">
  import type { HTMLAttributes } from "svelte/elements";
  import Icon from "./Icon.svelte";
  import type { IconName } from "./icons.generated";

  /**
   * One active constraint in the active-filter bar (UI-SPEC §2 item 7): an outlined pill — the
   * prototype's pack and collection pills — with a ✕ that removes it. The colour says which axis
   * it is on and something else always says it too: an `icon` (the query's magnifier, a status
   * gem), a rotated `diamond` (a media type, as under the type tabs), or a mono `prefix`
   * (`lib`, `pack`).
   */
  let {
    label,
    tone = "neutral",
    icon,
    diamond = false,
    prefix,
    removeLabel,
    onremove,
    ...rest
  }: HTMLAttributes<HTMLSpanElement> & {
    label: string;
    tone?: TokenTone;
    icon?: IconName;
    diamond?: boolean;
    prefix?: string;
    /** The ✕'s accessible name; "Remove filter: <label>" unless given. */
    removeLabel?: string;
    onremove?: () => void;
  } = $props();
</script>

<span class="token {tone}" {...rest}>
  {#if icon}<Icon name={icon} size="10" />{/if}
  {#if diamond}<i class="diamond" aria-hidden="true"></i>{/if}
  {#if prefix}<span class="prefix">{prefix}</span>{/if}
  <span class="label">{label}</span>
  <button type="button" class="remove" aria-label={removeLabel ?? `Remove filter: ${prefix ? `${prefix} ` : ""}${label}`} title="Remove this filter" onclick={onremove}>
    <Icon name="action-clear" size="9" />
  </button>
</span>

<style>
  .token {
    display: inline-flex;
    align-items: center;
    flex: none;
    gap: var(--space-6);
    padding: var(--space-2) var(--space-9);
    border: 1px solid color-mix(in srgb, var(--tone) 45%, transparent);
    border-radius: var(--r-pill);
    font-size: var(--fs-11);
    color: var(--tone);
    white-space: nowrap;
  }

  .neutral {
    --tone: var(--ls-text-muted);
    border-color: var(--ls-border-strong);
  }

  .gold {
    --tone: var(--ls-gold);
  }

  .violet {
    --tone: var(--ls-violet-bright);
  }

  .model {
    --tone: var(--ls-type-model);
  }

  .material {
    --tone: var(--ls-type-material);
  }

  .sound {
    --tone: var(--ls-type-sound);
  }

  .image {
    --tone: var(--ls-type-image);
  }

  .unidentified {
    --tone: var(--ls-status-unidentified);
  }

  .identified {
    --tone: var(--ls-status-identified);
  }

  .confirmed {
    --tone: var(--ls-status-confirmed);
  }

  .S {
    --tone: var(--ls-success);
  }

  .M {
    --tone: var(--ls-type-sound);
  }

  .L {
    --tone: var(--ls-warning);
  }

  .XL {
    --tone: var(--ls-danger);
  }

  .diamond {
    width: var(--space-7);
    height: var(--space-7);
    flex: none;
    border-radius: var(--r-1);
    background: currentColor;
    transform: rotate(45deg);
  }

  .prefix {
    font-family: var(--ff-mono);
    font-size: var(--fs-9);
    color: var(--ls-text-dim);
  }

  .label {
    max-width: var(--token-label-max-w);
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .remove {
    display: flex;
    color: var(--ls-text-muted);
    transition: color var(--dur-120) ease;
  }

  .remove:hover {
    color: var(--ls-text);
  }
</style>
