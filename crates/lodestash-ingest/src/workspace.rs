//! Extracted-package workspace retention: size-cap-then-age eviction planning.
//! Spec: INGESTION.md §2 "Workspace retention (warm reopens)".
//!
//! Pure planning only — no disk I/O. The caller (job engine) executes the returned hashes'
//! deletion; eviction is always safe by construction since re-extraction is idempotent by hash
//! ([`crate::unitypackage::extract_to_workspace`]).

/// One workspace entry: its content hash, size on disk, and the last time it was touched
/// (viewed/exported), as unix seconds.
#[derive(Debug, Clone)]
pub struct WorkspaceEntry {
    pub content_hash: String,
    pub size_bytes: u64,
    pub last_touched: i64,
}

/// Decide which entries to evict. Size-cap-first: nothing is evicted while total size is within
/// `cap_bytes`. Under cap pressure, oldest-touched-first — but anything touched within
/// `retention_days` of `now` is evicted only if evicting everything outside the window still
/// isn't enough to get back under cap.
pub fn plan_eviction(
    entries: &[WorkspaceEntry],
    cap_bytes: u64,
    retention_days: u32,
    now: i64,
) -> Vec<String> {
    let total: u64 = entries.iter().map(|e| e.size_bytes).sum();
    if total <= cap_bytes {
        return Vec::new();
    }

    let retention_secs = retention_days as i64 * 86_400;
    let mut candidates: Vec<&WorkspaceEntry> = entries.iter().collect();
    candidates.sort_by_key(|e| {
        let within_retention = now - e.last_touched < retention_secs;
        (within_retention, e.last_touched)
    });

    let mut evicted = Vec::new();
    let mut remaining = total;
    for e in candidates {
        if remaining <= cap_bytes {
            break;
        }
        evicted.push(e.content_hash.clone());
        remaining -= e.size_bytes;
    }
    evicted
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(hash: &str, size: u64, last_touched: i64) -> WorkspaceEntry {
        WorkspaceEntry {
            content_hash: hash.to_string(),
            size_bytes: size,
            last_touched,
        }
    }

    #[test]
    fn under_cap_evicts_nothing() {
        let entries = vec![entry("a", 100, 1), entry("b", 100, 2)];
        assert!(plan_eviction(&entries, 1000, 7, 1000).is_empty());
    }

    #[test]
    fn over_cap_evicts_oldest_first_until_under_cap() {
        let entries = vec![
            entry("old", 100, 1),
            entry("mid", 100, 50),
            entry("new", 100, 100),
        ];
        // cap 150: must evict at least 150 bytes worth -> oldest two
        let evicted = plan_eviction(&entries, 150, 0, 1_000_000);
        assert_eq!(evicted, vec!["old".to_string(), "mid".to_string()]);
    }

    #[test]
    fn retention_window_protects_recent_entries_unless_necessary() {
        let now = 1_000_000;
        let retention_days = 7;
        let within_window = now - (retention_days as i64 * 86_400) + 10; // touched just inside window
        let entries = vec![
            entry("ancient", 100, 0),
            entry("recent", 100, within_window),
        ];
        // cap only needs "ancient" evicted to get under — "recent" (in-window) survives
        let evicted = plan_eviction(&entries, 150, retention_days, now);
        assert_eq!(evicted, vec!["ancient".to_string()]);
    }

    #[test]
    fn retention_window_still_evicted_if_cap_requires_it() {
        let now = 1_000_000;
        let entries = vec![entry("only-entry-in-window", 500, now - 10)];
        // even a fresh entry is evicted if it's the only thing keeping us over cap
        let evicted = plan_eviction(&entries, 100, 7, now);
        assert_eq!(evicted, vec!["only-entry-in-window".to_string()]);
    }
}
