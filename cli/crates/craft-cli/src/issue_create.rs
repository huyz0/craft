//! Jira create receipts and journaled execution with optional reviewed-material constraints.

pub use craft_core::contracts::CredentialCacheUpdate;
use craft_core::contracts::{Envelope, GrantRef, OperationReceipt, RequestState, ResourceRef};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{NativeResponse, RetryAfter};
use craft_core::projection::{CompiledProjection, ProjectionContract, ProjectionMode};
use craft_core::registry::response_schema;
use craft_core::storage::{JournalRecord, OperationBinding, StoreError, SubmissionState};
use craft_provider_jira::created_issue::{self, COMMAND, ROUTE, SecondaryResults};
use craft_runtime::journal::StepOutcome;
use craft_runtime::transport::MutationAttempt;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::dispatch::CommandResult;
use crate::output::OutputPolicy;

pub mod command;
pub mod host;
pub(crate) mod material;
#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PersistenceState {
    Recorded,
    Unrecorded,
    Uncertain,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueCreateData {
    pub receipt: OperationReceipt,
    pub target: ResourceRef,
    pub step: String,
    pub issue_key: Option<String>,
    pub secondary: SecondaryResults,
    pub persistence: PersistenceState,
    pub journal_revision: Option<u64>,
    pub http_status: Option<u16>,
    pub provider_request_id: Option<String>,
    pub retry_after: Option<RetryAfter>,
    pub reconciliation_required: bool,
    #[schemars(extend("const" = false))]
    pub replay_authorized: bool,
    pub credential_cache_update: Option<CredentialCacheUpdate>,
}

pub struct CreateCompletion {
    binding: OperationBinding,
    request_id: String,
    projection: CompiledProjection,
    cache_update: Option<CredentialCacheUpdate>,
}

struct AttemptView<'a> {
    binding: &'a OperationBinding,
    command: &'a str,
    route: &'a str,
    step: &'a str,
    response: Result<&'a NativeResponse, &'a CraftError>,
    pending_failed: bool,
    prior: Option<&'a JournalRecord>,
}

pub fn schema() -> Result<Value, CraftError> {
    response_schema::<IssueCreateData>()
}

pub fn projection() -> ProjectionContract {
    ProjectionContract {
        item_pointer: "".into(),
        always_returned: [
            "receipt",
            "target",
            "step",
            "persistence",
            "journal_revision",
            "reconciliation_required",
            "replay_authorized",
            "credential_cache_update",
        ]
        .map(String::from)
        .into(),
        default_selected: [
            "receipt",
            "target",
            "step",
            "issue_key",
            "secondary",
            "persistence",
            "journal_revision",
            "http_status",
            "provider_request_id",
            "retry_after",
            "reconciliation_required",
            "replay_authorized",
            "credential_cache_update",
        ]
        .map(String::from)
        .into(),
        mode: ProjectionMode::Local,
    }
}

fn failure(code: ErrorCode, state: RequestState, message: &str) -> CraftError {
    let mut error = CraftError::new(code, message);
    error.details.write_state = state;
    error.next_action = "Retain the issue receipt and inspect the same operation journal; reconcile before another create.".into();
    error
}

fn request_state(state: SubmissionState) -> RequestState {
    match state {
        SubmissionState::Confirmed => RequestState::Confirmed,
        SubmissionState::Failed => RequestState::Failed,
        _ => RequestState::SentUnknown,
    }
}

impl CreateCompletion {
    /// Preflights ordinary receipt/recovery bounds before the caller obtains a send claim.
    pub fn prepare(
        binding: OperationBinding,
        request_id: &str,
        fields: &[String],
        max_bytes: usize,
    ) -> Result<Self, CraftError> {
        binding
            .validate()
            .map_err(|_| CraftError::field("/binding", "invalid_mutation_binding"))?;
        created_issue::validate_target(&binding.target)?;
        craft_core::contracts::validate_identity("/request_id", request_id)?;
        if request_id.len() > 80 || !(512..=16_777_216).contains(&max_bytes) {
            return Err(CraftError::field(
                "/output",
                "invalid_output_identity_or_budget",
            ));
        }
        let projection = projection().compile(&schema()?, fields)?;
        let worst = IssueCreateData {
            receipt: OperationReceipt {
                operation_id: binding.operation_id.clone(),
                request_state: RequestState::SentUnknown,
                confirmed_resources: vec![ResourceRef {
                    resource_type: "issue".into(),
                    id: "9".repeat(512),
                    ..binding.target.clone()
                }],
                job: None,
            },
            target: binding.target.clone(),
            step: ROUTE.into(),
            issue_key: Some("K".repeat(512)),
            secondary: SecondaryResults {
                transition: Some(created_issue::SecondaryState::Confirmed),
                watchers: Some(created_issue::SecondaryState::Confirmed),
            },
            persistence: PersistenceState::Unrecorded,
            journal_revision: Some(u64::MAX),
            http_status: Some(599),
            provider_request_id: Some("r".repeat(128)),
            retry_after: Some(RetryAfter::AtUnixMillis(u64::MAX)),
            reconciliation_required: true,
            replay_authorized: false,
            credential_cache_update: Some(CredentialCacheUpdate {
                grant: GrantRef {
                    id: "g".repeat(512),
                },
                write_state: RequestState::SentUnknown,
                cleanup_pending: Some(false),
            }),
        };
        let data = projection.data(&json!(worst))?;
        let mut error = failure(
            ErrorCode::WriteOutcomeUnknown,
            RequestState::SentUnknown,
            &"x".repeat(500),
        );
        error.next_action = "x".repeat(512);
        let mut envelope = Envelope::failure_with_data(COMMAND, request_id.into(), error, data);
        envelope.warnings = vec!["x".repeat(256), "x".repeat(256)];
        if serde_json::to_vec(&envelope)
            .map_err(|_| {
                failure(
                    ErrorCode::Internal,
                    RequestState::NotSent,
                    "Cannot preflight issue recovery output.",
                )
            })?
            .len()
            .saturating_add(1)
            > max_bytes
        {
            let mut error = CraftError::new(
                ErrorCode::OutputBudgetTooSmall,
                "Output budget cannot carry the bounded issue receipt and recovery evidence.",
            );
            error.next_action = "Increase --max-bytes before acquiring a send claim.".into();
            return Err(error);
        }
        Ok(Self {
            binding,
            request_id: request_id.into(),
            projection,
            cache_update: None,
        })
    }

    pub fn set_cache_update(&mut self, update: Option<CredentialCacheUpdate>) {
        self.cache_update = update;
    }

    fn before_send_failure(self, error: CraftError) -> CommandResult {
        self.before_send_failure_with_record(error, None)
    }

    fn before_send_failure_with_record(
        self,
        mut error: CraftError,
        prior: Option<&JournalRecord>,
    ) -> CommandResult {
        // Read/token failures describe a different exchange. No issue send began here.
        error.details.write_state = if error.code == ErrorCode::WriteOutcomeUnknown {
            RequestState::SentUnknown
        } else {
            RequestState::NotSent
        };
        let prior =
            prior.filter(|record| record.binding == self.binding && record.validate().is_ok());
        let saved = prior.and_then(|record| record.steps.get(ROUTE));
        if let Some(saved) = saved {
            error.details.write_state = if saved.state == SubmissionState::Prepared {
                RequestState::NotSent
            } else {
                request_state(saved.state)
            };
        }
        let data = IssueCreateData {
            receipt: OperationReceipt {
                operation_id: self.binding.operation_id,
                request_state: error.details.write_state,
                confirmed_resources: saved
                    .map_or_else(Vec::new, |saved| saved.confirmed_resources.clone()),
                job: saved.and_then(|saved| saved.job.clone()),
            },
            target: self.binding.target,
            step: ROUTE.into(),
            issue_key: None,
            secondary: SecondaryResults {
                transition: None,
                watchers: None,
            },
            persistence: if prior.is_some() {
                PersistenceState::Recorded
            } else {
                PersistenceState::Unrecorded
            },
            journal_revision: prior.map(|record| record.revision),
            http_status: None,
            provider_request_id: None,
            retry_after: None,
            reconciliation_required: matches!(error.details.write_state, RequestState::SentUnknown),
            replay_authorized: false,
            credential_cache_update: self.cache_update,
        };
        let raw = json!(data);
        let selected = self.projection.data(&raw).unwrap_or(raw);
        CommandResult {
            envelope: Envelope::failure_with_data(COMMAND, self.request_id, error, selected),
            output_policy: OutputPolicy::PreserveEvidence,
        }
    }

    /// Completion never returns an error that could discard an already observed effect.
    pub fn finish(self, attempt: &MutationAttempt) -> CommandResult {
        self.finish_with(
            AttemptView {
                binding: attempt.binding(),
                command: attempt.command(),
                route: attempt.route(),
                step: attempt.step(),
                response: attempt.response.as_ref(),
                pending_failed: attempt.pending_evidence.is_err(),
                prior: attempt.pending_evidence.as_ref().ok(),
            },
            |outcome| attempt.record_outcome(outcome),
        )
    }

    fn finish_with(
        self,
        view: AttemptView<'_>,
        record: impl FnOnce(&StepOutcome) -> Result<JournalRecord, StoreError>,
    ) -> CommandResult {
        let AttemptView {
            binding,
            command,
            route,
            step,
            response,
            pending_failed,
            prior,
        } = view;
        let recognized = command == COMMAND && route == ROUTE && step == ROUTE;
        let mut observation = match response {
            Ok(response) if recognized => created_issue::interpret(&binding.target, response),
            _ => created_issue::CreateObservation {
                state: SubmissionState::Unknown,
                resources: Vec::new(),
                key: None,
                secondary: SecondaryResults {
                    transition: None,
                    watchers: None,
                },
                error: Some(failure(
                    response
                        .err()
                        .map_or(ErrorCode::PolicyDenied, |error| error.code),
                    RequestState::SentUnknown,
                    "Issue creation lacks qualified response evidence.",
                )),
            },
        };
        if &self.binding != binding || !recognized {
            observation.error = Some(failure(
                ErrorCode::PolicyDenied,
                request_state(observation.state),
                "Completion binding does not match the original issue-create attempt.",
            ));
        }
        let mut retained_job = None;
        if let Some(prior) =
            prior.filter(|record| &record.binding == binding && record.validate().is_ok())
            && let Some(saved) = prior.steps.get(step)
        {
            for resource in &saved.confirmed_resources {
                if !observation.resources.contains(resource) {
                    observation.resources.push(resource.clone());
                }
            }
            retained_job = saved.job.clone();
            if observation.state == SubmissionState::Failed && !observation.resources.is_empty() {
                observation.state = SubmissionState::Unknown;
                observation.error = Some(failure(
                    ErrorCode::WriteOutcomeUnknown,
                    RequestState::SentUnknown,
                    "Creation refusal conflicts with already recorded effect evidence.",
                ));
            }
        }
        let outcome = StepOutcome {
            state: observation.state,
            confirmed_resources: observation.resources.clone(),
            job: None,
        };
        let persisted = record(&outcome).and_then(|record| {
            record.validate()?;
            if &record.binding != binding
                || record.steps.get(step).is_none_or(|saved| {
                    saved.state != outcome.state
                        || outcome
                            .confirmed_resources
                            .iter()
                            .any(|resource| !saved.confirmed_resources.contains(resource))
                })
            {
                return Err(StoreError::Conflict);
            }
            Ok(record)
        });
        let mut warnings = Vec::new();
        if pending_failed {
            warnings.push("The initial uncertainty update was not acknowledged; inspect the final persistence disposition.".into());
        }
        let (persistence, revision) = match persisted {
            Ok(record) => {
                if let Some(saved) = record.steps.get(step) {
                    for resource in &saved.confirmed_resources {
                        if !observation.resources.contains(resource) {
                            observation.resources.push(resource.clone());
                        }
                    }
                    retained_job = saved.job.clone().or(retained_job);
                }
                (PersistenceState::Recorded, Some(record.revision))
            }
            Err(error) => {
                warnings.push("Local receipt recording did not complete; retain the returned evidence and inspect the journal before another create.".into());
                if observation.error.is_none() {
                    observation.error = Some(failure(
                        ErrorCode::Internal,
                        request_state(observation.state),
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
        if observation.error.is_none()
            && (observation.resources.len() > 1 || retained_job.is_some())
        {
            observation.error = Some(failure(
                ErrorCode::PartialApply,
                request_state(observation.state),
                "Additional recorded resource or job evidence requires inspection.",
            ));
        }
        let data = IssueCreateData {
            receipt: OperationReceipt {
                operation_id: binding.operation_id.clone(),
                request_state: request_state(observation.state),
                confirmed_resources: observation.resources,
                job: retained_job,
            },
            target: binding.target.clone(),
            step: step.into(),
            issue_key: observation.key,
            secondary: observation.secondary,
            persistence,
            journal_revision: revision,
            http_status: response.ok().map(|response| response.status),
            provider_request_id: response
                .ok()
                .and_then(|response| response.request_id.clone()),
            retry_after: response.ok().and_then(|response| response.retry_after),
            reconciliation_required: observation.error.is_some(),
            replay_authorized: false,
            credential_cache_update: self.cache_update,
        };
        let unprojected = json!(data);
        let data = match self.projection.data(&unprojected) {
            Ok(data) => data,
            Err(_) => {
                observation.error = Some(failure(
                    ErrorCode::Internal,
                    data.receipt.request_state,
                    "Output projection failed after the observed issue result; full evidence is retained.",
                ));
                unprojected
            }
        };
        let mut envelope = match observation.error {
            Some(error) => Envelope::failure_with_data(COMMAND, self.request_id, error, data),
            None => Envelope::success(COMMAND, self.request_id, data),
        };
        envelope.warnings = warnings;
        CommandResult {
            envelope,
            output_policy: OutputPolicy::PreserveEvidence,
        }
    }
}
