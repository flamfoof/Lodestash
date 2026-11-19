//! The agent planes' view of the engine's job registry (ENGINE-API §4 "Async",
//! MCP-AND-ENGINES §5). A long op returns a `job_id` immediately; the client polls
//! `GET /v1/jobs/{id}` and may `POST /v1/jobs/{id}/cancel`.
//!
//! **A view, not a registry** (v2 spec §4.1, review R1). The jobs are the engine's
//! ([`lodestash_engine::jobs::JobRegistry`]) — the same ones the shell's tray draws. A job an
//! agent starts here is in the tray; a job the writer starts (a scan, an unpack, an appraisal)
//! answers `get_job_status` by its id, with why it failed when it did. This used to be a second
//! registry that knew only its own jobs.
//!
//! Side-effecting jobs start in `awaiting_approval` and do nothing until the desktop app says
//! yes. That's the spec's "confirm mutating ops **server-side**, don't rely on the client's
//! human-in-the-loop" — an agent can ask to write files; only the person at the machine can
//! actually let it.

use std::sync::Arc;

use lodestash_engine::jobs::JobRegistry;
pub use lodestash_engine::jobs::{Job, JobState};
use lodestash_engine::Origin;
use serde_json::{json, Value};

/// The state as `/v1` spells it (ENGINE-API §3): `queued|running|done|error|cancelled`, plus
/// `awaiting_approval`. The engine calls a failed job `failed`; the wire has always said `error`,
/// and `/v1` only grows.
pub fn wire_state(state: JobState) -> &'static str {
    match state {
        JobState::Failed => "error",
        other => other.as_str(),
    }
}

/// ENGINE-API §3 `Job` — what `get_job_status` answers.
pub trait JobWire {
    fn to_json(&self) -> Value;
}

impl JobWire for Job {
    fn to_json(&self) -> Value {
        json!({
            "job_id": self.id,
            "kind": self.kind,
            "state": wire_state(self.state()),
            "summary": self.label,
            "progress": self.progress(),
            "result": self.result(),
            "error": self.error(),
            "detail": self.detail(),
        })
    }
}

/// The agent planes' handle on the engine's jobs. In memory by design (ENGINE-API §5 "Transient
/// vs durable"): a crash loses the queue, never the catalog.
#[derive(Clone, Default)]
pub struct Jobs {
    registry: JobRegistry,
}

impl Jobs {
    /// A view over `registry` — the engine's, so the shell and the agents see one set of jobs.
    pub fn over(registry: JobRegistry) -> Self {
        Self { registry }
    }

    /// Register a job an agent asked for, in `state`. Side-effecting work passes
    /// `AwaitingApproval`.
    pub fn create(&self, kind: &str, summary: String, state: JobState) -> Arc<Job> {
        self.registry
            .create(kind, summary, Origin::Agent, state, true)
    }

    /// Any job of the session by its id — an agent's, or one the app started itself.
    pub fn get(&self, id: &str) -> Option<Arc<Job>> {
        self.registry.get(id)
    }

    /// Every job of the session, in the order they started.
    pub fn list(&self) -> Vec<Arc<Job>> {
        self.registry.list()
    }

    /// Idempotent. A job that hasn't started yet flips straight to `cancelled`; a running one is
    /// asked to stop and flips when its worker notices. A finished job is left alone.
    pub fn cancel(&self, id: &str) -> Option<Arc<Job>> {
        let job = self.get(id)?;
        job.cancel();
        Some(job)
    }

    /// Jobs waiting on the person at the machine.
    pub fn awaiting_approval(&self) -> Vec<Arc<Job>> {
        self.registry.awaiting_approval()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lodestash_engine::jobs::Outcome;

    /// One registry, two views: a job the app's own side starts is the agent planes' to read,
    /// with its failure text; a job an agent starts is in the tray.
    #[test]
    fn the_view_and_the_tray_show_the_same_jobs() {
        let registry = JobRegistry::default();
        let jobs = Jobs::over(registry.clone());

        let (handles, scan) = registry.open("Import Rocks".into(), "import", false);
        scan.finish(Outcome::Failed("the source folder is gone".into()));
        let seen = jobs.get(&handles.job.id).expect("the tray's job, by id");
        let body = seen.to_json();
        assert_eq!(body["state"], "error", "`failed` is `error` on the wire");
        assert_eq!(body["error"], "the source folder is gone");
        assert_eq!(body["kind"], "import");

        let export = jobs.create("export", "2 files".into(), JobState::AwaitingApproval);
        let tray = registry.snapshot();
        assert!(
            tray.active
                .iter()
                .any(|j| j.job_id == export.id && j.state == "awaiting_approval"),
            "an agent's job is missing from the tray"
        );
        assert_eq!(jobs.list().len(), 2, "both views list both jobs");
    }
}
