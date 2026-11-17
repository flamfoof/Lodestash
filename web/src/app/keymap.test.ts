import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Keymap, normalizeCombo } from "./keymap.svelte";

const press = (key: string, init: KeyboardEventInit = {}, target: EventTarget = document.body) => {
  const e = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init });
  target.dispatchEvent(e);
  return e;
};

describe("normalizeCombo", () => {
  it("orders modifiers and lower-cases letters", () => {
    expect(normalizeCombo("Shift+Mod+Z")).toBe("Mod+Shift+z");
    expect(normalizeCombo("ctrl+a")).toBe("Mod+a");
  });
  it("drops Shift from printed symbols and names Space", () => {
    expect(normalizeCombo("Shift+?")).toBe("?");
    expect(normalizeCombo("Space")).toBe("Space");
    expect(normalizeCombo("escape")).toBe("Escape");
  });
});

describe("Keymap scope stack", () => {
  let km: Keymap;
  let detach: () => void;
  beforeEach(() => {
    km = new Keymap();
    detach = km.attach(window);
  });
  afterEach(() => detach());

  it("fires a global binding and prevents the default", () => {
    const fn = vi.fn();
    km.bind("/", fn);
    const e = press("/");
    expect(fn).toHaveBeenCalledOnce();
    expect(e.defaultPrevented).toBe(true);
  });

  it("a blocking scope stops global shortcuts until it is popped", () => {
    const global = vi.fn();
    km.bind("/", global);
    const scope = km.push("dialog");
    press("/");
    expect(global).not.toHaveBeenCalled();
    expect(km.blockingDepth).toBe(1);
    scope.pop();
    press("/");
    expect(global).toHaveBeenCalledOnce();
    expect(km.blockingDepth).toBe(0);
  });

  it("offers a key to the top-most scope first", () => {
    const lower = vi.fn();
    const upper = vi.fn();
    km.push("a").bind("Escape", lower);
    km.push("b").bind("Escape", upper);
    press("Escape");
    expect(upper).toHaveBeenCalledOnce();
    expect(lower).not.toHaveBeenCalled();
  });

  it("a non-blocking scope lets unbound keys fall through", () => {
    const global = vi.fn();
    km.bind("f", global);
    km.push("hint", { blocking: false });
    press("f");
    expect(global).toHaveBeenCalledOnce();
  });

  it("ignores typing in a text field, except Escape and opted-in bindings", () => {
    const input = document.createElement("input");
    document.body.append(input);
    const slash = vi.fn();
    const esc = vi.fn();
    const save = vi.fn();
    km.bind("/", slash);
    km.bind("Escape", esc);
    km.bind("Mod+s", save, { inInputs: true });
    press("/", {}, input);
    press("Escape", {}, input);
    press("s", { ctrlKey: true }, input);
    expect(slash).not.toHaveBeenCalled();
    expect(esc).toHaveBeenCalledOnce();
    expect(save).toHaveBeenCalledOnce();
    input.remove();
  });

  it("leaves an event a component already handled alone", () => {
    const fn = vi.fn();
    km.bind("ArrowDown", fn);
    const el = document.createElement("div");
    document.body.append(el);
    el.addEventListener("keydown", (e) => e.preventDefault());
    press("ArrowDown", {}, el);
    expect(fn).not.toHaveBeenCalled();
    el.remove();
  });

  it("popping a lower scope also drops what was pushed above it", () => {
    const a = km.push("a");
    km.push("b");
    a.pop();
    expect(km.scopes).toEqual(["global"]);
  });
});
