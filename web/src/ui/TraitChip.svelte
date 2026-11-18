<script lang="ts">
  import type { HTMLAttributes } from "svelte/elements";
  import Chip from "./Chip.svelte";

  /**
   * A tag on an asset, drawn by where it came from: a human tag (`source: "human"`) is the trait
   * chip in the trait's colour; a machine tag (`"ai"`) is the arcane outline with its confidence;
   * an importer's tag (`"imported"`) is neutral slate. Provenance is shape and colour together (the
   * bar), never colour alone. `icon` is a library trait's own mark, written before its name.
   */
  let {
    name,
    source,
    confidence,
    color,
    icon,
    size = "sm",
    onclick,
    ...rest
  }: HTMLAttributes<HTMLElement> & {
    name: string;
    source: "human" | "ai" | "imported";
    icon?: string;
    /** `sm` on a card, `md` in the inspector. */
    size?: "sm" | "md";
    confidence?: number;
    color?: string;
    onclick?: (event: MouseEvent) => void;
  } = $props();
</script>

<Chip
  label={icon ? `${icon} ${name}` : name}
  tone={source === "ai" ? "ai" : source === "imported" ? "imported" : "trait"}
  confidence={source === "ai" ? (confidence ?? 0.7) : undefined}
  color={source === "human" ? color : undefined}
  {size}
  {onclick}
  {...rest}
/>
