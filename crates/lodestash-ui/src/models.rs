//! In-app model manager (P4, AI-MODELS-BY-GPU.md "Download recommended models"): fetches the
//! SigLIP 2 search model into `<data>/Lodestash/models/siglip/` so Search-with-AI works with no
//! env vars or manual copying. Blocking download on a worker thread, reporting through the shared
//! [`JobProgress`] atomics the job tray already renders.
//!
//! ponytail: one hard-coded model (the CPU-tier default). GPU-tier selection + a model registry
//! arrive with the full P4 model-manager card; a stranger just needs "download and it works".

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::jobs::JobProgress;

const BASE: &str =
    "https://huggingface.co/onnx-community/siglip2-base-patch16-256-ONNX/resolve/main";
/// (remote path, local filename). uint8 towers = the CPU-tier default (vision 90MB, text 270MB).
const FILES: &[(&str, &str)] = &[
    ("onnx/vision_model_uint8.onnx", "vision.onnx"),
    ("onnx/text_model_uint8.onnx", "text.onnx"),
    ("tokenizer.json", "tokenizer.json"),
];
/// Approximate combined size, set as the progress total up front so the bar is smooth (real byte
/// counts drive `done`; if HF sizes drift the bar just ends a hair off 100%). ~412 MB.
const APPROX_TOTAL: u64 = 412_538_967;

pub fn siglip_dir() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("Lodestash").join("models").join("siglip"))
}

/// Are all three SigLIP files already on disk?
pub fn siglip_installed() -> bool {
    siglip_dir().is_some_and(|d| FILES.iter().all(|(_, dest)| d.join(dest).exists()))
}

/// Download the SigLIP files into the models dir, streaming each to a `.part` and renaming on
/// completion (so a crash never leaves a half-file that looks installed). Cooperative cancel via
/// `progress.cancelled`. Runs on a worker thread; `progress` is shared with the job tray.
pub fn download_siglip(progress: Arc<JobProgress>) -> Result<(), String> {
    let dir = siglip_dir().ok_or("no data directory")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create models dir: {e}"))?;
    progress.total.store(APPROX_TOTAL, Ordering::Relaxed);

    for (rel, dest) in FILES {
        if progress.is_cancelled() {
            return Err("cancelled".into());
        }
        let final_path = dir.join(dest);
        if final_path.exists() {
            // Resume a partial install: count it toward progress, skip the fetch.
            if let Ok(m) = final_path.metadata() {
                progress.done.fetch_add(m.len(), Ordering::Relaxed);
            }
            continue;
        }
        let url = format!("{BASE}/{rel}");
        let resp = ureq::get(&url).call().map_err(|e| format!("GET {dest}: {e}"))?;
        let tmp = dir.join(format!("{dest}.part"));
        let mut file = std::fs::File::create(&tmp).map_err(|e| format!("create {dest}: {e}"))?;
        let mut reader = resp.into_reader();
        let mut buf = [0u8; 64 * 1024];
        loop {
            if progress.is_cancelled() {
                let _ = std::fs::remove_file(&tmp);
                return Err("cancelled".into());
            }
            let n = reader.read(&mut buf).map_err(|e| format!("read {dest}: {e}"))?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n]).map_err(|e| format!("write {dest}: {e}"))?;
            progress.done.fetch_add(n as u64, Ordering::Relaxed);
        }
        file.flush().map_err(|e| e.to_string())?;
        drop(file);
        std::fs::rename(&tmp, &final_path).map_err(|e| format!("finalize {dest}: {e}"))?;
    }
    Ok(())
}
