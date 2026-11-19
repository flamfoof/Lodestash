// Runes are allowed here (the file name carries .svelte): one test changes the grid's layout prop.
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

function render(assets: AssetSummary[] = ASSETS, layout: "grid" | "list" = "list") {
  const selection = new Selection();
  const props = $state({ assets, selection, layout });
  app = mount(GridHarness, { target: document.body, props });
  flushSync();
  return { props, selection };
}

const row = (id: number) => document.querySelector<HTMLElement>(`[role="gridcell"][data-id="${id}"]`)!;
const text = (el: Element | null) => el?.textContent?.replace(/\s+/g, " ").trim();
const column = (id: number, name: string) => text(row(id).querySelector(`.${name}`));

describe("AssetRow", () => {
  it("puts the card's facts in columns: name, pack, size, stats, status", () => {
    render();
    expect(column(1, "open")).toBe("Beaver_LowPoly");
    expect(column(1, "pack")).toBe("Forest Pack");
    expect(column(1, "size")).toBe("L · 4.1 MB");
    expect(column(1, "stats")).toBe("1.2k tris · 4.1 MB");
    expect(column(1, "state")).toBe("Confirmed");
    expect(column(8, "stats")).toBe("0:01 · 44.1 kHz");
    expect(column(8, "size")).toBe("S · 210 KB");
  });

  it("says Unidentified in full, and names the type beside its colour", () => {
    render();
    expect(column(5, "state")).toBe("Unidentified");
    expect(row(5).classList.contains("unidentified")).toBe(true);
    expect(row(5).querySelector('[role="img"][aria-label="Model"]')).not.toBeNull();
    expect(row(8).querySelector('[role="img"][aria-label="Sound"]')).not.toBeNull();
  });

  it("a picture has a thumb image; a sound has the sound mark and no image", () => {
    render();
    expect(row(13).querySelector(".thumb img")).not.toBeNull();
    expect(row(8).querySelector(".thumb img")).toBeNull();
    expect(row(8).querySelector(".thumb use")?.getAttribute("href")).toContain("type-sound");
  });

  it("is a row of a one-column grid: its own row index, every second one the zebra", () => {
    render();
    expect([1, 2, 19].map((id) => [row(id).getAttribute("aria-rowindex"), row(id).getAttribute("aria-colindex")])).toEqual([
      ["1", "1"],
      ["2", "1"],
      ["19", "1"],
    ]);
    expect([1, 2, 3, 4].map((id) => row(id).classList.contains("alt"))).toEqual([false, true, false, true]);
  });

  it("reflects favourite, staged and picked from what it is handed — it keeps no copy", () => {
    const { selection } = render(ASSETS.map((a) => (a.id === 2 ? { ...a, favorite: true, staged: true } : a)));
    expect(row(2).querySelector('[aria-label="Remove from Favorites"]')?.getAttribute("aria-pressed")).toBe("true");
    expect(row(1).querySelector('[aria-label="Add to Favorites"]')?.getAttribute("aria-pressed")).toBe("false");
    expect(text([...row(2).querySelectorAll("button")].at(-1)!)).toBe("✓ staged");
    selection.toggle(3);
    flushSync();
    expect(row(3).querySelector<HTMLInputElement>('input[type="checkbox"]')!.checked).toBe(true);
    // While anything is picked every row keeps its pick box showing.
    expect(row(9).querySelector(".pick")?.classList.contains("pin")).toBe(true);
  });

  it("the lead is selected; so is a picked row", () => {
    const { selection } = render();
    selection.moveTo(4);
    selection.toggle(6);
    flushSync();
    expect([4, 6, 7].map((id) => row(id).getAttribute("aria-selected"))).toEqual(["true", "true", "false"]);
    expect(row(4).classList.contains("selected")).toBe(true);
    expect(row(6).classList.contains("selected")).toBe(false);
  });
});

describe("AssetGrid — switching between cards and rows", () => {
  it("keeps the lead, the picks and the tab stop; only the cells are redrawn", () => {
    const { props, selection } = render(ASSETS, "grid");
    selection.moveTo(7);
    selection.toggle(2);
    selection.toggle(9);
    flushSync();
    expect(row(7).querySelector(".media .tray")).not.toBeNull(); // a card
    props.layout = "list";
    flushSync();
    expect(document.querySelector('[role="grid"]')?.getAttribute("aria-colcount")).toBe("1");
    expect(row(7).querySelector(".media .tray")).toBeNull(); // a row
    expect([selection.lead, [...selection.picks]]).toEqual([7, [2, 9]]);
    expect(row(7).tabIndex).toBe(0);
    expect(row(7).classList.contains("selected")).toBe(true);
    expect([2, 9].map((id) => row(id).querySelector<HTMLInputElement>('input[type="checkbox"]')!.checked)).toEqual([true, true]);
    props.layout = "grid";
    flushSync();
    expect(document.querySelector('[role="grid"]')?.getAttribute("aria-colcount")).toBe("5");
    expect([selection.lead, selection.picks.size, row(7).tabIndex]).toEqual([7, 2, 0]);
  });
});
