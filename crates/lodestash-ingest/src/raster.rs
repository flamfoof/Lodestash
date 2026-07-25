//! One decode seam for every raster source the app previews: standard formats via
//! `image`, BCn/DDS via `image_dds`, and flattened PSD via `psd`. Both the thumbnail
//! generator and the Quick Look viewer route through [`decode`] so format support
//! never diverges between them (INGESTION.md §3, TECH-STACK.md §1).

use std::io;
use std::path::Path;

use image::DynamicImage;

/// Decode any supported raster file into RGBA. Dispatch is by extension — the same
/// signal `detect::by_extension` classified the asset with.
pub fn decode(path: &Path) -> io::Result<DynamicImage> {
    let ext = path
        .extension()
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.to_str() {
        Some("dds") => decode_dds(path),
        Some("psd") => decode_psd_bytes(&std::fs::read(path)?),
        _ => image::open(path).map_err(invalid),
    }
}

/// Decode a raster from **bytes already in memory** — the in-pack identify path
/// (DATA-MODEL §7 / M14): a payload surgically pulled from a `.unitypackage` never
/// touches disk, so it can't go through [`decode`]. `ext` is the classifying extension
/// (lowercased, no dot) from the in-archive pathname; `None` falls through to content
/// sniffing. Shares the DDS/PSD decoders with [`decode`] so format support can't diverge.
pub fn decode_bytes(bytes: &[u8], ext: Option<&str>) -> io::Result<DynamicImage> {
    match ext {
        Some("dds") => decode_dds_bytes(bytes),
        Some("psd") => decode_psd_bytes(bytes),
        _ => image::load_from_memory(bytes).map_err(invalid),
    }
}

/// BCn/uncompressed DDS → RGBA (mip 0 only — previews never need the chain).
fn decode_dds(path: &Path) -> io::Result<DynamicImage> {
    let mut file = std::fs::File::open(path)?;
    let dds = image_dds::ddsfile::Dds::read(&mut file).map_err(invalid)?;
    let rgba = image_dds::image_from_dds(&dds, 0).map_err(invalid)?;
    Ok(DynamicImage::ImageRgba8(rgba))
}

fn decode_dds_bytes(bytes: &[u8]) -> io::Result<DynamicImage> {
    let dds = image_dds::ddsfile::Dds::read(&mut io::Cursor::new(bytes)).map_err(invalid)?;
    let rgba = image_dds::image_from_dds(&dds, 0).map_err(invalid)?;
    Ok(DynamicImage::ImageRgba8(rgba))
}

/// Flattened PSD composite → RGBA (per-layer access is a later viewer feature).
fn decode_psd_bytes(bytes: &[u8]) -> io::Result<DynamicImage> {
    let psd = psd::Psd::from_bytes(bytes).map_err(invalid)?;
    let buffer = image::RgbaImage::from_raw(psd.width(), psd.height(), psd.rgba())
        .ok_or_else(|| invalid("psd composite has wrong buffer size"))?;
    Ok(DynamicImage::ImageRgba8(buffer))
}

fn invalid(e: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a tiny uncompressed RGBA8 DDS with ddsfile, then decode it back through
    /// the public seam — covers the dds dispatch + container path end to end (BCn
    /// decompression itself is image_dds's tested domain; encoding would drag in the
    /// heavy `encode` feature just to build a fixture).
    #[test]
    fn dds_rgba8_round_trips_through_decode() {
        use image_dds::ddsfile::{Dds, DxgiFormat, NewDxgiParams};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tex.dds");

        let mut dds = Dds::new_dxgi(NewDxgiParams {
            height: 8,
            width: 8,
            depth: None,
            format: DxgiFormat::R8G8B8A8_UNorm,
            mipmap_levels: None,
            array_layers: None,
            caps2: None,
            is_cubemap: false,
            resource_dimension: image_dds::ddsfile::D3D10ResourceDimension::Texture2D,
            alpha_mode: image_dds::ddsfile::AlphaMode::Straight,
        })
        .unwrap();
        dds.data = [255u8, 0, 0, 255].repeat(8 * 8);
        let mut f = std::fs::File::create(&path).unwrap();
        dds.write(&mut f).unwrap();

        let img = decode(&path).expect("dds decodes");
        assert_eq!((img.width(), img.height()), (8, 8));
        assert_eq!(img.to_rgba8().get_pixel(4, 4).0, [255, 0, 0, 255]);
    }

    #[test]
    fn decode_bytes_decodes_in_memory_and_matches_disk() {
        // The in-pack identify path: a PNG that only ever exists as bytes decodes the same
        // as it would from disk, so identify never needs to touch the filesystem.
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(3, 3, image::Rgba([10, 20, 30, 255]))
            .write_to(&mut io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let img = decode_bytes(&png, Some("png")).expect("png bytes decode");
        assert_eq!((img.width(), img.height()), (3, 3));
        assert_eq!(img.to_rgba8().get_pixel(1, 1).0, [10, 20, 30, 255]);

        // Content sniffing when the extension is unknown/absent still finds PNG.
        assert!(decode_bytes(&png, None).is_ok());
        assert!(decode_bytes(b"garbage", Some("png")).is_err());
    }

    #[test]
    fn unknown_extension_falls_through_to_image_and_errors_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not_an_image.xyz");
        std::fs::write(&path, b"garbage").unwrap();
        assert!(decode(&path).is_err());
        let png = dir.path().join("ok.png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([0, 255, 0, 255]))
            .save(&png)
            .unwrap();
        assert_eq!(decode(&png).expect("png decodes").width(), 2);
    }
}
