// Runes are allowed here (the file name carries .svelte): the test owns `quickLook`, as the app's
// owner of the page does, and changes it.
import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { keymap } from "../app/keymap.svelte";
import { resetOverlays, setAppRoot } from "../app/overlay";
import { intentFor, type AssetIntent } from "../features/asset/actions";
import type { AssetSummary } from "../features/asset/model";
import { DEFAULT_VIEW, type LibraryView } from "../features/library/view";
import { ASSETS, PROJECT, SIZE_THRESHOLDS, TRAITS, wavePeaks } from "../fixtures/library";
import type { Playback } from "../state/playback";
import { Selection } from "../state/selection.svelte";
import LibraryScreen from "./LibraryScreen.svelte";

let app: ReturnType<typeof mount> | null = null;
let detach: () => void;
let root: HTMLElement;

beforeEach(() => {
  keymap.reset();
  detach = keymap.attach(window);
  root = document.createElement("div");
  root.id = "app";
  document.body.appendChild(root);
  setAppRoot(root);
});

afterEach(() => {
  if (app) unmount(app);
  app = null;
  detach();
  setAppRoot(null);
  resetOverlays();
  document.body.innerHTML = "";
});

function render(options: { view?: LibraryView; assets?: AssetSummary[] } = {}) {
  const selection = new Selection();
  const intents: AssetIntent[] = [];
  const onseek = vi.fn();
  const props = $state({
    assets: options.assets ?? (ASSETS as AssetSummary[]),
    view: options.view ?? DEFAULT_VIEW,
    selection,
    project: PROJECT,
    traits: TRAITS,
    thresholds: SIZE_THRESHOLDS,
    quickLook: null as number | null,
    playback: null as Playback | null,
    peaks: wavePeaks(8) as readonly number[] | null,
    // The owner: a quick-look intent opens it, whoever asked; everything else is recorded.
    onintent: (intent: AssetIntent) => {
      intents.push(intent);
      if (intent.action === "quick-look") props.quickLook = intent.ids[0];
    },
    onquicklook: (id: number | null) => {
      props.quickLook = id;
    },
    onseek,
  });
  app = mount(LibraryScreen, { target: root, props });
  flushSync();
  return { props, selection, intents, onseek };
}

const cell = (id: number) => document.querySelector<HTMLElement>(`[role="gridcell"][data-id="${id}"]`)!;
const dialog = () => document.querySelector<HTMLElement>('[role="dialog"][aria-modal="true"]');
const inDialog = (el: Element | null) => !!el && !!dialog()?.contains(el);
const button = (text: string) => [...dialog()!.querySelectorAll<HTMLButtonElement>("button")].find((b) => b.textContent?.trim().includes(text))!;
const target = (id: number) => ({ targets: ASSETS.filter((a) => a.id === id), project: PROJECT });

function press(key: string, init: KeyboardEventInit = {}) {
  const event = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init });
  (document.activeElement ?? document.body).dispatchEvent(event);
  flushSync();
  return event;
}

/** Focus a card and press Space on it: the keyboard's way in. */
function quickLook(id: number) {
  cell(id).focus();
  press(" ");
}

describe("Quick Look — opening", () => {
  it("Space on a card opens it for that asset, which becomes the lead", () => {
    const { selection, intents } = render();
    quickLook(3);
    expect(intents).toEqual([intentFor("quick-look", target(3))]);
    expect(dialog()?.getAttribute("aria-label")).toBe("Quick Look: Sword_Iron_02");
    expect(dialog()?.querySelector("h2")?.textContent).toBe("Sword_Iron_02");
    expect(selection.lead).toBe(3);
  });

  it("a card's ⤢ opens it too, and the pick set is left alone", () => {
    const { selection } = render();
    selection.toggle(5);
    selection.toggle(6);
    cell(9).querySelector<HTMLElement>('[aria-label="Quick Look"]')!.click();
    flushSync();
    expect(dialog()?.getAttribute("aria-label")).toBe("Quick Look: footstep_grass_03");
    expect([selection.lead, [...selection.picks]]).toEqual([9, [5, 6]]);
  });

  it("is modal: the page behind is inert and focus is inside", () => {
    render();
    quickLook(3);
    expect(root.hasAttribute("inert")).toBe(true);
    expect(document.activeElement).toBe(dialog());
    expect(keymap.scopes).toEqual(["global", "dialog"]);
  });
});

describe("Quick Look — focus", () => {
  it("Tab and Shift+Tab cycle inside the dialog", () => {
    render();
    quickLook(3);
    const seen = new Set<Element>();
    for (let i = 0; i < 8; i++) {
      press("Tab");
      expect(inDialog(document.activeElement)).toBe(true);
      seen.add(document.activeElement!);
      // jsdom does not move focus on Tab itself: walk on from wherever the trap left it.
      const controls = [...dialog()!.querySelectorAll<HTMLElement>("button")];
      const at = controls.indexOf(document.activeElement as HTMLElement);
      if (at >= 0 && at < controls.length - 1) controls[at + 1].focus();
    }
    button("next").focus();
    press("Tab");
    expect(document.activeElement).toBe(dialog()!.querySelector("button"));
    press("Tab", { shiftKey: true });
    expect(document.activeElement).toBe(button("next"));
  });

  it("Escape closes it and focus returns to the card that opened it", () => {
    const { props } = render();
    quickLook(3);
    press("Escape");
    expect(dialog()).toBeNull();
    expect(props.quickLook).toBeNull();
    expect(document.activeElement).toBe(cell(3));
    expect(root.hasAttribute("inert")).toBe(false);
    expect(keymap.scopes).toEqual(["global"]);
  });

  it("opened by a card's ⤢, focus returns to the card — not to a button inside it", () => {
    render();
    const opener = cell(9).querySelector<HTMLElement>('[aria-label="Quick Look"]')!;
    opener.focus();
    opener.click();
    flushSync();
    press("Escape");
    expect(document.activeElement).toBe(cell(9));
  });

  it("after stepping, focus returns to the card of the asset last shown", () => {
    const { selection } = render();
    quickLook(3);
    press("ArrowRight");
    press("ArrowRight");
    press("Escape");
    expect(selection.lead).toBe(5);
    expect(document.activeElement).toBe(cell(5));
    expect(cell(5).tabIndex).toBe(0);
  });

  it("opened from outside the grid, focus goes back there", () => {
    const { props, selection } = render();
    const outside = document.createElement("button");
    root.appendChild(outside);
    outside.focus();
    selection.moveTo(4);
    props.quickLook = 4; // the inspector's "full size"
    flushSync();
    expect(dialog()).not.toBeNull();
    press("Escape");
    expect(document.activeElement).toBe(outside);
  });

  it("in the list view it is a row that gets focus back", () => {
    render({ view: { ...DEFAULT_VIEW, layout: "list" } });
    quickLook(7);
    press("ArrowLeft");
    press(" ");
    expect(dialog()).toBeNull();
    expect(document.activeElement).toBe(cell(6));
    expect(cell(6).querySelector(".chips")).toBeNull();
  });
});

describe("Quick Look — keys", () => {
  it("Space closes it; on a focused button Space is the button's", () => {
    const { props } = render();
    quickLook(3);
    button("Stage").focus();
    const event = press(" ");
    expect(event.defaultPrevented).toBe(false);
    expect(props.quickLook).toBe(3);
    dialog()!.focus();
    press(" ");
    expect(props.quickLook).toBeNull();
  });

  it("← → step through the page in the order shown, the lead goes along, and the ends wrap", () => {
    const { props, selection } = render();
    quickLook(1);
    press("ArrowRight");
    expect([props.quickLook, selection.lead]).toEqual([2, 2]);
    expect(dialog()?.getAttribute("aria-label")).toBe("Quick Look: Rock_Mossy_03");
    press("ArrowLeft");
    press("ArrowLeft");
    expect([props.quickLook, selection.lead]).toEqual([19, 19]);
    press("ArrowRight");
    expect(props.quickLook).toBe(1);
    // The same dialog all along: stepping changes what it shows, not whether it is open.
    expect(document.activeElement).toBe(dialog());
  });

  it("prev and next do what the arrows do", () => {
    const { props } = render();
    quickLook(4);
    button("next").click();
    flushSync();
    expect(props.quickLook).toBe(5);
    button("prev").click();
    button("prev").click();
    flushSync();
    expect(props.quickLook).toBe(3);
  });

  it("nothing global fires behind it: not F, not Delete, not the grid's arrows", () => {
    const { intents, selection } = render();
    selection.toggle(2);
    quickLook(3);
    intents.length = 0;
    ["f", "Delete", "ArrowDown", "Home", "Enter"].forEach((key) => press(key));
    press("a", { ctrlKey: true });
    expect(intents).toEqual([]);
    expect([selection.lead, [...selection.picks]]).toEqual([3, [2]]);
    // Escape closes Quick Look and nothing else: the picks and the lead are still there.
    press("Escape");
    expect([selection.lead, selection.picks.size]).toEqual([3, 1]);
  });

  it("the arrows on the scrubber move the scrubber, not the asset", () => {
    const { props } = render();
    quickLook(8);
    const scrubber = dialog()!.querySelector<HTMLInputElement>('input[type="range"]')!;
    scrubber.focus();
    const event = press("ArrowRight");
    expect(event.defaultPrevented).toBe(false);
    expect(props.quickLook).toBe(8);
  });
});

describe("Quick Look — what it shows", () => {
  it("a model: its pre-render and the caption that says what will replace it", () => {
    render();
    quickLook(1);
    const stage = dialog()!.querySelector('[data-media="model_3d"]')!;
    expect(stage.querySelector("img")).not.toBeNull();
    expect(stage.textContent).toContain("low-res pre-render · orbit viewport in app");
    expect(dialog()!.textContent).toContain("Model — low-res pre-render");
    expect(dialog()!.querySelector('input[type="range"]')).toBeNull();
  });

  it("an image: the picture, whole, and no caption", () => {
    render();
    quickLook(13);
    const stage = dialog()!.querySelector('[data-media="image"]')!;
    expect(stage.querySelector("img")).not.toBeNull();
    expect(stage.textContent?.trim()).toBe("");
    expect(dialog()!.textContent).toContain("Image — full preview");
  });

  it("a sound: thirty bars and the transport; play is an intent, a seek is reported", () => {
    const { intents, onseek } = render();
    quickLook(8);
    intents.length = 0;
    const stage = dialog()!.querySelector('[data-media="audio"]')!;
    expect(stage.querySelector("img")).toBeNull();
    expect(stage.querySelectorAll(".wave > i").length).toBe(30);
    expect(stage.textContent).toContain("0:00.0 / 0:01.2");
    stage.querySelector<HTMLElement>('[aria-label="Play rock_pickup_01"]')!.click();
    expect(intents).toEqual([intentFor("play", target(8))]);
    const scrubber = stage.querySelector<HTMLInputElement>('input[type="range"]')!;
    scrubber.value = "0.6";
    scrubber.dispatchEvent(new Event("input", { bubbles: true }));
    expect(onseek).toHaveBeenCalledWith(0.6);
  });

  it("shows playback only for the sound that is playing, with the played part of the wave marked", () => {
    const { props } = render();
    quickLook(8);
    const heard = () => dialog()!.querySelectorAll(".wave > i.heard").length;
    expect(heard()).toBe(0);
    props.playback = { id: 8, playing: true, position: 0.6 };
    flushSync();
    expect(dialog()!.querySelector('[aria-label="Pause rock_pickup_01"]')).not.toBeNull();
    expect(dialog()!.textContent).toContain("0:00.6 / 0:01.2");
    expect(heard()).toBe(16);
    props.playback = { id: 9, playing: true, position: 0.4 };
    flushSync();
    expect(dialog()!.querySelector('[aria-label="Play rock_pickup_01"]')).not.toBeNull();
    expect(heard()).toBe(0);
  });

  it("Stage and Import emit the intents actions.ts defines; the stat block and traits are the asset's", () => {
    const { intents } = render();
    quickLook(2);
    intents.length = 0;
    button("Stage").click();
    button("Import").click();
    expect(intents).toEqual([intentFor("stage", target(2)), intentFor("import", target(2))]);
    expect([...dialog()!.querySelectorAll("dt")].map((d) => d.textContent)).toEqual(["Format", "Tris", "Materials", "Size"]);
    expect([...dialog()!.querySelectorAll(".chips .chip")].map((c) => c.textContent?.trim())).toEqual(["rock", "moss", "environment"]);
    // Read-only here: a trait is not a button.
    expect(dialog()!.querySelector(".chips button")).toBeNull();
  });

  it("an unidentified asset shows neither Stage nor Import, and no traits", () => {
    render();
    quickLook(5);
    expect(dialog()!.textContent).toContain("Unidentified");
    expect(button("Stage")).toBeUndefined();
    expect(button("Import")).toBeUndefined();
    expect(dialog()!.querySelector(".chips")).toBeNull();
    expect(dialog()!.querySelector('[data-media="model_3d"]')?.textContent).not.toContain("pre-render");
  });

  it("reflects staged and imported from the asset it is handed", () => {
    const { props } = render();
    quickLook(1);
    expect(button("Imported")).toBeDefined();
    expect(button("Stage").getAttribute("aria-pressed")).toBe("false");
    props.assets = ASSETS.map((a) => (a.id === 1 ? { ...a, staged: true } : a));
    flushSync();
    expect(button("Staged").getAttribute("aria-pressed")).toBe("true");
  });

  it("closes when the asset it shows leaves the page", () => {
    const { props } = render();
    quickLook(3);
    props.assets = ASSETS.filter((a) => a.id !== 3);
    flushSync();
    expect(props.quickLook).toBeNull();
    expect(dialog()).toBeNull();
  });
});
