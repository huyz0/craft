//! Journal-bound read-only investigation. Observed current state never supplies replay authority.
use crate::dispatch::CommandResult;
use crate::output::OutputPolicy;
use craft_core::contracts::{
    CanonicalRequest, CredentialCacheUpdate, Envelope, GrantRef, JobState, RequestState,
    ResourceRef, Selection, validate_identity,
};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{
    NativeCall, NativeResponse, OperationContext, PreparedOperation, RetryAfter,
};
use craft_core::projection::{CompiledProjection, ProjectionContract, ProjectionMode};
use craft_core::registry::{Registry, response_schema};
use craft_core::storage::{
    JournalRecord, OperationBinding, OperationJournal, StoreError, SubmissionState,
};
use craft_provider_jira::created_issue;
use craft_provider_jira::issue_context::IssueState;
use craft_provider_jira::issue_reads::numeric_id;
use craft_provider_jira::reconciliation::{self, IssueInspection, JiraReconciliation, Visibility};
use craft_runtime::auth::{ServiceOAuthClient, VerifiedServiceAccess};
use craft_runtime::journal::NativeJournal;
use craft_runtime::secrets::LockedCredentials;
use craft_runtime::transport::TlsPolicy;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
pub mod command;
#[cfg(test)]
mod tests;

pub const COMMAND: &str = "operation.reconcile";
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecoveryInput {
    pub operation_id: String,
    pub original: CanonicalRequest,
    #[serde(default)]
    #[schemars(length(max=20),extend("uniqueItems"=true))]
    pub candidate_issue_ids: Vec<String>,
}
/// Separate trusted authority; existing write enablement is not investigation permission.
pub struct RecoveryPolicy {
    pub enabled: bool,
    pub project: ResourceRef,
    pub grant: GrantRef,
    pub expected_actor: String,
    pub allowed_commands: Vec<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Assessment {
    LocalEvidenceMissing,
    NotLocallyStarted,
    RecordedConfirmation,
    RecordedFailure,
    Unresolved,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecoveryData {
    pub operation_id: String,
    pub initial_record: Option<JournalRecord>,
    pub latest_record: Option<JournalRecord>,
    /// Initial stored request-submission evidence, not current state or workflow/job completion.
    pub assessment: Assessment,
    /// Describes recorded uncertainty only; false does not resolve unrecorded secondary effects.
    pub reconciliation_required: Option<bool>,
    pub journal_matches_initial: Option<bool>,
    pub observations: Vec<IssueInspection>,
    pub observations_complete: bool,
    pub inspection_error: Option<CraftError>,
    pub http_status: Option<u16>,
    pub retry_after: Option<RetryAfter>,
    pub credential_cache_update: Option<CredentialCacheUpdate>,
    #[schemars(extend("const"=false))]
    pub atomic_snapshot: bool,
    #[schemars(extend("const"=false))]
    pub attribution_upgraded: bool,
    #[schemars(extend("const"=false))]
    pub journal_updated: bool,
    #[schemars(extend("const"=false))]
    pub replay_authorized: bool,
}
pub fn schema() -> Result<Value, CraftError> {
    response_schema::<RecoveryData>()
}
pub fn projection() -> ProjectionContract {
    let required = [
        "operation_id",
        "initial_record",
        "latest_record",
        "assessment",
        "reconciliation_required",
        "journal_matches_initial",
        "observations",
        "observations_complete",
        "inspection_error",
        "http_status",
        "retry_after",
        "credential_cache_update",
        "atomic_snapshot",
        "attribution_upgraded",
        "journal_updated",
        "replay_authorized",
    ]
    .map(String::from)
    .to_vec();
    let defaults = required.clone();
    ProjectionContract {
        item_pointer: "".into(),
        always_returned: required,
        default_selected: defaults,
        mode: ProjectionMode::Local,
    }
}
pub struct PreparedRecovery {
    binding: OperationBinding,
    project: ResourceRef,
    step: String,
    create: bool,
    candidates: Vec<String>,
    projection: CompiledProjection,
    maximum: usize,
    request_id: String,
}
fn failure(code: ErrorCode, message: &str) -> CraftError {
    let mut error = CraftError::new(code, message);
    error.next_action="Retain the complete operation evidence and inspect the same journal before recovery; this result authorizes no replay.".into();
    error
}
fn store_error() -> CraftError {
    failure(
        ErrorCode::Internal,
        "Operation evidence could not be read; its absence was not established.",
    )
}
fn journal_read_error(error: StoreError) -> CraftError {
    match error {
        StoreError::Cancelled => failure(
            ErrorCode::Cancelled,
            "Recovery evidence read was cancelled.",
        ),
        StoreError::DeadlineExceeded => failure(
            ErrorCode::DeadlineExceeded,
            "Recovery evidence read exceeded its deadline.",
        ),
        _ => store_error(),
    }
}
fn supported(command: &str) -> bool {
    matches!(
        command,
        "jira.issue.create"
            | "jira.issue.update"
            | "jira.transition.apply"
            | "jira.issue.assign"
            | "jira.issue.unassign"
            | "jira.issue.delete"
    )
}
impl PreparedRecovery {
    /// Pure validation binds the original descriptor and logical write without requiring write scopes.
    pub fn prepare(
        input: RecoveryInput,
        fields: &[String],
        maximum: usize,
        request_id: &str,
        policy: RecoveryPolicy,
    ) -> Result<Self, CraftError> {
        craft_core::input::parse_object(
            &serde_json::to_vec(&input)
                .map_err(|_| CraftError::field("/input", "cannot_encode_recovery"))?,
        )?;
        validate_identity("/request_id", request_id)?;
        if request_id.len() > 80 || !(512..=16_777_216).contains(&maximum) {
            return Err(CraftError::field(
                "/output",
                "invalid_recovery_output_budget",
            ));
        }
        created_issue::validate_target(&policy.project)?;
        policy.grant.validate()?;
        validate_identity("/actor", &policy.expected_actor)?;
        let mut allowed = BTreeSet::new();
        if !policy.enabled
            || matches!(policy.expected_actor.as_str(), "unknown" | "anonymous")
            || policy.allowed_commands.is_empty()
            || policy.allowed_commands.len() > 6
            || policy
                .allowed_commands
                .iter()
                .any(|c| !supported(c) || !allowed.insert(c))
            || !allowed.contains(&input.original.command)
        {
            return Err(failure(
                ErrorCode::PolicyDenied,
                "Recovery is outside the trusted project/operation policy.",
            ));
        }
        let create = input.original.command == created_issue::COMMAND;
        let original = if create {
            crate::issue_create::host::prepare_original(input.original)?
        } else {
            crate::issue_mutation::host::prepare_original(input.original)?
        };
        let project = if create {
            original.request().target.id.as_str()
        } else {
            original.request().input["expected"]["project_id"]
                .as_str()
                .ok_or_else(|| CraftError::field("/expected", "missing_project"))?
        };
        if policy.project.tenant_ref() != original.request().target.tenant_ref()
            || policy.project.id != project
        {
            return Err(failure(
                ErrorCode::PolicyDenied,
                "Original write is outside the trusted recovery project.",
            ));
        }
        let mut ids = BTreeSet::new();
        if input.candidate_issue_ids.len() > 20
            || (!create && !input.candidate_issue_ids.is_empty())
            || input
                .candidate_issue_ids
                .iter()
                .any(|id| !numeric_id(id) || !ids.insert(id))
        {
            return Err(CraftError::field(
                "/candidate_issue_ids",
                "expected_unique_bounded_create_candidates_only",
            ));
        }
        let binding = original.mutation_binding(
            &input.operation_id,
            &policy.grant,
            &policy.expected_actor,
        )?;
        let step = if create {
            created_issue::ROUTE
        } else {
            crate::issue_mutation::route(&original.request().command)?
        }
        .into();
        Ok(Self {
            binding,
            project: policy.project,
            step,
            create,
            candidates: input.candidate_issue_ids,
            projection: projection().compile(&schema()?, fields)?,
            maximum,
            request_id: request_id.into(),
        })
    }
    pub fn grant(&self) -> &GrantRef {
        &self.binding.grant
    }
    fn data(&self, record: Option<JournalRecord>) -> RecoveryData {
        let assessment = match record
            .as_ref()
            .and_then(|r| r.steps.get(&self.step))
            .map(|s| s.state)
        {
            None | Some(SubmissionState::Prepared) => {
                if record.is_none() {
                    Assessment::LocalEvidenceMissing
                } else {
                    Assessment::NotLocallyStarted
                }
            }
            Some(SubmissionState::Confirmed) => Assessment::RecordedConfirmation,
            Some(SubmissionState::Failed) => Assessment::RecordedFailure,
            _ => Assessment::Unresolved,
        };
        let needs = record.as_ref().map(|r| {
            r.steps.values().any(|s| {
                s.state.requires_reconciliation()
                    || s.job
                        .as_ref()
                        .is_some_and(|job| job.state == JobState::Unknown)
            })
        });
        RecoveryData {
            operation_id: self.binding.operation_id.clone(),
            initial_record: record,
            latest_record: None,
            assessment,
            reconciliation_required: needs,
            journal_matches_initial: None,
            observations: vec![],
            observations_complete: false,
            inspection_error: None,
            http_status: None,
            retry_after: None,
            credential_cache_update: None,
            atomic_snapshot: false,
            attribution_upgraded: false,
            journal_updated: false,
            replay_authorized: false,
        }
    }
    fn plan(&self, data: &RecoveryData) -> Result<Vec<String>, CraftError> {
        let Some(record) = &data.initial_record else {
            return Ok(vec![]);
        };
        record.validate().map_err(|_| store_error())?;
        if record.binding != self.binding {
            return Err(failure(
                ErrorCode::OperationIdReused,
                "Original request differs from the stored operation binding.",
            ));
        }
        let Some(step) = record.steps.get(&self.step) else {
            return Ok(vec![]);
        };
        if step.state == SubmissionState::Prepared {
            return Ok(vec![]);
        }
        let mut ids: BTreeSet<String> = self.candidates.iter().cloned().collect();
        if !self.create {
            ids.insert(self.binding.target.id.clone());
        }
        for resource in &step.confirmed_resources {
            if resource.tenant_ref() != self.project.tenant_ref()
                || resource.resource_type != "issue"
                || !numeric_id(&resource.id)
                || (!self.create && resource != &self.binding.target)
            {
                return Err(failure(
                    ErrorCode::PolicyDenied,
                    "Stored resource evidence is outside this issue recovery strategy.",
                ));
            }
            ids.insert(resource.id.clone());
        }
        if ids.len() > 20 {
            return Err(CraftError::field(
                "/issue_ids",
                "recovery_set_exceeds_twenty_without_truncation",
            ));
        }
        Ok(ids.into_iter().collect())
    }
    fn envelope(
        &self,
        data: &RecoveryData,
        error: Option<CraftError>,
    ) -> Result<Envelope, CraftError> {
        let value = self.projection.data(&json!(data))?;
        Ok(if let Some(error) = error {
            Envelope::failure_with_data(COMMAND, self.request_id.clone(), error, value)
        } else {
            Envelope::success(COMMAND, self.request_id.clone(), value)
        })
    }
    fn finish(&self, data: RecoveryData, error: Option<CraftError>) -> CommandResult {
        let envelope = self.envelope(&data, error).unwrap_or_else(|error| {
            Envelope::failure_with_data(COMMAND, self.request_id.clone(), error, json!(data))
        });
        CommandResult {
            envelope,
            output_policy: OutputPolicy::PreserveEvidence,
        }
    }
    fn preflight(&self, data: &RecoveryData, ids: &[String]) -> Result<(), CraftError> {
        let mut reserved = self.data(data.initial_record.clone());
        reserved.latest_record = data.initial_record.clone();
        reserved.credential_cache_update = Some(CredentialCacheUpdate {
            grant: self.binding.grant.clone(),
            write_state: RequestState::SentUnknown,
            cleanup_pending: Some(false),
        });
        reserved.retry_after = Some(RetryAfter::AtUnixMillis(u64::MAX));
        for id in ids {
            reserved.observations.push(IssueInspection {
                issue_id: id.clone(),
                visibility: Visibility::Visible,
                state: Some(IssueState {
                    id: id.clone(),
                    key: format!("{}-1", "A".repeat(509)),
                    project_id: self.project.id.clone(),
                    issue_type_id: "9".repeat(512),
                    status_id: "9".repeat(512),
                    updated: "\"".repeat(128),
                }),
                assignee_account_id: craft_core::presence::InputField::Value("\"".repeat(128)),
                observed_at: u64::MAX,
            });
        }
        let mut error = failure(ErrorCode::WriteOutcomeUnknown, &"x".repeat(500));
        error.next_action = "x".repeat(512);
        reserved.inspection_error = Some(error.clone());
        let envelope = self.envelope(&reserved, Some(error))?;
        if serde_json::to_vec(&envelope)
            .map_err(|_| store_error())?
            .len()
            + 1
            > self.maximum
        {
            return Err(failure(
                ErrorCode::OutputBudgetTooSmall,
                "Recovery needs room for complete journal, observation and credential evidence before auth.",
            ));
        }
        Ok(())
    }
    fn check_access(
        &self,
        access: &VerifiedServiceAccess,
        registry: &Registry,
        read: &PreparedOperation,
    ) -> Result<(), CraftError> {
        if access.grant() != &self.binding.grant
            || access.tenant() != &self.project.tenant_ref()
            || access.identity().actor() != self.binding.actor
        {
            return Err(failure(
                ErrorCode::PolicyDenied,
                "Recovery access differs from the exact journal grant, actor or tenant.",
            ));
        }
        access.check_operation(registry, read)
    }
    fn compare(&self, journal: &NativeJournal, data: &mut RecoveryData) -> Result<(), CraftError> {
        let latest = journal
            .get(&self.binding.operation_id)
            .map_err(journal_read_error)?;
        if latest == data.initial_record {
            data.journal_matches_initial = Some(true);
            Ok(())
        } else {
            data.journal_matches_initial = Some(false);
            data.latest_record = latest;
            data.observations.clear();
            data.observations_complete = false;
            Err(failure(
                ErrorCode::VersionConflict,
                "Operation evidence changed during investigation; both observations of the journal are retained.",
            ))
        }
    }
    /// Reads and budgets journal evidence before any credential backend is constructed.
    pub(crate) fn begin(
        &self,
        journal: &NativeJournal,
        control: InvocationControl,
        check: impl Fn() -> Result<(), CraftError>,
    ) -> Result<RecoveryReadPlan, Box<CommandResult>> {
        let journal = journal.for_invocation(&control);
        let mut data = self.data(None);
        let ready = (|| {
            control.check()?;
            check()?;
            data = self.data(
                journal
                    .get(&self.binding.operation_id)
                    .map_err(journal_read_error)?,
            );
            let ids = self.plan(&data)?;
            self.preflight(&data, &ids)?;
            Ok(ids)
        })();
        let ids = match ready {
            Ok(ids) => ids,
            Err(error) => return Err(Box::new(self.finish(data, Some(error)))),
        };
        if ids.is_empty() {
            let result = check()
                .and_then(|()| control.check())
                .and_then(|()| self.compare(&journal, &mut data));
            return Err(Box::new(self.finish(data, result.err())));
        }
        Ok(RecoveryReadPlan {
            journal,
            control,
            data,
            ids,
        })
    }
    pub(crate) fn fail_plan(&self, plan: RecoveryReadPlan, error: CraftError) -> CommandResult {
        self.finish(plan.data, Some(error))
    }
    /// Uses only read-scoped service access. It never begins, changes or claims an operation record.
    pub fn execute_cached_with_policy_check(
        &self,
        client: &ServiceOAuthClient,
        locked: &LockedCredentials<'_>,
        journal: &NativeJournal,
        tls: TlsPolicy,
        control: InvocationControl,
        check: impl Fn() -> Result<(), CraftError>,
    ) -> CommandResult {
        match self.begin(journal, control, &check) {
            Ok(plan) => self.execute_plan(plan, client, locked, tls, check),
            Err(result) => *result,
        }
    }
    pub(crate) fn execute_plan(
        &self,
        plan: RecoveryReadPlan,
        client: &ServiceOAuthClient,
        locked: &LockedCredentials<'_>,
        tls: TlsPolicy,
        check: impl Fn() -> Result<(), CraftError>,
    ) -> CommandResult {
        let RecoveryReadPlan {
            journal,
            control,
            mut data,
            ids,
        } = plan;
        let ready = (|| {
            check()?;
            control.check()?;
            if locked.grant() != self.binding.grant {
                return Err(failure(
                    ErrorCode::PolicyDenied,
                    "Locked grant differs from recovery authority.",
                ));
            }
            Ok(())
        })();
        if let Err(error) = ready {
            return self.finish(data, Some(error));
        }
        let client = match client.for_invocation(control.clone()) {
            Ok(client) => client,
            Err(error) => return self.finish(data, Some(error)),
        };
        let access = match client.open_service_access(locked, &self.project.tenant_ref()) {
            Ok(access) => access,
            Err(failed) => {
                data.retry_after = failed.retry_after;
                data.credential_cache_update = failed
                    .credential_update
                    .map(|receipt| CredentialCacheUpdate {
                        grant: self.binding.grant.clone(),
                        write_state: RequestState::Confirmed,
                        cleanup_pending: Some(receipt.cleanup_pending),
                    })
                    .or_else(|| {
                        failed
                            .credential_write_state
                            .map(|write_state| CredentialCacheUpdate {
                                grant: self.binding.grant.clone(),
                                write_state,
                                cleanup_pending: None,
                            })
                    });
                return self.finish(data, Some(failed.error));
            }
        };
        data.credential_cache_update = access.cache_update().map(|receipt| CredentialCacheUpdate {
            grant: self.binding.grant.clone(),
            write_state: RequestState::Confirmed,
            cleanup_pending: Some(receipt.cleanup_pending),
        });
        let result = (|| {
            let mut registry = Registry::default();
            registry.register_provider(&JiraReconciliation)?;
            let read = registry.prepare(CanonicalRequest {
                command: reconciliation::COMMAND.into(),
                target: self.project.clone(),
                input: json!({"issue_ids":ids}),
                selection: Selection::default(),
                preconditions: None,
            })?;
            self.check_access(&access, &registry, &read)?;
            check()?;
            control.check()?;
            let mut native = access.read_context(&registry, &read, tls, control.clone())?;
            let mut context = PolicyRead {
                native: &mut native,
                check: &check,
            };
            let batch = reconciliation::inspect(&read, &mut context, access.clock().as_ref());
            data.observations = batch.observations;
            data.observations_complete = batch.error.is_none();
            data.inspection_error = batch.error;
            data.http_status = batch.http_status;
            data.retry_after = batch.retry_after;
            self.check_access(&access, &registry, &read)?;
            let _ = self.envelope(&data, data.inspection_error.clone())?;
            self.check_access(&access, &registry, &read)?;
            check()?;
            control.check()?;
            self.compare(&journal, &mut data)?;
            self.envelope(&data, data.inspection_error.clone())
        })();
        match result {
            Ok(envelope) => CommandResult {
                envelope,
                output_policy: OutputPolicy::PreserveEvidence,
            },
            Err(error) => {
                data.observations.clear();
                data.observations_complete = false;
                self.finish(data, Some(error))
            }
        }
    }
}
pub(crate) struct RecoveryReadPlan {
    journal: NativeJournal,
    control: InvocationControl,
    data: RecoveryData,
    ids: Vec<String>,
}
struct PolicyRead<'a> {
    native: &'a mut dyn OperationContext,
    check: &'a dyn Fn() -> Result<(), CraftError>,
}
impl OperationContext for PolicyRead<'_> {
    fn checkpoint(&self) -> Result<(), CraftError> {
        (self.check)()?;
        self.native.checkpoint()
    }
    fn verify_operation(&self, operation: &PreparedOperation) -> Result<(), CraftError> {
        self.checkpoint()?;
        self.native.verify_operation(operation)
    }
    fn invoke(&mut self, call: &NativeCall) -> Result<NativeResponse, CraftError> {
        self.checkpoint()?;
        self.native.invoke(call)
    }
}
