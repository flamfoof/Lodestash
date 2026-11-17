//! Job tray + toast feedback (DESIGN-GUIDELINES §7 "Jobs / progress", UI-SPEC §18).
//!
//! One registry owned by the app. Workers share an `Arc<JobProgress>` (atomics only);
//! the UI thread starts/finishes rows inside its `receive_*` handlers — workers never
//! touch the registry, so there is no locking beyond the atomics.
//!
//! ponytail: no job queue/engine — jobs here are *visibility* over threads the app
//! already spawns. A real prioritised job engine arrives with the P4 identify runs.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Shared progress of a background job (written by the worker, read by the UI).
#[derive(Default)]
pub struct JobProgress {
    pub done: AtomicU64,
    /// 0 = total unknown → the tray shows a spinner instead of a bar.
    pub total: AtomicU64,
    /// Assets the cloud tagger could not tag (rate-limit exhausted or hard API error). Surfaced in
    /// the tray so a silently-degraded run (e.g. bad key → local fallback) is visible, not invisible.
    pub failed: AtomicU64,
    /// Set by the tray's ✕. Cooperative: workers poll it where they can stop safely.
    pub cancelled: AtomicBool,
}

impl JobProgress {
    pub fn fraction(&self) -> f32 {
        let total = self.total.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        (self.done.load(Ordering::Relaxed) as f32 / total as f32).clamp(0.0, 1.0)
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}

/// How long a finished row lingers in the tray (a brief "done" flash).
const DONE_LINGER: Duration = Duration::from_millis(2400);
/// Toast lifetime (UI-SPEC §18: ~3.2s).
const TOAST_LIFE: Duration = Duration::from_millis(3200);

pub struct Job {
    pub id: u64,
    pub label: String,
    pub progress: Arc<JobProgress>,
    /// Whether ✕ is offered (cooperative cancel is actually wired for this op).
    pub cancellable: bool,
    pub finished: Option<Instant>,
}

/// Toast accent intent (UI-SPEC §18 left-border colors).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Intent {
    Success,
    Warning,
    Danger,
    Info,
}

pub struct Toast {
    pub text: String,
    pub intent: Intent,
    pub at: Instant,
}

#[derive(Default)]
pub struct Jobs {
    next_id: u64,
    pub jobs: Vec<Job>,
    pub toasts: Vec<Toast>,
}

impl Jobs {
    /// Register a running job; returns the id to `finish` with.
    pub fn start(&mut self, label: String, progress: Arc<JobProgress>, cancellable: bool) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.jobs.push(Job {
            id,
            label,
            progress,
            cancellable,
            finished: None,
        });
        id
    }

    /// Mark a job done (row lingers briefly, then `prune` drops it). Unknown ids are
    /// ignored — a superseded generation may finish a job that was already replaced.
    pub fn finish(&mut self, id: u64) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
            job.finished = Some(Instant::now());
        }
    }

    pub fn toast(&mut self, text: impl Into<String>, intent: Intent) {
        self.toasts.push(Toast {
            text: text.into(),
            intent,
            at: Instant::now(),
        });
    }

    /// Drop finished rows past their linger and expired toasts. Call once per frame.
    pub fn prune(&mut self) {
        let now = Instant::now();
        // ponytail: map_or, not is_none_or — the workspace MSRV is 1.80.
        self.jobs
            .retain(|j| j.finished.map_or(true, |t| now - t < DONE_LINGER));
        self.toasts.retain(|t| now - t.at < TOAST_LIFE);
    }

    /// Anything on screen that needs repaint ticks?
    pub fn any_active(&self) -> bool {
        !self.jobs.is_empty() || !self.toasts.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_lifecycle_and_prune() {
        let mut jobs = Jobs::default();
        let p = Arc::new(JobProgress::default());
        let id = jobs.start("Scanning".into(), p.clone(), true);
        assert!(jobs.any_active());
        assert!(jobs.jobs[0].finished.is_none());

        p.done.store(5, Ordering::Relaxed);
        p.total.store(10, Ordering::Relaxed);
        assert!((jobs.jobs[0].progress.fraction() - 0.5).abs() < f32::EPSILON);

        jobs.finish(id);
        assert!(jobs.jobs[0].finished.is_some());
        jobs.finish(999); // unknown id is a no-op, not a panic
        jobs.prune(); // within linger — still visible
        assert_eq!(jobs.jobs.len(), 1);

        // Force-expire and prune.
        jobs.jobs[0].finished = Some(Instant::now() - DONE_LINGER * 2);
        jobs.toast("done", Intent::Success);
        jobs.toasts[0].at = Instant::now() - TOAST_LIFE * 2;
        jobs.prune();
        assert!(!jobs.any_active());
    }

    #[test]
    fn zero_total_means_indeterminate() {
        let p = JobProgress::default();
        assert_eq!(p.fraction(), 0.0);
        assert!(!p.is_cancelled());
        p.cancelled.store(true, Ordering::Relaxed);
        assert!(p.is_cancelled());
    }
}
