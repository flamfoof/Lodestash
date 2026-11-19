import { describe, expect, it } from "vitest";
import { ASSETS, SIZE_THRESHOLDS } from "../../fixtures/library";
import type { AssetSummary } from "../asset/model";
import { activeFilters } from "./active";
import { filterCount, NO_FILTERS, passes, sizeRanges, toggleFilter, traitCounts, triBucket, type LibraryFilters } from "./filters";

const names = (filters: LibraryFilters, assets: readonly AssetSummary[] = ASSETS) =>
  assets.filter((a) => passes(a, filters, SIZE_THRESHOLDS)).map((a) => a.name);
const with_ = (patch: Partial<LibraryFilters>): LibraryFilters => ({ ...NO_FILTERS, ...patch });

describe("the ⚑ filter predicate", () => {
  it("lets everything through with nothing chosen", () => {
    expect(names(NO_FILTERS)).toHaveLength(ASSETS.length);
  });

  it("is OR within a group", () => {
    expect(names(with_({ traits: ["rock"] }))).toEqual(["Rock_Mossy_03", "rock_pickup_01", "Rock_Mossy_L"]);
    expect(names(with_({ traits: ["rock", "sword"] }))).toEqual(["Rock_Mossy_03", "Sword_Iron_02", "rock_pickup_01", "Rock_Mossy_L"]);
    expect(names(with_({ sizes: ["XL"] }))).toEqual(["SM_Asset_0117", "ambience_cave_loop"]);
    expect(names(with_({ sizes: ["XL", "L"] }))).toHaveLength(7);
  });

  it("is AND across groups — the prototype's rock ∧ ≤ 1k ∧ M leaves the two mossy rocks", () => {
    expect(names(with_({ traits: ["rock"], tris: ["low"], sizes: ["M"] }))).toEqual(["Rock_Mossy_03", "Rock_Mossy_L"]);
    expect(names(with_({ traits: ["rock"], tris: ["mid"] }))).toEqual([]);
  });

  it("an active triangles group excludes everything without a triangle count", () => {
    const low = names(with_({ tris: ["low"] }));
    expect(low).toEqual(["Rock_Mossy_03", "Fern_Cluster_01", "Rock_Mossy_L"]);
    const unmeasured: AssetSummary = { ...ASSETS[0], id: 99, stats: {} };
    expect(passes(unmeasured, with_({ tris: ["low", "mid", "high", "ultra"] }), SIZE_THRESHOLDS)).toBe(false);
    // …but it is not excluded by the other groups for lacking one.
    expect(passes(unmeasured, with_({ sizes: ["L"] }), SIZE_THRESHOLDS)).toBe(true);
  });

  it("an unidentified asset has no tags, so a trait filter leaves it out", () => {
    const unidentified = ASSETS.filter((a) => a.status === "indexed").map((a) => a.name);
    const all = traitCounts(ASSETS).map((t) => t.name);
    const shown = names(with_({ traits: all }));
    for (const name of unidentified) expect(shown).not.toContain(name);
  });

  it("buckets triangles with inclusive upper bounds", () => {
    expect([1, 1000, 1001, 3000, 3001, 6000, 6001].map(triBucket)).toEqual(["low", "low", "mid", "mid", "high", "high", "ultra"]);
    expect(triBucket(undefined)).toBeNull();
    expect(triBucket(0)).toBeNull();
  });
});

describe("the filter set", () => {
  it("toggles a value in and out without changing the set it was given", () => {
    const one = toggleFilter(NO_FILTERS, "traits", "rock");
    expect(one.traits).toEqual(["rock"]);
    expect(NO_FILTERS.traits).toEqual([]);
    const two = toggleFilter(toggleFilter(one, "tris", "low"), "sizes", "M");
    expect(filterCount(two)).toBe(3);
    expect(toggleFilter(two, "traits", "rock").traits).toEqual([]);
    expect(filterCount(toggleFilter(two, "traits", "rock"))).toBe(2);
  });

  it("clears to a set that constrains nothing", () => {
    expect(filterCount(NO_FILTERS)).toBe(0);
    expect(names(NO_FILTERS)).toHaveLength(19);
  });

  it("lists traits most frequent first, ties in first-seen order — the prototype's chip order", () => {
    const traits = traitCounts(ASSETS);
    expect(traits.slice(0, 7).map((t) => t.name)).toEqual(["rock", "moss", "weapon", "grass", "tileable", "animal", "lowpoly"]);
    expect(traits[0]).toEqual({ name: "rock", count: 3 });
    expect(traits).toHaveLength(32);
  });

  it("states each size tier's live range", () => {
    expect(sizeRanges(SIZE_THRESHOLDS)).toEqual({ S: "0–1.0 MB", M: "1.0 MB–4.0 MB", L: "4.0 MB–8.0 MB", XL: "8.0 MB+" });
    expect(sizeRanges({ m: 512, l: 2048, xl: 16384 }).M).toBe("512 KB–2.0 MB");
  });
});

describe("the active-filter list", () => {
  it("is empty when nothing constrains the result", () => {
    expect(activeFilters({}, NO_FILTERS, false, SIZE_THRESHOLDS)).toEqual([]);
    expect(activeFilters({ query: "   " }, NO_FILTERS, false, SIZE_THRESHOLDS)).toEqual([]);
  });

  it("lists every constraint in the order the filters compose", () => {
    const list = activeFilters(
      {
        query: "moss",
        category: { id: "model", label: "Models" },
        status: "identified",
        library: { id: 1, name: "Synty" },
        pack: { id: 1, name: "Forest Pack" },
        collection: { id: 2, name: "Forest ambience" },
      },
      { traits: ["rock"], tris: ["mid", "low"], sizes: ["XL", "M"] },
      true,
      SIZE_THRESHOLDS,
    );
    expect(list.map((f) => f.axis)).toEqual(["query", "type", "status", "library", "pack", "collection", "trait", "tris", "tris", "size", "size", "favorite"]);
    expect(list.map((f) => f.label)).toEqual([
      "moss",
      "Models",
      "Identified",
      "Synty",
      "Forest Pack",
      "Forest ambience",
      "rock",
      "≤ 1k tris",
      "1k–3k tris",
      "M 1.0 MB–4.0 MB",
      "XL 8.0 MB+",
      "Favorites",
    ]);
    expect(new Set(list.map((f) => f.key)).size).toBe(list.length);
  });

  it("gives library, pack and collection a prefix that names the axis", () => {
    const list = activeFilters({ library: { id: 1, name: "Synty" }, pack: { id: 1, name: "Forest Pack" } }, NO_FILTERS, false, SIZE_THRESHOLDS);
    expect(list.map((f) => f.prefix)).toEqual(["lib", "pack"]);
  });
});
