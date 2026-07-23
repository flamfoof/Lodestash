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

/// The `guid: <32 hex>` value inside one inline YAML mapping, if present.
fn guid_in(fragment: &str) -> Option<String> {
    let after = fragment.split_once("guid:")?.1;
    let guid: String = after
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_hexdigit())
        .collect();
    (guid.len() == 32).then_some(guid)
}

#[cfg(test)]
mod tests {
    use super::*;

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
