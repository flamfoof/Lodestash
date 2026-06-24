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
        | "ico" | "psd" | "kra" | "aseprite" | "ase" | "pdn" => Some(MediaType::Image),

        // Animated sprites (explicit sprite formats)
        "anim" => Some(MediaType::Sprite),

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
    if header.starts_with(b"ID3") || (header.len() >= 2 && header[0] == 0xff && header[1] & 0xe0 == 0xe0) {
        return Some(MediaType::Audio);
    }
    // FLAC
    if header.starts_with(b"fLaC") {
        return Some(MediaType::Audio);
    }
    // TTF / OTF (various magic patterns)
    if header.starts_with(&[0x00, 0x01, 0x00, 0x00]) || header.starts_with(b"OTTO") || header.starts_with(b"true") {
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
}
