//! The one job registry (v2 spec §4.1, review R1).
//!
//! Every long-running piece of work — a scan, an unpack, an appraisal kick, a model download, an
//! agent's export waiting for approval — is one [`Job`] here: an id, a kind, who asked, progress,
//! a state, and **why it failed** when it did. There used to be three registries: the writer's
//! tray (numbered jobs, an ok/cancelled flag and no reason), the HTTP API's (string ids, states,
//! an error text — but only its own jobs) and an egui-era list nothing read. A job started in one
//! was invisible to the other two, and a failure in the writer had nowhere to put its text.
//!
//! The registry stores; two views read it. The tray ([`JobRegistry::snapshot`]) is what the shell
//! draws; `lodestash-api`'s `Jobs` is what `get_job_status` answers. Both see every job.
//!
//! In memory by design (ENGINE-API §5 "Transient vs durable"): a crash loses the queue, never the
//! catalog.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::changes::{Changes, Origin};

/// Shared progress of a background job (written by the worker, read by whoever shows it).
#[derive(Default)]
pub struct JobProgress {
    pub done: AtomicU64,
    /// 0 = total unknown → a spinner instead of a bar.
    pub total: AtomicU64,
    /// Assets the cloud tagger could not tag (rate-limit exhausted or hard API error). Surfaced so
    /// a silently-degraded run (e.g. bad key → local fallback) is visible, not invisible.
    pub failed: AtomicU64,
    /// Set when the job is asked to stop. Cooperative: workers poll it where they can stop safely.
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

/// Where a job stands (v2 spec §4.1). `awaiting_approval` is the server-side confirmation gate:
/// a side-effecting agent job does nothing until the person at the machine says yes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    AwaitingApproval,
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl JobState {
    /// The engine's name for the state. (The `/v1` wire says `error` for [`JobState::Failed`] —
    /// ENGINE-API §3 — and the API's view maps it.)
    pub fn as_str(self) -> &'static str {
        match self {
            JobState::AwaitingApproval => "awaiting_approval",
            JobState::Queued => "queued",
            JobState::Running => "running",
            JobState::Done => "done",
            JobState::Failed => "failed",
            JobState::Cancelled => "cancelled",
        }
    }

    fn from_u8(v: u8) -> Self {
        match v {
            0 => JobState::AwaitingApproval,
            1 => JobState::Queued,
            2 => JobState::Running,
            3 => JobState::Done,
            4 => JobState::Failed,
            _ => JobState::Cancelled,
        }
    }

    /// Finished one way or another — nothing more will happen to it.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            JobState::Done | JobState::Failed | JobState::Cancelled
        )
    }
}

/// How a job ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// It finished; `changed` is what the tray reports (rows, files).
    Done { changed: usize },
    /// It failed, and this is why.
    Failed(String),
    /// It was asked to stop and did; `changed` is what it had done by then.
    Cancelled { changed: usize },
}

impl Outcome {
    /// The outcome of work that returns its count or its error. A job that was cancelled says
    /// cancelled, whatever the work then returned.
    pub fn of(result: Result<usize, String>, cancelled: bool) -> Self {
        match (result, cancelled) {
            (Ok(changed), false) => Outcome::Done { changed },
            (Ok(changed), true) => Outcome::Cancelled { changed },
            (Err(_), true) => Outcome::Cancelled { changed: 0 },
            (Err(why), false) => Outcome::Failed(why),
        }
    }
}

/// One job. Workers hold an `Arc<Job>` and touch only atomics and three small mutexes, so
/// reporting progress never blocks a reader.
pub struct Job {
    /// The public id, `<kind>_<32 hex>` — what `get_job_status` is asked for.
    pub id: String,
    /// The job's number in this registry, in the order jobs started — the tray's id.
    pub seq: u64,
    pub kind: String,
    /// One line saying what the job is: the tray's label, the approval prompt's summary.
    pub label: String,
    /// Who asked (ADR-0016).
    pub origin: Origin,
    /// Whether stopping it is offered: cooperative cancel is wired for this job.
    pub cancellable: bool,
    state: AtomicU8,
    progress: Arc<JobProgress>,
    changed: AtomicUsize,
    finished_unix: AtomicI64,
    error: Mutex<Option<String>>,
    result: Mutex<Option<Value>>,
    /// What an approval prompt shows beyond the label — for `write_tags`, the tag and caption
    /// diff (ENGINE-API §8.3).
    detail: Mutex<Option<Value>>,
    changes: Changes,
}

impl Job {
    pub fn state(&self) -> JobState {
        JobState::from_u8(self.state.load(Ordering::Relaxed))
    }

    /// Move the job to `state`. A job that has finished stays finished: a worker that reports
    /// late cannot bring a cancelled or failed job back.
    pub fn set_state(&self, state: JobState) {
        let moved = self
            .state
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |now| {
                (!JobState::from_u8(now).is_terminal()).then_some(state as u8)
            })
            .is_ok();
        if moved {
            if state.is_terminal() {
                self.finished_unix.store(unix_now(), Ordering::Relaxed);
            }
            self.changes.jobs();
        }
    }

    /// The counters a worker writes and a view reads.
    pub fn progress_handle(&self) -> &Arc<JobProgress> {
        &self.progress
    }

    pub fn set_total(&self, total: u64) {
        self.progress.total.store(total, Ordering::Relaxed);
        self.changes.jobs();
    }

    pub fn advance(&self) {
        self.progress.done.fetch_add(1, Ordering::Relaxed);
        self.changes.jobs();
    }

    /// `0.0..=1.0`, or `0.0` while the total is unknown.
    pub fn progress(&self) -> f32 {
        self.progress.fraction()
    }

    /// Cooperative: workers check this between units of work.
    pub fn is_cancelled(&self) -> bool {
        self.progress.is_cancelled()
    }

    /// Ask the job to stop. One that has not started flips straight to `cancelled`; a running one
    /// is asked, and flips when its worker notices; a finished one is left alone. Idempotent.
    pub fn cancel(&self) {
        let state = self.state();
        if state.is_terminal() {
            return;
        }
        self.progress.cancelled.store(true, Ordering::Relaxed);
        if state != JobState::Running {
            self.set_state(JobState::Cancelled);
        } else {
            self.changes.jobs();
        }
    }

    /// Done, with a result for whoever polls it.
    pub fn finish(&self, result: Value) {
        *lock(&self.result) = Some(result);
        self.set_state(JobState::Done);
    }

    /// Failed, and why — the text `get_job_status` answers and the tray keeps.
    pub fn fail(&self, message: impl Into<String>) {
        if self.state().is_terminal() {
            return;
        }
        *lock(&self.error) = Some(message.into());
        self.set_state(JobState::Failed);
    }

    /// End the job as `outcome` says.
    pub fn end(&self, outcome: Outcome) {
        match outcome {
            Outcome::Done { changed } => {
                self.changed.store(changed, Ordering::Relaxed);
                self.set_state(JobState::Done);
            }
            Outcome::Cancelled { changed } => {
                self.changed.store(changed, Ordering::Relaxed);
                self.set_state(JobState::Cancelled);
            }
            Outcome::Failed(why) => self.fail(why),
        }
    }

    pub fn set_detail(&self, detail: Value) {
        *lock(&self.detail) = Some(detail);
    }

    /// Why it failed. `None` for a job that has not failed.
    pub fn error(&self) -> Option<String> {
        lock(&self.error).clone()
    }

    pub fn result(&self) -> Option<Value> {
        lock(&self.result).clone()
    }

    pub fn detail(&self) -> Option<Value> {
        lock(&self.detail).clone()
    }

    /// What the job changed, as the tray reports it.
    pub fn changed(&self) -> usize {
        self.changed.load(Ordering::Relaxed)
    }
}

/// A job's small mutexes hold plain data; a panic while one was held leaves nothing to protect.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// What a caller holds after starting a tray job, without touching the registry again.
pub struct JobHandles {
    /// The job's number ([`Job::seq`]).
    pub id: u64,
    /// The job's counters, when it is cancellable: the worker writes `done`/`total` and polls
    /// `cancelled`.
    pub progress: Option<Arc<JobProgress>>,
    pub job: Arc<Job>,
}

/// Ends a job when dropped — as **failed**, unless [`JobGuard::finish`] reported how it really
/// ended (review finding 11).
///
/// A job used to be opened and closed by two separate calls with the work between them, so a
/// panic in that work — caught by the writer's supervision, which is the point of supervising —
/// left the job spinning in the tray for the rest of the session, and one of kind `extract`
/// blocked every later extraction. Open jobs through [`JobRegistry::open`] and the end is owned.
pub struct JobGuard {
    job: Arc<Job>,
    open: bool,
}

impl JobGuard {
    pub fn id(&self) -> u64 {
        self.job.seq
    }

    pub fn job(&self) -> &Arc<Job> {
        &self.job
    }

    /// Report how the job ended.
    pub fn finish(mut self, outcome: Outcome) {
        self.open = false;
        self.job.end(outcome);
    }

    /// Someone else ends this job now: the writer, once a worker's report has reached it.
    pub(crate) fn hand_over(mut self) {
        self.open = false;
    }
}

impl Drop for JobGuard {
    fn drop(&mut self) {
        if self.open {
            self.job.fail(
                "the work stopped without reporting how it ended — it panicked or was abandoned",
            );
        }
    }
}

/// One active job, as the tray draws it.
#[derive(Clone, serde::Serialize)]
pub struct JobSnapshot {
    pub id: u64,
    /// The public id — the same job at `GET /v1/jobs/{job_id}`.
    pub job_id: String,
    pub label: String,
    pub kind: String,
    pub origin: &'static str,
    pub state: &'static str,
    /// `(done, total)`; `None` = indeterminate (spinner).
    pub progress: Option<(u64, u64)>,
    pub cancellable: bool,
}

/// One finished job in the session ring.
#[derive(Clone, serde::Serialize)]
pub struct RecentJob {
    pub id: u64,
    pub job_id: String,
    pub label: String,
    pub kind: String,
    pub origin: &'static str,
    pub state: &'static str,
    pub ok: bool,
    pub cancelled: bool,
    /// Why it failed. `None` unless `state` is `failed`.
    pub error: Option<String>,
    pub changed: usize,
    pub at_unix: i64,
}

/// The tray: every job that has not finished, and the most recent that have.
#[derive(Default, serde::Serialize)]
pub struct JobsView {
    pub active: Vec<JobSnapshot>,
    pub recent: Vec<RecentJob>,
}

/// How many finished jobs the tray shows. The registry itself keeps every job of the session,
/// so a client can still ask about one that has scrolled out.
const RECENT_CAP: usize = 20;

fn origin_str(origin: Origin) -> &'static str {
    match origin {
        Origin::User => "user",
        Origin::Agent => "agent",
        Origin::Background => "background",
    }
}

/// Every job of this engine's session. Cheap to clone; every clone is the same registry.
#[derive(Clone, Default)]
pub struct JobRegistry {
    jobs: Arc<Mutex<Vec<Arc<Job>>>>,
    /// Marked whenever a job starts, moves or finishes — the tray is one of the two domains the
    /// shell is told about (ADR-0016).
    changes: Changes,
}

impl JobRegistry {
    /// Register a job in `state`. A side-effecting agent job passes `AwaitingApproval`.
    pub fn create(
        &self,
        kind: &str,
        label: String,
        origin: Origin,
        state: JobState,
        cancellable: bool,
    ) -> Arc<Job> {
        let mut jobs = lock(&self.jobs);
        let job = Arc::new(Job {
            id: format!("{}_{}", kind, uuid::Uuid::new_v4().simple()),
            seq: jobs.len() as u64,
            kind: kind.to_string(),
            label,
            origin,
            cancellable,
            state: AtomicU8::new(state as u8),
            progress: Arc::default(),
            changed: AtomicUsize::new(0),
            finished_unix: AtomicI64::new(0),
            error: Mutex::new(None),
            result: Mutex::new(None),
            detail: Mutex::new(None),
            changes: self.changes.clone(),
        });
        jobs.push(job.clone());
        drop(jobs);
        self.changes.jobs();
        job
    }

    /// Start a running job the shell asked for; returns the handles its worker uses.
    /// Prefer [`Self::open`] wherever work happens between the start and the end.
    pub fn start(&self, label: String, kind: &str, cancellable: bool) -> JobHandles {
        let job = self.create(kind, label, Origin::User, JobState::Running, cancellable);
        JobHandles {
            id: job.seq,
            progress: cancellable.then(|| job.progress.clone()),
            job,
        }
    }

    /// [`Self::start`], with the job's end owned: the guard ends it as failed if it is dropped
    /// without [`JobGuard::finish`] — on a panic, an early return, a thread that never ran.
    pub fn open(&self, label: String, kind: &str, cancellable: bool) -> (JobHandles, JobGuard) {
        let handles = self.start(label, kind, cancellable);
        let guard = JobGuard {
            job: handles.job.clone(),
            open: true,
        };
        (handles, guard)
    }

    /// End the job numbered `id`. Unknown ids and jobs already ended are left alone.
    pub fn finish(&self, id: u64, outcome: Outcome) {
        if let Some(job) = self.by_seq(id) {
            job.end(outcome);
        }
    }

    /// The job with this public id.
    pub fn get(&self, id: &str) -> Option<Arc<Job>> {
        lock(&self.jobs).iter().find(|j| j.id == id).cloned()
    }

    fn by_seq(&self, seq: u64) -> Option<Arc<Job>> {
        lock(&self.jobs).get(seq as usize).cloned()
    }

    /// Every job of the session, in the order they started.
    pub fn list(&self) -> Vec<Arc<Job>> {
        lock(&self.jobs).clone()
    }

    /// Jobs waiting on the person at the machine.
    pub fn awaiting_approval(&self) -> Vec<Arc<Job>> {
        let mut out: Vec<Arc<Job>> = lock(&self.jobs)
            .iter()
            .filter(|j| j.state() == JobState::AwaitingApproval)
            .cloned()
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// Is a pack being analysed or extracted right now? The one-at-a-time queue gates on it, and
    /// the workspace reclaim refuses under it.
    pub(crate) fn has_active_extract(&self) -> bool {
        lock(&self.jobs)
            .iter()
            .any(|j| j.kind == "extract" && !j.state().is_terminal())
    }

    /// The process's change publication, which this registry marks whenever a job changes.
    pub fn changes(&self) -> &Changes {
        &self.changes
    }

    /// A running job's progress moved. Coalesced by the publisher, so a worker can call this per
    /// chunk read without flooding the shell.
    pub fn progressed(&self) {
        self.changes.jobs();
    }

    /// Ask the job numbered `id` to stop ([`Job::cancel`]). `false` when there is no such job or
    /// it has already finished.
    pub fn cancel(&self, id: u64) -> bool {
        match self.by_seq(id) {
            Some(job) if !job.state().is_terminal() => {
                job.cancel();
                true
            }
            _ => false,
        }
    }

    /// The tray view: what has not finished, and the last [`RECENT_CAP`] that have, newest first.
    pub fn snapshot(&self) -> JobsView {
        let jobs = lock(&self.jobs);
        let active = jobs
            .iter()
            .filter(|j| !j.state().is_terminal())
            .map(|j| {
                let (done, total) = (
                    j.progress.done.load(Ordering::Relaxed),
                    j.progress.total.load(Ordering::Relaxed),
                );
                JobSnapshot {
                    id: j.seq,
                    job_id: j.id.clone(),
                    label: j.label.clone(),
                    kind: j.kind.clone(),
                    origin: origin_str(j.origin),
                    state: j.state().as_str(),
                    progress: (j.cancellable || total > 0).then_some((done, total)),
                    cancellable: j.cancellable,
                }
            })
            .collect();
        let mut finished: Vec<&Arc<Job>> =
            jobs.iter().filter(|j| j.state().is_terminal()).collect();
        // Newest first, by when they finished; the start order breaks a tie within a second.
        finished
            .sort_by_key(|j| std::cmp::Reverse((j.finished_unix.load(Ordering::Relaxed), j.seq)));
        let recent = finished
            .into_iter()
            .take(RECENT_CAP)
            .map(|j| {
                let state = j.state();
                RecentJob {
                    id: j.seq,
                    job_id: j.id.clone(),
                    label: j.label.clone(),
                    kind: j.kind.clone(),
                    origin: origin_str(j.origin),
                    state: state.as_str(),
                    ok: state == JobState::Done,
                    cancelled: state == JobState::Cancelled,
                    error: j.error(),
                    changed: j.changed(),
                    at_unix: j.finished_unix.load(Ordering::Relaxed),
                }
            })
            .collect();
        JobsView { active, recent }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> JobRegistry {
        JobRegistry::default()
    }

    #[test]
    fn zero_total_means_indeterminate() {
        let p = JobProgress::default();
        assert_eq!(p.fraction(), 0.0);
        assert!(!p.is_cancelled());
        p.cancelled.store(true, Ordering::Relaxed);
        assert!(p.is_cancelled());
    }

    /// The reason a failure had nowhere to go: the tray recorded `ok: false` and dropped the text.
    #[test]
    fn a_failed_job_keeps_why_and_a_cancelled_one_says_cancelled() {
        let jobs = registry();
        let (_, failed) = jobs.open("Extract Rocks".into(), "extract", true);
        let failed_id = failed.job().id.clone();
        failed.finish(Outcome::Failed(
            "the archive is truncated at byte 512".into(),
        ));
        let (handles, stopped) = jobs.open("Extract Trees".into(), "extract", true);
        assert!(
            jobs.cancel(handles.id),
            "an active job can be asked to stop"
        );
        assert!(handles.job.is_cancelled(), "its worker is told");
        // The worker noticed, and reports what it had done — an error on the way out is not
        // what the job is remembered for.
        stopped.finish(Outcome::of(Err("interrupted".into()), true));

        let job = jobs.get(&failed_id).expect("the job, by its public id");
        assert_eq!(job.state(), JobState::Failed);
        assert_eq!(
            job.error().as_deref(),
            Some("the archive is truncated at byte 512")
        );
        let tray = jobs.snapshot();
        assert!(tray.active.is_empty());
        let by_label = |label: &str| {
            tray.recent
                .iter()
                .find(|j| j.label == label)
                .expect("in the tray")
        };
        let rocks = by_label("Extract Rocks");
        assert_eq!(
            (rocks.state, rocks.ok, rocks.cancelled),
            ("failed", false, false)
        );
        assert_eq!(
            rocks.error.as_deref(),
            Some("the archive is truncated at byte 512"),
            "the tray dropped the reason"
        );
        let trees = by_label("Extract Trees");
        assert_eq!(
            (trees.state, trees.ok, trees.cancelled),
            ("cancelled", false, true)
        );
        assert_eq!(trees.error, None);
        assert!(
            !jobs.cancel(handles.id),
            "a finished job is not cancelled again"
        );
    }

    /// Cancel is idempotent and never resurrects a finished job; a job that has not started flips
    /// at once, a running one when its worker notices.
    #[test]
    fn cancel_is_idempotent_and_a_finished_job_stays_finished() {
        let jobs = registry();
        let agent = |state| jobs.create("export", "2 files".into(), Origin::Agent, state, true);

        let queued = agent(JobState::Queued);
        queued.cancel();
        queued.cancel();
        assert_eq!(queued.state(), JobState::Cancelled);

        let running = agent(JobState::Running);
        running.cancel();
        assert_eq!(
            running.state(),
            JobState::Running,
            "a running job stays running until its worker notices"
        );
        assert!(running.is_cancelled(), "but it has been asked to stop");

        let done = agent(JobState::Queued);
        done.finish(serde_json::json!({"written": 2}));
        done.cancel();
        done.fail("too late");
        assert_eq!(done.state(), JobState::Done, "finished work is not undone");
        assert_eq!(done.error(), None);

        // A worker reporting after the job was cancelled does not bring it back.
        queued.set_state(JobState::Running);
        queued.finish(serde_json::json!({"written": 2}));
        assert_eq!(queued.state(), JobState::Cancelled);
        assert!(jobs.get("nope").is_none());
    }

    #[test]
    fn the_approval_queue_lists_only_unapproved_jobs() {
        let jobs = registry();
        let waiting = jobs.create(
            "export",
            "1 file".into(),
            Origin::Agent,
            JobState::AwaitingApproval,
            true,
        );
        jobs.create(
            "export",
            "1 file".into(),
            Origin::Agent,
            JobState::Running,
            true,
        );

        let ids: Vec<String> = jobs
            .awaiting_approval()
            .iter()
            .map(|j| j.id.clone())
            .collect();
        assert_eq!(ids, vec![waiting.id.clone()]);
        // It is in the tray too: the same job, seen by the other view.
        let tray = jobs.snapshot();
        assert!(tray.active.iter().any(|j| j.job_id == waiting.id
            && j.state == "awaiting_approval"
            && j.origin == "agent"));

        waiting.set_state(JobState::Queued);
        assert!(jobs.awaiting_approval().is_empty());
    }

    #[test]
    fn progress_is_bounded_and_zero_until_the_total_is_known() {
        let jobs = registry();
        let j = jobs.create(
            "export",
            "3 files".into(),
            Origin::Agent,
            JobState::Running,
            true,
        );
        j.advance();
        assert_eq!(j.progress(), 0.0, "unknown total reads as 0, not NaN");
        j.set_total(2);
        assert_eq!(j.progress(), 0.5);
        j.advance();
        j.advance();
        assert_eq!(j.progress(), 1.0, "over-reporting clamps");
    }
}
