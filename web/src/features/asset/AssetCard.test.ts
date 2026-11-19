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

function render(assets: AssetSummary[] = ASSETS) {
  const selection = new Selection();
  app = mount(GridHarness, { target: document.body, props: { assets, selection } });
  flushSync();
  return selection;
}

const card = (id: number) => document.querySelector<HTMLElement>(`[role="gridcell"][data-id="${id}"]`)!;
const text = (el: Element | null) => el?.textContent?.replace(/\s+/g, " ").trim();
const chips = (id: number) => [...card(id).querySelectorAll(".chips .chip")].map(text);

describe("AssetCard", () => {
  it("shows the name, the pack and the stat line", () => {
    render();
    expect(text(card(1).querySelector(".open"))).toBe("Beaver_LowPoly");
    expect(text(card(1).querySelector(".meta"))).toBe("Forest Pack · 1.2k tris · 4.1 MB");
    expect(text(card(8).querySelector(".meta"))).toBe("RPG SFX · 0:01 · 44.1 kHz");
  });

  it("shows the first three tags and a +N expander that opens the rest", () => {
    render();
    expect(chips(1)).toEqual(["animal", "lowpoly", "creature", "+1"]);
    const more = [...card(1).querySelectorAll<HTMLElement>(".chips button")].at(-1)!;
    expect(more.getAttribute("aria-expanded")).toBe("false");
    more.click();
    flushSync();
    expect(chips(1)).toEqual(["animal", "lowpoly", "creature", "forest", "− less"]);
    // Three tags: nothing to expand.
    expect(chips(2)).toEqual(["rock", "moss", "environment"]);
  });

  it("marks a machine tag with its confidence, and a hand-written one without", () => {
    render();
    const [animal, lowpoly] = [...card(1).querySelectorAll(".chips .chip")];
    expect(animal.querySelector('[role="meter"]')).toBeNull();
    expect(lowpoly.querySelector('[role="meter"]')?.getAttribute("aria-valuenow")).toBe("94");
  });

  it("an unidentified asset shows ??? in place of tags, and its status three ways", () => {
    render();
    expect(chips(5)).toEqual(["???"]);
    expect(card(5).classList.contains("unidentified")).toBe(true);
    expect(card(5).querySelector('[role="img"][aria-label="Unidentified"]')).not.toBeNull();
    expect(card(5).querySelector(".thumb .mark")?.textContent).toBe("?");
  });

  it("names the type beside the colour", () => {
    render();
    expect(card(1).querySelector('.title [role="img"]')?.getAttribute("aria-label")).toBe("Model");
    expect(card(8).querySelector('.title [role="img"]')?.getAttribute("aria-label")).toBe("Sound");
    expect(card(13).querySelector('.title [role="img"]')?.getAttribute("aria-label")).toBe("Image");
  });

  it("shows the size tier as its letter, with the size in the tooltip", () => {
    render();
    const tier = card(1).querySelector(".tier")!;
    expect(text(tier)).toBe("L");
    expect(tier.getAttribute("title")).toBe("4.1 MB on disk");
  });

  it("badges an asset already in the active project, and only those", () => {
    render();
    expect(text(card(1).querySelector(".imported"))).toBe("in MyRPG");
    expect(card(2).querySelector(".imported")).toBeNull();
  });

  it("a picture has a thumbnail image; a sound has a play button and no image", () => {
    render();
    expect(card(1).querySelector(".thumb img")?.getAttribute("src")).toMatch(/^data:image/);
    expect(card(8).querySelector(".thumb img")).toBeNull();
    expect(card(8).querySelector('button[aria-label="Play rock_pickup_01"]')).not.toBeNull();
    expect(card(1).querySelector('button[aria-label^="Play"]')).toBeNull();
  });

  it("reflects favourite and staged from the asset it is given — it keeps no copy", () => {
    const starred = ASSETS.map((a) => (a.id === 2 ? { ...a, favorite: true, staged: true } : a));
    render(starred);
    expect(card(2).querySelector('[aria-label="Remove from Favorites"]')?.getAttribute("aria-pressed")).toBe("true");
    expect([...card(2).querySelectorAll("button")].some((b) => text(b) === "✓ staged")).toBe(true);
    expect(card(1).querySelector('[aria-label="Add to Favorites"]')?.getAttribute("aria-pressed")).toBe("false");
  });

  it("the lead is selected; so is a picked card", () => {
    const selection = render();
    selection.moveTo(3);
    selection.toggle(6);
    flushSync();
    const selected = [...document.querySelectorAll('[role="gridcell"][aria-selected="true"]')].map((c) => (c as HTMLElement).dataset.id);
    expect(selected).toEqual(["3", "6"]);
    expect(card(3).classList.contains("selected")).toBe(true);
    expect(card(6).classList.contains("selected")).toBe(false);
  });
});
