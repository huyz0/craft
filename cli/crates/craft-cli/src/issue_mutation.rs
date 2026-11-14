//! Internal issue mutation completion. Public command discovery remains separate.
pub mod command;
pub mod host;
#[cfg(test)]
mod tests;

use crate::dispatch::CommandResult;
pub use crate::issue_create::PersistenceState;
use crate::output::OutputPolicy;
use craft_core::contracts::{
    CredentialCacheUpdate, Envelope, OperationReceipt, RequestState, ResourceRef,
};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{NativeResponse, RetryAfter};
use craft_core::projection::{CompiledProjection, ProjectionContract, ProjectionMode};
use craft_core::registry::response_schema;
use craft_core::storage::{JournalRecord, OperationBinding, StoreError, SubmissionState};
use craft_provider_jira::issue_context::mutation::actions::{
    self, ASSIGN_ISSUE, ASSIGN_ROUTE, DELETE_ISSUE, DELETE_ROUTE, UNASSIGN_ISSUE,
};
use craft_provider_jira::issue_context::mutation::{
    self, APPLY_ROUTE, EDIT_ISSUE, TRANSITION_ISSUE, UPDATE_ROUTE,
};
use craft_runtime::journal::StepOutcome;
use craft_runtime::transport::MutationAttempt;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueMutationData {
    pub receipt: OperationReceipt,
    pub target: ResourceRef,
    pub step: String,
    pub persistence: PersistenceState,
    pub journal_revision: Option<u64>,
    pub http_status: Option<u16>,
    pub provider_request_id: Option<String>,
    pub retry_after: Option<RetryAfter>,
    pub reconciliation_required: bool,
    #[schemars(extend("const"=false))]
    pub replay_authorized: bool,
    pub credential_cache_update: Option<CredentialCacheUpdate>,
    pub prior_record: Option<JournalRecord>,
}
pub fn schema() -> Result<Value, CraftError> {
    response_schema::<IssueMutationData>()
}
pub fn projection() -> ProjectionContract {
    let fields = [
        "receipt",
        "target",
        "step",
        "persistence",
        "journal_revision",
        "http_status",
        "provider_request_id",
        "retry_after",
        "reconciliation_required",
        "replay_authorized",
        "credential_cache_update",
        "prior_record",
    ]
    .map(String::from)
    .to_vec();
    ProjectionContract {
        item_pointer: "".into(),
        always_returned: fields.clone(),
        default_selected: fields,
        mode: ProjectionMode::Local,
    }
}
pub fn route(command: &str) -> Result<&'static str, CraftError> {
    match command {
        EDIT_ISSUE => Ok(UPDATE_ROUTE),
        TRANSITION_ISSUE => Ok(APPLY_ROUTE),
        ASSIGN_ISSUE | UNASSIGN_ISSUE => Ok(ASSIGN_ROUTE),
        DELETE_ISSUE => Ok(DELETE_ROUTE),
        _ => Err(CraftError::field(
            "/command",
            "expected_qualified_issue_mutation",
        )),
    }
}
fn failure(code: ErrorCode, state: RequestState, message: &str) -> CraftError {
    let mut error = CraftError::new(code, message);
    error.details.write_state = state;
    error.next_action="Retain the receipt and inspect the same operation journal before any recovery decision; this result authorizes no replay.".into();
    error
}
fn request_state(state: SubmissionState) -> RequestState {
    match state {
        SubmissionState::Prepared => RequestState::NotSent,
        SubmissionState::Confirmed => RequestState::Confirmed,
        SubmissionState::Failed => RequestState::Failed,
        _ => RequestState::SentUnknown,
    }
}
pub struct MutationCompletion {
    binding: OperationBinding,
    command: String,
    request_id: String,
    projection: CompiledProjection,
    cache_update: Option<CredentialCacheUpdate>,
}
struct AttemptView<'a> {
    binding: &'a OperationBinding,
    command: &'a str,
    step: &'a str,
    route: &'a str,
    response: Result<&'a NativeResponse, &'a CraftError>,
    pending: Result<&'a JournalRecord, &'a StoreError>,
}
impl MutationCompletion {
    pub fn prepare(
        binding: OperationBinding,
        command: &str,
        request_id: &str,
        fields: &[String],
        maximum: usize,
    ) -> Result<Self, CraftError> {
        binding
            .validate()
            .map_err(|_| CraftError::field("/binding", "invalid_mutation_binding"))?;
        let step = route(command)?;
        craft_provider_jira::issue_context::validate_target(&binding.target, &binding.target.id)?;
        if !craft_provider_jira::issue_reads::numeric_id(&binding.target.id) {
            return Err(CraftError::field("/target", "expected_numeric_issue"));
        }
        craft_core::contracts::validate_identity("/request_id", request_id)?;
        if request_id.len() > 80 || !(512..=16_777_216).contains(&maximum) {
            return Err(CraftError::field(
                "/output",
                "invalid_output_identity_or_budget",
            ));
        }
        let selected = projection().compile(&schema()?, fields)?;
        let data = IssueMutationData {
            receipt: OperationReceipt {
                operation_id: binding.operation_id.clone(),
                request_state: RequestState::SentUnknown,
                confirmed_resources: vec![binding.target.clone()],
                job: None,
            },
            target: binding.target.clone(),
            step: step.into(),
            persistence: PersistenceState::Unrecorded,
            journal_revision: Some(u64::MAX),
            http_status: Some(599),
            provider_request_id: Some("\"".repeat(128)),
            retry_after: Some(RetryAfter::AtUnixMillis(u64::MAX)),
            reconciliation_required: true,
            replay_authorized: false,
            credential_cache_update: Some(CredentialCacheUpdate {
                grant: binding.grant.clone(),
                write_state: RequestState::SentUnknown,
                cleanup_pending: Some(false),
            }),
            prior_record: None,
        };
        let mut error = failure(
            ErrorCode::WriteOutcomeUnknown,
            RequestState::SentUnknown,
            &"x".repeat(500),
        );
        error.next_action = "x".repeat(512);
        let mut envelope = Envelope::failure_with_data(
            command,
            request_id.into(),
            error,
            selected.data(&json!(data))?,
        );
        envelope.warnings = vec!["x".repeat(256), "x".repeat(256)];
        if serde_json::to_vec(&envelope)
            .map_err(|_| {
                CraftError::new(ErrorCode::Internal, "Cannot preflight mutation receipt.")
            })?
            .len()
            + 1
            > maximum
        {
            return Err(CraftError::new(
                ErrorCode::OutputBudgetTooSmall,
                "Increase the output budget before acquiring mutation authority.",
            ));
        }
        Ok(Self {
            binding,
            command: command.into(),
            request_id: request_id.into(),
            projection: selected,
            cache_update: None,
        })
    }
    pub fn set_cache_update(&mut self, update: Option<CredentialCacheUpdate>) {
        self.cache_update = update;
    }
    fn render(
        self,
        data: IssueMutationData,
        mut error: Option<CraftError>,
        mut warnings: Vec<String>,
    ) -> CommandResult {
        let raw = json!(data);
        let selected = match self.projection.data(&raw) {
            Ok(selected) => selected,
            Err(_) => {
                error.get_or_insert_with(|| {
                    failure(
                        ErrorCode::Internal,
                        data.receipt.request_state,
                        "Output projection failed; full mutation evidence is retained.",
                    )
                });
                warnings
                    .push("Output projection failed; full mutation evidence is retained.".into());
                raw
            }
        };
        let mut envelope = match error {
            Some(error) => {
                Envelope::failure_with_data(&self.command, self.request_id, error, selected)
            }
            None => Envelope::success(&self.command, self.request_id, selected),
        };
        envelope.warnings = warnings;
        CommandResult {
            envelope,
            output_policy: OutputPolicy::PreserveEvidence,
        }
    }
    pub fn before_send_failure(
        self,
        mut error: CraftError,
        prior: Option<&JournalRecord>,
    ) -> CommandResult {
        let prior = prior.filter(|value| value.validate().is_ok());
        let saved = prior
            .filter(|value| value.binding == self.binding)
            .and_then(|value| value.steps.get(route(&self.command).ok()?));
        let uncertain = error.details.write_state == RequestState::SentUnknown;
        let state = saved.map_or(
            if uncertain {
                RequestState::SentUnknown
            } else {
                RequestState::NotSent
            },
            |step| {
                if uncertain && step.state == SubmissionState::Prepared {
                    RequestState::SentUnknown
                } else {
                    request_state(step.state)
                }
            },
        );
        error.details.write_state = state;
        let data = IssueMutationData {
            receipt: OperationReceipt {
                operation_id: self.binding.operation_id.clone(),
                request_state: state,
                confirmed_resources: saved
                    .map_or_else(Vec::new, |step| step.confirmed_resources.clone()),
                job: saved.and_then(|step| step.job.clone()),
            },
            target: self.binding.target.clone(),
            step: route(&self.command).unwrap_or("").into(),
            persistence: if prior.is_some() {
                PersistenceState::Recorded
            } else {
                PersistenceState::Unrecorded
            },
            journal_revision: prior.map(|value| value.revision),
            http_status: None,
            provider_request_id: None,
            retry_after: None,
            reconciliation_required: prior.is_some() || state == RequestState::SentUnknown,
            replay_authorized: false,
            credential_cache_update: self.cache_update.clone(),
            prior_record: prior.cloned(),
        };
        self.render(data, Some(error), vec![])
    }
    pub fn finish(self, attempt: &MutationAttempt) -> CommandResult {
        self.finish_with(
            AttemptView {
                binding: attempt.binding(),
                command: attempt.command(),
                step: attempt.step(),
                route: attempt.route(),
                response: attempt.response.as_ref(),
                pending: attempt.pending_evidence.as_ref(),
            },
            |outcome| attempt.record_outcome(outcome),
        )
    }
    fn finish_with(
        self,
        view: AttemptView<'_>,
        record: impl FnOnce(&StepOutcome) -> Result<JournalRecord, StoreError>,
    ) -> CommandResult {
        let recognized =
            route(view.command).is_ok_and(|route| route == view.route && route == view.step);
        let mut observed = match view.response {
            Ok(response) if recognized => {
                mutation::result::interpret(view.command, &view.binding.target, response)
            }
            _ => mutation::result::MutationObservation {
                state: SubmissionState::Unknown,
                confirmed_resources: vec![],
                error: Some(failure(
                    view.response
                        .err()
                        .map_or(ErrorCode::PolicyDenied, |error| error.code),
                    RequestState::SentUnknown,
                    "Issue mutation lacks qualified response evidence.",
                )),
            },
        };
        if self.binding != *view.binding || self.command != view.command || !recognized {
            observed.error = Some(failure(
                ErrorCode::PolicyDenied,
                request_state(observed.state),
                "Completion differs from the original mutation attempt; actual evidence is retained.",
            ));
        }
        let prior = view
            .pending
            .ok()
            .filter(|prior| prior.binding == *view.binding && prior.validate().is_ok());
        let mut retained_record = prior.filter(|record| record.steps.len() != 1).cloned();
        let mut job = None;
        if let Some(saved) = prior.and_then(|prior| prior.steps.get(view.step)) {
            for resource in &saved.confirmed_resources {
                if !observed.confirmed_resources.contains(resource) {
                    observed.confirmed_resources.push(resource.clone());
                }
            }
            job = saved.job.clone();
            if saved.state == SubmissionState::Confirmed
                && observed.state != SubmissionState::Confirmed
            {
                observed.state = SubmissionState::Confirmed;
                observed.error = Some(failure(
                    ErrorCode::PartialApply,
                    RequestState::Confirmed,
                    "Recorded confirmation conflicts with the current response; prior evidence is retained.",
                ));
            }
            if observed.state == SubmissionState::Failed
                && (!observed.confirmed_resources.is_empty() || job.is_some())
            {
                observed.state = SubmissionState::Unknown;
                observed.error = Some(failure(
                    ErrorCode::WriteOutcomeUnknown,
                    RequestState::SentUnknown,
                    "Refusal conflicts with earlier effect evidence.",
                ));
            }
        }
        let outcome = StepOutcome {
            state: observed.state,
            confirmed_resources: observed.confirmed_resources.clone(),
            job: job.clone(),
        };
        let persisted = record(&outcome).and_then(|saved| {
            saved.validate()?;
            if saved.binding != *view.binding
                || saved.steps.get(view.step).is_none_or(|step| {
                    step.state != outcome.state
                        || outcome
                            .confirmed_resources
                            .iter()
                            .any(|resource| !step.confirmed_resources.contains(resource))
                        || outcome
                            .job
                            .as_ref()
                            .is_some_and(|job| step.job.as_ref() != Some(job))
                })
            {
                return Err(StoreError::Conflict);
            }
            Ok(saved)
        });
        let mut warnings = vec![];
        if view.pending.is_err() {
            warnings.push("Initial uncertainty recording was not acknowledged; retain the final persistence disposition.".into());
        }
        let (persistence, revision) = match persisted {
            Ok(saved) => {
                if saved.steps.len() != 1 {
                    retained_record = Some(saved.clone());
                }
                if let Some(step) = saved.steps.get(view.step) {
                    for resource in &step.confirmed_resources {
                        if !observed.confirmed_resources.contains(resource) {
                            observed.confirmed_resources.push(resource.clone());
                        }
                    }
                    job = step.job.clone().or(job);
                }
                (PersistenceState::Recorded, Some(saved.revision))
            }
            Err(error) => {
                warnings.push("Local receipt recording was not acknowledged; retain the returned effect evidence and inspect the journal.".into());
                if observed.error.is_none() {
                    observed.error = Some(failure(
                        ErrorCode::Internal,
                        request_state(observed.state),
                        "The observed issue result could not be durably acknowledged locally.",
                    ));
                }
                (
                    if error == StoreError::OutcomeUnknown {
                        PersistenceState::Uncertain
                    } else {
                        PersistenceState::Unrecorded
                    },
                    None,
                )
            }
        };
        if observed.error.is_none()
            && (observed.confirmed_resources != vec![view.binding.target.clone()]
                || job.is_some()
                || retained_record.is_some())
        {
            observed.error = Some(failure(
                ErrorCode::PartialApply,
                request_state(observed.state),
                "Additional recorded effect evidence requires inspection.",
            ));
        }
        let data = IssueMutationData {
            receipt: OperationReceipt {
                operation_id: view.binding.operation_id.clone(),
                request_state: request_state(observed.state),
                confirmed_resources: observed.confirmed_resources,
                job,
            },
            target: view.binding.target.clone(),
            step: view.step.into(),
            persistence,
            journal_revision: revision,
            http_status: view.response.ok().map(|response| response.status),
            provider_request_id: view
                .response
                .ok()
                .and_then(|response| response.request_id.clone()),
            retry_after: view.response.ok().and_then(|response| response.retry_after),
            reconciliation_required: observed.error.is_some(),
            replay_authorized: false,
            credential_cache_update: self.cache_update.clone(),
            prior_record: retained_record,
        };
        self.render(data, observed.error, warnings)
    }
}
