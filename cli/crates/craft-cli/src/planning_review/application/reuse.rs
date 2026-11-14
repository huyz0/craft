//! Explicit reviewed reuse records a decision, never a replacement native create outcome.
use super::*;
use craft_core::storage::SubmissionState;

const MEDIA: &str = "application/vnd.craft.ticket-reuse+json";

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReuseEvidence {
    pub task_id: String,
    pub issue: ResourceRef,
    pub origin: JournalRecord,
    pub origin_attribution_unresolved: bool,
    pub artifact: ArtifactReceipt,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    run_digest: String,
    spec: StepSpec,
    issue: ResourceRef,
    origin: JournalRecord,
}
fn artifact_name(run: &str, spec: &StepSpec) -> Result<String, CraftError> {
    Ok(format!(
        "{}-reuse.json",
        ledger::operation_id(run, &spec.key)?
    ))
}
fn validate_origin(
    spec: &StepSpec,
    issue: &ResourceRef,
    origin: &JournalRecord,
    descriptor: &str,
) -> Result<bool, CraftError> {
    origin.validate().map_err(local_error)?;
    let step = origin
        .steps
        .get(craft_provider_jira::created_issue::ROUTE)
        .ok_or_else(|| invalid("original_create_submission_required_for_reuse"))?;
    if spec.kind != Kind::Reuse
        || spec.origin_operation_id.as_deref() != Some(origin.binding.operation_id.as_str())
        || spec.expected_resources.as_slice() != [issue.clone()]
        || issue.tenant_ref() != spec.target.tenant_ref()
        || issue.resource_type != "issue"
        || !numeric(&issue.id)
        || origin.binding.descriptor_digest != descriptor
        || origin.binding.target != spec.target
        || origin.steps.len() != 1
        || step.job.is_some()
        || !matches!(
            step.state,
            SubmissionState::SendStarted | SubmissionState::Unknown | SubmissionState::Confirmed
        )
        || step.confirmed_resources.len() > 1
        || step.confirmed_resources.iter().any(|r| r != issue)
        || (step.state == SubmissionState::Confirmed
            && step.confirmed_resources.as_slice() != [issue.clone()])
    {
        return Err(invalid(
            "original_create_evidence_contradicts_reviewed_reuse",
        ));
    }
    Ok(step.state.requires_reconciliation())
}

pub(super) fn inspect(
    run: &str,
    spec: &StepSpec,
    artifacts: &dyn ArtifactStore,
    source: &ResourceRef,
    descriptor: &str,
) -> Result<Observed, CraftError> {
    let operation_id = ledger::operation_id(run, &spec.key)?;
    let mut observed = Observed {
        report: StepReport {
            key: spec.key.clone(),
            operation_id,
            kind: Kind::Reuse,
            state: RequestState::NotSent,
            complete: false,
            resources: vec![],
            outcome_artifact: None,
            recovered_from_journal: false,
        },
        record: None,
        envelope: None,
        reuse: None,
    };
    let Some(blob) = artifacts
        .read(&artifact_name(run, spec)?, SNAPSHOT_BYTES as u64)
        .map_err(local_error)?
    else {
        return Ok(observed);
    };
    if blob.receipt.media_type != MEDIA || blob.receipt.source.as_ref() != Some(source) {
        return Err(invalid("reuse_record_provenance_mismatch"));
    }
    let record: Record = serde_json::from_value(craft_core::input::parse_document_json(
        &blob.bytes,
        SNAPSHOT_BYTES,
    )?)
    .map_err(|_| invalid("invalid_reuse_record"))?;
    if record.run_digest != run || record.spec != *spec {
        return Err(invalid("reuse_record_binding_mismatch"));
    }
    let unresolved = validate_origin(spec, &record.issue, &record.origin, descriptor)?;
    observed.report.state = RequestState::Confirmed;
    observed.report.complete = true;
    observed.report.resources = vec![record.issue.clone()];
    observed.report.outcome_artifact = Some(blob.receipt.clone());
    observed.reuse = Some(ReuseEvidence {
        task_id: spec.key.trim_start_matches("issue:").into(),
        issue: record.issue,
        origin: record.origin,
        origin_attribution_unresolved: unresolved,
        artifact: blob.receipt,
    });
    Ok(observed)
}

impl Run<'_> {
    pub(super) fn reuse_descriptor(&mut self) -> Result<String, CraftError> {
        if self.reuse_descriptor_digest.is_none() {
            let registry = crate::issue_create::host::registry()?;
            self.reuse_descriptor_digest = Some(digest(
                registry.descriptor(craft_provider_jira::created_issue::COMMAND)?,
            )?);
        }
        self.reuse_descriptor_digest
            .clone()
            .ok_or_else(|| invalid("missing_create_descriptor"))
    }

    pub(super) fn reuse_issue(
        &mut self,
        task: &ResolvedTask,
        spec: &StepSpec,
        batch: &verification::RevalidatedManifest<'_>,
    ) -> Result<(), CraftError> {
        batch.check()?;
        let selected = task
            .existing_issue
            .as_ref()
            .ok_or_else(|| invalid("reviewed_reuse_selection_missing"))?;
        let issue = ResourceRef {
            resource_type: "issue".into(),
            id: selected.issue_id.clone(),
            ..task.target.clone()
        };
        let origin = self
            .journal
            .get(&selected.origin_operation_id)
            .map_err(local_error)?
            .ok_or_else(|| invalid("original_create_journal_missing_for_reuse"))?;
        self.data.unresolved_record = Some(origin.clone());
        validate_origin(spec, &issue, &origin, &self.reuse_descriptor()?)?;
        batch.check()?;
        let result = ledger::ensure(
            self.artifacts,
            &artifact_name(&self.run_digest, spec)?,
            &json!(Record {
                run_digest: self.run_digest.clone(),
                spec: spec.clone(),
                issue,
                origin
            }),
            MEDIA,
            &self.preview.source,
        );
        if let Err(error) = &result {
            self.data.artifact_write_state = error.details.write_state;
            self.data.artifact_error = Some(error.clone());
        }
        result?;
        self.data.artifact_write_state = RequestState::Confirmed;
        let observed = self.observe(spec)?;
        if !observed.report.complete {
            return Err(invalid("reviewed_reuse_not_recorded"));
        }
        self.data.unresolved_record = None;
        batch.check()
    }
}
