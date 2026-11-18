import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ASSETS, PROJECT } from "../../fixtures/library";
import { Selection } from "../../state/selection.svelte";
import { intentFor, type AssetIntent } from "./actions";
import type { AssetSummary } from "./model";
import GridHarness from "./test/GridHarness.svelte";

let app: ReturnType<typeof mount> | null = null;

afterEach(() => {
  if (app) unmount(app);
  app = null;
  document.body.innerHTML = "";
});

function render(options: { assets?: AssetSummary[]; columns?: number } = {}) {
  const selection = new Selection();
  const intents: AssetIntent[] = [];
  const onmenu = vi.fn();
  const ontag = vi.fn();
  const onpack = vi.fn();
  app = mount(GridHarness, {
    target: document.body,
    props: { assets: options.assets ?? ASSETS, selection, columns: options.columns ?? 5, onintent: (i: AssetIntent) => intents.push(i), onmenu, ontag, onpack },
  });
  flushSync();
  return { selection, intents, onmenu, ontag, onpack };
}

const grid = () => document.querySelector<HTMLElement>('[role="grid"]')!;
const cells = () => [...grid().querySelectorAll<HTMLElement>('[role="gridcell"]')];
const cell = (id: number) => grid().querySelector<HTMLElement>(`[role="gridcell"][data-id="${id}"]`)!;
const controls = (id: number) => [...cell(id).querySelectorAll<HTMLElement>("button, input")];
const tabStops = () => [...grid().querySelectorAll<HTMLElement>("*")].filter((el) => el.tabIndex >= 0);
const focusedCell = () => Number((document.activeElement as HTMLElement | null)?.closest<HTMLElement>('[role="gridcell"]')?.dataset.id);
const picks = (s: Selection) => [...s.picks].sort((a, b) => a - b);

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

describe("AssetGrid — structure", () => {
  it("is an ARIA grid whose cells state their own row and column, five to a row", () => {
    render();
    expect(grid().getAttribute("aria-multiselectable")).toBe("true");
    expect(grid().getAttribute("aria-rowcount")).toBe("4");
    expect(grid().getAttribute("aria-colcount")).toBe("5");
    expect(grid().querySelectorAll(':scope > [role="row"] > [role="gridcell"]').length).toBe(19);
    const place = (id: number) => [cell(id).getAttribute("aria-rowindex"), cell(id).getAttribute("aria-colindex")];
    expect([place(1), place(5), place(6), place(19)]).toEqual([
      ["1", "1"],
      ["1", "5"],
      ["2", "1"],
      ["4", "4"],
    ]);
  });

  it("a card is a container, never a button, and no button contains another control", () => {
    render();
    for (const c of cells()) expect(c.tagName).toBe("DIV");
    expect(grid().querySelectorAll("button button, button input, button a, a button").length).toBe(0);
    expect(grid().querySelectorAll('[role="button"]').length).toBe(0);
  });

  it("each card has exactly one primary action: its name, a real button", () => {
    render();
    for (const asset of ASSETS) {
      const open = cell(asset.id).querySelectorAll<HTMLButtonElement>("button.open");
      expect(open.length).toBe(1);
      expect(open[0].textContent).toBe(asset.name);
    }
  });
});

describe("AssetGrid — one tab stop", () => {
  it("has exactly one tabbable element: the first card, before anything is selected", () => {
    render();
    expect(tabStops()).toEqual([cell(1)]);
  });

  it("no control inside any card is ever a tab stop", () => {
    render();
    const inner = cells().flatMap((c) => [...c.querySelectorAll<HTMLElement>("button, input, a")]);
    // 19 cards carry well over a hundred controls between them — the old grid's ~500 tab stops.
    expect(inner.length).toBeGreaterThan(100);
    expect(inner.every((el) => el.tabIndex === -1)).toBe(true);
  });

  it("the tab stop is the lead when there is one", () => {
    const { selection } = render();
    selection.moveTo(7);
    flushSync();
    expect(tabStops()).toEqual([cell(7)]);
  });

  it("with no lead, the tab stop stays where focus last was", () => {
    render();
    cell(4).focus();
    flushSync();
    document.querySelector<HTMLElement>("#after")!.focus();
    flushSync();
    expect(tabStops()).toEqual([cell(4)]);
  });
});

describe("AssetGrid — arrows move the lead, and focus follows", () => {
  it("→ and ← step by one card", () => {
    const { selection } = render();
    cell(1).focus();
    press("ArrowRight");
    expect([selection.lead, focusedCell()]).toEqual([2, 2]);
    expect(tabStops()).toEqual([cell(2)]);
    press("ArrowLeft");
    press("ArrowLeft");
    expect([selection.lead, focusedCell()]).toEqual([1, 1]);
  });

  it("↓ and ↑ step by a whole row", () => {
    const { selection } = render();
    cell(2).focus();
    press("ArrowDown");
    expect([selection.lead, focusedCell()]).toEqual([7, 7]);
    press("ArrowDown");
    press("ArrowDown");
    expect(selection.lead).toBe(17);
    press("ArrowUp");
    expect([selection.lead, focusedCell()]).toEqual([12, 12]);
  });

  it("the rows are the column count it was given", () => {
    const { selection } = render({ columns: 4 });
    cell(2).focus();
    press("ArrowDown");
    expect(selection.lead).toBe(6);
  });

  it("Home and End go to the ends of the page", () => {
    const { selection } = render();
    cell(8).focus();
    press("End");
    expect([selection.lead, focusedCell()]).toEqual([19, 19]);
    press("Home");
    expect([selection.lead, focusedCell()]).toEqual([1, 1]);
  });

  it("Shift with an arrow extends the pick set from the lead", () => {
    const { selection } = render();
    cell(3).focus();
    press("ArrowRight");
    press("ArrowRight", { shiftKey: true });
    press("ArrowDown", { shiftKey: true });
    expect(selection.lead).toBe(10);
    expect(picks(selection)).toEqual([4, 5, 6, 7, 8, 9, 10]);
    expect(focusedCell()).toBe(10);
  });

  it("an arrow key the grid handles does not scroll the page or reach the global keymap", () => {
    render();
    cell(1).focus();
    expect(press("ArrowDown").defaultPrevented).toBe(true);
  });

  it("Enter makes the focused card the lead; Ctrl-A picks the page", () => {
    const { selection } = render();
    cell(5).focus();
    press("Enter");
    expect(selection.lead).toBe(5);
    press("a", { ctrlKey: true });
    expect(selection.picks.size).toBe(19);
  });
});

describe("AssetGrid — the pointer", () => {
  it("a click on a card selects it, and a second click deselects", () => {
    const { selection } = render();
    click(cell(3).querySelector("button.open")!);
    expect(selection.lead).toBe(3);
    expect(cell(3).getAttribute("aria-selected")).toBe("true");
    expect(document.activeElement).toBe(cell(3));
    click(cell(3).querySelector("button.open")!);
    expect(selection.lead).toBeNull();
  });

  it("Ctrl-click toggles the pick set; Shift-click picks the run", () => {
    const { selection } = render();
    click(cell(2).querySelector("button.open")!);
    click(cell(5).querySelector("button.open")!, { ctrlKey: true });
    expect([selection.lead, picks(selection)]).toEqual([2, [5]]);
    click(cell(8).querySelector("button.open")!, { shiftKey: true });
    expect(picks(selection)).toEqual([5, 6, 7, 8]);
  });

  it("the pick box toggles the pick set and leaves the lead alone", () => {
    const { selection } = render();
    const box = cell(4).querySelector<HTMLInputElement>('input[type="checkbox"]')!;
    box.click();
    flushSync();
    expect([selection.lead, picks(selection)]).toEqual([null, [4]]);
    expect(cell(4).getAttribute("aria-selected")).toBe("true");
  });

  it("a right-click asks for the menu at the pointer, for that asset", () => {
    const { onmenu, selection } = render();
    const event = new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 300, clientY: 200 });
    cell(6).dispatchEvent(event);
    expect(onmenu).toHaveBeenCalledWith(ASSETS[5], { event });
    expect(selection.lead).toBeNull();
  });
});

describe("AssetGrid — a card's own buttons", () => {
  const ctx = (asset: AssetSummary) => ({ targets: [asset], project: PROJECT });

  it("emit the intents actions.ts defines, and do not open the card", () => {
    const { intents, selection } = render();
    const [beaver] = ASSETS;
    click(cell(1).querySelector('[aria-label="Add to Favorites"]')!);
    click(cell(1).querySelector('[aria-label="Quick Look"]')!);
    click([...cell(1).querySelectorAll("button")].find((b) => b.textContent?.trim() === "+ stage")!);
    expect(intents).toEqual([intentFor("favorite", ctx(beaver)), intentFor("quick-look", ctx(beaver)), intentFor("stage", ctx(beaver))]);
    expect(selection.lead).toBeNull();
  });

  it("a sound's play button plays that sound", () => {
    const { intents } = render();
    click(cell(8).querySelector('[aria-label^="Play"]')!);
    expect(intents).toEqual([intentFor("play", ctx(ASSETS[7]))]);
  });

  it("a chip searches for its tag and the pack name opens its pack", () => {
    const { ontag, onpack, selection } = render();
    click([...cell(1).querySelectorAll("button")].find((b) => b.textContent?.includes("lowpoly"))!);
    expect(ontag).toHaveBeenCalledWith("lowpoly");
    click([...cell(1).querySelectorAll("button")].find((b) => b.textContent === "Forest Pack")!);
    expect(onpack).toHaveBeenCalledWith(ASSETS[0].package);
    expect(selection.lead).toBeNull();
  });
});

describe("AssetGrid — the keyboard and a card's buttons (the F2 pattern)", () => {
  it("F2 moves focus to the card's first control; arrows move among them; Escape returns", () => {
    render();
    cell(1).focus();
    press("F2");
    const inner = controls(1);
    expect(document.activeElement).toBe(inner[0]);
    press("ArrowRight");
    expect(document.activeElement).toBe(inner[1]);
    press("ArrowDown");
    expect(document.activeElement).toBe(inner[2]);
    press("ArrowLeft");
    expect(document.activeElement).toBe(inner[1]);
    press("End");
    expect(document.activeElement).toBe(inner[inner.length - 1]);
    press("ArrowRight");
    expect(document.activeElement).toBe(inner[inner.length - 1]);
    press("Home");
    expect(document.activeElement).toBe(inner[0]);
    const escape = press("Escape");
    expect(document.activeElement).toBe(cell(1));
    // Escape was the card's: it must not also clear the selection through the keymap.
    expect(escape.defaultPrevented).toBe(true);
  });

  it("the arrows inside a card do not move the lead", () => {
    const { selection } = render();
    cell(2).focus();
    press("ArrowRight");
    press("F2");
    press("ArrowRight");
    press("ArrowRight");
    expect(selection.lead).toBe(3);
    expect(focusedCell()).toBe(3);
  });

  it("F2 again returns to the card, and arrows move between cards once more", () => {
    const { selection } = render();
    cell(1).focus();
    press("F2");
    press("F2");
    expect(document.activeElement).toBe(cell(1));
    press("ArrowRight");
    expect(selection.lead).toBe(2);
  });

  it("the grid stays one tab stop throughout", () => {
    render();
    cell(1).focus();
    press("F2");
    press("ArrowRight");
    expect(tabStops()).toEqual([cell(1)]);
    expect(controls(1).every((el) => el.tabIndex === -1)).toBe(true);
  });

  it("after a mouse click on a card's button, arrows still move between cards", () => {
    const { selection } = render();
    const star = cell(3).querySelector<HTMLElement>('[aria-label="Add to Favorites"]')!;
    star.focus();
    click(star);
    press("ArrowRight");
    expect([selection.lead, focusedCell()]).toEqual([4, 4]);
  });

  it("Enter and Space on a card's button are the button's, not the card's", () => {
    const { intents, selection } = render();
    cell(1).focus();
    press("F2");
    press("ArrowRight");
    expect(press(" ").defaultPrevented).toBe(false);
    expect(press("Enter").defaultPrevented).toBe(false);
    expect(intents).toEqual([]);
    expect(selection.lead).toBeNull();
  });

  it("Space on a card quick-looks it, even before anything is selected", () => {
    const { intents } = render();
    cell(9).focus();
    const event = press(" ");
    expect(intents).toEqual([intentFor("quick-look", { targets: [ASSETS[8]], project: PROJECT })]);
    expect(event.defaultPrevented).toBe(true);
  });

  it("Shift+F10 and the Menu key ask for the menu at the card", () => {
    const { onmenu } = render();
    cell(2).focus();
    press("F10", { shiftKey: true });
    press("ContextMenu");
    expect(onmenu.mock.calls).toEqual([
      [ASSETS[1], { anchor: cell(2) }],
      [ASSETS[1], { anchor: cell(2) }],
    ]);
  });
});
