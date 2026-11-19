// Runes are allowed here (the file name carries .svelte): the tests change the panel's props.
import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ASSETS, COLLECTIONS, PROJECT, TRAITS } from "../../fixtures/library";
import { intentFor, type AssetIntent } from "../asset/actions";
import type { AssetDetail, ProjectRef } from "../asset/model";
import Inspector from "./Inspector.svelte";

let app: ReturnType<typeof mount> | null = null;

afterEach(() => {
  if (app) unmount(app);
  app = null;
  document.body.innerHTML = "";
});

const byName = (name: string) => ASSETS.find((a) => a.name === name)!;
const beaver = byName("Beaver_LowPoly");

function render(asset: AssetDetail = beaver, project: ProjectRef | null = PROJECT) {
  const intents: AssetIntent[] = [];
  const handlers = { onclose: vi.fn(), oncollection: vi.fn(), onaddtag: vi.fn(), onopenasset: vi.fn(), onsearchtag: vi.fn(), onpack: vi.fn() };
  const props = $state({ asset, collections: COLLECTIONS, traits: TRAITS, project, onintent: (i: AssetIntent) => intents.push(i), ...handlers });
  app = mount(Inspector, { target: document.body, props });
  flushSync();
  return { props, intents, ...handlers };
}

const panel = () => document.querySelector<HTMLElement>('aside[aria-label="Inspector"]')!;
const text = (el: Element | null | undefined) => el?.textContent?.replace(/\s+/g, " ").trim();
const button = (label: string) => [...panel().querySelectorAll<HTMLButtonElement>("button")].find((b) => text(b) === label)!;
const section = (label: string) => panel().querySelector<HTMLElement>(`section[aria-label="${label}"]`);
const traitChips = () => [...section("Traits")!.querySelectorAll(".chips:not(.offers) .chip")].map(text);
const ctx = (asset: AssetDetail, project: ProjectRef | null = PROJECT) => ({ targets: [asset], project });

describe("Inspector — what it shows", () => {
  it("names the asset, its status in words and its type", () => {
    render();
    expect(text(panel().querySelector("h2"))).toBe("Beaver_LowPoly");
    expect(text(panel().querySelector(".line"))).toBe("Confirmed · Model");
  });

  it("lists the stat block as key/value pairs", () => {
    render();
    const rows = [...panel().querySelectorAll("dl .row")].map((r) => [text(r.querySelector("dt")), text(r.querySelector("dd"))]);
    expect(rows).toEqual([
      ["Format", "FBX"],
      ["Tris", "1,248"],
      ["Materials", "2"],
      ["Rig", "none"],
      ["Size", "4.1 MB"],
    ]);
  });

  it("shows every trait — the card's +N has no place here", () => {
    render();
    expect(traitChips()).toEqual(["animal", "lowpoly", "creature", "forest"]);
  });

  it("offers library traits that fit the asset and are not on it", () => {
    const { onaddtag } = render();
    const offers = [...section("Traits")!.querySelectorAll<HTMLElement>(".offers .chip")];
    expect(offers.map(text)).toEqual(["+hero-prop"]);
    offers[0].click();
    expect(onaddtag).toHaveBeenCalledWith("hero-prop");
  });

  it("lists linked files with where each was found", () => {
    render(byName("Cart_Wooden"));
    const rows = [...section("Linked files")!.querySelectorAll(".link")].map((r) => [text(r.querySelector(".name")), text(r.querySelector(".state"))]);
    expect(rows).toEqual([
      ["M_Wood_Planks", "in pack"],
      ["bark_oak_albedo.png", "in library"],
      ["M_Iron_Fittings", "in pack"],
      ["T_Iron_Rough.png", "in pack"],
      ["M_Rope", "in pack"],
      ["T_Rope_Albedo.png", "missing"],
    ]);
    expect(text(section("Linked files"))).toContain("parsed from FBX bindings");
    expect(text(section("Linked files"))).toContain("A missing link is skipped on export.");
  });

  it("a file found in another pack opens that asset; one inside the pack opens nothing", () => {
    const { onopenasset } = render(byName("Cart_Wooden"));
    const rows = [...section("Linked files")!.querySelectorAll<HTMLElement>(".link")];
    expect(rows.map((r) => r.tagName)).toEqual(["DIV", "BUTTON", "DIV", "DIV", "DIV", "DIV"]);
    rows[1].click();
    expect(onopenasset).toHaveBeenCalledWith(byName("bark_oak_albedo").id);
  });

  it("has no linked-files section for an asset with no dependencies", () => {
    render(byName("rock_pickup_01"));
    expect(section("Linked files")).toBeNull();
  });

  it("states provenance, and omits a licence nobody stated", () => {
    const { onpack } = render();
    const lines = () => [...panel().querySelectorAll(".prov .line")].map(text);
    expect(lines()).toEqual(["Source · Synty · Forest Pack", "License · Synty EULA", "imported in MyRPG → Assets/Imported/"]);
    button("Synty · Forest Pack").click();
    expect(onpack).toHaveBeenCalledWith(beaver.package);
    if (app) unmount(app);
    app = null;
    render({ ...byName("Rock_Mossy_03"), license: null });
    expect(lines()).toEqual(["Source · Synty · Forest Pack"]);
  });

  it("an unidentified asset: no traits yet, no collections, and Identify now is the only action", () => {
    const unidentified = byName("SM_Asset_0117");
    const { intents } = render(unidentified);
    expect(text(section("Traits"))).toContain("No traits yet — run Identify.");
    expect(section("Collections")).toBeNull();
    expect(section("Linked files")).not.toBeNull();
    expect([...panel().querySelectorAll(".actions button")].map(text)).toEqual(["Identify now"]);
    button("Identify now").click();
    expect(intents).toEqual([intentFor("identify", ctx(unidentified))]);
  });
});

describe("Inspector — its actions are the ones actions.ts defines", () => {
  it("Stage, Import and Re-identify emit the intents the card and the menu emit", () => {
    const rock = byName("Rock_Mossy_03");
    const { intents } = render(rock);
    button("Stage").click();
    button("Import").click();
    button("Re-identify").click();
    expect(intents).toEqual([intentFor("stage", ctx(rock)), intentFor("import", ctx(rock)), intentFor("identify", ctx(rock))]);
  });

  it("full size asks for Quick Look; ✕ asks to close", () => {
    const { intents, onclose } = render();
    button("full size").click();
    panel().querySelector<HTMLElement>('[aria-label="Close inspector"]')!.click();
    expect(intents).toEqual([intentFor("quick-look", ctx(beaver))]);
    expect(onclose).toHaveBeenCalledOnce();
  });

  it("Import is disabled, with the reason, when no project is connected", () => {
    const rock = byName("Rock_Mossy_03");
    const { intents } = render(rock, null);
    const importButton = button("Import");
    expect(importButton.disabled).toBe(true);
    expect(importButton.title).toMatch(/No project is connected/);
    importButton.click();
    expect(intents).toEqual([]);
  });

  it("says Staged and Imported from the asset's own state", () => {
    render({ ...beaver, staged: true });
    expect(button("Staged").getAttribute("aria-pressed")).toBe("true");
    expect(button("Imported")).toBeDefined();
  });
});

describe("Inspector — it keeps no copy of the asset (review U3)", () => {
  it("the chips are the asset's tags the moment the asset changes", () => {
    const { props } = render();
    props.asset = { ...beaver, tags: [...beaver.tags, { namespace: null, value: "hero-prop", confidence: null, source: "manual" }] };
    flushSync();
    expect(traitChips()).toEqual(["animal", "lowpoly", "creature", "forest", "★ hero-prop"]);
    // …and the trait is no longer offered.
    expect(section("Traits")!.querySelector(".offers")).toBeNull();
    props.asset = { ...beaver, tags: beaver.tags.slice(0, 1) };
    flushSync();
    expect(traitChips()).toEqual(["animal"]);
  });

  it("asking to add a trait changes nothing until the owner hands back the changed asset", () => {
    const { onaddtag } = render();
    section("Traits")!.querySelector<HTMLElement>(".offers .chip")!.click();
    flushSync();
    expect(onaddtag).toHaveBeenCalledOnce();
    expect(traitChips()).toEqual(["animal", "lowpoly", "creature", "forest"]);
  });

  it("collection chips report the toggle and follow the asset's membership", () => {
    const { props, oncollection } = render(byName("Rock_Mossy_03"));
    const chips = () => [...section("Collections")!.querySelectorAll<HTMLElement>(".chip")];
    expect(chips().map((c) => [text(c), c.getAttribute("aria-pressed")])).toEqual([
      ["✓Dungeon tileset picks", "true"],
      ["+Forest ambience", "false"],
    ]);
    chips()[1].click();
    chips()[0].click();
    expect(oncollection.mock.calls).toEqual([
      [2, true],
      [1, false],
    ]);
    // Nothing moved by itself.
    expect(chips().map((c) => c.getAttribute("aria-pressed"))).toEqual(["true", "false"]);
    props.asset = { ...props.asset, collection_ids: [2] };
    flushSync();
    expect(chips().map((c) => c.getAttribute("aria-pressed"))).toEqual(["false", "true"]);
  });

  it("selecting another asset replaces everything, with nothing left over", () => {
    const { props } = render();
    props.asset = byName("rock_pickup_01");
    flushSync();
    expect(text(panel().querySelector("h2"))).toBe("rock_pickup_01");
    expect(text(panel().querySelector(".line"))).toBe("Confirmed · Sound");
    expect(traitChips()).toEqual(["pickup", "rock", "sfx"]);
    expect(section("Linked files")).toBeNull();
    expect([...section("Traits")!.querySelectorAll(".offers .chip")].map(text)).toEqual(["+loopable"]);
  });
});
