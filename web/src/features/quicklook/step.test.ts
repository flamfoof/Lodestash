import { describe, expect, it } from "vitest";
import { stepId } from "./step";

describe("stepId — Quick Look's ← →", () => {
  const order = [4, 9, 2, 7];

  it("steps through the set in the order shown", () => {
    expect(stepId(order, 4, 1)).toBe(9);
    expect(stepId(order, 2, -1)).toBe(9);
  });

  it("is a ring: past the last is the first, before the first is the last", () => {
    expect(stepId(order, 7, 1)).toBe(4);
    expect(stepId(order, 4, -1)).toBe(7);
    expect(stepId([5], 5, 1)).toBe(5);
  });

  it("steps from an end when the current asset has left the set", () => {
    expect(stepId(order, 99, 1)).toBe(4);
    expect(stepId(order, null, -1)).toBe(7);
  });

  it("has nowhere to go in an empty set", () => {
    expect(stepId([], 4, 1)).toBeNull();
  });
});
