import { describe, expect, it } from "vitest";
import { ASSETS, PROJECT, TRAITS } from "../../fixtures/library";
import type { AssetLink, AssetTag } from "./model";
import { chipOf, formatBytes, importedState, linksCaption, linkState, quickAddTraits, sizeTier, statLine, typeLabel, typeTone, userStatus } from "./present";

const tag = (value: string, source: string, confidence: number | null = null): AssetTag => ({ namespace: null, value, confidence, source });

describe("present", () => {
  it("folds discovered and indexed into Unidentified", () => {
    expect(["discovered", "indexed", "identified", "confirmed"].map((s) => userStatus(s as never))).toEqual([
      "unidentified",
      "unidentified",
      "identified",
      "confirmed",
    ]);
  });

  it("names and colours all seven media types", () => {
    const types = ["model_3d", "texture", "audio", "image", "sprite", "material", "font"] as const;
    expect(types.map(typeTone)).toEqual(["model", "texture", "sound", "image", "sprite", "material", "font"]);
    expect(types.map(typeLabel)).toEqual(["Model", "Texture", "Sound", "Image", "Sprite", "Material", "Font"]);
  });

  it("formats sizes with one decimal above a megabyte and whole kilobytes below", () => {
    expect(formatBytes(98 * 1024)).toBe("98 KB");
    expect(formatBytes(Math.round(0.9 * 1024 * 1024))).toBe("922 KB");
    expect(formatBytes(1024 * 1024)).toBe("1.0 MB");
    expect(formatBytes(1.5 * 1024 * 1024 * 1024)).toBe("1.5 GB");
  });

  it("puts a size on the boundary in the upper tier", () => {
    const t = { m: 1024, l: 4096, xl: 8192 };
    expect([1023, 1024, 4095, 4096, 8191, 8192].map((kb) => sizeTier(kb * 1024, t))).toEqual(["S", "M", "M", "L", "L", "XL"]);
  });

  it("falls back to the size when a modality's metric is not known", () => {
    expect(statLine({ ...ASSETS[0], stats: {} })).toBe("4.1 MB");
    expect(statLine({ ...ASSETS[0], media_type: "material", stats: {} })).toBe("4.1 MB");
  });

  it("draws a tag by where it came from", () => {
    expect(chipOf(tag("rock", "ai:siglip", 0.96))).toEqual({ name: "rock", source: "ai", confidence: 0.96 });
    expect(chipOf(tag("rock", "ai:legacy"))).toEqual({ name: "rock", source: "ai", confidence: undefined });
    expect(chipOf(tag("Props", "imported"))).toEqual({ name: "Props", source: "imported" });
    expect(chipOf(tag("animal", "manual"), TRAITS)).toEqual({ name: "animal", source: "human", color: undefined, icon: undefined });
    expect(chipOf(tag("hero-prop", "manual"), TRAITS)).toMatchObject({ source: "human", color: TRAITS[0].color, icon: "★" });
  });

  it("reads a link's four states", () => {
    const base: AssetLink = { id: 1, kind: "uses_texture", source: "unity_meta", state: "resolved", same_pack: true, to_asset_id: 9, name: "t", declared_path: null, depth: 1 };
    expect(linkState(base)).toBe("in_pack");
    expect(linkState({ ...base, same_pack: false })).toBe("in_library");
    expect(linkState({ ...base, state: "missing", to_asset_id: null })).toBe("missing");
    expect(linkState({ ...base, to_asset_id: null })).toBe("missing");
    expect(linkState({ ...base, state: "ignored" })).toBe("ignored");
  });

  it("captions linked files with where they were parsed from", () => {
    expect(linksCaption(ASSETS[0].links)).toBe("parsed from FBX bindings");
    expect(linksCaption(ASSETS[5].links)).toBe("parsed from .mtl sidecar");
    const unity = [
      { ...ASSETS[0].links[0], source: "unity_meta" as const },
      { ...ASSETS[0].links[1], source: "unity_prefab" as const },
    ];
    expect(linksCaption(unity)).toBe("parsed from .meta GUID refs · prefab bindings");
    expect(linksCaption([])).toBe("");
  });

  it("distinguishes imported here, imported elsewhere and never imported", () => {
    expect(importedState(ASSETS[0], PROJECT)).toEqual({ tone: "active", label: "in MyRPG", line: "imported in MyRPG → Assets/Imported/" });
    expect(importedState({ imported_in: [{ project_id: 7, dest_path: "Assets/X" }] }, PROJECT)).toEqual({ tone: "others", label: "in 1 other" });
    expect(importedState(ASSETS[0], null)).toEqual({ tone: "others", label: "in 1 other" });
    expect(importedState(ASSETS[1], PROJECT)).toBeNull();
  });

  it("offers the library traits that fit an asset and are not on it", () => {
    expect(quickAddTraits(ASSETS[0], TRAITS).map((t) => t.name)).toEqual(["hero-prop"]);
    expect(quickAddTraits(ASSETS[7], TRAITS).map((t) => t.name)).toEqual(["loopable"]);
    const tagged = { ...ASSETS[0], tags: [...ASSETS[0].tags, tag("hero-prop", "manual")] };
    expect(quickAddTraits(tagged, TRAITS)).toEqual([]);
  });
});
