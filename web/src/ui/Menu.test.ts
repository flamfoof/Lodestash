import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { keymap } from "../app/keymap.svelte";
import type { MenuEntry } from "./Menu.svelte";
import MenuHarness from "./test/MenuHarness.svelte";

const $ = <T extends HTMLElement = HTMLElement>(sel: string) => document.querySelector<T>(sel)!;
const press = (key: string) => {
  (document.activeElement ?? document.body).dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
  flushSync();
};
const menu = () => document.querySelector<HTMLElement>('[role="menu"]');
const focusedId = () => (document.activeElement as HTMLElement | null)?.dataset.id;

const ITEMS: MenuEntry[] = [
  { id: "open", label: "Open" },
  { id: "look", label: "Quick Look", shortcut: "Space" },
  { separator: true },
  { id: "import", label: "Import…", disabled: true, hint: "No project open" },
  { id: "trash", label: "Move to Trash", danger: true },
];

const rect = (x: number, y: number, width: number, height: number): DOMRect =>
  ({ x, y, left: x, top: y, width, height, right: x + width, bottom: y + height, toJSON() {} }) as DOMRect;

let app: ReturnType<typeof mount> | null = null;
let detach: () => void;
let openerTop = 100;

beforeEach(() => {
  keymap.reset();
  detach = keymap.attach(window);
  Object.defineProperty(window, "innerWidth", { value: 1440, configurable: true });
  Object.defineProperty(window, "innerHeight", { value: 900, configurable: true });
  // jsdom has no layout: give the opener and the menu the boxes a browser would.
  vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (this: Element) {
    if (this.id === "opener") return rect(100, openerTop, 80, 30);
    if (this.getAttribute("role") === "menu") return rect(0, 0, 180, 200);
    return rect(0, 0, 0, 0);
  });
});

afterEach(() => {
  if (app) unmount(app);
  app = null;
  detach();
  vi.restoreAllMocks();
  openerTop = 100;
  document.body.innerHTML = "";
});

function openMenu(onpick = vi.fn()) {
  app = mount(MenuHarness, { target: document.body, props: { onpick, items: ITEMS } });
  flushSync();
  $("#opener").focus();
  $("#opener").click();
  flushSync();
  return onpick;
}

describe("Menu", () => {
  it("opens below its anchor with focus on the first item", () => {
    openMenu();
    expect(menu()?.dataset.side).toBe("below");
    expect(menu()?.style.top).toBe("134px");
    expect(focusedId()).toBe("open");
  });

  it("flips upward when its measured height would cross the bottom of the viewport", () => {
    openerTop = 800;
    openMenu();
    expect(menu()?.dataset.side).toBe("above");
    expect(menu()?.style.top).toBe(`${800 - 4 - 200}px`);
  });

  it("arrow keys move between enabled items, wrapping, and skip disabled ones", () => {
    openMenu();
    press("ArrowDown");
    expect(focusedId()).toBe("look");
    press("ArrowDown");
    expect(focusedId()).toBe("trash");
    press("ArrowDown");
    expect(focusedId()).toBe("open");
    press("ArrowUp");
    expect(focusedId()).toBe("trash");
    press("Home");
    expect(focusedId()).toBe("open");
    press("End");
    expect(focusedId()).toBe("trash");
  });

  it("Enter activates the focused item, closes, and gives focus back", () => {
    const onpick = openMenu();
    press("ArrowDown");
    press("Enter");
    expect(onpick).toHaveBeenCalledWith("look");
    expect(menu()).toBeNull();
    expect(document.activeElement).toBe($("#opener"));
  });

  it("a disabled item does nothing", () => {
    const onpick = openMenu();
    $<HTMLButtonElement>('[data-id="import"]').click();
    flushSync();
    expect(onpick).not.toHaveBeenCalled();
    expect(menu()).not.toBeNull();
  });

  it("Escape closes it and restores focus to the opener", () => {
    const onpick = openMenu();
    press("Escape");
    expect(menu()).toBeNull();
    expect(onpick).not.toHaveBeenCalled();
    expect(document.activeElement).toBe($("#opener"));
  });

  it("blocks global shortcuts while open", () => {
    const shortcut = vi.fn();
    keymap.bind("Delete", shortcut);
    openMenu();
    press("Delete");
    expect(shortcut).not.toHaveBeenCalled();
    press("Escape");
    press("Delete");
    expect(shortcut).toHaveBeenCalledOnce();
  });
});
