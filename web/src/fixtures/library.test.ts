import { describe, expect, it } from "vitest";
import { linkState, sizeTier, statLine, statRows, userStatus } from "../features/asset/present";
import { ASSETS, COLLECTIONS, LIBRARIES, PACKAGES, PENDING_TAGS, SIZE_THRESHOLDS, shellData } from "./library";

const byName = (name: string) => ASSETS.find((a) => a.name === name)!;

describe("the fixture library is the prototype's", () => {
  it("has the 19 assets, 8 packs and 4 libraries", () => {
    expect(ASSETS).toHaveLength(19);
    expect(ASSETS.map((a) => a.id)).toEqual(Array.from({ length: 19 }, (_, i) => i + 1));
    expect(PACKAGES).toHaveLength(8);
    expect(LIBRARIES.map((l) => l.name)).toEqual(["Synty", "Human3", "Asset Store", "Humble"]);
  });

  it("splits 5 unidentified / 9 identified / 5 confirmed", () => {
    const n = (s: string) => ASSETS.filter((a) => userStatus(a.status) === s).length;
    expect([n("unidentified"), n("identified"), n("confirmed")]).toEqual([5, 9, 5]);
  });

  it("counts each library's assets as the sidebar shows them", () => {
    const counted = LIBRARIES.map((l) => ASSETS.filter((a) => a.source_id === l.id).length);
    expect(counted).toEqual([3, 2, 8, 6]);
    expect(LIBRARIES.map((l) => l.asset_count)).toEqual(counted);
  });

  it("derives the shell's counts from the assets", () => {
    const shell = shellData();
    expect(shell.health).toEqual({ identified: 14, total: 19 });
    expect(shell.badges.identify).toBe(5);
    expect(shell.status.counts).toEqual({ all: 19, unidentified: 5, identified: 9, confirmed: 5 });
  });

  it("gives every size the tier the prototype draws", () => {
    const tiers = ASSETS.map((a) => sizeTier(a.size_bytes, SIZE_THRESHOLDS)).join(" ");
    expect(tiers).toBe("L M M L XL M S S S XL S S L L S M M L M");
  });

  it("writes the card's stat line as the prototype does", () => {
    expect(statLine(byName("Beaver_LowPoly"))).toBe("1.2k tris · 4.1 MB");
    expect(statLine(byName("Rock_Mossy_03"))).toBe("640 tris · 1.8 MB");
    expect(statLine(byName("rock_pickup_01"))).toBe("0:01 · 44.1 kHz");
    expect(statLine(byName("footstep_grass_03"))).toBe("0:00.8 · 44.1 kHz");
    expect(statLine(byName("ambience_cave_loop"))).toBe("1:24 · 48 kHz");
    expect(statLine(byName("bark_oak_albedo"))).toBe("2048² · 5.2 MB");
    expect(statLine(byName("ui_frame_gold"))).toBe("512² · 240 KB");
  });

  it("writes the stat block's rows as the prototype does", () => {
    expect(statRows(byName("Beaver_LowPoly"))).toEqual([
      ["Format", "FBX"],
      ["Tris", "1,248"],
      ["Materials", "2"],
      ["Rig", "none"],
      ["Size", "4.1 MB"],
    ]);
    expect(statRows(byName("rock_pickup_01"))).toEqual([
      ["Format", "WAV"],
      ["Duration", "0:01.2"],
      ["Sample rate", "44.1 kHz"],
      ["Channels", "mono"],
      ["Size", "210 KB"],
    ]);
    expect(statRows(byName("cliff_rock_normal"))).toEqual([
      ["Format", "PNG"],
      ["Resolution", "2048 × 2048"],
      ["Channels", "RGB"],
      ["Color space", "linear"],
      ["Size", "4.8 MB"],
    ]);
  });

  it("carries the dependency states: in pack, in library, missing", () => {
    expect(byName("Beaver_LowPoly").links.map(linkState)).toEqual(Array(5).fill("in_pack"));
    const rock = byName("Rock_Mossy_03").links;
    expect(rock.map(linkState)).toEqual(["in_pack", "in_pack", "in_library"]);
    expect(rock[2].to_asset_id).toBe(byName("cliff_rock_normal").id);
    expect(byName("Sword_Iron_02").links.map(linkState)).toEqual(["in_pack", "in_pack", "missing"]);
    expect(ASSETS.filter((a) => a.media_type !== "model_3d").every((a) => a.links.length === 0)).toBe(true);
  });

  it("leaves unidentified assets untagged, their pending tags aside", () => {
    const unidentified = ASSETS.filter((a) => userStatus(a.status) === "unidentified");
    expect(unidentified.every((a) => a.tags.length === 0)).toBe(true);
    expect(Object.keys(PENDING_TAGS).map(Number)).toEqual(unidentified.map((a) => a.id));
  });

  it("marks the three assets already in the active project, and the collections", () => {
    expect(ASSETS.filter((a) => a.imported_in.length).map((a) => a.name)).toEqual(["Beaver_LowPoly", "rock_pickup_01", "bark_oak_albedo"]);
    expect(byName("Rock_Mossy_03").collection_ids).toEqual([1]);
    expect(COLLECTIONS.map((c) => c.asset_ids.length)).toEqual([3, 2]);
  });

  it("gives sounds no thumbnail and everything else a plain image", () => {
    for (const a of ASSETS) {
      if (a.media_type === "audio") expect(a.preview_url).toBeNull();
      else expect(a.preview_url).toMatch(/^data:image\/svg\+xml,/);
    }
  });
});
