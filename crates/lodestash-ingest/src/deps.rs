//! Dependency harvesting: an asset's own Unity GUID (from its `.meta` sidecar) and the GUIDs it
//! declares, which become `asset_relationship` edges (DATA-MODEL §10, INGESTION §5 step 1).
//!
//! Only Unity's own bookkeeping is read here — deterministic refs, `source = 'unity_meta'`. The
//! path/naming heuristic for raw exports (step 2) is a separate pass.

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

/// GUIDs a Unity material references as **textures**.
///
/// A `.mat` is YAML with lines like
/// `m_Texture: {fileID: 2800000, guid: b24b57e0cfed05643be34b0256df875e, type: 3}`.
/// Only `m_Texture` bindings are taken: the same file also carries `m_Shader` and `m_Script`
/// GUIDs, which point at things Lodestash never catalogues, so harvesting them would mint
/// permanently-`missing` edges. Empty slots (`{fileID: 0}`) carry no GUID and drop out.
///
/// ponytail: line scan, not a YAML parse. Unity writes these one binding per line and the file
/// can be megabytes; if a hand-edited multi-line binding ever shows up, parse it properly then.
pub fn material_texture_guids(yaml: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in yaml.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        if key.trim() != "m_Texture" {
            continue;
        }
        if let Some(guid) = guid_in(rest) {
            if !out.contains(&guid) {
                out.push(guid);
            }
        }
    }
    out
}

/// What a `.prefab` binds together: the meshes it instantiates and the materials it paints them
/// with. Both are GUID refs into other assets.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PrefabRefs {
    /// `m_Mesh: {fileID: …, guid: …}` — the model asset (an FBX, usually).
    pub meshes: Vec<String>,
    /// The list items under `m_Materials:`.
    pub materials: Vec<String>,
}

impl PrefabRefs {
    /// Every (mesh, material) pair the prefab implies. Whole-file co-occurrence, not per
    /// GameObject: a prefab that instantiates one model with its materials — the overwhelmingly
    /// common shape for a store asset — gives exactly the right pairs.
    ///
    /// ponytail: a hand-built *scene-like* prefab holding several unrelated models would
    /// over-connect (n×m instead of the true pairing). Group by the owning `m_GameObject` fileID
    /// if that ever produces visible junk; it needs a real per-document parse.
    pub fn pairs(&self) -> Vec<(&str, &str)> {
        self.meshes
            .iter()
            .flat_map(|m| {
                self.materials
                    .iter()
                    .map(move |mat| (m.as_str(), mat.as_str()))
            })
            .collect()
    }
}

/// Mesh and material GUIDs referenced by a Unity prefab.
///
/// This is the only source of **mesh → material** edges: a stock FBX embeds its materials
/// (`externalObjects` in its `.meta` is empty unless the user remapped them), so the model asset
/// itself declares nothing. The prefab is where the binding is actually written down.
pub fn prefab_refs(yaml: &str) -> PrefabRefs {
    let mut refs = PrefabRefs::default();
    let mut in_materials = false;
    for line in yaml.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("- ") {
            // A list item only belongs to `m_Materials` while we're still inside that block.
            if in_materials {
                if let Some(guid) = guid_in(rest) {
                    push_unique(&mut refs.materials, guid);
                }
            }
            continue;
        }
        in_materials = false;
        let Some((key, rest)) = trimmed.split_once(':') else {
            continue;
        };
        match key {
            "m_Materials" => in_materials = true,
            "m_Mesh" => {
                if let Some(guid) = guid_in(rest) {
                    push_unique(&mut refs.meshes, guid);
                }
            }
            _ => {}
        }
    }
    refs
}

/// Every `(mesh GUID, material GUID)` pair declared by the `.prefab` files under `root`, sorted
/// and deduped. Unreadable files are skipped — a prefab we can't parse is missing evidence, not an
/// error.
pub fn prefab_pairs_under(root: &Path) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("prefab"))
        })
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .flat_map(|text| {
            prefab_refs(&text)
                .pairs()
                .into_iter()
                .map(|(m, mat)| (m.to_string(), mat.to_string()))
                .collect::<Vec<_>>()
        })
        .collect();
    pairs.sort();
    pairs.dedup();
    pairs
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
/// [`material_texture_guids`] is the same reasoning applied one level up; this catches the case
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

#[cfg(test)]
mod tests {
    use super::*;
    /// A prefab painted with Unity's *default* material names the built-in resources GUID, which
    /// nothing in any library will ever be. Harvesting it mints an edge that is `missing` for
    /// good - so the Inspector warns that links "are skipped on export" over a link that is not
    /// broken, and offers a relink the user has nothing to point at. One such edge existed in the
    /// reference library before this filter.
    #[test]
    fn builtin_resource_refs_are_not_dependencies() {
        const PREFAB: &str = "\
GameObject:
  m_Mesh: {fileID: 4300000, guid: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb, type: 3}
  m_Materials:
  - {fileID: 10303, guid: 0000000000000000f000000000000000, type: 0}
  - {fileID: 2100000, guid: cccccccccccccccccccccccccccccccc, type: 2}
";
        let refs = prefab_refs(PREFAB);
        assert_eq!(refs.meshes, vec!["bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"]);
        assert_eq!(
            refs.materials,
            vec!["cccccccccccccccccccccccccccccccc"],
            "Unity's default material is not a file to link to"
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

    #[test]
    fn malformed_bindings_yield_nothing() {
        assert!(material_texture_guids("m_Texture: {fileID: 0}").is_empty());
        assert!(
            material_texture_guids("m_Texture: {guid: short, type: 3}").is_empty(),
            "a non-32-char value is not a GUID"
        );
        assert!(material_texture_guids("").is_empty());
    }

    /// Shaped like a real Synty prefab: SkinnedMeshRenderer material list, MeshFilter mesh ref,
    /// an unrelated `m_Avatar` GUID that must not be mistaken for either.
    const PREFAB: &str = "\
--- !u!137 &137
SkinnedMeshRenderer:
  m_Materials:
  - {fileID: 2100000, guid: 42b64fdb315e3054ea757d8d1c4bcfa7, type: 2}
  - {fileID: 2100000, guid: 11111111111111111111111111111111, type: 2}
  m_Mesh: {fileID: 4300000, guid: b6ef2f4d0154b2747860a3332fa05f97, type: 3}
--- !u!95 &95
Animator:
  m_Avatar: {fileID: 9000000, guid: ea24e01b03f54464eadda8d044dc3d2e, type: 3}
  m_ApplyRootMotion: 0
--- !u!33 &33
MeshFilter:
  m_Mesh: {fileID: 4300000, guid: b6ef2f4d0154b2747860a3332fa05f97, type: 3}
";

    #[test]
    fn prefab_refs_separate_meshes_from_materials() {
        let refs = prefab_refs(PREFAB);
        assert_eq!(refs.meshes, vec!["b6ef2f4d0154b2747860a3332fa05f97"]);
        assert_eq!(
            refs.materials,
            vec![
                "42b64fdb315e3054ea757d8d1c4bcfa7",
                "11111111111111111111111111111111"
            ]
        );
        assert_eq!(
            refs.pairs().len(),
            2,
            "one mesh × two materials, deduped first"
        );
        assert!(
            !refs
                .meshes
                .iter()
                .chain(&refs.materials)
                .any(|g| g == "ea24e01b03f54464eadda8d044dc3d2e"),
            "the avatar GUID is neither"
        );
    }

    #[test]
    fn a_list_item_outside_m_materials_is_not_a_material() {
        let yaml = "\
m_Materials:
- {fileID: 1, guid: 42b64fdb315e3054ea757d8d1c4bcfa7, type: 2}
m_Component:
- {fileID: 1, guid: 99999999999999999999999999999999, type: 2}
";
        assert_eq!(
            prefab_refs(yaml).materials,
            vec!["42b64fdb315e3054ea757d8d1c4bcfa7"],
            "the block ends at the next key"
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
