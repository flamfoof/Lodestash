//! File-type detection: extension-first, magic-byte fallback.
//! Maps to the MediaType enum. Spec: INGESTION.md §1.

use std::path::Path;

use lodestash_core::types::MediaType;

/// Detect media type from file extension (fast path — used during crawl).
pub fn by_extension(path: &Path) -> Option<MediaType> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    match ext.as_str() {
        // 3D models
        "fbx" | "obj" | "gltf" | "glb" | "dae" | "3ds" | "blend" | "stl" | "ply" | "x3d"
        | "abc" | "usd" | "usda" | "usdc" | "usdz" => Some(MediaType::Model3d),

        // Textures (GPU-compressed / render targets)
        "dds" | "ktx" | "ktx2" | "astc" | "pvr" | "crn" => Some(MediaType::Texture),

        // General images (also used as textures but stored as image)
        "png" | "jpg" | "jpeg" | "bmp" | "tga" | "tiff" | "tif" | "hdr" | "exr" | "webp"
        | "ico" | "psd" | "kra" | "pdn" => Some(MediaType::Image),

        // Animated sprites — Aseprite authoring files are animated-2D sources (INGESTION.md §5).
        // Unity `.anim` is an animation-clip YAML (not a sprite) and stays unclassified;
        // spritesheet PNGs classify at ingest (frame data), not by extension.
        "aseprite" | "ase" => Some(MediaType::Sprite),

        // Audio
        "wav" | "mp3" | "ogg" | "flac" | "aif" | "aiff" | "m4a" | "opus" | "wma" | "fsb" => {
            Some(MediaType::Audio)
        }

        // Materials
        "mat" | "mtl" | "materialx" | "mxm" | "sbsar" | "sbs" => Some(MediaType::Material),

        // Fonts
        "ttf" | "otf" | "woff" | "woff2" | "fnt" | "bdf" => Some(MediaType::Font),

        _ => None,
    }
}

/// Extensions that are pipeline plumbing, never browseable content (INGESTION.md §3).
const NON_CONTENT_EXTENSIONS: &[&str] = &[
    "cs", "js", "dll", "asmdef", "asmref", "shader", "cginc", "hlsl", "compute", "unity",
];

/// `.meta` importer types that are always non-content, regardless of extension.
const NON_CONTENT_IMPORTERS: &[&str] = &[
    "MonoImporter",
    "PluginImporter",
    "AssemblyDefinitionImporter",
];

/// `.meta` importer types that are non-content only when the file isn't already a recognized
/// media type — a raw PNG imported via a custom `ScriptedImporter` is still an image.
const NON_CONTENT_IMPORTERS_IF_UNCLASSIFIED: &[&str] = &["ScriptedImporter", "DefaultImporter"];

/// Editor-only folder names — anything nested under one is tooling, not shippable content.
const NON_CONTENT_FOLDERS: &[&str] = &["Editor", "Gizmos"];

/// Classify a file as browseable content vs. non-content (scripts, DLLs, shaders, editor
/// tooling — INGESTION.md §3). Non-content rows are kept, not deleted, but excluded from the
/// default grid/search.
///
/// `ext` is the lowercase extension (no dot); `importer` is the Unity `.meta` importer type when
/// known (from [`crate::unitypackage::UnityMeta::importer`]); `rel_path` is checked for
/// editor-only folders; `has_media_type` is whether `by_extension`/`by_magic` already classified
/// the file as a real media type.
pub fn is_content(
    ext: Option<&str>,
    importer: Option<&str>,
    rel_path: &Path,
    has_media_type: bool,
) -> bool {
    if let Some(ext) = ext {
        if NON_CONTENT_EXTENSIONS.contains(&ext) {
            return false;
        }
    }

    if rel_path.components().any(|c| {
        NON_CONTENT_FOLDERS
            .iter()
            .any(|folder| c.as_os_str() == *folder)
    }) {
        return false;
    }

    if let Some(importer) = importer {
        if NON_CONTENT_IMPORTERS.contains(&importer) {
            return false;
        }
        if !has_media_type && NON_CONTENT_IMPORTERS_IF_UNCLASSIFIED.contains(&importer) {
            return false;
        }
    }

    true
}

/// Detect media type from magic bytes (first 16 bytes of the file).
/// Called as fallback when extension detection fails or is ambiguous.
pub fn by_magic(header: &[u8]) -> Option<MediaType> {
    // GLB (binary glTF)
    if header.starts_with(b"glTF") {
        return Some(MediaType::Model3d);
    }
    // DDS texture
    if header.starts_with(b"DDS ") {
        return Some(MediaType::Texture);
    }
    // PNG
    if header.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(MediaType::Image);
    }
    // JPEG
    if header.starts_with(b"\xff\xd8\xff") {
        return Some(MediaType::Image);
    }
    // RIFF (WAV / AVI)
    if header.starts_with(b"RIFF") && header.len() >= 12 && &header[8..12] == b"WAVE" {
        return Some(MediaType::Audio);
    }
    // OGG
    if header.starts_with(b"OggS") {
        return Some(MediaType::Audio);
    }
    // MP3 (ID3 tag or sync word)
    if header.starts_with(b"ID3")
        || (header.len() >= 2 && header[0] == 0xff && header[1] & 0xe0 == 0xe0)
    {
        return Some(MediaType::Audio);
    }
    // FLAC
    if header.starts_with(b"fLaC") {
        return Some(MediaType::Audio);
    }
    // TTF / OTF (various magic patterns). Mac 'true' fonts: require the following
    // numTables high byte to be 0x00 so text files starting with "true…" don't match.
    if header.starts_with(&[0x00, 0x01, 0x00, 0x00])
        || header.starts_with(b"OTTO")
        || header.starts_with(b"true\0")
    {
        return Some(MediaType::Font);
    }
    None
}

/// Best-effort detection: extension first, then magic bytes from `header` if available.
pub fn detect(path: &Path, header: Option<&[u8]>) -> Option<MediaType> {
    by_extension(path).or_else(|| header.and_then(by_magic))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn extension_detection() {
        let cases: &[(&str, MediaType)] = &[
            ("mesh.fbx", MediaType::Model3d),
            ("mesh.glb", MediaType::Model3d),
            ("albedo.png", MediaType::Image),
            ("bump.dds", MediaType::Texture),
            ("sfx.wav", MediaType::Audio),
            ("sfx.ogg", MediaType::Audio),
            ("ui.ttf", MediaType::Font),
            ("ground.mat", MediaType::Material),
            ("walk.aseprite", MediaType::Sprite),
            ("walk.ase", MediaType::Sprite),
        ];
        for (name, expected) in cases {
            let result = by_extension(&PathBuf::from(name));
            assert_eq!(result, Some(*expected), "failed for {name}");
        }
    }

    #[test]
    fn magic_detection() {
        assert_eq!(by_magic(b"\x89PNG\r\n\x1a\n"), Some(MediaType::Image));
        assert_eq!(by_magic(b"\xff\xd8\xff\xe0"), Some(MediaType::Image));
        assert_eq!(by_magic(b"glTF\x00\x00"), Some(MediaType::Model3d));
        assert_eq!(by_magic(b"DDS foo"), Some(MediaType::Texture));
        assert_eq!(by_magic(b"OggSxxxxxx"), Some(MediaType::Audio));
        assert_eq!(by_magic(b"fLaC"), Some(MediaType::Audio));
    }

    #[test]
    fn is_content_excludes_by_extension() {
        assert!(!is_content(
            Some("cs"),
            None,
            &PathBuf::from("Scripts/Player.cs"),
            false
        ));
        assert!(!is_content(
            Some("dll"),
            None,
            &PathBuf::from("Plugins/Native.dll"),
            false
        ));
        assert!(is_content(
            Some("png"),
            None,
            &PathBuf::from("Textures/albedo.png"),
            true
        ));
    }

    #[test]
    fn is_content_excludes_editor_only_folders() {
        assert!(!is_content(
            Some("png"),
            None,
            &PathBuf::from("Assets/Editor/icon.png"),
            true
        ));
        assert!(!is_content(
            Some("fbx"),
            None,
            &PathBuf::from("Assets/Gizmos/marker.fbx"),
            true
        ));
        assert!(is_content(
            Some("png"),
            None,
            &PathBuf::from("Assets/Textures/icon.png"),
            true
        ));
    }

    #[test]
    fn is_content_excludes_by_importer() {
        // MonoImporter/PluginImporter/AssemblyDefinitionImporter are always non-content
        assert!(!is_content(
            None,
            Some("MonoImporter"),
            &PathBuf::from("Scripts/Player"),
            false
        ));
        assert!(!is_content(
            None,
            Some("AssemblyDefinitionImporter"),
            &PathBuf::from("Foo.asmdef"),
            false
        ));

        // DefaultImporter/ScriptedImporter are non-content only for unclassified files
        assert!(!is_content(
            None,
            Some("DefaultImporter"),
            &PathBuf::from("Data/config"),
            false
        ));
        assert!(
            is_content(
                Some("png"),
                Some("DefaultImporter"),
                &PathBuf::from("Textures/albedo.png"),
                true
            ),
            "a media file imported via DefaultImporter is still content"
        );
    }
}
