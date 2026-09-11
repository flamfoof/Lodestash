//! Dependency harvesting: an asset's own Unity GUID (from its `.meta` sidecar) and the GUIDs it
//! declares, which become `asset_relationship` edges (DATA-MODEL §10, INGESTION §5 step 1).
//!
//! Only Unity's own bookkeeping is read here — a material's texture refs (`source = 'unity_meta'`)
//! and the renderer bindings a `.prefab` declares (`unity_prefab`, ADR-0018). The path/naming
//! heuristic for raw exports (step 2) is a separate pass.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::unitypackage::parse_meta;

/// The `.meta` sidecar Unity writes beside every asset: `Rock.fbx` → `Rock.fbx.meta`.
pub fn meta_sidecar(asset_path: &Path) -> PathBuf {
    let mut s = asset_path.as_os_str().to_os_string();
    s.push(".meta");
    PathBuf::from(s)
}

/// This asset's own GUID, read from its `.meta` sidecar. `None` when the sidecar is absent
/// (raw exports outside a Unity project) or unreadable — the caller treats that as "nothing to
/// harvest", not an error.
pub fn guid_of(asset_path: &Path) -> Option<String> {
    let yaml = std::fs::read_to_string(meta_sidecar(asset_path)).ok()?;
    parse_meta(&yaml).guid
}

/// GUIDs a Unity material references as **textures**, deduped, in declaration order — the
/// `uses_texture` edges. [`material_texture_bindings`] keeps which property each one fills.
pub fn material_texture_guids(yaml: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (_, guid) in material_texture_bindings(yaml) {
        push_unique(&mut out, guid);
    }
    out
}

/// Every texture binding a Unity material declares, as `(property, GUID)` — `_BaseMap`,
/// `_MainTex`, `_BumpMap`… The property is what says which texture is the albedo and which a
/// normal or a mask; without it the texture ids are all that is left to choose by (v1 plan S06).
///
/// A `.mat` is YAML with lines like
/// `m_Texture: {fileID: 2800000, guid: b24b57e0cfed05643be34b0256df875e, type: 3}`.
/// Only `m_Texture` bindings are taken: the same file also carries `m_Shader` and `m_Script`
/// GUIDs, which point at things Lodestash never catalogues, so harvesting them would mint
/// permanently-`missing` edges. Empty slots (`{fileID: 0}`) carry no GUID and drop out. Both
/// serializations are read: the current `- _BaseMap:` list item, and the older
/// `first: {name: _MainTex}` / `second: {m_Texture: …}` pair.
///
/// ponytail: line scan, not a YAML parse. Unity writes these one binding per logical line and the
/// file can be megabytes.
pub fn material_texture_bindings(yaml: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut property = String::new();
    for line in logical_lines(yaml) {
        let trimmed = line.trim_start();
        let item = trimmed.strip_prefix("- ").unwrap_or(trimmed);
        let Some((key, rest)) = item.split_once(':') else {
            continue;
        };
        match key.trim() {
            "m_Texture" => {
                if let Some(guid) = guid_in(rest) {
                    out.push((property.clone(), guid));
                }
            }
            "name" => property = rest.trim().to_string(),
            key if key.starts_with('_') && rest.trim().is_empty() => property = key.to_string(),
            _ => {}
        }
    }
    out
}

/// One renderer slot a prefab binds: `mesh` is drawn with `material` in slot `slot` of the
/// renderer component `renderer`, on the GameObject `game_object` (both are document fileIDs
/// within the prefab).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefabBinding {
    pub game_object: i64,
    pub renderer: i64,
    pub slot: u32,
    pub mesh: String,
    pub material: String,
}

/// What one `.prefab` declares about catalogued assets.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PrefabEvidence {
    pub bindings: Vec<PrefabBinding>,
    /// Nested prefab instances (a variant's base among them). Their renderers live in *another*
    /// prefab, and what this one says about them is a list of property overrides, not a binding —
    /// so they are counted, not guessed at.
    pub nested_instances: usize,
}

/// One YAML document of a Unity asset file: `--- !u!<class> &<fileID>` and its body.
#[derive(Default)]
struct Doc {
    file_id: i64,
    kind: String,
    game_object: Option<i64>,
    mesh: Option<String>,
    /// A particle renderer's further meshes (`m_Mesh1`…`m_Mesh3`), and its render mode — `4` is
    /// mesh mode, the only one in which it draws a mesh.
    extra_meshes: Vec<String>,
    render_mode: Option<i64>,
    /// One entry per `m_Materials` slot — `None` for an empty or built-in slot, which still holds
    /// its index.
    materials: Vec<Option<String>>,
}

/// The renderer bindings a Unity prefab declares (DATA-MODEL §10 step 1b, ADR-0018).
///
/// A stock FBX embeds its materials, so the model asset itself declares nothing: the prefab is
/// where "this mesh is painted with that material" is written down. The file is read **per
/// document** — every component is its own `--- !u!` document, tied to its GameObject by
/// `m_GameObject` — so a `MeshRenderer` pairs only with the `MeshFilter` on its own GameObject, and
/// a `SkinnedMeshRenderer` carries its own mesh. Two GameObjects give two pairings, never the four
/// a whole-file mesh × material product gave. A particle renderer in mesh mode paints its meshes with
/// its first material. A `MeshCollider`'s mesh, and the mesh a particle system emits from, are not
/// painted and are not read.
///
/// ponytail: a line scan over documents, not a YAML parser — Unity's writer is regular and prefabs
/// run to megabytes. Nested instances and variant overrides are counted, not resolved.
pub fn prefab_evidence(yaml: &str) -> PrefabEvidence {
    let mut docs: Vec<Doc> = Vec::new();
    let mut in_materials = false;
    for line in logical_lines(yaml) {
        if let Some(header) = line.strip_prefix("--- !u!") {
            // `<class> &<fileID>`, then ` stripped` on a nested instance's stand-ins.
            let file_id = header
                .split_once('&')
                .and_then(|(_, rest)| rest.split_whitespace().next())
                .and_then(|id| id.parse().ok())
                .unwrap_or(0);
            docs.push(Doc {
                file_id,
                ..Doc::default()
            });
            in_materials = false;
            continue;
        }
        let Some(doc) = docs.last_mut() else {
            continue; // the `%YAML` / `%TAG` preamble
        };
        if doc.kind.is_empty() {
            if let Some(kind) = line.strip_suffix(':').filter(|k| !k.starts_with(' ')) {
                doc.kind = kind.to_string();
            }
            continue;
        }
        let trimmed = line.trim_start();
        if let Some(item) = trimmed.strip_prefix("- ") {
            // A list item belongs to `m_Materials` only while we are still inside that block.
            if in_materials {
                doc.materials.push(guid_in(item));
            }
            continue;
        }
        in_materials = false;
        let Some((key, rest)) = trimmed.split_once(':') else {
            continue;
        };
        match key {
            "m_Materials" => in_materials = true,
            "m_Mesh" => doc.mesh = guid_in(rest),
            "m_Mesh1" | "m_Mesh2" | "m_Mesh3" => doc.extra_meshes.extend(guid_in(rest)),
            "m_RenderMode" => doc.render_mode = rest.trim().parse().ok(),
            "m_GameObject" => doc.game_object = file_id_in(rest),
            _ => {}
        }
    }

    let filters: HashMap<i64, &str> = docs
        .iter()
        .filter(|d| d.kind == "MeshFilter")
        .filter_map(|d| Some((d.game_object?, d.mesh.as_deref()?)))
        .collect();
    let mut evidence = PrefabEvidence::default();
    for doc in &docs {
        // The meshes this renderer draws, and how many of its material slots paint them.
        let (meshes, slots): (Vec<&str>, usize) = match doc.kind.as_str() {
            "MeshRenderer" => (
                doc.game_object
                    .and_then(|go| filters.get(&go).copied())
                    .into_iter()
                    .collect(),
                doc.materials.len(),
            ),
            "SkinnedMeshRenderer" => (
                doc.mesh.as_deref().into_iter().collect(),
                doc.materials.len(),
            ),
            // In mesh mode a particle renderer draws up to four meshes with its first material —
            // its second slot is the trail's. In any other mode it draws no mesh at all.
            "ParticleSystemRenderer" if doc.render_mode == Some(4) => (
                doc.mesh
                    .as_deref()
                    .into_iter()
                    .chain(doc.extra_meshes.iter().map(String::as_str))
                    .collect(),
                1,
            ),
            "PrefabInstance" => {
                evidence.nested_instances += 1;
                continue;
            }
            _ => continue,
        };
        for mesh in meshes {
            for (slot, material) in doc.materials.iter().take(slots).enumerate() {
                if let Some(material) = material {
                    evidence.bindings.push(PrefabBinding {
                        game_object: doc.game_object.unwrap_or(0),
                        renderer: doc.file_id,
                        slot: slot as u32,
                        mesh: mesh.to_string(),
                        material: material.clone(),
                    });
                }
            }
        }
    }
    evidence
}

/// A `.prefab` on disk and its revision: it is re-read only when the size or mtime moves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefabFile {
    pub path: PathBuf,
    pub size: u64,
    /// Nanoseconds since the Unix epoch — seconds would miss two saves inside one second.
    pub mtime_ns: i64,
}

/// Every `.prefab` under `root`, and whether the walk saw the whole tree. A walk that could not
/// open a directory — or the root itself, when its drive is gone — has not proven that any prefab
/// was deleted, so the caller keeps what that tree recorded before.
pub fn prefab_files_under(root: &Path) -> (Vec<PrefabFile>, bool) {
    let mut complete = true;
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(root) {
        let Ok(entry) = entry else {
            complete = false;
            continue;
        };
        let is_prefab = entry.file_type().is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("prefab"));
        if !is_prefab {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            complete = false;
            continue;
        };
        let mtime_ns = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos() as i64);
        files.push(PrefabFile {
            path: entry.into_path(),
            size: meta.len(),
            mtime_ns,
        });
    }
    (files, complete)
}

/// Path/naming heuristic (DATA-MODEL §10 step 2): does the texture named `tex` (file stem) belong
/// to the model named `model` (file stem)? True when `tex` equals `model`, or begins with `model`
/// followed by a separator (`_`, `-`, `.`, or space) — so `Human3.fbx` claims `Human3_Albedo` and
/// `Human3 Normal` but **not** `Human30`. Case-insensitive, names only. The caller applies this only
/// to a model with no real (GUID-derived) edges, minting `uses_texture` with `source='inferred'`.
pub fn texture_matches_model(model: &str, tex: &str) -> bool {
    let (m, t) = (model.to_ascii_lowercase(), tex.to_ascii_lowercase());
    if m.is_empty() {
        return false;
    }
    t == m
        || t.strip_prefix(&m)
            .is_some_and(|rest| rest.starts_with(['_', '-', '.', ' ']))
}

fn push_unique(v: &mut Vec<String>, item: String) {
    if !v.contains(&item) {
        v.push(item);
    }
}

/// Unity's YAML writer wraps a long flow mapping onto a continuation line —
/// `{fileID: -2432090755550338912, guid: 04d0…,` and then `    type: 3}` — so one reference is not
/// always one physical line. Rejoin them: a line that leaves a `{` open takes the lines after it
/// until the brace closes. A document header always starts afresh, so a stray brace costs at most
/// the rest of its own document.
fn logical_lines(yaml: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut open = 0usize;
    for line in yaml.lines() {
        if line.starts_with("--- ") {
            open = 0;
        }
        match out.last_mut() {
            Some(last) if open > 0 => {
                last.push(' ');
                last.push_str(line.trim());
            }
            _ => out.push(line.to_string()),
        }
        for c in line.chars() {
            match c {
                '{' => open += 1,
                '}' => open = open.saturating_sub(1),
                _ => {}
            }
        }
    }
    out
}

/// Unity's own asset libraries, which ship with the editor and are never files in anyone's
/// project — the default material a fresh prefab paints with, the white/bump fallback textures,
/// the editor's own icons. A reference to one is not a dependency Lodestash can catalogue.
///
/// `f000…` is *unity default resources*, `e000…` *unity builtin extra*, `d000…` *unity editor
/// resources*.
const BUILTIN_RESOURCE_GUIDS: [&str; 3] = [
    "0000000000000000f000000000000000",
    "0000000000000000e000000000000000",
    "0000000000000000d000000000000000",
];

/// The `guid: <32 hex>` value inside one inline YAML mapping, if present.
///
/// **Built-in resource GUIDs are dropped here**, at the one place every harvester reads a GUID.
/// Nothing in the catalog can ever resolve them, so an edge minted from one is `missing` forever:
/// it fires §13.6's "✕ missing links are skipped on export" warning about a link that is not
/// broken, and offers a relink to something the user cannot supply. The `m_Shader` exclusion in
/// [`material_texture_bindings`] is the same reasoning applied one level up; this catches the case
/// that slipped past it — a prefab whose `m_Materials` list names Unity's default material
/// (one such edge existed in the reference library).
fn guid_in(fragment: &str) -> Option<String> {
    let after = fragment.split_once("guid:")?.1;
    let guid: String = after
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_hexdigit())
        .collect();
    (guid.len() == 32 && !BUILTIN_RESOURCE_GUIDS.contains(&guid.as_str())).then_some(guid)
}

/// The `fileID: <n>` inside one inline mapping — a document's id within its own file.
fn file_id_in(fragment: &str) -> Option<i64> {
    let after = fragment.split_once("fileID:")?.1.trim_start();
    let end = after
        .find(|c: char| c != '-' && !c.is_ascii_digit())
        .unwrap_or(after.len());
    after[..end].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(ev: &PrefabEvidence) -> Vec<(&str, u32, &str)> {
        let mut v: Vec<_> = ev
            .bindings
            .iter()
            .map(|b| (b.mesh.as_str(), b.slot, b.material.as_str()))
            .collect();
        v.sort();
        v
    }

    /// A prefab painted with Unity's *default* material names the built-in resources GUID, which
    /// nothing in any library will ever be. Harvesting it mints an edge that is `missing` for
    /// good - so the Inspector warns that links "are skipped on export" over a link that is not
    /// broken, and offers a relink the user has nothing to point at. One such edge existed in the
    /// reference library before this filter.
    #[test]
    fn builtin_resource_refs_are_not_dependencies() {
        const PREFAB: &str = "\
--- !u!137 &10
SkinnedMeshRenderer:
  m_GameObject: {fileID: 1}
  m_Materials:
  - {fileID: 10303, guid: 0000000000000000f000000000000000, type: 0}
  - {fileID: 2100000, guid: cccccccccccccccccccccccccccccccc, type: 2}
  m_Mesh: {fileID: 4300000, guid: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb, type: 3}
";
        assert_eq!(
            pairs(&prefab_evidence(PREFAB)),
            vec![(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                1,
                "cccccccccccccccccccccccccccccccc"
            )],
            "Unity's default material is not a file to link to - but it still holds slot 0"
        );

        const MAT: &str = "\
Material:
  m_SavedProperties:
    m_TexEnvs:
    - _BumpMap:
        m_Texture: {fileID: 0, guid: 0000000000000000e000000000000000, type: 0}
    - _BaseMap:
        m_Texture: {fileID: 2800000, guid: dddddddddddddddddddddddddddddddd, type: 3}
";
        assert_eq!(
            material_texture_guids(MAT),
            vec!["dddddddddddddddddddddddddddddddd"],
            "the built-in fallback texture is not a dependency either"
        );
    }

    const MAT: &str = "\
Material:
  m_Shader: {fileID: -6465566751694194690, guid: 0730dae39bc73f34796280af9875ce14, type: 3}
  m_SavedProperties:
    m_TexEnvs:
    - _BaseMap:
        m_Texture: {fileID: 2800000, guid: b24b57e0cfed05643be34b0256df875e, type: 3}
    - _BumpMap:
        m_Texture: {fileID: 2800000, guid: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa, type: 3}
    - _EmissionMap:
        m_Texture: {fileID: 0}
    - _Dup:
        m_Texture: {fileID: 2800000, guid: b24b57e0cfed05643be34b0256df875e, type: 3}
    m_Floats:
    - _Cutoff: 0.5
";

    #[test]
    fn material_guids_are_textures_only_and_deduped() {
        let guids = material_texture_guids(MAT);
        assert_eq!(
            guids,
            vec![
                "b24b57e0cfed05643be34b0256df875e",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            ],
            "shader GUID excluded, empty slot skipped, duplicate collapsed"
        );
    }

    /// Which texture is the albedo is written in the property name, not the texture id. Both of
    /// Unity's serializations name it.
    #[test]
    fn texture_bindings_keep_their_property() {
        let got = material_texture_bindings(MAT);
        let got: Vec<(&str, &str)> = got.iter().map(|(p, g)| (p.as_str(), g.as_str())).collect();
        assert_eq!(
            got,
            vec![
                ("_BaseMap", "b24b57e0cfed05643be34b0256df875e"),
                ("_BumpMap", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
                ("_Dup", "b24b57e0cfed05643be34b0256df875e"),
            ]
        );

        const OLDER: &str = "\
Material:
  m_SavedProperties:
    m_TexEnvs:
      data:
        first:
          name: _MainTex
        second:
          m_Texture: {fileID: 2800000, guid: 99999999999999999999999999999999,
            type: 3}
";
        assert_eq!(
            material_texture_bindings(OLDER),
            vec![(
                "_MainTex".to_string(),
                "99999999999999999999999999999999".to_string()
            )]
        );
    }

    #[test]
    fn malformed_bindings_yield_nothing() {
        assert!(material_texture_guids("m_Texture: {fileID: 0}").is_empty());
        assert!(
            material_texture_guids("m_Texture: {guid: short, type: 3}").is_empty(),
            "a non-32-char value is not a GUID"
        );
        assert!(material_texture_guids("").is_empty());
        assert_eq!(prefab_evidence(""), PrefabEvidence::default());
    }

    /// Shaped like a real Synty prefab: a SkinnedMeshRenderer's own mesh and two material slots,
    /// an unrelated `m_Avatar` GUID that must not be mistaken for either, and a MeshFilter with no
    /// renderer beside it — which paints nothing.
    const PREFAB: &str = "\
--- !u!137 &137
SkinnedMeshRenderer:
  m_GameObject: {fileID: 1}
  m_Materials:
  - {fileID: 2100000, guid: 42b64fdb315e3054ea757d8d1c4bcfa7, type: 2}
  - {fileID: 2100000, guid: 11111111111111111111111111111111, type: 2}
  m_Mesh: {fileID: 4300000, guid: b6ef2f4d0154b2747860a3332fa05f97, type: 3}
--- !u!95 &95
Animator:
  m_GameObject: {fileID: 1}
  m_Avatar: {fileID: 9000000, guid: ea24e01b03f54464eadda8d044dc3d2e, type: 3}
  m_ApplyRootMotion: 0
--- !u!33 &33
MeshFilter:
  m_GameObject: {fileID: 2}
  m_Mesh: {fileID: 4300000, guid: 77777777777777777777777777777777, type: 3}
";

    #[test]
    fn a_skinned_mesh_keeps_its_own_slots() {
        let ev = prefab_evidence(PREFAB);
        assert_eq!(
            pairs(&ev),
            vec![
                (
                    "b6ef2f4d0154b2747860a3332fa05f97",
                    0,
                    "42b64fdb315e3054ea757d8d1c4bcfa7"
                ),
                (
                    "b6ef2f4d0154b2747860a3332fa05f97",
                    1,
                    "11111111111111111111111111111111"
                ),
            ],
            "one renderer, two slots; the avatar is neither, the lone filter paints nothing"
        );
        assert!(ev
            .bindings
            .iter()
            .all(|b| b.renderer == 137 && b.game_object == 1));
    }

    /// The defect S06 exists for. Two GameObjects, each a MeshFilter + MeshRenderer: the
    /// whole-file product paired both meshes with both materials — four edges, two of them false.
    /// Joined on `m_GameObject` it is exactly two, whatever order the documents come in, and the
    /// MeshCollider's mesh is not a painted one.
    #[test]
    fn two_game_objects_give_two_pairings_not_four() {
        const TWO: &str = "\
%YAML 1.1
%TAG !u! tag:unity3d.com,2011:
--- !u!1 &100
GameObject:
  m_Component:
  - component: {fileID: 101}
  - component: {fileID: 102}
  m_Name: Rock
--- !u!33 &101
MeshFilter:
  m_GameObject: {fileID: 100}
  m_Mesh: {fileID: 4300000, guid: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa, type: 3}
--- !u!23 &102
MeshRenderer:
  m_GameObject: {fileID: 100}
  m_Materials:
  - {fileID: 2100000, guid: 11111111111111111111111111111111, type: 2}
--- !u!1 &200
GameObject:
  m_Name: Tree
--- !u!23 &202
MeshRenderer:
  m_GameObject: {fileID: 200}
  m_Materials:
  - {fileID: 2100000, guid: 22222222222222222222222222222222, type: 2}
--- !u!33 &201
MeshFilter:
  m_GameObject: {fileID: 200}
  m_Mesh: {fileID: 4300000, guid: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb, type: 3}
--- !u!64 &203
MeshCollider:
  m_GameObject: {fileID: 200}
  m_Mesh: {fileID: 4300000, guid: cccccccccccccccccccccccccccccccc, type: 3}
";
        let ev = prefab_evidence(TWO);
        assert_eq!(
            pairs(&ev),
            vec![
                (
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    0,
                    "11111111111111111111111111111111"
                ),
                (
                    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    0,
                    "22222222222222222222222222222222"
                ),
            ]
        );
        let mut owners: Vec<(i64, i64)> = ev
            .bindings
            .iter()
            .map(|b| (b.game_object, b.renderer))
            .collect();
        owners.sort();
        assert_eq!(owners, vec![(100, 102), (200, 202)]);
    }

    /// Unity wraps a long reference onto a continuation line. The continuation must neither end
    /// the `m_Materials` block (losing every slot after it) nor hide a GUID it carries.
    #[test]
    fn a_wrapped_reference_is_one_reference() {
        const WRAPPED: &str = "\
--- !u!137 &5
SkinnedMeshRenderer:
  m_GameObject: {fileID: -4}
  m_Materials:
  - {fileID: -4418383934286548916, guid: 42b64fdb315e3054ea757d8d1c4bcfa7,
      type: 3}
  - {fileID: 2100000, guid: 11111111111111111111111111111111, type: 2}
  m_Mesh: {fileID: -2432090755550338912,
    guid: b6ef2f4d0154b2747860a3332fa05f97, type: 3}
";
        let ev = prefab_evidence(WRAPPED);
        assert_eq!(
            pairs(&ev),
            vec![
                (
                    "b6ef2f4d0154b2747860a3332fa05f97",
                    0,
                    "42b64fdb315e3054ea757d8d1c4bcfa7"
                ),
                (
                    "b6ef2f4d0154b2747860a3332fa05f97",
                    1,
                    "11111111111111111111111111111111"
                ),
            ]
        );
        assert_eq!(ev.bindings[0].game_object, -4, "fileIDs can be negative");
    }

    /// A nested instance's material override is a property modification on another prefab's
    /// renderer. Reading it as a binding would be a guess; it is counted instead.
    #[test]
    fn nested_instances_are_counted_not_guessed() {
        const NESTED: &str = "\
--- !u!1001 &7
PrefabInstance:
  m_Modification:
    m_Modifications:
    - target: {fileID: 2300000, guid: dddddddddddddddddddddddddddddddd, type: 3}
      propertyPath: m_Materials.Array.data[0]
      value:
      objectReference: {fileID: 2100000, guid: eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee, type: 2}
  m_SourcePrefab: {fileID: 100100000, guid: ffffffffffffffffffffffffffffffff, type: 3}
--- !u!23 &8 stripped
MeshRenderer:
  m_CorrespondingSourceObject: {fileID: 2300000, guid: dddddddddddddddddddddddddddddddd, type: 3}
  m_PrefabInstance: {fileID: 7}
";
        let ev = prefab_evidence(NESTED);
        assert!(ev.bindings.is_empty(), "{ev:?}");
        assert_eq!(ev.nested_instances, 1);
    }

    /// Shaped like Synty's candle preset: the candle is a MeshFilter + MeshRenderer; its flame is a
    /// particle renderer in mesh mode, painting its meshes with its first material (the second slot
    /// is the trail's); the particle system's emission shape names a mesh nothing paints; and a
    /// billboard renderer draws no mesh at all.
    #[test]
    fn a_particle_renderer_in_mesh_mode_paints_its_meshes() {
        const CANDLE: &str = "\
--- !u!23 &2
MeshRenderer:
  m_GameObject: {fileID: 1}
  m_Materials:
  - {fileID: 2100000, guid: 42b64fdb315e3054ea757d8d1c4bcfa7, type: 2}
--- !u!33 &3
MeshFilter:
  m_GameObject: {fileID: 1}
  m_Mesh: {fileID: 4300000, guid: 76fc226ee9513324da4a7d7c3fed826b, type: 3}
--- !u!198 &4
ParticleSystem:
  m_GameObject: {fileID: 1}
  ShapeModule:
    type: 13
    m_Mesh: {fileID: 4300000, guid: 0e92798a1a457754d96520018956ba78, type: 3}
--- !u!199 &5
ParticleSystemRenderer:
  m_GameObject: {fileID: 1}
  m_Materials:
  - {fileID: 2100000, guid: 169bd604eb7f4bf42bdb6c877f677c9c, type: 2}
  - {fileID: 2100000, guid: dddddddddddddddddddddddddddddddd, type: 2}
  m_RenderMode: 4
  m_Mesh: {fileID: 4300000, guid: 6f9115bd4317a23498b706a4cc7bc5ed, type: 3}
  m_Mesh1: {fileID: 4300000, guid: eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee, type: 3}
  m_Mesh2: {fileID: 0}
  m_Mesh3: {fileID: 0}
--- !u!199 &6
ParticleSystemRenderer:
  m_GameObject: {fileID: 7}
  m_Materials:
  - {fileID: 2100000, guid: 16025479cbc59c145aed336aa4c0d776, type: 2}
  m_RenderMode: 0
  m_Mesh: {fileID: 4300000, guid: 49ca538ce209a82448b832a3d0c09ba3, type: 3}
";
        assert_eq!(
            pairs(&prefab_evidence(CANDLE)),
            vec![
                (
                    "6f9115bd4317a23498b706a4cc7bc5ed",
                    0,
                    "169bd604eb7f4bf42bdb6c877f677c9c"
                ),
                (
                    "76fc226ee9513324da4a7d7c3fed826b",
                    0,
                    "42b64fdb315e3054ea757d8d1c4bcfa7"
                ),
                (
                    "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
                    0,
                    "169bd604eb7f4bf42bdb6c877f677c9c"
                ),
            ],
            "the candle's paint, the flame's paint on both its meshes; no trail slot, no emission \
             shape, no billboard"
        );
    }

    #[test]
    fn a_list_item_outside_m_materials_is_not_a_material() {
        let yaml = "\
--- !u!137 &1
SkinnedMeshRenderer:
  m_Materials:
  - {fileID: 1, guid: 42b64fdb315e3054ea757d8d1c4bcfa7, type: 2}
  m_Bones:
  - {fileID: 1, guid: 99999999999999999999999999999999, type: 2}
  m_Mesh: {fileID: 4300000, guid: b6ef2f4d0154b2747860a3332fa05f97, type: 3}
";
        let materials: Vec<String> = prefab_evidence(yaml)
            .bindings
            .into_iter()
            .map(|b| b.material)
            .collect();
        assert_eq!(
            materials,
            vec!["42b64fdb315e3054ea757d8d1c4bcfa7"],
            "the block ends at the next key"
        );
    }

    /// A tree the walk cannot see proves nothing about what was deleted from it.
    #[test]
    fn a_walk_reports_revisions_and_whether_it_saw_everything() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/A.prefab"), "x").unwrap();
        std::fs::write(dir.path().join("B.PREFAB"), "xy").unwrap();
        std::fs::write(dir.path().join("C.mat"), "x").unwrap();
        let (mut files, complete) = prefab_files_under(dir.path());
        files.sort_by(|a, b| a.path.cmp(&b.path));
        assert!(complete);
        assert_eq!(
            files.iter().map(|f| f.size).collect::<Vec<_>>(),
            vec![2, 1],
            "both prefabs, any case, and nothing else"
        );
        assert!(files.iter().all(|f| f.mtime_ns > 0));

        let (none, complete) = prefab_files_under(&dir.path().join("gone"));
        assert!(
            none.is_empty() && !complete,
            "a root that is not there is unavailable, not empty"
        );
    }

    #[test]
    fn texture_matches_model_by_name_prefix() {
        // Belongs: exact stem, or stem + separator (§10 example Human3.fbx ↔ Human3_Albedo.png).
        assert!(texture_matches_model("Human3", "Human3"));
        assert!(texture_matches_model("Human3", "Human3_Albedo"));
        assert!(texture_matches_model("Human3", "Human3-Normal"));
        assert!(texture_matches_model("Human3", "Human3 Spec"));
        assert!(
            texture_matches_model("Human3", "human3_albedo"),
            "case-insensitive"
        );
        // Does not belong: a longer stem with no separator, or an unrelated name.
        assert!(!texture_matches_model("Human3", "Human30"));
        assert!(!texture_matches_model("Human3", "Rock_Albedo"));
        assert!(
            !texture_matches_model("", "anything"),
            "empty model matches nothing"
        );
    }

    #[test]
    fn sidecar_path_appends_to_the_whole_file_name() {
        assert_eq!(
            meta_sidecar(Path::new("C:/a/Rock.fbx")),
            PathBuf::from("C:/a/Rock.fbx.meta"),
            "not Rock.meta — Unity keeps the asset extension"
        );
    }

    #[test]
    fn guid_of_reads_the_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let asset = dir.path().join("Rock.fbx");
        std::fs::write(&asset, b"fake").unwrap();
        assert_eq!(guid_of(&asset), None, "no sidecar yet");
        std::fs::write(
            meta_sidecar(&asset),
            "fileFormatVersion: 2\nguid: 1a64cf0a23f83924f9debcb452685f6f\n",
        )
        .unwrap();
        assert_eq!(
            guid_of(&asset).as_deref(),
            Some("1a64cf0a23f83924f9debcb452685f6f")
        );
    }
}
