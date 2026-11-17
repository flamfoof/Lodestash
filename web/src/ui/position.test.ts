import { describe, expect, it } from "vitest";
import { place } from "./position";

const viewport = { width: 1440, height: 900 };
const layer = { width: 180, height: 200 };

describe("place", () => {
  it("opens below the anchor when it fits", () => {
    const p = place({ x: 100, y: 100, width: 80, height: 30 }, layer, viewport);
    expect(p).toEqual({ x: 100, y: 134, above: false });
  });

  it("flips above when it would cross the bottom edge", () => {
    const p = place({ x: 100, y: 800, width: 80, height: 30 }, layer, viewport);
    expect(p).toEqual({ x: 100, y: 596, above: true });
  });

  it("takes the roomier side, clamped, when it fits neither", () => {
    const tall = { width: 180, height: 700 };
    const p = place({ x: 100, y: 500, width: 80, height: 30 }, tall, viewport);
    expect(p.above).toBe(true);
    expect(p.y).toBe(8);
  });

  it("aligns to the anchor's right edge and stays inside the viewport", () => {
    expect(place({ x: 1000, y: 10, width: 80, height: 30 }, layer, viewport, { align: "end" }).x).toBe(900);
    expect(place({ x: 1400, y: 10, width: 30, height: 30 }, layer, viewport).x).toBe(1440 - 8 - 180);
    expect(place({ x: 0, y: 10, width: 30, height: 30 }, layer, viewport, { align: "end" }).x).toBe(8);
  });
});
