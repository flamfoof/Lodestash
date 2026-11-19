import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { keymap } from "../app/keymap.svelte";
import { resetOverlays } from "../app/overlay";
import { intentFor, type AssetIntent } from "../features/asset/actions";
import type { AssetSummary, ProjectRef } from "../features/asset/model";
import type { Constraints } from "../features/filters/active";
import { NO_FILTERS, traitCounts, type LibraryFilters } from "../features/filters/filters";
import { DEFAULT_VIEW, type LibraryView } from "../features/library/view";
import { ASSETS, PROJECT, SIZE_THRESHOLDS, TRAITS } from "../fixtures/library";
import { Selection } from "../state/selection.svelte";
import LibraryScreen from "./LibraryScreen.svelte";

let app: ReturnType<typeof mount> | null = null;
let detach: () => void;

beforeEach(() => {
  keymap.reset();
  detach = keymap.attach(window);
  Object.defineProperty(window, "innerWidth", { value: 1440, configurable: true });
  Object.defineProperty(window, "innerHeight", { value: 900, configurable: true });
});

afterEach(() => {
  if (app) unmount(app);
  app = null;
  detach();
  resetOverlays();
  document.body.innerHTML = "";
});

interface Options {
  assets?: AssetSummary[];
  view?: LibraryView;
  unidentified?: number;
  nudgeDismissed?: boolean;
  project?: ProjectRef | null;
  favoriteCount?: number;
  filters?: LibraryFilters;
  constraints?: Constraints;
  total?: number;
}

function render(options: Options = {}) {
  const selection = new Selection();
  const intents: AssetIntent[] = [];
  const onview = vi.fn();
  const onidentify = vi.fn();
  const ondismissnudge = vi.fn();
  const onfilterschange = vi.fn();
  const onunconstrain = vi.fn();
  const onclearfilters = vi.fn();
  app = mount(LibraryScreen, {
    target: document.body,
    props: {
      assets: options.assets ?? ASSETS,
      total: options.total ?? ASSETS.length,
      unidentified: options.unidentified ?? 5,
      nudgeDismissed: options.nudgeDismissed ?? false,
      view: options.view ?? DEFAULT_VIEW,
      favoriteCount: options.favoriteCount ?? 0,
      filters: options.filters ?? NO_FILTERS,
      traitOptions: traitCounts(ASSETS),
      constraints: options.constraints ?? {},
      selection,
      project: options.project === undefined ? PROJECT : options.project,
      traits: TRAITS,
      thresholds: SIZE_THRESHOLDS,
      onintent: (i: AssetIntent) => intents.push(i),
      onview,
      onidentify,
      ondismissnudge,
      onfilterschange,
      onunconstrain,
      onclearfilters,
    },
  });
  flushSync();
  return { selection, intents, onview, onidentify, ondismissnudge, onfilterschange, onunconstrain, onclearfilters };
}

const cell = (id: number) => document.querySelector<HTMLElement>(`[role="gridcell"][data-id="${id}"]`)!;
const menu = () => document.querySelector<HTMLElement>('[role="menu"]');
const item = (id: string) => menu()!.querySelector<HTMLElement>(`[data-id="${id}"]`)!;
const targets = (...ids: number[]) => ({ targets: ASSETS.filter((a) => ids.includes(a.id)), project: PROJECT });

function press(key: string, init: KeyboardEventInit = {}) {
  const event = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init });
  (document.activeElement ?? document.body).dispatchEvent(event);
  flushSync();
  return event;
}

function rightClick(id: number) {
  cell(id).dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 400, clientY: 300 }));
  flushSync();
}

function choose(id: string) {
  item(id).click();
  flushSync();
}

describe("LibraryScreen — composition", () => {
  it("shows the nudge, the result label and one card per asset", () => {
    render();
    expect(document.querySelector('[role="status"]')?.textContent).toMatch(/5\s+assets are Unidentified/);
    expect(document.querySelector('[role="toolbar"]')?.textContent).toContain("19 assets · all libraries");
    expect(document.querySelectorAll('[role="gridcell"]').length).toBe(19);
  });

  it("drops the nudge when nothing is unidentified, or when it was dismissed", () => {
    render({ unidentified: 0 });
    expect(document.querySelector('[role="status"]')).toBeNull();
    if (app) unmount(app);
    app = null;
    render({ nudgeDismissed: true });
    expect(document.querySelector('[role="status"]')).toBeNull();
  });

  it("the nudge leads to Identify and can be dismissed — it decides neither itself", () => {
    const { onidentify, ondismissnudge } = render();
    const banner = document.querySelector('[role="status"]')!;
    [...banner.querySelectorAll("button")].find((b) => b.textContent?.includes("Open Identify mode"))!.click();
    banner.querySelector<HTMLElement>('[aria-label="Dismiss"]')!.click();
    expect(onidentify).toHaveBeenCalledOnce();
    expect(ondismissnudge).toHaveBeenCalledOnce();
    // Still there: the owner of the session has not said it is dismissed.
    expect(document.querySelector('[role="status"]')).not.toBeNull();
  });

  it("says so when nothing matches, and points at the Unidentified backlog", () => {
    const { onidentify } = render({ assets: [] });
    expect(document.querySelector('[role="grid"]')).toBeNull();
    expect(document.body.textContent).toContain("Nothing matches");
    expect(document.body.textContent).toContain("5 assets are still Unidentified");
    [...document.querySelectorAll("button")].find((b) => b.textContent?.trim() === "Open Identify mode")!.click();
    expect(onidentify).toHaveBeenCalledOnce();
  });

  it("the toolbar reports view changes and changes nothing itself", () => {
    const { onview } = render({ favoriteCount: 2 });
    const toolbar = document.querySelector('[role="toolbar"]')!;
    const select = toolbar.querySelector("select")!;
    select.value = "name";
    select.dispatchEvent(new Event("change", { bubbles: true }));
    toolbar.querySelector<HTMLElement>('[title="Show only favorites"]')!.click();
    toolbar.querySelector<HTMLElement>('[title="Large"]')!.click();
    toolbar.querySelector<HTMLElement>('[title="List view"]')!.click();
    flushSync();
    expect(onview.mock.calls.map((c) => c[0])).toEqual([{ sort: "name" }, { favoritesOnly: true }, { size: "large" }, { layout: "list" }]);
    expect(toolbar.querySelector('[title="Show only favorites"]')?.textContent).toContain("2");
    // The grid is still the size it was given.
    expect(document.querySelector('[role="grid"]')?.getAttribute("data-size")).toBe("cozy");
  });
});

describe("LibraryScreen — the context menu acts on the selection", () => {
  it("on an unpicked card, it acts on that card alone", () => {
    const { intents, selection } = render();
    selection.toggle(2);
    selection.toggle(3);
    flushSync();
    rightClick(7);
    expect(menu()?.getAttribute("aria-label")).toBe("Actions for Fern_Cluster_01");
    choose("stage");
    expect(intents).toEqual([intentFor("stage", targets(7))]);
    expect(menu()).toBeNull();
  });

  it("on a picked card, it acts on the whole pick set", () => {
    const { intents, selection } = render();
    [2, 3, 9].forEach((id) => selection.toggle(id));
    flushSync();
    rightClick(3);
    expect(menu()?.getAttribute("aria-label")).toBe("Actions for 3 assets");
    choose("favorite");
    expect(intents).toEqual([intentFor("favorite", targets(2, 3, 9))]);
    expect(intents[0].ids).toEqual([2, 3, 9]);
  });

  it("disables single-asset items for a pick set, and they do nothing", () => {
    const { intents, selection } = render();
    [2, 3].forEach((id) => selection.toggle(id));
    flushSync();
    rightClick(2);
    expect(item("reveal").getAttribute("aria-disabled")).toBe("true");
    choose("reveal");
    expect(intents).toEqual([]);
  });

  it("grays Import out without a project, with the reason", () => {
    render({ project: null });
    rightClick(1);
    expect(item("import").getAttribute("aria-disabled")).toBe("true");
    expect(item("import").getAttribute("title")).toMatch(/No project is connected/);
  });

  it("Open makes the card the lead instead of leaving the screen", () => {
    const { intents, selection } = render();
    rightClick(4);
    choose("open");
    expect(selection.lead).toBe(4);
    expect(intents).toEqual([]);
  });

  it("a right-click does not move the lead (the inspector must not shift under the menu)", () => {
    const { selection } = render();
    selection.moveTo(1);
    flushSync();
    rightClick(8);
    expect(selection.lead).toBe(1);
  });

  it("opens from the keyboard at the focused card", () => {
    const { intents } = render();
    cell(5).focus();
    press("F10", { shiftKey: true });
    expect(menu()?.getAttribute("aria-label")).toBe("Actions for SM_Asset_0117");
    choose("identify");
    expect(intents).toEqual([intentFor("identify", targets(5))]);
  });
});

describe("LibraryScreen — keys act on the selection, never behind an overlay", () => {
  it("Space emits Quick Look for the lead", () => {
    const { intents, selection } = render();
    selection.moveTo(6);
    press(" ");
    expect(intents).toEqual([intentFor("quick-look", targets(6))]);
  });

  it("F favourites the pick set, or with none the lead; Delete trashes the lead only", () => {
    const { intents, selection } = render();
    selection.moveTo(6);
    press("f");
    selection.toggle(2);
    selection.toggle(3);
    press("f");
    press("Delete");
    expect(intents).toEqual([intentFor("favorite", targets(6)), intentFor("favorite", targets(2, 3)), intentFor("trash", targets(6))]);
  });

  it("Delete and F do nothing while the context menu is open", () => {
    const { intents, selection } = render();
    selection.moveTo(6);
    flushSync();
    rightClick(6);
    expect(menu()).not.toBeNull();
    press("Delete");
    press("f");
    expect(intents).toEqual([]);
    press("Escape");
    expect(menu()).toBeNull();
    // Escape closed the menu and nothing else: the selection is still there.
    expect(selection.lead).toBe(6);
    press("Delete");
    expect(intents).toEqual([intentFor("trash", targets(6))]);
  });

  it("Delete, F and Space do nothing while a dialog's scope is open", () => {
    const { intents, selection } = render();
    selection.moveTo(6);
    const dialog = keymap.push("dialog");
    ["Delete", "f", " "].forEach((k) => press(k));
    expect(intents).toEqual([]);
    dialog.pop();
    press("Delete");
    expect(intents.map((i) => i.action)).toEqual(["trash"]);
  });

  it("no key fires with nothing selected", () => {
    const { intents } = render();
    ["Delete", "f", " "].forEach((k) => press(k));
    expect(intents).toEqual([]);
  });

  it("Escape clears the pick set, then the lead", () => {
    const { selection } = render();
    selection.moveTo(6);
    selection.toggle(2);
    press("Escape");
    expect([selection.picks.size, selection.lead]).toEqual([0, 6]);
    press("Escape");
    expect(selection.lead).toBeNull();
  });

  it("the keys are unbound when the screen goes away", () => {
    const { intents, selection } = render();
    selection.moveTo(6);
    if (app) unmount(app);
    app = null;
    press("f");
    expect(intents).toEqual([]);
    expect(keymap.scopes).toEqual(["global"]);
  });
});

describe("LibraryScreen — the ⚑ filters and the active-filter bar", () => {
  const filtersButton = () => [...document.querySelectorAll<HTMLElement>('[role="toolbar"] button')].find((b) => b.textContent?.includes("Filters"))!;
  const popover = () => document.querySelector<HTMLElement>('[role="dialog"][aria-label="Filter library"]');
  const chip = (text: string) => [...popover()!.querySelectorAll<HTMLButtonElement>("button[aria-pressed]")].find((b) => b.textContent?.trim().startsWith(text))!;
  const bar = () => document.querySelector<HTMLElement>("[data-active-filters]");
  const tokens = () => [...bar()!.querySelectorAll<HTMLElement>("[data-axis]")];
  const remove = (axis: string, nth = 0) => tokens().filter((t) => t.dataset.axis === axis)[nth].querySelector<HTMLElement>("button")!.click();

  function openFilters() {
    filtersButton().click();
    flushSync();
  }

  it("⚑ opens the popover; a chip reports the whole next set and the screen keeps none", () => {
    const { onfilterschange } = render();
    expect(popover()).toBeNull();
    openFilters();
    expect(filtersButton().getAttribute("aria-expanded")).toBe("true");
    expect(popover()!.id).toBe(filtersButton().getAttribute("aria-controls"));
    chip("rock").click();
    flushSync();
    expect(onfilterschange).toHaveBeenLastCalledWith({ traits: ["rock"], tris: [], sizes: [] });
    // Nobody handed it a changed set, so nothing is chosen yet.
    expect(chip("rock").getAttribute("aria-pressed")).toBe("false");
    expect(bar()).toBeNull();
  });

  it("filters compose across groups, and Clear all hands back the empty set", () => {
    const { onfilterschange } = render({ filters: { traits: ["rock"], tris: ["low"], sizes: [] } });
    expect(filtersButton().textContent).toContain("2");
    openFilters();
    expect([chip("rock"), chip("≤ 1k"), chip("moss"), chip("M")].map((c) => c.getAttribute("aria-pressed"))).toEqual(["true", "true", "false", "false"]);
    chip("M").click();
    expect(onfilterschange).toHaveBeenLastCalledWith({ traits: ["rock"], tris: ["low"], sizes: ["M"] });
    chip("rock").click();
    expect(onfilterschange).toHaveBeenLastCalledWith({ traits: [], tris: ["low"], sizes: [] });
    [...popover()!.querySelectorAll<HTMLElement>("button")].find((b) => b.textContent?.trim() === "Clear all")!.click();
    expect(onfilterschange).toHaveBeenLastCalledWith(NO_FILTERS);
  });

  it("the trait search narrows the chips, and says so when none match", () => {
    render();
    openFilters();
    const search = popover()!.querySelector<HTMLInputElement>('input[placeholder="search traits…"]')!;
    const traits = () => [...popover()!.querySelectorAll('[aria-label="Trait"] button')].map((b) => b.textContent?.trim());
    expect(traits()).toHaveLength(32);
    search.value = "MOS";
    search.dispatchEvent(new Event("input", { bubbles: true }));
    flushSync();
    expect(traits()).toEqual(["moss"]);
    search.value = "zzz";
    search.dispatchEvent(new Event("input", { bubbles: true }));
    flushSync();
    expect(traits()).toEqual([]);
    expect(popover()!.textContent).toContain("No traits match.");
  });

  it("Escape closes the popover only, and no asset key fires behind it", () => {
    const { intents, selection } = render();
    selection.moveTo(6);
    selection.toggle(2);
    openFilters();
    ["f", "Delete", " "].forEach((k) => press(k));
    expect(intents).toEqual([]);
    press("Escape");
    expect(popover()).toBeNull();
    expect([selection.lead, selection.picks.size]).toEqual([6, 1]);
    expect(document.activeElement?.closest("[role=toolbar]")).not.toBeNull();
  });

  it("the bar is absent with nothing active, and lists every constraint in composition order", () => {
    render();
    expect(bar()).toBeNull();
    if (app) unmount(app);
    render({
      assets: ASSETS.slice(0, 2),
      total: 19,
      view: { ...DEFAULT_VIEW, favoritesOnly: true },
      filters: { traits: ["rock"], tris: ["low"], sizes: ["M"] },
      constraints: { query: "moss", category: { id: "model", label: "Models" }, status: "identified", library: { id: 1, name: "Synty" } },
    });
    expect(tokens().map((t) => t.dataset.axis)).toEqual(["query", "type", "status", "library", "trait", "tris", "size", "favorite"]);
    expect(bar()!.textContent).toContain("2 of 19 shown");
    expect(tokens()[3].textContent).toContain("lib");
  });

  it("removing a chip tells the owner of that axis — ⚑, ★ or the shell", () => {
    const { onfilterschange, onview, onunconstrain, onclearfilters } = render({
      view: { ...DEFAULT_VIEW, favoritesOnly: true },
      filters: { traits: ["rock", "moss"], tris: ["low"], sizes: ["M"] },
      constraints: { query: "moss", status: "identified" },
    });
    remove("trait", 1);
    expect(onfilterschange).toHaveBeenLastCalledWith({ traits: ["rock"], tris: ["low"], sizes: ["M"] });
    remove("tris");
    expect(onfilterschange).toHaveBeenLastCalledWith({ traits: ["rock", "moss"], tris: [], sizes: ["M"] });
    remove("size");
    expect(onfilterschange).toHaveBeenLastCalledWith({ traits: ["rock", "moss"], tris: ["low"], sizes: [] });
    remove("favorite");
    expect(onview).toHaveBeenLastCalledWith({ favoritesOnly: false });
    remove("status");
    remove("query");
    expect(onunconstrain.mock.calls.map((c) => c[0])).toEqual(["status", "query"]);
    [...bar()!.querySelectorAll<HTMLElement>("button")].find((b) => b.textContent?.trim() === "Clear all")!.click();
    expect(onclearfilters).toHaveBeenCalledOnce();
  });

  it("an empty result under a filter offers the reset, not only the Identify backlog", () => {
    const { onclearfilters } = render({ assets: [], filters: { traits: ["rock"], tris: ["ultra"], sizes: [] } });
    expect(document.body.textContent).toContain("Nothing matches");
    [...document.querySelectorAll<HTMLElement>("button")].find((b) => b.textContent?.trim() === "Clear all filters")!.click();
    expect(onclearfilters).toHaveBeenCalledOnce();
  });
});

describe("LibraryScreen — the bulk bar acts on the pick set", () => {
  const bulk = () => document.querySelector<HTMLElement>('[role="toolbar"][aria-label="Bulk actions"]');
  const bulkButton = (text: string) => [...bulk()!.querySelectorAll<HTMLElement>("button")].find((b) => b.textContent?.includes(text))!;

  it("appears with the first pick and goes with the last", () => {
    const { selection } = render();
    expect(bulk()).toBeNull();
    selection.toggle(2);
    flushSync();
    expect(bulk()!.textContent).toContain("1 selected");
    selection.toggle(9);
    flushSync();
    expect(bulk()!.textContent).toContain("2 selected");
    selection.toggle(2);
    selection.toggle(9);
    flushSync();
    expect(bulk()).toBeNull();
  });

  it("its buttons emit for the picks in page order, whatever order they were picked in — never for the lead", () => {
    const { intents, selection } = render();
    selection.moveTo(6);
    [9, 2, 3].forEach((id) => selection.toggle(id));
    flushSync();
    bulkButton("Stage all").click();
    const field = bulk()!.querySelector<HTMLInputElement>("input")!;
    field.value = "keeper";
    field.dispatchEvent(new Event("input", { bubbles: true }));
    flushSync();
    bulkButton("Add tag").click();
    expect(intents).toEqual([intentFor("stage", targets(2, 3, 9)), intentFor("tag-add", targets(2, 3, 9), { tag: "keeper" })]);
  });

  it("✕ Clear empties the pick set and leaves the lead", () => {
    const { intents, selection } = render();
    selection.moveTo(6);
    selection.toggle(2);
    selection.toggle(3);
    flushSync();
    bulkButton("Clear").click();
    flushSync();
    expect([selection.picks.size, selection.lead]).toEqual([0, 6]);
    expect(bulk()).toBeNull();
    expect(intents).toEqual([]);
  });

  it("typing a tag name does not trigger the asset keys", () => {
    const { intents, selection } = render();
    selection.moveTo(6);
    selection.toggle(2);
    flushSync();
    const field = bulk()!.querySelector<HTMLInputElement>("input")!;
    field.focus();
    ["f", " ", "Delete"].forEach((k) => press(k));
    expect(intents).toEqual([]);
  });
});
