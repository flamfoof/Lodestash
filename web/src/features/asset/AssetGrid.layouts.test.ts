// The list view keeps the grid's selection and roving-focus model — proved by running one set of
// expectations over both layouts. What differs between them is stated once, in STEP: an arrow's
// reach.
import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ASSETS, PROJECT } from "../../fixtures/library";
import { Selection } from "../../state/selection.svelte";
import { intentFor, type AssetIntent } from "./actions";
import GridHarness from "./test/GridHarness.svelte";

let app: ReturnType<typeof mount> | null = null;

afterEach(() => {
  if (app) unmount(app);
  app = null;
  document.body.innerHTML = "";
});

type Layout = "grid" | "list";

function render(layout: Layout) {
  const selection = new Selection();
  const intents: AssetIntent[] = [];
  const onmenu = vi.fn();
  const onpack = vi.fn();
  app = mount(GridHarness, {
    target: document.body,
    props: { assets: ASSETS, selection, columns: 5, layout, onintent: (i: AssetIntent) => intents.push(i), onmenu, onpack },
  });
  flushSync();
  return { selection, intents, onmenu, onpack };
}

const grid = () => document.querySelector<HTMLElement>('[role="grid"]')!;
const cell = (id: number) => grid().querySelector<HTMLElement>(`[role="gridcell"][data-id="${id}"]`)!;
const controls = (id: number) => [...cell(id).querySelectorAll<HTMLElement>("button, input")];
const tabStops = () => [...grid().querySelectorAll<HTMLElement>("*")].filter((el) => el.tabIndex >= 0);
const focusedCell = () => Number((document.activeElement as HTMLElement | null)?.closest<HTMLElement>('[role="gridcell"]')?.dataset.id);
const picks = (s: Selection) => [...s.picks].sort((a, b) => a - b);
const target = (id: number) => ({ targets: ASSETS.filter((a) => a.id === id), project: PROJECT });

function press(key: string, init: KeyboardEventInit = {}) {
  const event = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init });
  (document.activeElement ?? document.body).dispatchEvent(event);
  flushSync();
  return event;
}

function click(el: Element, init: MouseEventInit = {}) {
  el.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, ...init }));
  flushSync();
}

/** Where ↓ lands from the first asset: a row of five cards below, or the next row of the list. */
const STEP: Record<Layout, { down: number; colcount: string; rowcount: string }> = {
  grid: { down: 6, colcount: "5", rowcount: "4" },
  list: { down: 2, colcount: "1", rowcount: "19" },
};

describe.each<Layout>(["grid", "list"])("AssetGrid as a %s — one selection and focus model", (layout) => {
  it("is one ARIA grid of container cells, each with one primary action", () => {
    render(layout);
    expect(grid().getAttribute("aria-colcount")).toBe(STEP[layout].colcount);
    expect(grid().getAttribute("aria-rowcount")).toBe(STEP[layout].rowcount);
    expect(grid().querySelectorAll('[role="row"] > [role="gridcell"]').length).toBe(19);
    expect(grid().querySelectorAll("button button, button input, button a, a button").length).toBe(0);
    expect(cell(3).tagName).toBe("DIV");
    expect(cell(3).querySelectorAll("button.open").length).toBe(1);
    expect(cell(3).querySelector("button.open")?.textContent).toBe("Sword_Iron_02");
  });

  it("is one tab stop, and nothing inside a cell ever is", () => {
    const { selection } = render(layout);
    expect(tabStops().map((el) => el.dataset.id)).toEqual(["1"]);
    selection.moveTo(7);
    flushSync();
    expect(tabStops().map((el) => el.dataset.id)).toEqual(["7"]);
    for (const control of grid().querySelectorAll<HTMLElement>("button, input, a")) expect(control.tabIndex).toBe(-1);
  });

  it("arrows move the lead and focus follows it", () => {
    const { selection } = render(layout);
    cell(1).focus();
    press("ArrowRight");
    expect([selection.lead, focusedCell()]).toEqual([2, 2]);
    press("ArrowLeft");
    press("ArrowDown");
    expect([selection.lead, focusedCell()]).toEqual([STEP[layout].down, STEP[layout].down]);
    press("ArrowUp");
    expect(selection.lead).toBe(1);
    press("End");
    expect([selection.lead, focusedCell()]).toEqual([19, 19]);
    press("Home");
    expect(selection.lead).toBe(1);
    expect(selection.picks.size).toBe(0);
  });

  it("Shift with an arrow or a click extends the pick set; Ctrl-A picks the page", () => {
    const { selection } = render(layout);
    cell(2).focus();
    press("Enter");
    expect(selection.lead).toBe(2);
    press("ArrowRight", { shiftKey: true });
    press("ArrowRight", { shiftKey: true });
    expect([picks(selection), selection.lead]).toEqual([[2, 3, 4], 4]);
    click(cell(7).querySelector("button.open")!, { shiftKey: true });
    expect(picks(selection)).toEqual([2, 3, 4, 5, 6, 7]);
    press("a", { ctrlKey: true });
    expect(selection.picks.size).toBe(19);
  });

  it("a click selects and a second click deselects; Ctrl-click and the pick box change picks only", () => {
    const { selection } = render(layout);
    const name = cell(4).querySelector("button.open")!;
    click(name);
    expect([selection.lead, focusedCell()]).toEqual([4, 4]);
    click(name);
    expect(selection.lead).toBeNull();
    click(cell(5).querySelector("button.open")!, { ctrlKey: true });
    click(cell(8).querySelector('input[type="checkbox"]')!);
    expect([picks(selection), selection.lead]).toEqual([[5, 8], null]);
    expect(cell(8).getAttribute("aria-selected")).toBe("true");
  });

  it("F2 enters a cell's controls, arrows step through them, Escape returns — still one tab stop", () => {
    const { selection } = render(layout);
    cell(2).focus();
    press("F2");
    const inner = controls(2);
    expect(document.activeElement).toBe(inner[0]);
    press("ArrowRight");
    expect(document.activeElement).toBe(inner[1]);
    press("End");
    expect(document.activeElement).toBe(inner[inner.length - 1]);
    expect(selection.lead).toBeNull();
    expect(tabStops().length).toBe(1);
    press("Escape");
    expect(document.activeElement).toBe(cell(2));
    press("ArrowRight");
    expect(focusedCell()).toBe(3);
  });

  it("a cell's buttons emit the intents actions.ts defines, for that asset, and do not open it", () => {
    const { intents, selection } = render(layout);
    click(cell(2).querySelector('[aria-label="Add to Favorites"]')!);
    click(cell(2).querySelector('[aria-label="Quick Look"]')!);
    click([...cell(2).querySelectorAll("button")].find((b) => b.textContent?.trim() === "+ stage")!);
    expect(intents).toEqual([intentFor("favorite", target(2)), intentFor("quick-look", target(2)), intentFor("stage", target(2))]);
    expect(selection.lead).toBeNull();
  });

  it("Space on a cell quick-looks it; the Menu key asks for the menu at the cell; the pack opens its pack", () => {
    const { intents, onmenu, onpack } = render(layout);
    cell(9).focus();
    press(" ");
    expect(intents).toEqual([intentFor("quick-look", target(9))]);
    press("ContextMenu");
    expect(onmenu.mock.calls[0][0].id).toBe(9);
    expect(onmenu.mock.calls[0][1].anchor).toBe(cell(9));
    click([...cell(9).querySelectorAll("button")].find((b) => b.textContent?.trim() === "RPG SFX")!);
    expect(onpack).toHaveBeenCalledWith({ id: 5, name: "RPG SFX" });
  });
});
