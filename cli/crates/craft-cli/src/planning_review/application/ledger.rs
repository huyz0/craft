use super::*;
use craft_core::storage::{
    ArtifactSpec, JournalRecord, OperationBinding, OperationJournal, SubmissionState,
};
use std::io::Write;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Kind {
    Issue,
    Reuse,
    Link,
    ResultsPage,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StepSpec {
    pub key: String,
    pub kind: Kind,
    pub target: ResourceRef,
    pub logical_digest: String,
    pub route: String,
    pub expected_resources: Vec<ResourceRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_operation_id: Option<String>,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StepIntent {
    pub run_digest: String,
    pub spec: StepSpec,
    pub binding: OperationBinding,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Outcome {
    intent: StepIntent,
    envelope: Envelope,
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct StepReport {
    pub key: String,
    pub operation_id: String,
    pub kind: Kind,
    pub state: RequestState,
    pub complete: bool,
    pub resources: Vec<ResourceRef>,
    pub outcome_artifact: Option<ArtifactReceipt>,
    pub recovered_from_journal: bool,
}
pub(super) struct Observed {
    pub report: StepReport,
    pub record: Option<JournalRecord>,
    pub envelope: Option<Envelope>,
    pub reuse: Option<super::reuse::ReuseEvidence>,
}
impl Observed {
    pub fn ready(&self) -> bool {
        self.report.state == RequestState::NotSent
    }
}
pub(super) fn operation_id(run: &str, key: &str) -> Result<String, CraftError> {
    Ok(format!(
        "tickets-{}",
        &digest(&json!({"run":run,"step":key}))?[7..]
    ))
}
fn name(run: &str, key: &str, suffix: &str) -> Result<String, CraftError> {
    Ok(format!("{}-{suffix}.json", operation_id(run, key)?))
}
pub(super) fn ensure(
    artifacts: &dyn ArtifactStore,
    name: &str,
    value: &Value,
    media: &str,
    source: &ResourceRef,
) -> Result<ArtifactReceipt, CraftError> {
    if let Some(blob) = artifacts
        .read(name, SNAPSHOT_BYTES as u64)
        .map_err(local_error)?
    {
        if blob.receipt.media_type != media
            || blob.receipt.source.as_ref() != Some(source)
            || craft_core::input::parse_document_json(&blob.bytes, SNAPSHOT_BYTES)? != *value
        {
            return Err(CraftError::new(
                ErrorCode::VersionConflict,
                "Immutable ticket artifact differs from this run; it was not replaced.",
            ));
        }
        return Ok(blob.receipt);
    }
    let bytes = serde_json::to_vec(value).map_err(|_| invalid("cannot_encode_ticket_record"))?;
    craft_core::input::parse_document_json(&bytes, SNAPSHOT_BYTES)?;
    let mut writer = artifacts
        .begin(ArtifactSpec {
            name: name.into(),
            media_type: media.into(),
            max_bytes: SNAPSHOT_BYTES as u64,
            source: Some(source.clone()),
        })
        .map_err(local_error)?;
    writer
        .write_all(&bytes)
        .map_err(|_| local_error(StoreError::Unavailable))?;
    writer.finish().map_err(local_error)
}
fn load<T: serde::de::DeserializeOwned>(
    artifacts: &dyn ArtifactStore,
    name: &str,
    media: &str,
    source: &ResourceRef,
) -> Result<Option<(T, ArtifactReceipt)>, CraftError> {
    artifacts
        .read(name, SNAPSHOT_BYTES as u64)
        .map_err(local_error)?
        .map(|blob| {
            if blob.receipt.media_type != media || blob.receipt.source.as_ref() != Some(source) {
                return Err(invalid("ticket_record_provenance_mismatch"));
            }
            let value = craft_core::input::parse_document_json(&blob.bytes, SNAPSHOT_BYTES)?;
            Ok((
                serde_json::from_value(value).map_err(|_| invalid("invalid_ticket_record"))?,
                blob.receipt,
            ))
        })
        .transpose()
}
pub(super) fn prepare_intent(
    run: &str,
    spec: &StepSpec,
    binding: &OperationBinding,
    artifacts: &dyn ArtifactStore,
    source: &ResourceRef,
) -> Result<(), CraftError> {
    if binding.operation_id != operation_id(run, &spec.key)? || binding.target != spec.target {
        return Err(invalid("ticket_native_binding_mismatch"));
    }
    binding.validate().map_err(local_error)?;
    let intent = StepIntent {
        run_digest: run.into(),
        spec: spec.clone(),
        binding: binding.clone(),
    };
    ensure(
        artifacts,
        &name(run, &spec.key, "intent")?,
        &json!(intent),
        "application/vnd.craft.ticket-step+json",
        source,
    )?;
    Ok(())
}
fn qualified_resources(spec: &StepSpec, resources: &[ResourceRef]) -> bool {
    if spec.kind == Kind::Link {
        return resources.len() == spec.expected_resources.len()
            && resources
                .iter()
                .all(|r| spec.expected_resources.contains(r));
    }
    resources.len() == 1
        && resources[0].tenant_ref() == spec.target.tenant_ref()
        && resources[0].resource_type
            == if spec.kind == Kind::Issue {
                "issue"
            } else {
                "page"
            }
        && numeric(&resources[0].id)
}
pub(super) fn validate_import(
    spec: &StepSpec,
    envelope: &Envelope,
    record: &JournalRecord,
) -> Result<(), CraftError> {
    let step = record
        .steps
        .get(&spec.route)
        .ok_or_else(|| invalid("confirmed_native_step_required"))?;
    let data: crate::saved_requests::PublicData<crate::issue_create::IssueCreateData> =
        serde_json::from_value(
            envelope
                .data
                .clone()
                .ok_or_else(|| invalid("full_native_completion_required"))?,
        )
        .map_err(|_| invalid("full_native_completion_required"))?;
    let data = data.native;
    use craft_provider_jira::created_issue::SecondaryState;
    if spec.kind != Kind::Issue
        || record.steps.len() != 1
        || step.state != SubmissionState::Confirmed
        || step.job.is_some()
        || !qualified_resources(spec, &step.confirmed_resources)
        || envelope.schema_version != craft_core::contracts::SCHEMA_VERSION
        || !envelope.ok
        || envelope.error.is_some()
        || envelope.page.is_some()
        || envelope.command != craft_provider_jira::created_issue::COMMAND
        || data.receipt.operation_id != record.binding.operation_id
        || data.receipt.request_state != RequestState::Confirmed
        || data.receipt.job.is_some()
        || data.receipt.confirmed_resources != step.confirmed_resources
        || data.target != record.binding.target
        || data.step != spec.route
        || !matches!(
            data.persistence,
            crate::issue_create::PersistenceState::Recorded
        )
        || data.journal_revision != Some(record.revision)
        || !data
            .http_status
            .is_some_and(|status| (200..300).contains(&status))
        || data.reconciliation_required
        || data.replay_authorized
        || ![data.secondary.transition, data.secondary.watchers]
            .iter()
            .all(|value| matches!(value, None | Some(SecondaryState::Confirmed)))
    {
        return Err(invalid(
            "completion_does_not_match_confirmed_native_evidence",
        ));
    }
    Ok(())
}
pub(super) fn inspect(
    run: &str,
    spec: &StepSpec,
    artifacts: &dyn ArtifactStore,
    journal: &dyn OperationJournal,
    source: &ResourceRef,
) -> Result<Observed, CraftError> {
    if spec.kind == Kind::Reuse {
        return Err(invalid("reuse_is_not_a_native_submission"));
    }
    let operation_id = operation_id(run, &spec.key)?;
    let intent = load::<StepIntent>(
        artifacts,
        &name(run, &spec.key, "intent")?,
        "application/vnd.craft.ticket-step+json",
        source,
    )?
    .map(|v| v.0);
    let outcome = load::<Outcome>(
        artifacts,
        &name(run, &spec.key, "outcome")?,
        "application/vnd.craft.ticket-outcome+json",
        source,
    )?;
    if let Some(intent) = &intent {
        if intent.run_digest != run
            || intent.spec != *spec
            || intent.binding.operation_id != operation_id
            || intent.binding.target != spec.target
        {
            return Err(invalid("ticket_step_intent_mismatch"));
        }
        intent.binding.validate().map_err(local_error)?;
    }
    if outcome
        .as_ref()
        .is_some_and(|(outcome, _)| Some(&outcome.intent) != intent.as_ref())
    {
        return Err(invalid("ticket_outcome_binding_mismatch"));
    }
    let record = journal.get(&operation_id).map_err(local_error)?;
    if record.as_ref().is_some_and(|record| {
        intent
            .as_ref()
            .is_none_or(|intent| intent.binding != record.binding)
    }) {
        return Err(invalid("ticket_journal_binding_mismatch"));
    }
    let step = record.as_ref().and_then(|r| r.steps.get(&spec.route));
    if record
        .as_ref()
        .is_some_and(|r| r.steps.keys().any(|route| route != &spec.route))
    {
        return Err(invalid("unexpected_ticket_journal_step"));
    }
    let state = match step.map(|s| s.state) {
        None | Some(SubmissionState::Prepared) => RequestState::NotSent,
        Some(SubmissionState::Confirmed) => RequestState::Confirmed,
        Some(SubmissionState::Failed) => RequestState::Failed,
        _ => RequestState::SentUnknown,
    };
    let resources = step
        .map(|s| s.confirmed_resources.clone())
        .unwrap_or_default();
    if outcome.is_some() && state == RequestState::NotSent {
        return Err(invalid("terminal_ticket_outcome_has_no_native_claim"));
    }
    let envelope = outcome.as_ref().map(|(o, _)| o.envelope.clone());
    let native_complete = envelope.as_ref().is_some_and(|e| {
        e.ok && e.data.as_ref().is_some_and(|d| {
            d["receipt"]["operation_id"] == operation_id
                && d["receipt"]["request_state"] == "confirmed"
                && d["reconciliation_required"] == false
                && serde_json::from_value::<Vec<ResourceRef>>(
                    d["receipt"]["confirmed_resources"].clone(),
                )
                .is_ok_and(|r| {
                    r.len() == resources.len() && r.iter().all(|v| resources.contains(v))
                })
                && (spec.kind != Kind::Link || d["relation_confirmed"] == true)
        }) && e.command
            == match spec.kind {
                Kind::Issue => craft_provider_jira::created_issue::COMMAND,
                Kind::Link => craft_provider_jira::issue_links::write::CREATE,
                Kind::ResultsPage => craft_provider_confluence::pages::PAGE_CREATE,
                Kind::Reuse => "",
            }
    });
    let complete = state == RequestState::Confirmed
        && step.is_some_and(|step| step.job.is_none())
        && qualified_resources(spec, &resources)
        && (native_complete || (spec.kind == Kind::ResultsPage && outcome.is_none()));
    Ok(Observed {
        report: StepReport {
            key: spec.key.clone(),
            operation_id,
            kind: spec.kind,
            state,
            complete,
            resources,
            outcome_artifact: outcome.map(|v| v.1),
            recovered_from_journal: envelope.is_none() && state != RequestState::NotSent,
        },
        record,
        envelope,
        reuse: None,
    })
}
pub(super) fn save_outcome(
    run: &str,
    spec: &StepSpec,
    envelope: &Envelope,
    artifacts: &dyn ArtifactStore,
    source: &ResourceRef,
) -> Result<(), CraftError> {
    publish_outcome(run, spec, envelope, artifacts, source).map(|_| ())
}
pub(super) fn publish_outcome(
    run: &str,
    spec: &StepSpec,
    envelope: &Envelope,
    artifacts: &dyn ArtifactStore,
    source: &ResourceRef,
) -> Result<Option<ArtifactReceipt>, CraftError> {
    // Only the exchange owner has an HTTP response. A concurrent claim loser must not poison its outcome.
    if envelope
        .data
        .as_ref()
        .is_none_or(|data| data["http_status"].is_null())
    {
        return Ok(None);
    }
    let intent = load::<StepIntent>(
        artifacts,
        &name(run, &spec.key, "intent")?,
        "application/vnd.craft.ticket-step+json",
        source,
    )?
    .ok_or_else(|| invalid("missing_ticket_step_intent"))?
    .0;
    let outcome = Outcome {
        intent,
        envelope: envelope.clone(),
    };
    ensure(
        artifacts,
        &name(run, &spec.key, "outcome")?,
        &json!(outcome),
        "application/vnd.craft.ticket-outcome+json",
        source,
    )
    .map(Some)
}
