import { afterEach, describe, expect, it, vi } from "vitest";
import { Keymap } from "../../app/keymap.svelte";
import { ASSETS, PROJECT } from "../../fixtures/library";
import { ACTIONS, MENU, bindAssetKeys, intentFor, isActionId, menuFor, whyNot, type AssetActionId, type AssetIntent } from "./actions";
import type { AssetSummary } from "./model";

const [beaver, rock, sword] = ASSETS;
const unidentified = ASSETS[4];
const sound = ASSETS[7];
const ctx = (targets: AssetSummary[], project = PROJECT as typeof PROJECT | null) => ({ targets, project });

function keys(lead: AssetSummary | null, picks: AssetSummary[] = [], project = PROJECT as typeof PROJECT | null) {
  const keymap = new Keymap();
  const emitted: AssetIntent[] = [];
  const unbind = bindAssetKeys(
    keymap,
    { lead: () => lead, selection: () => (picks.length ? picks : lead ? [lead] : []), project: () => project },
    (intent) => emitted.push(intent),
  );
  const press = (key: string, target?: HTMLElement) => {
    const event = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true });
    if (target) {
      const detach = keymap.attach(target);
      target.dispatchEvent(event);
      detach();
    } else keymap.handle(event);
    return event;
  };
  return { keymap, emitted, press, unbind };
}

afterEach(() => {
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

describe("asset actions", () => {
  it("produces an intent naming the action and its targets, in order", () => {
    expect(intentFor("quick-look", ctx([rock, beaver]))).toEqual({ action: "quick-look", ids: [rock.id, beaver.id] });
    expect(intentFor("trash", ctx([sword]))).toEqual({ action: "trash", ids: [sword.id] });
  });

  it("resolves a toggle against its targets: on unless every one already is", () => {
    const starred = { ...beaver, favorite: true };
    expect(intentFor("favorite", ctx([beaver]))?.on).toBe(true);
    expect(intentFor("favorite", ctx([starred]))?.on).toBe(false);
    expect(intentFor("favorite", ctx([starred, rock]))?.on).toBe(true);
    expect(ACTIONS.favorite.label(ctx([starred]))).toBe("Remove from Favorites");
    expect(ACTIONS.favorite.label(ctx([starred, rock]))).toBe("Add to Favorites");
    expect(intentFor("stage", ctx([{ ...beaver, staged: true }]))).toEqual({ action: "stage", ids: [beaver.id], on: false });
  });

  it("yields no intent, and says why, when an action cannot run", () => {
    expect(intentFor("import", ctx([beaver], null))).toBeNull();
    expect(whyNot("import", ctx([beaver], null))).toMatch(/No project is connected/);
    expect(intentFor("import", ctx([beaver]))).toEqual({ action: "import", ids: [beaver.id] });
    expect(intentFor("open", ctx([beaver, rock]))).toBeNull();
    expect(intentFor("find-similar", ctx([unidentified]))).toBeNull();
    expect(intentFor("play", ctx([beaver]))).toBeNull();
    expect(intentFor("play", ctx([sound]))).toEqual({ action: "play", ids: [sound.id] });
    expect(intentFor("trash", ctx([]))).toBeNull();
  });

  it("names Identify by what it will do", () => {
    expect(ACTIONS.identify.label(ctx([unidentified]))).toBe("Identify");
    expect(ACTIONS.identify.label(ctx([beaver]))).toBe("Re-identify");
    expect(ACTIONS.identify.label(ctx([beaver, unidentified]))).toBe("Re-identify");
  });

  it("an intent cannot be changed after it is made", () => {
    const intent = intentFor("stage", ctx([beaver]))!;
    expect(Object.isFrozen(intent)).toBe(true);
    expect(Object.isFrozen(intent.ids)).toBe(true);
  });
});

describe("the context menu", () => {
  const labels = (c: ReturnType<typeof ctx>) => menuFor(c).map((e) => ("separator" in e ? "—" : e.label));

  it("lists DESIGN-GUIDELINES §7's items in order", () => {
    expect(labels(ctx([beaver]))).toEqual([
      "Open",
      "Quick Look",
      "—",
      "Reveal in Explorer",
      "Open with default app",
      "Copy name",
      "Copy path",
      "—",
      "Add to Favorites",
      "Stage",
      "Import into MyRPG…",
      "Export to folder…",
      "Add to collection…",
      "Edit tags…",
      "—",
      "Re-identify",
      "Find similar",
      "—",
      "Move to Trash",
    ]);
  });

  it("is the same menu for many assets, with single-asset items disabled", () => {
    const entries = menuFor(ctx([beaver, rock, sword]));
    const disabled = entries.flatMap((e) => ("separator" in e || !e.disabled ? [] : [e.id]));
    expect(disabled).toEqual(["open", "reveal", "open-default", "find-similar"]);
    expect(labels(ctx([beaver, rock]))).toContain("Copy names");
    expect(entries.length).toBe(MENU.length);
  });

  it("grays engine actions out, with the reason, when no project is active", () => {
    const item = menuFor(ctx([beaver], null)).find((e) => !("separator" in e) && e.id === "import");
    expect(item).toMatchObject({ label: "Import…", disabled: true, hint: expect.stringMatching(/Export to folder/) });
  });

  it("every menu id is an action, and the shortcuts are the keys that are bound", () => {
    const ids = MENU.filter((id): id is AssetActionId => id !== null);
    expect(ids.every(isActionId)).toBe(true);
    expect(isActionId("nonsense")).toBe(false);
    expect(ids.filter((id) => ACTIONS[id].shortcut).map((id) => [id, ACTIONS[id].shortcut])).toEqual([
      ["open", "Enter"],
      ["quick-look", "Space"],
      ["favorite", "F"],
      ["trash", "Del"],
    ]);
  });
});

describe("the asset keys", () => {
  it("Space quick-looks the lead; a key, a menu item and a direct call make the same intent", () => {
    const { emitted, press } = keys(beaver);
    const event = press(" ");
    expect(emitted).toEqual([intentFor("quick-look", ctx([beaver]))]);
    expect(event.defaultPrevented).toBe(true);
    const fromMenu = menuFor(ctx([beaver])).find((e) => !("separator" in e) && e.id === "quick-look");
    expect(fromMenu && !("separator" in fromMenu) && intentFor(fromMenu.id as AssetActionId, ctx([beaver]))).toEqual(emitted[0]);
  });

  it("F toggles the pick set when there is one, else the lead", () => {
    const one = keys(beaver);
    one.press("f");
    expect(one.emitted).toEqual([{ action: "favorite", ids: [beaver.id], on: true }]);
    const many = keys(beaver, [rock, sword]);
    many.press("f");
    expect(many.emitted).toEqual([{ action: "favorite", ids: [rock.id, sword.id], on: true }]);
  });

  it("Delete trashes the lead only, never the pick set", () => {
    const { emitted, press } = keys(beaver, [rock, sword]);
    press("Delete");
    expect(emitted).toEqual([{ action: "trash", ids: [beaver.id] }]);
  });

  it("does nothing, and leaves the key alone, with nothing selected", () => {
    const { emitted, press } = keys(null);
    const events = [" ", "f", "Delete"].map((k) => press(k));
    expect(emitted).toEqual([]);
    expect(events.some((e) => e.defaultPrevented)).toBe(false);
  });

  it("Space, F and Delete do not fire behind an open overlay", () => {
    const { keymap, emitted, press } = keys(beaver);
    const overlay = keymap.push("dialog");
    [" ", "f", "Delete"].forEach((k) => press(k));
    expect(emitted).toEqual([]);
    overlay.pop();
    [" ", "f", "Delete"].forEach((k) => press(k));
    expect(emitted.map((i) => i.action)).toEqual(["quick-look", "favorite", "trash"]);
  });

  it("Space is left to a focused button; F and Delete still act", () => {
    const { emitted, press } = keys(beaver);
    const button = document.body.appendChild(document.createElement("button"));
    const event = press(" ", button);
    expect(emitted).toEqual([]);
    expect(event.defaultPrevented).toBe(false);
    press("f", button);
    expect(emitted.map((i) => i.action)).toEqual(["favorite"]);
  });

  it("no key fires while typing in a field", () => {
    const { emitted, press } = keys(beaver);
    const input = document.body.appendChild(document.createElement("input"));
    [" ", "f", "Delete"].forEach((k) => press(k, input));
    expect(emitted).toEqual([]);
  });

  it("unbinds", () => {
    const { emitted, press, unbind } = keys(beaver);
    unbind();
    press("f");
    expect(emitted).toEqual([]);
  });
});

describe("actions.ts is the only place an intent is defined", () => {
  const sources = import.meta.glob<string>("/src/**/*.{ts,svelte}", { query: "?raw", import: "default", eager: true });
  const others = Object.entries(sources).filter(([file]) => !/features\/asset\/actions(\.test)?\.ts$/.test(file));

  it("scans the app's sources", () => {
    expect(others.length).toBeGreaterThan(40);
    expect(Object.keys(sources).some((f) => f.endsWith("features/asset/actions.ts"))).toBe(true);
  });

  it("no other module casts to AssetIntent or writes an intent literal", () => {
    const offenders = others
      .filter(([, text]) => /\bas\s+(unknown\s+as\s+)?AssetIntent\b/.test(text) || /\baction:\s*["'`](open|quick-look|play|reveal|favorite|stage|import|export|identify|trash|restore)["'`]/.test(text))
      .map(([file]) => file);
    expect(offenders).toEqual([]);
  });

  it("nothing in features/ or screens/ fetches", () => {
    const offenders = others.filter(([file, text]) => /\/(features|screens)\//.test(file) && !/\.test\.ts$/.test(file) && /\bfetch\s*\(/.test(text)).map(([file]) => file);
    expect(offenders).toEqual([]);
  });
});
