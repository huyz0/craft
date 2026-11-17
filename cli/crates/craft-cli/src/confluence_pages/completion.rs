use super::*;
use crate::output::OutputPolicy;
use craft_core::contracts::{CredentialCacheUpdate, Envelope, OperationReceipt, RequestState};
use craft_core::execution::RetryAfter;
use craft_core::registry::response_schema;
use craft_core::storage::{JournalStep, StoreError};
use craft_runtime::auth::AuthFailure;
use craft_runtime::journal::StepOutcome;
use craft_runtime::transport::MutationAttempt;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Persistence {
    Recorded,
    Unrecorded,
    Uncertain,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PublicationData {
    pub receipt: OperationReceipt,
    pub target: ResourceRef,
    pub space_id: String,
    pub version: Option<u32>,
    pub persistence: Persistence,
    pub journal: Option<JournalRecord>,
    pub http_status: Option<u16>,
    pub provider_request_id: Option<String>,
    pub retry_after: Option<RetryAfter>,
    pub reconciliation_required: bool,
    #[schemars(extend("const" = false))]
    pub replay_authorized: bool,
    /// Present for updates: the native publish may overwrite divergent unpublished editor drafts.
    pub editor_draft_policy: Option<pages::EditorDraftPolicy>,
    #[schemars(extend("const" = false))]
    pub editor_draft_preservation_verified: bool,
    /// Present for width writes: the per-property outcome of the two appearance writes.
    pub width: Option<PageWidthReceipt>,
    pub credential_cache_update: Option<CredentialCacheUpdate>,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageWidthReceipt {
    pub requested: pages::PageWidth,
    /// Set only when both appearance properties were confirmed with the requested value.
    pub confirmed: Option<pages::PageWidth>,
    /// `content-appearance-published`: what viewers see.
    pub published: RequestState,
    /// `content-appearance-draft`: what the editor opens with; sent only after published is
    /// confirmed.
    pub draft: RequestState,
}
/// One claimed width-property step after its response was interpreted and recorded.
pub(super) struct StepEvidence {
    observed: pages::PageWriteObservation,
    persisted: Result<JournalRecord, StoreError>,
    pending_acknowledged: bool,
    response: Option<(u16, Option<String>, Option<RetryAfter>)>,
}
impl StepEvidence {
    /// Only a confirmed and durably recorded step allows the next property write.
    pub(super) fn allows_next(&self) -> bool {
        self.observed.state == SubmissionState::Confirmed && self.persisted.is_ok()
    }
}
pub fn schema() -> Result<Value, CraftError> {
    response_schema::<PublicationData>()
}
pub(super) struct Completion {
    binding: OperationBinding,
    command: String,
    /// The first journal step; single-request writes have only this one.
    route: String,
    /// Every journal step this operation may claim, in send order.
    steps: Vec<String>,
    width: Option<pages::PageWidth>,
    space_id: String,
    request_id: String,
    draft_policy: Option<pages::EditorDraftPolicy>,
    cache: Option<CredentialCacheUpdate>,
}
/// The whole operation from its steps in send order: uncertain if any step is, confirmed only if
/// every step is, not sent if none was, and otherwise failed.
fn overall(states: &[RequestState]) -> RequestState {
    if states.contains(&RequestState::SentUnknown) {
        RequestState::SentUnknown
    } else if states.iter().all(|s| *s == RequestState::Confirmed) {
        RequestState::Confirmed
    } else if states.iter().all(|s| *s == RequestState::NotSent) {
        RequestState::NotSent
    } else {
        RequestState::Failed
    }
}
fn request_state(state: SubmissionState) -> RequestState {
    match state {
        SubmissionState::Confirmed => RequestState::Confirmed,
        SubmissionState::Failed => RequestState::Failed,
        SubmissionState::Prepared => RequestState::NotSent,
        _ => RequestState::SentUnknown,
    }
}
fn failure(code: ErrorCode, state: RequestState, message: &str) -> CraftError {
    let mut error = CraftError::new(code, message);
    error.details.write_state = state;
    error.next_action = "Retain this receipt and inspect the same operation journal before another page write. No replay is authorized.".into();
    error
}
impl Completion {
    pub(super) fn prepare(
        operation: &PreparedOperation,
        binding: OperationBinding,
        space_id: &str,
        request_id: &str,
    ) -> Result<Self, CraftError> {
        validate_identity("/request_id", request_id)?;
        if request_id.len() > 80 {
            return Err(CraftError::field(
                "/request_id",
                "expected_at_most_80_bytes",
            ));
        }
        let command = operation.request().command.clone();
        let kind = pages::PageWriteKind::from_command(&command).ok_or_else(denied)?;
        let steps: Vec<String> = kind.journal_steps().into_iter().map(String::from).collect();
        let route = steps.first().cloned().ok_or_else(denied)?;
        let width = if kind == pages::PageWriteKind::WidthSet {
            Some(
                serde_json::from_value::<PageWidthSetInput>(operation.request().input.clone())
                    .map_err(|_| denied())?
                    .width,
            )
        } else {
            None
        };
        let draft_policy = (command == PAGE_UPDATE)
            .then_some(pages::EditorDraftPolicy::NativeReconciliationMayOverwrite);
        let resource = ResourceRef {
            resource_type: "page".into(),
            id: i64::MAX.to_string(),
            ..binding.target.clone()
        };
        let journal = JournalRecord {
            binding: binding.clone(),
            revision: u64::MAX,
            steps: steps
                .iter()
                .map(|step| {
                    (
                        step.clone(),
                        JournalStep {
                            request_digest: Some(format!("sha256:{}", "f".repeat(64))),
                            state: SubmissionState::Unknown,
                            confirmed_resources: vec![resource.clone()],
                            job: None,
                        },
                    )
                })
                .collect(),
        };
        let data = PublicationData {
            receipt: OperationReceipt {
                operation_id: binding.operation_id.clone(),
                request_state: RequestState::SentUnknown,
                confirmed_resources: vec![resource],
                job: None,
            },
            target: binding.target.clone(),
            space_id: space_id.into(),
            version: Some(i32::MAX as u32),
            persistence: Persistence::Uncertain,
            journal: Some(journal),
            http_status: Some(599),
            provider_request_id: Some("r".repeat(128)),
            retry_after: Some(RetryAfter::AtUnixMillis(u64::MAX)),
            reconciliation_required: true,
            replay_authorized: false,
            editor_draft_policy: draft_policy,
            editor_draft_preservation_verified: false,
            width: width.map(|requested| PageWidthReceipt {
                requested,
                confirmed: Some(requested),
                published: RequestState::SentUnknown,
                draft: RequestState::SentUnknown,
            }),
            credential_cache_update: Some(CredentialCacheUpdate {
                grant: GrantRef {
                    id: "g".repeat(512),
                },
                write_state: RequestState::SentUnknown,
                cleanup_pending: Some(false),
            }),
        };
        let mut error = failure(
            ErrorCode::WriteOutcomeUnknown,
            RequestState::SentUnknown,
            &"x".repeat(500),
        );
        error.next_action = "x".repeat(512);
        let mut envelope =
            Envelope::failure_with_data(&command, request_id.into(), error, json!(data));
        envelope.warnings = vec!["x".repeat(256), "x".repeat(256)];
        if serde_json::to_vec(&envelope)
            .map_err(|_| denied())?
            .len()
            .saturating_add(1)
            > operation.request().selection.max_bytes as usize
        {
            return Err(CraftError::new(
                ErrorCode::OutputBudgetTooSmall,
                "The output budget cannot retain complete page publication and credential-cache receipts. Increase max-bytes before authentication.",
            ));
        }
        Ok(Self {
            binding,
            command,
            route,
            steps,
            width,
            space_id: space_id.into(),
            request_id: request_id.into(),
            draft_policy,
            cache: None,
        })
    }
    pub(super) fn cache_success(&mut self, access: &VerifiedServiceAccess) {
        self.cache = access.cache_update().map(|r| CredentialCacheUpdate {
            grant: access.grant().clone(),
            write_state: RequestState::Confirmed,
            cleanup_pending: Some(r.cleanup_pending),
        });
    }
    pub(super) fn cache_failure(&mut self, grant: GrantRef, failure: &AuthFailure) {
        self.cache = failure
            .credential_update
            .as_ref()
            .map(|r| CredentialCacheUpdate {
                grant: grant.clone(),
                write_state: RequestState::Confirmed,
                cleanup_pending: Some(r.cleanup_pending),
            })
            .or_else(|| {
                failure
                    .credential_write_state
                    .map(|s| CredentialCacheUpdate {
                        grant,
                        write_state: s,
                        cleanup_pending: None,
                    })
            });
    }
    fn data(&self, state: RequestState) -> PublicationData {
        PublicationData {
            receipt: OperationReceipt {
                operation_id: self.binding.operation_id.clone(),
                request_state: state,
                confirmed_resources: vec![],
                job: None,
            },
            target: self.binding.target.clone(),
            space_id: self.space_id.clone(),
            version: None,
            persistence: Persistence::Unrecorded,
            journal: None,
            http_status: None,
            provider_request_id: None,
            retry_after: None,
            reconciliation_required: state == RequestState::SentUnknown,
            replay_authorized: false,
            editor_draft_policy: self.draft_policy,
            editor_draft_preservation_verified: false,
            width: self.width.map(|requested| PageWidthReceipt {
                requested,
                confirmed: None,
                published: RequestState::NotSent,
                draft: RequestState::NotSent,
            }),
            credential_cache_update: self.cache.clone(),
        }
    }
    fn retain_record(&self, data: &mut PublicationData, record: &JournalRecord) {
        if record.binding != self.binding || record.validate().is_err() {
            return;
        }
        for step in record.steps.values() {
            for resource in &step.confirmed_resources {
                if !data.receipt.confirmed_resources.contains(resource) {
                    data.receipt.confirmed_resources.push(resource.clone());
                }
            }
            data.receipt.job = step.job.clone().or(data.receipt.job.take());
        }
        data.journal = Some(record.clone());
    }
    fn result(
        self,
        data: PublicationData,
        error: Option<CraftError>,
        warnings: Vec<String>,
    ) -> CommandResult {
        let mut envelope = match error {
            Some(error) => {
                Envelope::failure_with_data(&self.command, self.request_id, error, json!(data))
            }
            None => Envelope::success(&self.command, self.request_id, json!(data)),
        };
        envelope.warnings = warnings;
        CommandResult {
            envelope,
            output_policy: OutputPolicy::PreserveEvidence,
        }
    }
    pub(super) fn before_send(
        self,
        mut error: CraftError,
        prior: Option<&JournalRecord>,
    ) -> CommandResult {
        let prior = prior.filter(|p| p.binding == self.binding && p.validate().is_ok());
        // An unacknowledged durable claim must remain uncertain even without an attempt handle.
        // Authentication failures explicitly mark the separate page exchange as not sent.
        let initial = if error.code == ErrorCode::WriteOutcomeUnknown
            && error.details.write_state == RequestState::SentUnknown
        {
            RequestState::SentUnknown
        } else {
            RequestState::NotSent
        };
        // Each step reads as its recorded state; a single-step write keeps its original rule.
        let recorded: Vec<_> = self
            .steps
            .iter()
            .map(|step| {
                prior
                    .and_then(|p| p.steps.get(step))
                    .map_or(RequestState::NotSent, |s| request_state(s.state))
            })
            .collect();
        let state = if prior.is_some_and(|p| self.steps.iter().any(|s| p.steps.contains_key(s))) {
            let worst = overall(&recorded);
            if initial == RequestState::SentUnknown && worst == RequestState::NotSent {
                initial
            } else {
                worst
            }
        } else {
            initial
        };
        let mut data = self.data(state);
        if let Some(width) = data.width.as_mut() {
            width.published = recorded.first().copied().unwrap_or(RequestState::NotSent);
            width.draft = recorded.get(1).copied().unwrap_or(RequestState::NotSent);
        }
        if let Some(prior) = prior {
            self.retain_record(&mut data, prior);
            data.persistence = Persistence::Recorded;
            data.reconciliation_required = state != RequestState::NotSent
                || prior.steps.keys().any(|s| !self.steps.contains(s));
        }
        error.details.write_state = state;
        self.result(data, Some(error), vec![])
    }
    pub(super) fn claim_failure(
        self,
        mut error: CraftError,
        prior: Option<&JournalRecord>,
    ) -> CommandResult {
        if error.code == ErrorCode::WriteOutcomeUnknown {
            error.details.write_state = RequestState::SentUnknown;
        }
        self.before_send(error, prior)
    }
    pub(super) fn finish(
        self,
        attempt: &MutationAttempt,
        write: &PreparedPageWrite,
    ) -> CommandResult {
        let recognized = attempt.binding() == &self.binding
            && attempt.command() == self.command
            && attempt.route() == self.route
            && attempt.step() == self.route;
        let mut observed = match &attempt.response {
            Ok(response) if recognized => write.interpret(response),
            _ => pages::PageWriteObservation {
                state: SubmissionState::Unknown,
                resources: vec![],
                version: None,
                error: Some(failure(
                    attempt
                        .response
                        .as_ref()
                        .err()
                        .map_or(ErrorCode::PolicyDenied, |e| e.code),
                    RequestState::SentUnknown,
                    "The claimed page write lacks qualified response evidence.",
                )),
            },
        };
        let mut data = self.data(request_state(observed.state));
        if let Ok(record) = &attempt.pending_evidence {
            self.retain_record(&mut data, record);
        }
        for r in &data.receipt.confirmed_resources {
            if !observed.resources.contains(r) {
                observed.resources.push(r.clone());
            }
        }
        if observed.state == SubmissionState::Failed && !observed.resources.is_empty() {
            observed.state = SubmissionState::Unknown;
            observed.error = Some(failure(
                ErrorCode::WriteOutcomeUnknown,
                RequestState::SentUnknown,
                "Refusal conflicts with already recorded page effects.",
            ));
        }
        let outcome = StepOutcome {
            state: observed.state,
            confirmed_resources: observed.resources.clone(),
            job: data.receipt.job.clone(),
        };
        let persisted = attempt.record_outcome(&outcome).and_then(|record| {
            record.validate()?;
            if record.binding != self.binding
                || record.steps.get(&self.route).is_none_or(|s| {
                    s.state != outcome.state
                        || outcome
                            .confirmed_resources
                            .iter()
                            .any(|r| !s.confirmed_resources.contains(r))
                })
            {
                return Err(StoreError::Conflict);
            }
            Ok(record)
        });
        let mut warnings = vec![];
        if attempt.pending_evidence.is_err() {
            warnings.push("The initial uncertainty update was not acknowledged. Inspect final persistence and retain this receipt.".into());
        }
        data.receipt.request_state = request_state(observed.state);
        data.receipt.confirmed_resources = observed.resources;
        data.version = observed.version;
        match persisted {
            Ok(record) => {
                self.retain_record(&mut data, &record);
                data.persistence = Persistence::Recorded;
            }
            Err(error) => {
                data.persistence = if error == StoreError::OutcomeUnknown {
                    Persistence::Uncertain
                } else {
                    Persistence::Unrecorded
                };
                warnings.push("Final local recording was not acknowledged. Retain the remote evidence and inspect the journal before another page write.".into());
                if observed.error.is_none() {
                    observed.error = Some(failure(
                        ErrorCode::Internal,
                        data.receipt.request_state,
                        "Page publication was observed but its local receipt was not durably acknowledged.",
                    ));
                }
            }
        }
        if let Ok(response) = &attempt.response {
            data.http_status = Some(response.status);
            data.provider_request_id = response.request_id.clone();
            data.retry_after = response.retry_after;
        }
        if observed.error.is_none()
            && (data.receipt.confirmed_resources.len() != 1 || data.receipt.job.is_some())
        {
            observed.error = Some(failure(
                ErrorCode::PartialApply,
                data.receipt.request_state,
                "Additional recorded resource or job evidence requires inspection.",
            ));
        }
        data.reconciliation_required = observed.error.is_some();
        if let Some(error) = observed.error.as_mut() {
            error.details.write_state = data.receipt.request_state;
        }
        self.result(data, observed.error, warnings)
    }
}
/// Width writes claim one journal step per appearance property under the same operation.
impl Completion {
    /// Interprets and records one claimed property step. Only this step's own response and
    /// record decide its outcome; evidence from the other step never upgrades or downgrades it.
    pub(super) fn record_step(
        &self,
        attempt: &MutationAttempt,
        write: &PreparedPageWrite,
    ) -> StepEvidence {
        let step = write.width_property().unwrap_or_default();
        let recognized = attempt.binding() == &self.binding
            && attempt.command() == self.command
            && attempt.route() == write.call().route
            && attempt.step() == step
            && self.steps.iter().any(|s| s == step);
        let observed = match &attempt.response {
            Ok(response) if recognized => write.interpret(response),
            _ => pages::PageWriteObservation {
                state: SubmissionState::Unknown,
                resources: vec![],
                version: None,
                error: Some(failure(
                    attempt
                        .response
                        .as_ref()
                        .err()
                        .map_or(ErrorCode::PolicyDenied, |e| e.code),
                    RequestState::SentUnknown,
                    "The claimed width property write lacks qualified response evidence.",
                )),
            },
        };
        let outcome = StepOutcome {
            state: observed.state,
            confirmed_resources: observed.resources.clone(),
            job: None,
        };
        let persisted = attempt.record_outcome(&outcome).and_then(|record| {
            record.validate()?;
            if record.binding != self.binding
                || record.steps.get(step).is_none_or(|s| {
                    s.state != outcome.state
                        || outcome
                            .confirmed_resources
                            .iter()
                            .any(|r| !s.confirmed_resources.contains(r))
                })
            {
                return Err(StoreError::Conflict);
            }
            Ok(record)
        });
        StepEvidence {
            observed,
            persisted,
            pending_acknowledged: attempt.pending_evidence.is_ok(),
            response: attempt
                .response
                .as_ref()
                .ok()
                .map(|r| (r.status, r.request_id.clone(), r.retry_after)),
        }
    }
    /// Builds one receipt from the claimed steps in send order. `stopped` is the error that kept
    /// a later step from being claimed, if any; that step stays not sent.
    pub(super) fn finish_steps(
        self,
        steps: Vec<StepEvidence>,
        stopped: Option<CraftError>,
    ) -> CommandResult {
        // An unacknowledged claim of the next step may already have been sent.
        let stopped_unknown = stopped
            .as_ref()
            .is_some_and(|e| e.code == ErrorCode::WriteOutcomeUnknown);
        let states: Vec<_> = (0..self.steps.len())
            .map(|i| match steps.get(i) {
                Some(s) => request_state(s.observed.state),
                None if i == steps.len() && stopped_unknown => RequestState::SentUnknown,
                None => RequestState::NotSent,
            })
            .collect();
        let state = overall(&states);
        let mut data = self.data(state);
        let mut warnings = vec![];
        let mut error = None;
        for evidence in &steps {
            match &evidence.persisted {
                Ok(record) => {
                    self.retain_record(&mut data, record);
                    data.persistence = Persistence::Recorded;
                }
                Err(store) => {
                    data.persistence = if *store == StoreError::OutcomeUnknown {
                        Persistence::Uncertain
                    } else {
                        Persistence::Unrecorded
                    };
                    warnings.push("Final local recording was not acknowledged. Retain the remote evidence and inspect the journal before another page write.".into());
                    error.get_or_insert_with(|| {
                        failure(
                            ErrorCode::Internal,
                            state,
                            "A width property write was observed but its local receipt was not durably acknowledged.",
                        )
                    });
                }
            }
            if !evidence.pending_acknowledged {
                warnings.push("The initial uncertainty update was not acknowledged. Inspect final persistence and retain this receipt.".into());
            }
            if let Some((status, request_id, retry_after)) = &evidence.response {
                data.http_status = Some(*status);
                data.provider_request_id = request_id.clone();
                data.retry_after = *retry_after;
            }
        }
        // The first unconfirmed step explains the stop; a later step was never claimed.
        if let Some(step_error) = steps
            .iter()
            .find(|s| s.observed.state != SubmissionState::Confirmed)
            .and_then(|s| s.observed.error.clone())
        {
            error = Some(step_error);
        }
        let confirmed = states
            .iter()
            .filter(|s| **s == RequestState::Confirmed)
            .count();
        if confirmed > 0 && confirmed < states.len() {
            // Viewers may see the new width while the editor still opens with the old one.
            let mut partial = failure(
                ErrorCode::PartialApply,
                state,
                "Only some page appearance properties were confirmed.",
            );
            partial.next_action = "Retain this receipt and do not replay this operation. A new operation ID re-reads both properties and rewrites them.".into();
            error = Some(partial);
        } else if error.is_none()
            && let Some(stopped) = stopped
        {
            error = Some(stopped);
        }
        data.receipt.request_state = state;
        for evidence in &steps {
            for r in &evidence.observed.resources {
                if !data.receipt.confirmed_resources.contains(r) {
                    data.receipt.confirmed_resources.push(r.clone());
                }
            }
        }
        if error.is_none() && data.receipt.confirmed_resources.len() != 1 {
            error = Some(failure(
                ErrorCode::PartialApply,
                state,
                "Additional recorded resource evidence requires inspection.",
            ));
        }
        if let Some(width) = data.width.as_mut() {
            width.published = states.first().copied().unwrap_or(RequestState::NotSent);
            width.draft = states.get(1).copied().unwrap_or(RequestState::NotSent);
            width.confirmed =
                (state == RequestState::Confirmed && error.is_none()).then_some(width.requested);
        }
        data.reconciliation_required = error.is_some();
        if let Some(error) = error.as_mut() {
            error.details.write_state = state;
        }
        self.result(data, error, warnings)
    }
}
