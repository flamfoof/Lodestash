//! Where Lodestash keeps things on disk.
//!
//! Split out of `app.rs` (B1): the cache/catalog/model locations are shared by the GUI, the
//! `--mcp` stdio server and the drains, and none of them is a rendering concern.

use std::path::PathBuf;

/// Default cache locations, shared by the GUI and the `--mcp` entry point.
pub fn default_cache_dirs() -> (Option<PathBuf>, Option<PathBuf>) {
    (
        dirs::cache_dir().map(|d| d.join("Lodestash").join("thumbs")),
        dirs::cache_dir().map(|d| d.join("Lodestash").join("workspace")),
    )
}

pub fn catalog_db_path() -> Option<PathBuf> {
    let dir = dirs::data_dir()?.join("Lodestash");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("catalog.db"))
}

/// Model id recorded with every vector; the query side reads back under the same id. Bump if
/// the encoder changes (old vectors then simply don't match — no collision).
pub const SIGLIP_MODEL_ID: &str = "siglip2-base-patch16-256";

/// Locate the SigLIP towers + tokenizer: `SIGLIP_{VISION,TEXT,TOKENIZER}` env override (dev),
/// else `<data>/Lodestash/models/siglip/{vision.onnx,text.onnx,tokenizer.json}` where the model
/// manager downloads them. `None` unless all three are present.
pub fn siglip_model_paths() -> Option<(PathBuf, PathBuf, PathBuf)> {
    if let (Ok(v), Ok(t), Ok(k)) = (
        std::env::var("SIGLIP_VISION"),
        std::env::var("SIGLIP_TEXT"),
        std::env::var("SIGLIP_TOKENIZER"),
    ) {
        return Some((PathBuf::from(v), PathBuf::from(t), PathBuf::from(k)));
    }
    let dir = dirs::data_dir()?
        .join("Lodestash")
        .join("models")
        .join("siglip");
    let (v, t, k) = (
        dir.join("vision.onnx"),
        dir.join("text.onnx"),
        dir.join("tokenizer.json"),
    );
    (v.exists() && t.exists() && k.exists()).then_some((v, t, k))
}
