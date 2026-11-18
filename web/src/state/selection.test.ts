import { describe, expect, it } from "vitest";
import { Selection } from "./selection.svelte";

const ORDER = [1, 2, 3, 4, 5, 6];
const picks = (s: Selection) => [...s.picks].sort((a, b) => a - b);

describe("Selection", () => {
  it("a click selects the lead, and a second click on it deselects", () => {
    const s = new Selection();
    s.open(3);
    expect(s.lead).toBe(3);
    s.open(4);
    expect(s.lead).toBe(4);
    s.open(4);
    expect(s.lead).toBeNull();
  });

  it("moving the lead never changes the pick set", () => {
    const s = new Selection();
    s.toggle(2);
    s.toggle(5);
    s.moveTo(3);
    s.open(4);
    expect(s.lead).toBe(4);
    expect(picks(s)).toEqual([2, 5]);
  });

  it("the pick box toggles one asset in and out", () => {
    const s = new Selection();
    s.toggle(2);
    s.toggle(3);
    s.toggle(2);
    expect(picks(s)).toEqual([3]);
    expect(s.picking).toBe(true);
    s.toggle(3);
    expect(s.picking).toBe(false);
  });

  it("Shift extends the pick set from where the lead was, and the lead follows", () => {
    const s = new Selection();
    s.moveTo(3);
    s.extendTo(4, ORDER);
    expect([picks(s), s.lead]).toEqual([[3, 4], 4]);
    s.extendTo(5, ORDER);
    expect([picks(s), s.lead]).toEqual([[3, 4, 5], 5]);
    // Back across the anchor: the range is always anchor…target.
    s.extendTo(1, ORDER);
    expect([picks(s), s.lead]).toEqual([[1, 2, 3], 1]);
  });

  it("Shift with nothing selected picks just that asset", () => {
    const s = new Selection();
    s.extendTo(4, ORDER);
    expect([picks(s), s.lead]).toEqual([[4], 4]);
    s.extendTo(99, ORDER);
    expect(picks(s)).toEqual([4]);
  });

  it("Ctrl-A picks the page it is given, no more", () => {
    const s = new Selection();
    s.pickAll([1, 2, 3]);
    expect(picks(s)).toEqual([1, 2, 3]);
  });

  it("Escape drops the pick set first, then the lead, then reports nothing left", () => {
    const s = new Selection();
    s.moveTo(2);
    s.toggle(4);
    expect(s.clear()).toBe(true);
    expect([picks(s), s.lead]).toEqual([[], 2]);
    expect(s.clear()).toBe(true);
    expect(s.lead).toBeNull();
    expect(s.clear()).toBe(false);
  });

  it("an action on a picked asset targets the whole pick set; on an unpicked one, that asset", () => {
    const s = new Selection();
    s.toggle(2);
    s.toggle(3);
    s.toggle(5);
    expect(s.targets(3).sort()).toEqual([2, 3, 5]);
    expect(s.targets(4)).toEqual([4]);
  });

  it("a key targets the pick set, or with none, the lead", () => {
    const s = new Selection();
    expect(s.targets()).toEqual([]);
    s.moveTo(6);
    expect(s.targets()).toEqual([6]);
    s.toggle(1);
    expect(s.targets()).toEqual([1]);
  });

  it("follows the filter: ids no longer shown are forgotten", () => {
    const s = new Selection();
    s.moveTo(6);
    s.toggle(1);
    s.toggle(6);
    s.prune([1, 2, 3]);
    expect([picks(s), s.lead]).toEqual([[1], null]);
    // The anchor went with the lead: a Shift-click starts a fresh range.
    s.extendTo(3, [1, 2, 3]);
    expect(picks(s)).toEqual([3]);
  });
});
