// The prototype's library, as typed fixture data.
//
// Provenance: derived by hand from `docs/design/Lodestash Prototype.dc.html` (sha256 prefix
// aa58c48ec649d282, the hash `docs/design/contract.json` records) — `buildAssets()` for the 19
// assets, and the component's initial `state` for the collections, the trait library, the active
// project and the size-tier thresholds. The specimen pages (gallery.html#library,
// #library-inspector) and the tests render this, so the app is measured showing the same content
// the prototype was measured with. When the prototype's data changes, change this to match.
//
// What is deliberately different from the prototype:
//   - Thumbnails. The prototype paints procedural pixel art on a canvas; the app shows real images,
//     so each asset has a plain placeholder (a flat tint from its hue). Sounds have no thumbnail.
//   - Sizes are bytes; the prototype stores the formatted string ("4.1 MB").
//   - Unidentified assets carry no tags. The tags the prototype applies when one is identified
//     are in PENDING_TAGS, for the Identify screen's specimen.

import type { ShellData } from "../app/shell";
import type {
  AssetDetail,
  AssetLink,
  AssetStats,
  AssetStatus,
  AssetTag,
  Collection,
  Library,
  MediaType,
  PackageRef,
  ProjectRef,
  SizeThresholds,
  TraitDef,
} from "../features/asset/model";

const mb = (n: number) => Math.round(n * 1024 * 1024);
const kb = (n: number) => n * 1024;

const manual = (value: string): AssetTag => ({ namespace: null, value, confidence: null, source: "manual" });
const ai = (value: string, pct: number): AssetTag => ({
  namespace: null,
  value,
  confidence: pct / 100,
  source: "ai:fixture",
  model_id: "fixture",
});

/** A flat tint in place of a thumbnail: an image, as the app's previews are. */
const tint = (hue: number, sat: number) =>
  "data:image/svg+xml," +
  encodeURIComponent(
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 96 80"><rect width="96" height="80" fill="hsl(${hue} ${sat}% 24%)"/></svg>`,
  );

export const LIBRARIES: Library[] = [
  { id: 1, name: "Synty", kind: "folder", asset_count: 3 },
  { id: 2, name: "Human3", kind: "folder", asset_count: 2 },
  { id: 3, name: "Asset Store", kind: "asset_store_cache", asset_count: 8 },
  { id: 4, name: "Humble", kind: "folder", asset_count: 6 },
];

const PACKS = {
  forest: { id: 1, name: "Forest Pack", library: 1, license: "Synty EULA" },
  medieval: { id: 2, name: "Medieval Props", library: 2, license: "CC-BY 4.0" },
  city: { id: 3, name: "POLYGON City", library: 3, license: "Unity Asset Store" },
  fantasy: { id: 4, name: "Mega Fantasy Pack", library: 4, license: "Humble Bundle" },
  sfx: { id: 5, name: "RPG SFX", library: 3, license: "Unity Asset Store" },
  ambience: { id: 6, name: "Ambience Bundle", library: 4, license: "Humble Bundle" },
  nature: { id: 7, name: "Nature Textures", library: 3, license: "Unity Asset Store" },
  uikit: { id: 8, name: "Fantasy UI Kit", library: 4, license: "Humble Bundle" },
} as const;
type PackKey = keyof typeof PACKS;

export const PACKAGES: PackageRef[] = Object.values(PACKS).map(({ id, name }) => ({ id, name }));

export const PROJECT: ProjectRef = { id: 1, name: "MyRPG", engine: "Unity 6", dest: "Assets/Imported/" };

export const SIZE_THRESHOLDS: SizeThresholds = { m: 1024, l: 4096, xl: 8192 };

export const COLLECTIONS: Collection[] = [
  { id: 1, name: "Dungeon tileset picks", asset_ids: [2, 3, 18] },
  { id: 2, name: "Forest ambience", asset_ids: [7, 10] },
];

export const TRAITS: TraitDef[] = [
  {
    id: 1,
    name: "hero-prop",
    scope: "model",
    usage: "Set-dressing props important enough for close-ups — higher poly budget is fine.",
    color: "var(--ls-gold-bright)",
    icon: "★",
  },
  { id: 2, name: "loopable", scope: "sound", usage: "", color: "var(--ls-type-sound)", icon: "∞" },
  {
    id: 3,
    name: "trim-sheet",
    scope: "image",
    usage: "Textures laid out as reusable trim strips, not unique unwraps.",
    color: "var(--ls-type-image)",
    icon: "▦",
  },
];

type Dep = [kind: "mat" | "tex", name: string, state: "ok" | "miss" | number];

interface Row {
  id: number;
  media: MediaType;
  name: string;
  pack: PackKey;
  status: AssetStatus;
  format: string;
  size: number;
  stats: AssetStats;
  tags?: AssetTag[];
  /** Placeholder tint; absent for sounds. */
  hue?: [hue: number, sat: number];
  deps?: Dep[];
  imported?: boolean;
}

// `indexed` is the prototype's "unid"; `identified` its "id"; `confirmed` its "conf".
const ROWS: Row[] = [
  {
    id: 1, media: "model_3d", name: "Beaver_LowPoly", pack: "forest", status: "confirmed", format: "fbx", size: mb(4.1),
    stats: { tris: 1248, materials: 2, bones: 0 }, hue: [28, 50], imported: true,
    tags: [manual("animal"), ai("lowpoly", 94), ai("creature", 81), ai("forest", 72)],
    deps: [["mat", "M_Beaver_Body", "ok"], ["tex", "T_Beaver_Albedo.png", "ok"], ["tex", "T_Beaver_Normal.png", "ok"], ["mat", "M_Beaver_Eyes", "ok"], ["tex", "T_Eyes_Albedo.png", "ok"]],
  },
  {
    id: 2, media: "model_3d", name: "Rock_Mossy_03", pack: "forest", status: "identified", format: "fbx", size: mb(1.8),
    stats: { tris: 640, materials: 1 }, hue: [95, 22],
    tags: [ai("rock", 96), ai("moss", 84), manual("environment")],
    deps: [["mat", "M_Rock_Moss", "ok"], ["tex", "T_Rock_Moss_Albedo.png", "ok"], ["tex", "cliff_rock_normal.png", 14]],
  },
  {
    id: 3, media: "model_3d", name: "Sword_Iron_02", pack: "medieval", status: "confirmed", format: "fbx", size: mb(3.2),
    stats: { tris: 2104, materials: 1 }, hue: [212, 18],
    tags: [manual("weapon"), ai("sword", 97), ai("medieval", 88)],
    deps: [["mat", "M_Iron_Sword", "ok"], ["tex", "T_Sword_Albedo.png", "ok"], ["tex", "T_Sword_Metallic.png", "miss"]],
  },
  {
    id: 4, media: "model_3d", name: "Cart_Wooden", pack: "medieval", status: "identified", format: "fbx", size: mb(6.8),
    stats: { tris: 3410, materials: 3 }, hue: [32, 45],
    tags: [ai("prop", 91), ai("wood", 87), ai("vehicle", 64)],
    deps: [["mat", "M_Wood_Planks", "ok"], ["tex", "bark_oak_albedo.png", 13], ["mat", "M_Iron_Fittings", "ok"], ["tex", "T_Iron_Rough.png", "ok"], ["mat", "M_Rope", "ok"], ["tex", "T_Rope_Albedo.png", "miss"]],
  },
  {
    id: 5, media: "model_3d", name: "SM_Asset_0117", pack: "city", status: "indexed", format: "fbx", size: mb(9.4),
    stats: { tris: 5102, materials: 4 }, hue: [38, 35],
    deps: [["mat", "PolygonCity_Mat_01", "ok"], ["tex", "tex_0042.png", 16], ["mat", "PolygonCity_Glass", "ok"], ["mat", "PolygonCity_Trim", "ok"], ["mat", "PolygonCity_Signs", "ok"], ["tex", "tex_0087.png", "miss"]],
  },
  {
    id: 6, media: "model_3d", name: "mesh_export_final2", pack: "fantasy", status: "indexed", format: "obj", size: mb(3.7),
    stats: { tris: 1922, materials: 2 }, hue: [205, 20],
    deps: [["mat", "well_stone (well.mtl)", "ok"], ["tex", "well_diffuse.png", "miss"], ["mat", "well_wood (well.mtl)", "ok"], ["tex", "well_wood_diff.png", "miss"]],
  },
  {
    id: 7, media: "model_3d", name: "Fern_Cluster_01", pack: "forest", status: "identified", format: "fbx", size: mb(0.9),
    stats: { tris: 420, materials: 1 }, hue: [122, 42],
    tags: [ai("plant", 93), ai("foliage", 90)],
    deps: [["mat", "M_Foliage", "ok"], ["tex", "T_Fern_Atlas.png", "ok"]],
  },
  {
    id: 8, media: "audio", name: "rock_pickup_01", pack: "sfx", status: "confirmed", format: "wav", size: kb(210),
    stats: { duration_s: 1.2, sample_rate_hz: 44100, channels: 1 }, imported: true,
    tags: [manual("pickup"), ai("rock", 90), ai("sfx", 95)],
  },
  {
    id: 9, media: "audio", name: "footstep_grass_03", pack: "sfx", status: "identified", format: "wav", size: kb(140),
    stats: { duration_s: 0.8, sample_rate_hz: 44100, channels: 1 },
    tags: [ai("footstep", 96), ai("grass", 90)],
  },
  {
    id: 10, media: "audio", name: "ambience_cave_loop", pack: "ambience", status: "identified", format: "ogg", size: mb(12.4),
    stats: { duration_s: 84, sample_rate_hz: 48000, channels: 2 },
    tags: [ai("ambience", 94), ai("cave", 88), manual("loop")],
  },
  {
    id: 11, media: "audio", name: "audio_23_final", pack: "ambience", status: "indexed", format: "wav", size: kb(120),
    stats: { duration_s: 0.6, sample_rate_hz: 44100, channels: 2 },
  },
  {
    id: 12, media: "audio", name: "sword_swing_02", pack: "sfx", status: "confirmed", format: "wav", size: kb(98),
    stats: { duration_s: 0.5, sample_rate_hz: 44100, channels: 1 },
    tags: [manual("whoosh"), ai("swing", 93), ai("weapon", 87)],
  },
  {
    id: 13, media: "image", name: "bark_oak_albedo", pack: "nature", status: "confirmed", format: "png", size: mb(5.2),
    stats: { width: 2048, height: 2048, pixel_format: "RGB", color_space: "sRGB" }, hue: [26, 42], imported: true,
    tags: [manual("albedo"), ai("bark", 95), ai("tileable", 82)],
  },
  {
    id: 14, media: "image", name: "cliff_rock_normal", pack: "nature", status: "identified", format: "png", size: mb(4.8),
    stats: { width: 2048, height: 2048, pixel_format: "RGB", color_space: "linear" }, hue: [246, 55],
    tags: [ai("normalmap", 93), ai("cliff", 90)],
  },
  {
    id: 15, media: "image", name: "ui_frame_gold", pack: "uikit", status: "identified", format: "png", size: kb(240),
    stats: { width: 512, height: 512, pixel_format: "RGBA", color_space: "sRGB" }, hue: [44, 60],
    tags: [ai("ui", 96), ai("frame", 88), manual("gold")],
  },
  {
    id: 16, media: "image", name: "tex_0042", pack: "city", status: "indexed", format: "png", size: mb(1.1),
    stats: { width: 1024, height: 1024, pixel_format: "RGB", color_space: "sRGB" }, hue: [220, 8],
  },
  {
    id: 17, media: "image", name: "grass_tile_02", pack: "nature", status: "identified", format: "png", size: mb(2.0),
    stats: { width: 1024, height: 1024, pixel_format: "RGB", color_space: "sRGB" }, hue: [112, 45],
    tags: [ai("grass", 94), ai("tileable", 90)],
  },
  {
    id: 18, media: "model_3d", name: "untitled_scan_08", pack: "fantasy", status: "indexed", format: "obj", size: mb(4.9),
    stats: { tris: 2614, materials: 2 }, hue: [30, 40],
    deps: [["mat", "barrel_wood (untitled.mtl)", "ok"], ["tex", "barrel_diff.jpg", "ok"], ["mat", "barrel_metal (untitled.mtl)", "ok"], ["tex", "barrel_nrm.jpg", "miss"]],
  },
  {
    id: 19, media: "model_3d", name: "Rock_Mossy_L", pack: "fantasy", status: "identified", format: "obj", size: mb(1.9),
    stats: { tris: 655, materials: 1 }, hue: [96, 20],
    tags: [ai("rock", 95), ai("moss", 82)],
    deps: [["mat", "rock_moss (rock.mtl)", "ok"], ["tex", "rock_moss_diffuse.png", "ok"]],
  },
];

/** The tags the prototype applies when an unidentified asset is identified (its `aiTags`). */
export const PENDING_TAGS: Record<number, AssetTag[]> = {
  5: [ai("building", 92), ai("tavern", 77), ai("medieval", 71)],
  6: [ai("well", 89), ai("prop", 80), ai("stone", 74)],
  11: [ai("ui", 85), ai("chime", 80), ai("confirm", 66)],
  16: [ai("road", 87), ai("asphalt", 78), ai("tileable", 70)],
  18: [ai("barrel", 91), ai("prop", 85), ai("wood", 79)],
};

function links(row: Row): AssetLink[] {
  return (row.deps ?? []).map(([kind, name, state], i) => ({
    id: row.id * 100 + i,
    kind: kind === "mat" ? "uses_material" : "uses_texture",
    source: row.format === "obj" ? "mtl_sidecar" : "fbx_binding",
    state: state === "miss" ? "missing" : "resolved",
    same_pack: state === "ok",
    // A resolved edge points at an asset; the fixture gives in-pack ones a synthetic id.
    to_asset_id: typeof state === "number" ? state : state === "ok" ? row.id * 1000 + i : null,
    name,
    declared_path: `Textures/${name}`,
    depth: kind === "tex" ? 1 : 0,
  }));
}

function build(row: Row): AssetDetail {
  const pack = PACKS[row.pack];
  const library = LIBRARIES.find((l) => l.id === pack.library)!;
  const edges = links(row);
  return {
    id: row.id,
    uuid: `00000000-0000-4000-8000-${String(row.id).padStart(12, "0")}`,
    name: row.name,
    media_type: row.media,
    format: row.format,
    status: row.status,
    package: { id: pack.id, name: pack.name },
    tags: row.tags ?? [],
    preview_url: row.hue ? tint(...row.hue) : null,
    file_url: `/v1/assets/${row.id}/file`,
    source_id: library.id,
    package_id: pack.id,
    size_bytes: row.size,
    stats: row.stats,
    favorite: false,
    staged: false,
    imported_in: row.imported ? [{ project_id: PROJECT.id, dest_path: PROJECT.dest }] : [],
    description: null,
    categories: [],
    license: { name: pack.license },
    dependencies: { count: edges.length, asset_ids: edges.flatMap((e) => (e.to_asset_id === null ? [] : [e.to_asset_id])) },
    library: { id: library.id, name: library.name },
    links: edges,
    collection_ids: COLLECTIONS.filter((c) => c.asset_ids.includes(row.id)).map((c) => c.id),
  };
}

/** The 19 assets, in the prototype's order. A detail is a summary, so the grid takes these too. */
export const ASSETS: AssetDetail[] = ROWS.map(build);

const count = (status: AssetStatus[]) => ASSETS.filter((a) => status.includes(a.status)).length;

/** The shell's data in the prototype's `library` state, derived from the assets above. */
export function shellData(): ShellData {
  const unidentified = count(["discovered", "indexed"]);
  return {
    route: "library",
    badges: { identify: unidentified, duplicates: 1, trash: 0 },
    mode: "tags",
    query: "",
    category: "all",
    health: { identified: ASSETS.length - unidentified, total: ASSETS.length },
    project: { name: PROJECT.name, engine: PROJECT.engine, color: "var(--ls-success)" },
    device: "RTX 4070",
    status: {
      value: "all",
      counts: { all: ASSETS.length, unidentified, identified: count(["identified"]), confirmed: count(["confirmed"]) },
    },
    libraries: {
      value: "all",
      options: [
        { id: "all", label: "All libraries", count: ASSETS.length },
        ...LIBRARIES.map((l) => ({ id: String(l.id), label: l.name, count: l.asset_count })),
      ],
    },
  };
}
