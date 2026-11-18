//! Where Lodestash keeps things on disk.
//!
//! Split out of `app.rs` (B1): the cache/catalog/model locations are shared by the GUI, the
//! `--mcp` stdio server and the drains, and none of them is a rendering concern.

use std::path::{Path, PathBuf};

/// Default cache locations, shared by the GUI and the `--mcp` entry point.
pub fn default_cache_dirs() -> (Option<PathBuf>, Option<PathBuf>) {
    (
        dirs::cache_dir().map(|d| d.join("Lodestash").join("thumbs")),
        dirs::cache_dir().map(|d| d.join("Lodestash").join("workspace")),
    )
}

/// The locations one user's Lodestash is given by the OS — what
/// [`crate::EngineConfig::for_catalog`] lays a catalog out against. A value, so a test can stand
/// one up under a temp dir instead of reading the machine's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppDirs {
    /// The catalog the app opens when `LODESTASH_CATALOG` names no other.
    pub default_catalog: PathBuf,
    /// Where `config.json` lives.
    pub config_dir: PathBuf,
    /// The preview cache. Shared by every catalog: everything in it regenerates.
    pub cache_dir: PathBuf,
    /// The **default catalog's** extraction workspace. No other catalog uses it
    /// ([`workspace_for`]).
    pub workspace_dir: PathBuf,
}

impl AppDirs {
    /// This machine's. `None` when the OS reports no data, config or cache directory.
    pub fn os() -> Option<Self> {
        let (cache, workspace) = default_cache_dirs();
        Some(Self {
            default_catalog: default_catalog_path()?,
            config_dir: crate::config::config_file_path()?.parent()?.to_path_buf(),
            cache_dir: cache?,
            workspace_dir: workspace?,
        })
    }
}

/// `<data>/Lodestash/catalog.db` — the catalog the app opens when it is told no other. Nothing is
/// created here; [`catalog_db_path`] makes the folder when this is the one it answers.
pub fn default_catalog_path() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("Lodestash").join("catalog.db"))
}

/// Where `catalog` keeps the payloads it extracts from packs.
///
/// The workspace is swept by what **one** catalog references (`storage::plan_reclaim`), so two
/// catalogs in one folder each see the other's files as unreferenced and Reclaim deletes them —
/// which is what `<cache>/Lodestash/workspace` for every catalog amounted to. The default catalog
/// keeps that folder, so nothing already extracted moves; any other catalog (`LODESTASH_CATALOG`:
/// a second library, a test fixture) gets `<catalog file>.workspace` beside itself.
pub fn workspace_for(catalog: &Path, dirs: &AppDirs) -> PathBuf {
    if same_file(catalog, &dirs.default_catalog) {
        return dirs.workspace_dir.clone();
    }
    let mut name = catalog.as_os_str().to_owned();
    name.push(".workspace");
    PathBuf::from(name)
}

/// Do two paths name one file? Resolved when both exist; otherwise compared as written — made
/// absolute, and on Windows without regard to case or which separator was typed.
fn same_file(a: &Path, b: &Path) -> bool {
    if let (Ok(a), Ok(b)) = (a.canonicalize(), b.canonicalize()) {
        return a == b;
    }
    let spelled = |p: &Path| {
        let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
        let text = abs.to_string_lossy().replace('\\', "/");
        if cfg!(windows) {
            text.to_lowercase()
        } else {
            text
        }
    };
    spelled(a) == spelled(b)
}

pub fn catalog_db_path() -> Option<PathBuf> {
    // A catalog somewhere else: an isolated fixture for a spawned-process test, or a second library.
    // Taken as given — nothing is created for it, so a wrong path is a missing catalog, not a new one.
    if let Some(path) = std::env::var_os("LODESTASH_CATALOG") {
        return Some(PathBuf::from(path));
    }
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
