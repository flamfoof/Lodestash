import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { keymap } from "../app/keymap.svelte";
import { resetOverlays, setAppRoot } from "../app/overlay";
import ConfirmHarness from "./test/ConfirmHarness.svelte";
import DialogHarness from "./test/DialogHarness.svelte";

const $ = <T extends HTMLElement = HTMLElement>(sel: string) => document.querySelector<T>(sel)!;
const press = (key: string, init: KeyboardEventInit = {}) => {
  const target = document.activeElement ?? document.body;
  target.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init }));
  flushSync();
};
const click = (sel: string) => {
  $(sel).focus();
  $(sel).click();
  flushSync();
};
const dialogs = () => document.querySelectorAll('[role="dialog"]');

let app: ReturnType<typeof mount> | null = null;
let detach: () => void;

beforeEach(() => {
  keymap.reset();
  detach = keymap.attach(window);
});

afterEach(() => {
  if (app) unmount(app);
  app = null;
  detach();
  resetOverlays();
  setAppRoot(null);
  document.body.innerHTML = "";
});

function open(onpick = vi.fn()) {
  app = mount(DialogHarness, { target: document.body, props: { onpick } });
  flushSync();
  setAppRoot($("#app-root"));
  click("#opener");
  return onpick;
}

describe("Dialog", () => {
  it("moves focus to the first control of its body and makes the app inert", () => {
    open();
    expect(dialogs()).toHaveLength(1);
    expect(document.activeElement).toBe($("#a"));
    expect($("#app-root").hasAttribute("inert")).toBe(true);
    // Rendered in the overlay host, outside the app root.
    expect($("#ls-overlay-host").contains($('[role="dialog"]'))).toBe(true);
  });

  it("cycles Tab and Shift+Tab inside the dialog", () => {
    open();
    const close = $<HTMLButtonElement>('[role="dialog"] [aria-label="Close"]');
    $("#menu-button").focus();
    press("Tab");
    expect(document.activeElement).toBe(close);
    press("Tab", { shiftKey: true });
    expect(document.activeElement).toBe($("#menu-button"));
  });

  it("Escape closes it and focus returns to the opener", () => {
    open();
    press("Escape");
    expect(dialogs()).toHaveLength(0);
    expect(document.activeElement).toBe($("#opener"));
    expect($("#app-root").hasAttribute("inert")).toBe(false);
  });

  it("Escape closes only the top-most of two dialogs", () => {
    open();
    click("#nested");
    expect(dialogs()).toHaveLength(2);
    expect(document.activeElement).toBe($("#c"));
    // The lower dialog's layer is inert while the upper one is open.
    expect($("#a").closest("[inert]")).not.toBeNull();
    press("Escape");
    expect(dialogs()).toHaveLength(1);
    expect(document.activeElement).toBe($("#nested"));
    expect($("#a").closest("[inert]")).toBeNull();
    press("Escape");
    expect(dialogs()).toHaveLength(0);
    expect(document.activeElement).toBe($("#opener"));
  });

  it("Escape in a menu inside the dialog closes the menu, not the dialog", () => {
    open();
    click("#menu-button");
    expect(document.querySelector('[role="menu"]')).not.toBeNull();
    press("Escape");
    expect(document.querySelector('[role="menu"]')).toBeNull();
    expect(dialogs()).toHaveLength(1);
    expect(document.activeElement).toBe($("#menu-button"));
  });

  it("a global shortcut does not fire while it is open", () => {
    const shortcut = vi.fn();
    keymap.bind("/", shortcut);
    open();
    $("#b").focus();
    press("/");
    expect(shortcut).not.toHaveBeenCalled();
    press("Escape");
    $("#opener").focus();
    press("/");
    expect(shortcut).toHaveBeenCalledOnce();
  });

  it("clicking the scrim closes it", () => {
    open();
    $(".scrim").click();
    flushSync();
    expect(dialogs()).toHaveLength(0);
  });
});

describe("ConfirmDialog", () => {
  it("destructive: the safe action has focus, so Enter does not destroy by reflex", () => {
    const onconfirm = vi.fn();
    app = mount(ConfirmHarness, { target: document.body, props: { onconfirm } });
    flushSync();
    expect(document.activeElement?.textContent?.trim()).toBe("Cancel");
    click("[data-autofocus]");
    expect(onconfirm).not.toHaveBeenCalled();
    expect(dialogs()).toHaveLength(0);
  });

  it("non-destructive: confirm has focus and confirms", () => {
    const onconfirm = vi.fn();
    app = mount(ConfirmHarness, { target: document.body, props: { onconfirm, destructive: false } });
    flushSync();
    expect(document.activeElement?.textContent?.trim()).toBe("Move to Trash");
    click("[data-autofocus]");
    expect(onconfirm).toHaveBeenCalledOnce();
  });
});
