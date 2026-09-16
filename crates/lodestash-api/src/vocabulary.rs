use super::*;
use lodestash_core::catalog::VocabularyProposal;

impl ServerState {
    pub fn request_vocabulary_proposal(
        &self,
        proposal: VocabularyProposal,
        body_len: usize,
    ) -> Result<TagBatchKickoff, TagBatchError> {
        if body_len > MAX_TAG_BATCH_BYTES {
            return Err(TagBatchError::TooLarge(body_len));
        }
        if uuid::Uuid::parse_str(&proposal.request_id).is_err() {
            return Err(TagBatchError::Invalid("request_id must be a UUID".into()));
        }
        let producer = self
            .agent_producer
            .lock()
            .unwrap()
            .clone()
            .ok_or(TagBatchError::ProducerNotConfigured)?;
        let digest = blake3::hash(&serde_json::to_vec(&proposal).unwrap())
            .to_hex()
            .to_string();
        let cat = self.catalog.lock().unwrap();
        if let Some(receipt) = cat
            .tag_request_receipt(&proposal.request_id)
            .map_err(|e| TagBatchError::Catalog(e.to_string()))?
        {
            return if receipt.digest == digest {
                Ok(TagBatchKickoff::Replayed {
                    request_id: proposal.request_id,
                    result: receipt.result,
                })
            } else {
                Err(TagBatchError::RequestConflict(proposal.request_id))
            };
        }
        let mut pending = self.pending_vocabulary.lock().unwrap();
        if let Some((id, (_, previous))) = pending
            .iter()
            .find(|(_, (p, _))| p.request_id == proposal.request_id)
        {
            return if *previous == digest {
                self.jobs
                    .get(id)
                    .map(TagBatchKickoff::Awaiting)
                    .ok_or_else(|| TagBatchError::Catalog("missing pending job".into()))
            } else {
                Err(TagBatchError::RequestConflict(proposal.request_id))
            };
        }
        cat.validate_vocabulary_proposal(&proposal, &self.vocabulary_policy())
            .map_err(|e| {
                let message = e.to_string();
                if message.contains("vocabulary_conflict:") {
                    TagBatchError::VocabularyConflict
                } else {
                    TagBatchError::Invalid(message)
                }
            })?;
        let job = self.jobs.create(
            "propose_vocabulary_changes",
            format!("Review {} vocabulary changes", proposal.changes.len()),
            jobs::JobState::AwaitingApproval,
        );
        job.set_detail(serde_json::json!({"producer":producer,"changes":proposal.changes,"expected_revision":proposal.expected_revision}));
        pending.insert(job.id.clone(), (proposal, digest));
        drop(pending);
        drop(cat);
        self.wake();
        Ok(TagBatchKickoff::Awaiting(job))
    }

    pub(crate) fn run_vocabulary_proposal(
        &self,
        job: Arc<jobs::Job>,
        proposal: VocabularyProposal,
        digest: String,
    ) {
        job.set_state(jobs::JobState::Running);
        let request_id = proposal.request_id.clone();
        if job.is_cancelled() {
            job.set_state(jobs::JobState::Cancelled);
            return;
        }
        if !self.allows("propose_vocabulary_changes") {
            return job.fail("tool_disabled");
        }
        if let Err(error) = self.write(AgentWrite::VocabularyProposal { proposal, digest }) {
            return job.fail(error);
        }
        let receipt = self
            .catalog
            .lock()
            .unwrap()
            .tag_request_receipt(&request_id);
        match receipt {
            Ok(Some(receipt)) => {
                self.pending_vocabulary.lock().unwrap().remove(&job.id);
                job.finish(receipt.result);
            }
            Ok(None) => job.fail("the writer did not persist a proposal receipt"),
            Err(error) => job.fail(error.to_string()),
        }
    }
}
