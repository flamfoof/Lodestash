import { vocabulary } from "./api";

/**
 * The trait → chip-colour map (B8a). `set_trait_style` gives a trait its own colour, and every
 * chip that renders that trait — the ⚑ cloud, the card tag row, the Traits screen — reads it
 * from here so one setting paints the whole app.
 *
 * A colour is stored as a **CSS custom-property name** (e.g. `--ls-type-sound`); chips render it
 * as `var(<name>)`. Tokens, not hex: the design system owns the palette, and a chip in a stale
 * hex that outlives a token change is exactly the drift this repo avoids.
 */
export const traitColor = $state<Map<string, string>>(new Map());

/** Re-ask the vocabulary and rebuild the map. Called at startup and after any vocabulary write. */
export async function refreshTraitColors() {
  try {
    const rows = await vocabulary();
    const next = new Map<string, string>();
    for (const t of rows) {
      if (t.color) next.set(t.value, t.color);
    }
    traitColor.clear();
    for (const [k, v] of next) traitColor.set(k, v);
  } catch {
    // A dead backend leaves chips uncoloured — honest and harmless.
  }
}
