import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { keymap } from "../app/keymap.svelte";
import { resetOverlays } from "../app/overlay";
import { intentFor, type AssetIntent } from "../features/asset/actions";
import type { AssetSummary, ProjectRef } from "../features/asset/model";
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
  filterCount?: number;
}

function render(options: Options = {}) {
  const selection = new Selection();
  const intents: AssetIntent[] = [];
  const onview = vi.fn();
  const onidentify = vi.fn();
  const ondismissnudge = vi.fn();
  const onfilters = vi.fn();
  app = mount(LibraryScreen, {
    target: document.body,
    props: {
      assets: options.assets ?? ASSETS,
      unidentified: options.unidentified ?? 5,
      nudgeDismissed: options.nudgeDismissed ?? false,
      view: options.view ?? DEFAULT_VIEW,
      favoriteCount: options.favoriteCount ?? 0,
      filterCount: options.filterCount ?? 0,
      selection,
      project: options.project === undefined ? PROJECT : options.project,
      traits: TRAITS,
      thresholds: SIZE_THRESHOLDS,
      onintent: (i: AssetIntent) => intents.push(i),
      onview,
      onidentify,
      ondismissnudge,
      onfilters,
    },
  });
  flushSync();
  return { selection, intents, onview, onidentify, ondismissnudge, onfilters };
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
    const { onview, onfilters } = render({ favoriteCount: 2 });
    const toolbar = document.querySelector('[role="toolbar"]')!;
    const select = toolbar.querySelector("select")!;
    select.value = "name";
    select.dispatchEvent(new Event("change", { bubbles: true }));
    toolbar.querySelector<HTMLElement>('[title="Show only favorites"]')!.click();
    toolbar.querySelector<HTMLElement>('[title="Large"]')!.click();
    toolbar.querySelector<HTMLElement>('[title="List view"]')!.click();
    [...toolbar.querySelectorAll("button")].find((b) => b.textContent?.includes("Filters"))!.click();
    flushSync();
    expect(onview.mock.calls.map((c) => c[0])).toEqual([{ sort: "name" }, { favoritesOnly: true }, { size: "large" }, { layout: "list" }]);
    expect(onfilters).toHaveBeenCalledOnce();
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
