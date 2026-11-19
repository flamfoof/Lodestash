import { describe, expect, it } from "vitest";
import { columnsFor, moveIndex } from "./grid";

describe("columnsFor", () => {
  it("matches auto-fill at the prototype's library width for the three grid sizes", () => {
    // 1197 px is the grid's width at 1440 × 900 with no inspector; the gap is 12 px.
    expect([150, 205, 288].map((min) => columnsFor(1197, min, 12))).toEqual([7, 5, 4]);
    // 896 px with the inspector open.
    expect([150, 205, 288].map((min) => columnsFor(896, min, 12))).toEqual([5, 4, 3]);
  });

  it("changes exactly where one more card and its gap fit", () => {
    expect(columnsFor(205 * 3 + 12 * 2 - 0.5, 205, 12)).toBe(2);
    expect(columnsFor(205 * 3 + 12 * 2, 205, 12)).toBe(3);
  });

  it("is never less than one, whatever it is given", () => {
    expect(columnsFor(100, 205, 12)).toBe(1);
    expect(columnsFor(0, 205, 12)).toBe(1);
    expect(columnsFor(Number.NaN, 205, 12)).toBe(1);
    expect(columnsFor(1197, 0, 12)).toBe(1);
  });
});

describe("moveIndex", () => {
  // 19 items, 5 columns: rows of 5, 5, 5, 4.
  const move = (index: number, key: string) => moveIndex(index, key, 5, 19);

  it("← → step by one and stop at the ends", () => {
    expect(move(3, "ArrowRight")).toBe(4);
    expect(move(4, "ArrowRight")).toBe(5);
    expect(move(18, "ArrowRight")).toBe(18);
    expect(move(0, "ArrowLeft")).toBe(0);
  });

  it("↑ ↓ step by a whole row", () => {
    expect(move(7, "ArrowDown")).toBe(12);
    expect(move(7, "ArrowUp")).toBe(2);
    expect(move(2, "ArrowUp")).toBe(2);
  });

  it("↓ into a short last row lands on its last item; from the last row it stays", () => {
    expect(move(13, "ArrowDown")).toBe(18);
    expect(move(14, "ArrowDown")).toBe(18);
    expect(move(16, "ArrowDown")).toBe(16);
  });

  it("Home and End go to the ends", () => {
    expect(move(7, "Home")).toBe(0);
    expect(move(7, "End")).toBe(18);
  });

  it("answers null for a key that does not navigate, and for an empty grid", () => {
    expect(move(7, "Enter")).toBeNull();
    expect(moveIndex(0, "ArrowRight", 5, 0)).toBeNull();
  });

  it("is a list with one column", () => {
    expect(moveIndex(2, "ArrowDown", 1, 4)).toBe(3);
    expect(moveIndex(2, "ArrowUp", 1, 4)).toBe(1);
  });
});
