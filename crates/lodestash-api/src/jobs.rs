//! The job registry behind every long-running agent operation (ENGINE-API §4 "Async",
//! MCP-AND-ENGINES §5). A long op returns a `job_id` immediately; the client polls
//! `GET /v1/jobs/{id}` and may `POST /v1/jobs/{id}/cancel`.
//!
//! Side-effecting jobs start in `awaiting_approval` and do nothing until the desktop app says
//! yes. That's the spec's "confirm mutating ops **server-side**, don't rely on the client's
//! human-in-the-loop" — an agent can ask to write files; only the person at the machine can
//! actually let it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

/// Job lifecycle. `awaiting_approval` extends the ENGINE-API §3 set (`queued|running|done|
/// error|cancelled`) with the server-side confirmation gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    AwaitingApproval,
    Queued,
    Running,
    Done,
    Error,
    Cancelled,
}

impl JobState {
    pub fn as_str(self) -> &'static str {
        match self {
            JobState::AwaitingApproval => "awaiting_approval",
            JobState::Queued => "queued",
            JobState::Running => "running",
            JobState::Done => "done",
            JobState::Error => "error",
            JobState::Cancelled => "cancelled",
        }
    }

    fn from_u8(v: u8) -> Self {
        match v {
            0 => JobState::AwaitingApproval,
            1 => JobState::Queued,
            2 => JobState::Running,
            3 => JobState::Done,
            4 => JobState::Error,
            _ => JobState::Cancelled,
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, JobState::Done | JobState::Error | JobState::Cancelled)
    }
}

/// One job. Workers hold an `Arc<Job>` and only touch the atomics + the two mutexes, so
/// progress reporting never blocks a reader.
pub struct Job {
    pub id: String,
    pub kind: String,
    /// One line describing what will happen, shown in the desktop approval prompt.
    pub summary: String,
    state: AtomicU8,
    done: AtomicU64,
    total: AtomicU64,
    cancelled: AtomicBool,
    error: Mutex<Option<String>>,
    result: Mutex<Option<Value>>,
}

impl Job {
    pub fn state(&self) -> JobState {
        JobState::from_u8(self.state.load(Ordering::Relaxed))
    }

    pub fn set_state(&self, s: JobState) {
        self.state.store(s as u8, Ordering::Relaxed);
    }

    pub fn set_total(&self, total: u64) {
        self.total.store(total, Ordering::Relaxed);
    }

    pub fn advance(&self) {
        self.done.fetch_add(1, Ordering::Relaxed);
    }

    /// `0.0..=1.0`, or `0.0` while the total is unknown.
    pub fn progress(&self) -> f32 {
        let total = self.total.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        (self.done.load(Ordering::Relaxed) as f32 / total as f32).clamp(0.0, 1.0)
    }

    /// Cooperative: workers check this between units of work.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    pub fn finish(&self, result: Value) {
        *self.result.lock().unwrap() = Some(result);
        self.set_state(JobState::Done);
    }

    pub fn fail(&self, message: impl Into<String>) {
        *self.error.lock().unwrap() = Some(message.into());
        self.set_state(JobState::Error);
    }

    /// ENGINE-API §3 `Job`.
    pub fn to_json(&self) -> Value {
        json!({
            "job_id": self.id,
            "kind": self.kind,
            "state": self.state().as_str(),
            "summary": self.summary,
            "progress": self.progress(),
            "result": self.result.lock().unwrap().clone(),
            "error": self.error.lock().unwrap().clone(),
        })
    }
}

/// All jobs of a session. In-memory by design (ENGINE-API §5 "Transient vs durable"): a crash
/// loses the queue, never the catalog.
#[derive(Default)]
pub struct Jobs {
    jobs: Mutex<HashMap<String, Arc<Job>>>,
}

impl Jobs {
    /// Register a job in `state`. Side-effecting work passes `AwaitingApproval`.
    pub fn create(&self, kind: &str, summary: String, state: JobState) -> Arc<Job> {
        let job = Arc::new(Job {
            id: format!("{}_{}", kind, uuid::Uuid::new_v4().simple()),
            kind: kind.to_string(),
            summary,
            state: AtomicU8::new(state as u8),
            done: AtomicU64::new(0),
            total: AtomicU64::new(0),
            cancelled: AtomicBool::new(false),
            error: Mutex::new(None),
            result: Mutex::new(None),
        });
        self.jobs
            .lock()
            .unwrap()
            .insert(job.id.clone(), job.clone());
        job
    }

    pub fn get(&self, id: &str) -> Option<Arc<Job>> {
        self.jobs.lock().unwrap().get(id).cloned()
    }

    /// Idempotent. A job that hasn't started yet flips straight to `cancelled`; a running one is
    /// asked to stop and flips when its worker notices. A finished job is left alone.
    pub fn cancel(&self, id: &str) -> Option<Arc<Job>> {
        let job = self.get(id)?;
        if !job.state().is_terminal() {
            job.cancelled.store(true, Ordering::Relaxed);
            if job.state() != JobState::Running {
                job.set_state(JobState::Cancelled);
            }
        }
        Some(job)
    }

    /// Jobs waiting on the person at the machine.
    pub fn awaiting_approval(&self) -> Vec<Arc<Job>> {
        let mut out: Vec<Arc<Job>> = self
            .jobs
            .lock()
            .unwrap()
            .values()
            .filter(|j| j.state() == JobState::AwaitingApproval)
            .cloned()
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_is_idempotent_and_never_resurrects_a_finished_job() {
        let jobs = Jobs::default();

        let queued = jobs.create("export", "2 files".into(), JobState::Queued);
        jobs.cancel(&queued.id);
        jobs.cancel(&queued.id);
        assert_eq!(queued.state(), JobState::Cancelled);

        let running = jobs.create("export", "2 files".into(), JobState::Running);
        jobs.cancel(&running.id);
        assert_eq!(
            running.state(),
            JobState::Running,
            "a running job stays running until its worker notices"
        );
        assert!(running.is_cancelled(), "but it has been asked to stop");

        let done = jobs.create("export", "2 files".into(), JobState::Queued);
        done.finish(json!({"written": 2}));
        jobs.cancel(&done.id);
        assert_eq!(done.state(), JobState::Done, "finished work is not undone");

        assert!(jobs.get("nope").is_none());
    }

    #[test]
    fn approval_queue_lists_only_unapproved_jobs() {
        let jobs = Jobs::default();
        let waiting = jobs.create("export", "1 file".into(), JobState::AwaitingApproval);
        let _running = jobs.create("export", "1 file".into(), JobState::Running);

        let ids: Vec<String> = jobs
            .awaiting_approval()
            .iter()
            .map(|j| j.id.clone())
            .collect();
        assert_eq!(ids, vec![waiting.id.clone()]);

        waiting.set_state(JobState::Queued);
        assert!(jobs.awaiting_approval().is_empty());
    }

    #[test]
    fn progress_is_bounded_and_zero_until_the_total_is_known() {
        let jobs = Jobs::default();
        let j = jobs.create("export", "3 files".into(), JobState::Running);
        j.advance();
        assert_eq!(j.progress(), 0.0, "unknown total reads as 0, not NaN");
        j.set_total(2);
        assert_eq!(j.progress(), 0.5);
        j.advance();
        j.advance();
        assert_eq!(j.progress(), 1.0, "over-reporting clamps");
    }
}
