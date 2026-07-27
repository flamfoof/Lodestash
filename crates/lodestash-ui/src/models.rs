//! In-app model manager (P4, AI-MODELS-BY-GPU.md "Download recommended models"): fetches the
//! SigLIP 2 search model into `<data>/Lodestash/models/siglip/` so Search-with-AI works with no
//! env vars or manual copying. Blocking download on a worker thread, reporting through the shared
//! [`JobProgress`] atomics the job tray already renders.
//!
//! ponytail: one hard-coded model (the CPU-tier default). GPU-tier selection + a model registry
//! arrive with the full P4 model-manager card; a stranger just needs "download and it works".

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
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
    Some(
        dirs::data_dir()?
            .join("Lodestash")
            .join("models")
            .join("siglip"),
    )
}

/// CLAP (audio↔text) lives beside SigLIP; separate model, separate vector space.
pub fn clap_dir() -> Option<PathBuf> {
    Some(
        dirs::data_dir()?
            .join("Lodestash")
            .join("models")
            .join("clap"),
    )
}

const CLAP_BASE: &str = "https://huggingface.co/Xenova/clap-htsat-unfused/resolve/main";
/// Quantized towers keep the audio download reasonable (~33MB + ~121MB).
const CLAP_FILES: &[(&str, &str)] = &[
    ("onnx/audio_model_quantized.onnx", "audio.onnx"),
    ("onnx/text_model_quantized.onnx", "text.onnx"),
    ("tokenizer.json", "tokenizer.json"),
];
/// Approximate combined CLAP size, for the progress bar.
const CLAP_APPROX_TOTAL: u64 = 163_000_000;

pub fn clap_installed() -> bool {
    clap_dir().is_some_and(|d| CLAP_FILES.iter().all(|(_, dest)| d.join(dest).exists()))
}

/// Download the CLAP towers + tokenizer. Same streaming/`.part`/resume rules as SigLIP.
pub fn download_clap(progress: Arc<JobProgress>) -> Result<(), String> {
    let dir = clap_dir().ok_or("no data directory")?;
    progress
        .total
        .fetch_add(CLAP_APPROX_TOTAL, Ordering::Relaxed);
    fetch_all(&dir, CLAP_BASE, CLAP_FILES, &progress)
}

// ── ONNX Runtime (the native dll `ort` dlopens) ─────────────────────────────────────────────
//
// The version is *strict*: `ort 2.0.0-rc.10` binds ORT **1.22.x** — a mismatch makes ort panic
// (caught in `*::load`). We fetch the official win-x64 1.22.0 release and extract onnxruntime.dll
// into a writable data dir, then point `ORT_DYLIB_PATH` at it (set in main / after download), so a
// stock install needs neither a system-wide runtime nor bundling it in the installer.

const ORT_VERSION: &str = "1.22.0";
const ORT_ZIP_URL: &str =
    "https://github.com/microsoft/onnxruntime/releases/download/v1.22.0/onnxruntime-win-x64-1.22.0.zip";
/// Files pulled out of the release zip (basename match on the archive entries).
const ORT_DLLS: &[&str] = &["onnxruntime.dll", "onnxruntime_providers_shared.dll"];

pub fn runtime_dir() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("Lodestash").join("runtime"))
}

/// Path the app points `ORT_DYLIB_PATH` at.
pub fn ort_dll_path() -> Option<PathBuf> {
    Some(runtime_dir()?.join("onnxruntime.dll"))
}

pub fn ort_installed() -> bool {
    ort_dll_path().is_some_and(|p| p.exists())
}

/// Download + extract the ONNX Runtime dll. Windows-x64 only (the release archive is
/// platform-specific); other platforms get an instructive error and use the manual path.
pub fn download_ort(progress: Arc<JobProgress>) -> Result<(), String> {
    if std::env::consts::OS != "windows" || std::env::consts::ARCH != "x86_64" {
        return Err(format!(
            "auto-download is win-x64 only; install ONNX Runtime {ORT_VERSION} manually (AI-MODELS-BY-GPU.md)"
        ));
    }
    let dir = runtime_dir().ok_or("no data directory")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create runtime dir: {e}"))?;

    // Stream the zip to a temp file (progress by bytes), then extract just the dll(s).
    let resp = ureq::get(ORT_ZIP_URL)
        .call()
        .map_err(|e| format!("GET runtime: {e}"))?;
    let len: u64 = resp
        .header("Content-Length")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    progress.total.fetch_add(len, Ordering::Relaxed);
    let zip_path = dir.join("onnxruntime.zip.part");
    let mut file = std::fs::File::create(&zip_path).map_err(|e| format!("create zip: {e}"))?;
    let mut reader = resp.into_reader();
    let mut buf = [0u8; 64 * 1024];
    let mut written: u64 = 0;
    loop {
        if progress.is_cancelled() {
            let _ = std::fs::remove_file(&zip_path);
            return Err("cancelled".into());
        }
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("read runtime: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .map_err(|e| format!("write runtime: {e}"))?;
        written += n as u64;
        progress.done.fetch_add(n as u64, Ordering::Relaxed);
    }
    file.flush().map_err(|e| e.to_string())?;
    drop(file);
    if len != 0 && written != len {
        let _ = std::fs::remove_file(&zip_path);
        return Err(format!(
            "runtime download truncated ({written} of {len} bytes) — check the connection and retry"
        ));
    }

    // Extract onnxruntime.dll (+ providers_shared) by basename, ignoring the archive's dir prefix.
    let zf = std::fs::File::open(&zip_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(zf).map_err(|e| format!("open zip: {e}"))?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let name = entry.name().rsplit('/').next().unwrap_or("").to_string();
        if ORT_DLLS.contains(&name.as_str()) {
            let out = dir.join(&name);
            let tmp = dir.join(format!("{name}.part"));
            let mut w = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut w).map_err(|e| format!("extract {name}: {e}"))?;
            drop(w);
            std::fs::rename(&tmp, &out).map_err(|e| format!("finalize {name}: {e}"))?;
        }
    }
    let _ = std::fs::remove_file(&zip_path);
    if !ort_installed() {
        return Err("onnxruntime.dll not found in the release archive".into());
    }
    Ok(())
}

/// Point `ort` at the downloaded runtime, unless the user already set `ORT_DYLIB_PATH`. Safe to
/// call before the first embedder load (ort reads the var when it first dlopens).
pub fn apply_ort_dylib_env() {
    if std::env::var_os("ORT_DYLIB_PATH").is_some() {
        return; // respect an explicit override
    }
    if let Some(p) = ort_dll_path() {
        if p.exists() {
            std::env::set_var("ORT_DYLIB_PATH", p);
        }
    }
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
    progress.total.fetch_add(APPROX_TOTAL, Ordering::Relaxed);
    fetch_all(&dir, BASE, FILES, &progress)
}

/// Stream each `(remote, local)` file into `dir`, writing to a `.part` and renaming on completion
/// (so a crash never leaves a half-file that looks installed). Already-present files are counted
/// toward progress and skipped, which makes an interrupted install resumable. Cooperative cancel
/// via `progress.cancelled`. Shared by every model download.
fn fetch_all(
    dir: &Path,
    base: &str,
    files: &[(&str, &str)],
    progress: &Arc<JobProgress>,
) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("create models dir: {e}"))?;
    for (rel, dest) in files {
        if progress.is_cancelled() {
            return Err("cancelled".into());
        }
        let final_path = dir.join(dest);
        if final_path.exists() {
            if let Ok(m) = final_path.metadata() {
                progress.done.fetch_add(m.len(), Ordering::Relaxed);
            }
            continue;
        }
        let url = format!("{base}/{rel}");
        let resp = ureq::get(&url)
            .call()
            .map_err(|e| format!("GET {dest}: {e}"))?;
        // The server's declared size. A dropped connection ends the read loop at EOF and the file
        // would rename looking complete — so we verify the byte count against Content-Length and
        // reject a short (truncated) file rather than leave a corrupt model that fails at load.
        let expected: u64 = resp
            .header("Content-Length")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let tmp = dir.join(format!("{dest}.part"));
        let mut file = std::fs::File::create(&tmp).map_err(|e| format!("create {dest}: {e}"))?;
        let mut reader = resp.into_reader();
        let mut buf = [0u8; 64 * 1024];
        let mut written: u64 = 0;
        loop {
            if progress.is_cancelled() {
                let _ = std::fs::remove_file(&tmp);
                return Err("cancelled".into());
            }
            let n = reader
                .read(&mut buf)
                .map_err(|e| format!("read {dest}: {e}"))?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])
                .map_err(|e| format!("write {dest}: {e}"))?;
            written += n as u64;
            progress.done.fetch_add(n as u64, Ordering::Relaxed);
        }
        file.flush().map_err(|e| e.to_string())?;
        drop(file);
        if expected != 0 && written != expected {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!(
                "{dest} download truncated ({written} of {expected} bytes) — check the connection and retry"
            ));
        }
        std::fs::rename(&tmp, &final_path).map_err(|e| format!("finalize {dest}: {e}"))?;
    }
    Ok(())
}

/// Delete the SigLIP and CLAP model directories so a corrupt or partial install can be re-fetched
/// (the ONNX Runtime dll is left in place — it is reused and rarely the problem). Missing dirs are
/// not an error. Returns the first delete error, if any.
pub fn remove_models() -> Result<(), String> {
    for dir in [siglip_dir(), clap_dir()].into_iter().flatten() {
        if dir.exists() {
            std::fs::remove_dir_all(&dir)
                .map_err(|e| format!("remove {}: {e}", dir.display()))?;
        }
    }
    Ok(())
}
