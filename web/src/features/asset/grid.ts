// Grid math, kept out of the component so it can be tested without a layout engine.

/**
 * How many columns `repeat(auto-fill, minmax(minCard, 1fr))` makes in `width` — the same
 * arithmetic the browser does, so the grid's keyboard rows are its drawn rows.
 */
export function columnsFor(width: number, minCard: number, gap: number): number {
  if (!(width > 0) || !(minCard > 0)) return 1;
  return Math.max(1, Math.floor((width + gap) / (minCard + gap)));
}

/** `items` cut into rows of `columns`. */
export function rowsOf<T>(items: readonly T[], columns: number): T[][] {
  const size = Math.max(1, Math.floor(columns));
  const rows: T[][] = [];
  for (let i = 0; i < items.length; i += size) rows.push(items.slice(i, i + size));
  return rows;
}

/**
 * Where a navigation key takes focus from `index` in a grid of `count` items, `columns` wide.
 * ← → step by one; ↑ ↓ by a row (↓ from the last full row lands on the last item when the row
 * below is short); Home and End go to the ends. Returns null for a key that does not navigate.
 */
export function moveIndex(index: number, key: string, columns: number, count: number): number | null {
  if (count <= 0) return null;
  const last = count - 1;
  const at = Math.min(Math.max(index, 0), last);
  const cols = Math.max(1, Math.floor(columns));
  switch (key) {
    case "ArrowLeft":
      return Math.max(0, at - 1);
    case "ArrowRight":
      return Math.min(last, at + 1);
    case "ArrowUp":
      return at - cols >= 0 ? at - cols : at;
    case "ArrowDown":
      if (at + cols <= last) return at + cols;
      return Math.floor(at / cols) < Math.floor(last / cols) ? last : at;
    case "Home":
      return 0;
    case "End":
      return last;
    default:
      return null;
  }
}
