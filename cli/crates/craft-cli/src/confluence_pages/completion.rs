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
    pub credential_cache_update: Option<CredentialCacheUpdate>,
}
pub fn schema() -> Result<Value, CraftError> {
    response_schema::<PublicationData>()
}
pub(super) struct Completion {
    binding: OperationBinding,
    command: String,
    route: String,
    space_id: String,
    request_id: String,
    cache: Option<CredentialCacheUpdate>,
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
        let route = if command == PAGE_CREATE {
            pages::CREATE_ROUTE
        } else {
            pages::UPDATE_ROUTE
        }
        .to_string();
        let resource = ResourceRef {
            resource_type: "page".into(),
            id: i64::MAX.to_string(),
            ..binding.target.clone()
        };
        let journal = JournalRecord {
            binding: binding.clone(),
            revision: u64::MAX,
            steps: BTreeMap::from([(
                route.clone(),
                JournalStep {
                    request_digest: Some(format!("sha256:{}", "f".repeat(64))),
                    state: SubmissionState::Unknown,
                    confirmed_resources: vec![resource.clone()],
                    job: None,
                },
            )]),
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
            space_id: space_id.into(),
            request_id: request_id.into(),
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
        let state = prior
            .and_then(|p| p.steps.get(&self.route))
            .map_or(initial, |s| {
                if initial == RequestState::SentUnknown && s.state == SubmissionState::Prepared {
                    initial
                } else {
                    request_state(s.state)
                }
            });
        let mut data = self.data(state);
        if let Some(prior) = prior {
            self.retain_record(&mut data, prior);
            data.persistence = Persistence::Recorded;
            data.reconciliation_required =
                state != RequestState::NotSent || prior.steps.keys().any(|s| s != &self.route);
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
