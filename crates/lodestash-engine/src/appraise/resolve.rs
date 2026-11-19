//! Where a queued job's bytes are (DATA-MODEL §7): a file on disk, a file extracted to the
//! workspace, or one payload pulled out of its `.unitypackage`.

/// Largest in-pack payload we will pull into memory. `read_payload_at` buffers the whole thing —
/// its own doc says the caller decides whether a payload is small enough — and nothing checked, so
/// a single multi-GB asset inside a `.unitypackage` allocated multi-GB in the identify drain.
///
/// Above the cap the asset simply isn't identified from inside the archive; extracting the pack
/// once materializes it on disk and every drain then reads it normally. That is the same trade the
/// in-pack *thumbnail* cap already makes, and gzip has no random access to make streaming cheaper
/// (HANDOFF §3).
pub(super) const IN_PACK_PULL_MAX_BYTES: i64 = 256 * 1024 * 1024;

pub(super) fn in_pack_pull_ok(size_bytes: i64, pack: &str) -> bool {
    if size_bytes > IN_PACK_PULL_MAX_BYTES {
        tracing::warn!(
            size_bytes,
            cap = IN_PACK_PULL_MAX_BYTES,
            pack,
            "in-pack payload too large to pull into memory; extract the pack to identify it"
        );
        return false;
    }
    true
}

/// Decode a visual drain job's image from whichever of the three §7 byte-sources applies:
/// an on-disk file, a package-extracted workspace file, or — for an **in-pack** asset — one
/// payload pulled into memory and decoded from bytes (DATA-MODEL §7 / M14). The in-pack path
/// never writes to disk: identify surgically reads a single payload, one asset at a time.
/// `None` = no source resolved / undecodable (the caller stores a zero vector to advance).
pub fn resolve_job_image(
    job: &lodestash_core::catalog::VisualEmbedJob,
    workspace_dir: Option<&std::path::Path>,
) -> Option<image::DynamicImage> {
    if let Some(p) = &job.disk_path {
        return lodestash_ingest::raster::decode(std::path::Path::new(p))
            .map_err(|e| tracing::warn!(%e, "image decode failed"))
            .ok();
    }
    if let (Some(rel), Some(ws)) = (&job.workspace_rel, workspace_dir) {
        return lodestash_ingest::raster::decode(&ws.join(rel))
            .map_err(|e| tracing::warn!(%e, "extracted image decode failed"))
            .ok();
    }
    if let (Some(pack), Some(off), Some(sz)) = (&job.pack_path, job.pack_offset, job.pack_size) {
        if !in_pack_pull_ok(sz, pack) {
            return None;
        }
        let bytes = lodestash_ingest::unitypackage::read_payload_at(
            std::path::Path::new(pack),
            off as u64,
            sz as u64,
        )
        .map_err(|e| tracing::warn!(%e, "in-pack payload pull failed"))
        .ok()?;
        return lodestash_ingest::raster::decode_bytes(&bytes, job.ext.as_deref())
            .map_err(|e| tracing::warn!(%e, "in-pack image decode failed"))
            .ok();
    }
    None
}

/// Resolve a drain job to a readable on-disk path from whichever §7 byte-source applies. On-disk and
/// workspace-extracted files resolve to their real path with no guard; an **in-pack** asset has its
/// one payload surgically pulled to an OS-temp scratch file (DATA-MODEL §7 / M14), returned with a
/// `TempPath` guard that deletes it on drop — so the path-based metrics/audio/tag extractors can
/// identify an in-pack asset without a workspace extraction. Serial + manual by design (INGESTION
/// §2), so the pull-per-file cost is fine. `None` when nothing resolves (evicted workspace file, no
/// locator). Keep the returned guard alive for as long as the path is used.
pub fn resolve_job_path(
    job: &lodestash_core::catalog::VisualEmbedJob,
    workspace_dir: Option<&std::path::Path>,
) -> Option<(std::path::PathBuf, Option<tempfile::TempPath>)> {
    if let Some(p) = &job.disk_path {
        return Some((std::path::PathBuf::from(p), None));
    }
    if let (Some(rel), Some(ws)) = (&job.workspace_rel, workspace_dir) {
        return Some((ws.join(rel), None));
    }
    if let (Some(pack), Some(off), Some(sz)) = (&job.pack_path, job.pack_offset, job.pack_size) {
        if !in_pack_pull_ok(sz, pack) {
            return None;
        }
        let bytes = lodestash_ingest::unitypackage::read_payload_at(
            std::path::Path::new(pack),
            off as u64,
            sz as u64,
        )
        .map_err(|e| tracing::warn!(%e, "in-pack payload pull failed"))
        .ok()?;
        // Keep the extension so the path-based decoders dispatch on it exactly as for a real file.
        let suffix = job
            .ext
            .as_deref()
            .map(|e| format!(".{e}"))
            .unwrap_or_default();
        let mut tmp = tempfile::Builder::new()
            .suffix(&suffix)
            .tempfile()
            .map_err(|e| tracing::warn!(%e, "scratch temp create failed"))
            .ok()?;
        std::io::Write::write_all(&mut tmp, &bytes)
            .map_err(|e| tracing::warn!(%e, "scratch temp write failed"))
            .ok()?;
        let temp_path = tmp.into_temp_path();
        let pathbuf = temp_path.to_path_buf();
        return Some((pathbuf, Some(temp_path)));
    }
    None
}
