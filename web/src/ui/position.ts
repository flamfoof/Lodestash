// Place a floating layer (menu, popover, tooltip) next to its anchor by measuring both — never by
// assuming a height (spec §9.3 "positioned by measurement").

export interface Box {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface Placement {
  x: number;
  y: number;
  /** True when the layer opened above its anchor because there was no room below. */
  above: boolean;
}

export interface PlaceOptions {
  /** Horizontal alignment to the anchor: its left edge (`start`) or its right edge (`end`). */
  align?: "start" | "end" | "center";
  /** Gap between anchor and layer. The prototype opens its popovers 4 px below a trigger. */
  gap?: number;
  /** Minimum distance kept from the viewport edge. */
  margin?: number;
}

/**
 * Below the anchor if the layer fits there, otherwise above it if it fits there, otherwise on the
 * side with more room (clamped into the viewport). Horizontally aligned as asked, then clamped.
 */
export function place(anchor: Box, layer: { width: number; height: number }, viewport: { width: number; height: number }, options: PlaceOptions = {}): Placement {
  const { align = "start", gap = 4, margin = 8 } = options;
  const below = anchor.y + anchor.height + gap;
  const aboveY = anchor.y - gap - layer.height;
  const roomBelow = viewport.height - margin - below;
  const roomAbove = anchor.y - gap - margin;

  let y: number;
  let above = false;
  if (layer.height <= roomBelow) y = below;
  else if (layer.height <= roomAbove) {
    y = aboveY;
    above = true;
  } else if (roomAbove > roomBelow) {
    y = margin;
    above = true;
  } else y = Math.max(margin, viewport.height - margin - layer.height);

  let x =
    align === "end" ? anchor.x + anchor.width - layer.width : align === "center" ? anchor.x + (anchor.width - layer.width) / 2 : anchor.x;
  x = Math.min(x, viewport.width - margin - layer.width);
  x = Math.max(margin, x);
  return { x: Math.round(x), y: Math.round(y), above };
}

export function boxOf(el: Element): Box {
  const r = el.getBoundingClientRect();
  return { x: r.left, y: r.top, width: r.width, height: r.height };
}

export function viewportBox(): { width: number; height: number } {
  return { width: window.innerWidth, height: window.innerHeight };
}
