// Runes are allowed here (the file name carries .svelte): these tests change the grid's props.
import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it } from "vitest";
import { ASSETS } from "../../fixtures/library";
import { Selection } from "../../state/selection.svelte";
import type { AssetSummary } from "./model";
import GridHarness from "./test/GridHarness.svelte";

let app: ReturnType<typeof mount> | null = null;

afterEach(() => {
  if (app) unmount(app);
  app = null;
  document.body.innerHTML = "";
});

function render(columns = 5) {
  const selection = new Selection();
  const props = $state({ assets: ASSETS as AssetSummary[], selection, columns });
  app = mount(GridHarness, { target: document.body, props });
  flushSync();
  return { props, selection };
}

const cell = (id: number) => document.querySelector<HTMLElement>(`[role="gridcell"][data-id="${id}"]`)!;
const place = (id: number) => [cell(id).getAttribute("aria-rowindex"), cell(id).getAttribute("aria-colindex")];

describe("AssetGrid — the column count changes under the keyboard", () => {
  // Opening the inspector narrows the grid, and one arrow key opens the inspector.
  it("keeps every card's element, so the focused card keeps focus", () => {
    const { props } = render(5);
    const before = cell(5);
    before.focus();
    props.columns = 4;
    flushSync();
    expect(cell(5)).toBe(before);
    expect(document.activeElement).toBe(before);
    expect(place(5)).toEqual(["2", "1"]);
    expect(document.querySelector('[role="grid"]')?.getAttribute("aria-rowcount")).toBe("5");
  });

  it("keeps a card's own state: an expanded chip row stays expanded", () => {
    const { props } = render(5);
    const more = [...cell(1).querySelectorAll<HTMLElement>(".chips button")].at(-1)!;
    more.click();
    flushSync();
    expect(cell(1).querySelectorAll(".chips .chip").length).toBe(5);
    props.columns = 3;
    flushSync();
    expect(cell(1).querySelectorAll(".chips .chip").length).toBe(5);
  });

  it("arrows use the new count at once", () => {
    const { props, selection } = render(5);
    cell(2).focus();
    props.columns = 4;
    flushSync();
    document.activeElement!.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true, cancelable: true }));
    flushSync();
    expect(selection.lead).toBe(6);
  });
});

describe("AssetGrid — the focused card leaves the page", () => {
  it("moves focus to the card now in its place instead of dropping it on the document", () => {
    const { props } = render();
    cell(3).focus();
    props.assets = ASSETS.filter((a) => a.id !== 3);
    flushSync();
    expect(document.querySelector('[role="gridcell"][data-id="3"]')).toBeNull();
    expect(document.activeElement).toBe(cell(4));
    // …and that card is the grid's one tab stop.
    expect([...document.querySelectorAll<HTMLElement>('[role="gridcell"]')].filter((c) => c.tabIndex === 0)).toEqual([cell(4)]);
  });

  it("the last card going away leaves focus on the new last card", () => {
    const { props } = render();
    cell(19).focus();
    props.assets = ASSETS.filter((a) => a.id !== 19);
    flushSync();
    expect(document.activeElement).toBe(cell(18));
  });

  it("does not take focus when focus was elsewhere", () => {
    const { props } = render();
    const outside = document.querySelector<HTMLElement>("#after")!;
    outside.focus();
    props.assets = ASSETS.filter((a) => a.id !== 3);
    flushSync();
    expect(document.activeElement).toBe(outside);
  });

  it("does not take focus later, when only the lead changes", () => {
    const { props, selection } = render();
    cell(3).focus();
    props.assets = ASSETS.filter((a) => a.id !== 3);
    flushSync();
    const outside = document.querySelector<HTMLElement>("#after")!;
    outside.focus();
    selection.moveTo(9);
    flushSync();
    expect(document.activeElement).toBe(outside);
  });
});
