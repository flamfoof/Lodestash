// Stepping Quick Look through the visible set (UI-SPEC §0.4: "← → step prev/next within the
// visible set"). Pure, so the ends are a decision made once.

/**
 * The id `dir` steps from `current` in `order`. The set is a ring: stepping past the last asset
 * lands on the first, as the prototype's does — the keys never go dead at an end. An id that is no
 * longer in the set steps from the start; an empty set has nowhere to go.
 */
export function stepId(order: readonly number[], current: number | null, dir: 1 | -1): number | null {
  if (order.length === 0) return null;
  const at = current === null ? -1 : order.indexOf(current);
  if (at < 0) return dir === 1 ? order[0] : order[order.length - 1];
  return order[(at + dir + order.length) % order.length];
}
