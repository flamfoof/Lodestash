<script lang="ts">
  import type { HTMLAttributes } from "svelte/elements";
  import Chip from "./Chip.svelte";

  /**
   * A tag on an asset, drawn by where it came from: a human tag (`source: "human"`) is the trait
   * chip in the trait's colour; a machine tag (`"ai"`) is the arcane outline with its confidence.
   * Provenance is shape and colour together (the bar), never colour alone.
   */
  let {
    name,
    source,
    confidence,
    color,
    onclick,
    ...rest
  }: HTMLAttributes<HTMLElement> & {
    name: string;
    source: "human" | "ai";
    confidence?: number;
    color?: string;
    onclick?: (event: MouseEvent) => void;
  } = $props();
</script>

<Chip
  label={name}
  tone={source === "ai" ? "ai" : "trait"}
  confidence={source === "ai" ? (confidence ?? 0.7) : undefined}
  color={source === "human" ? color : undefined}
  {onclick}
  {...rest}
/>
