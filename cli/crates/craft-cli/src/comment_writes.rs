//! One claimed comment exchange retains native effects even when local acknowledgment fails.
pub mod command;
#[cfg(test)]
mod tests;
use crate::issue_create::PersistenceState;
use craft_core::contracts::{
    CanonicalRequest, CredentialCacheUpdate, GrantRef, OperationReceipt, RequestState, ResourceRef,
    Selection,
};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{
    NativeCall, NativeResponse, OperationContext, PreparedOperation, RetryAfter,
};
use craft_core::registry::Registry;
use craft_core::storage::{
    JournalRecord, OperationBinding, OperationJournal, StoreError, SubmissionState,
};
use craft_provider_jira::comments::{CommentRecord, mutation as native};
use craft_provider_jira::create_metadata::MetadataPrincipal;
use craft_runtime::auth::ServiceOAuthClient;
use craft_runtime::issue_mutation_policy::{
    IssueMutationOperation as Op, IssueMutationPolicyRecord,
};
use craft_runtime::journal::{MutationAuthority, NativeJournal, StepOutcome};
use craft_runtime::profiles::ProfileStore;
use craft_runtime::secrets::LockedCredentials;
use craft_runtime::transport::TlsPolicy;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const MINIMUM_OUTPUT: usize = 8_388_608;
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentWriteData {
    pub receipt: OperationReceipt,
    pub target: ResourceRef,
    pub issue_id: String,
    pub before: Option<CommentRecord>,
    pub comment: Option<CommentRecord>,
    pub acknowledged: bool,
    pub desired_fields_verified: bool,
    #[schemars(extend("const" = false))]
    pub atomic_revision_check: bool,
    pub persistence: PersistenceState,
    pub prior_record: Option<JournalRecord>,
    pub latest_record: Option<JournalRecord>,
    pub pending_record: Option<JournalRecord>,
    pub pending_record_state: PersistenceState,
    pub http_status: Option<u16>,
    pub provider_request_id: Option<String>,
    pub retry_after: Option<RetryAfter>,
    pub credential_cache_update: Option<CredentialCacheUpdate>,
    pub reconciliation_required: bool,
    #[schemars(extend("const" = false))]
    pub replay_authorized: bool,
}
pub fn registry() -> Result<Registry, CraftError> {
    let mut registry = Registry::default();
    native::register(&mut registry)?;
    Ok(registry)
}
fn denied(message: &str) -> CraftError {
    CraftError::new(ErrorCode::PolicyDenied, message)
}
fn storage(_: StoreError) -> CraftError {
    CraftError::new(
        ErrorCode::Internal,
        "Cannot inspect the exact comment operation journal.",
    )
}
fn journal_observation(
    result: Result<Option<JournalRecord>, StoreError>,
    data: &mut CommentWriteData,
) -> Result<Option<JournalRecord>, CraftError> {
    result.map_err(|error| {
        data.persistence = PersistenceState::Uncertain;
        data.reconciliation_required = true;
        storage(error)
    })
}
fn metadata(mut record: CommentRecord) -> CommentRecord {
    record.body = None;
    record.body_state = craft_provider_jira::comments::BodyState::NotRequested;
    record
}
fn request_state(state: SubmissionState) -> RequestState {
    match state {
        SubmissionState::Prepared => RequestState::NotSent,
        SubmissionState::Confirmed => RequestState::Confirmed,
        SubmissionState::Failed => RequestState::Failed,
        _ => RequestState::SentUnknown,
    }
}
fn empty(binding: &OperationBinding, issue_id: String) -> CommentWriteData {
    CommentWriteData {
        receipt: OperationReceipt {
            operation_id: binding.operation_id.clone(),
            request_state: RequestState::NotSent,
            confirmed_resources: vec![],
            job: None,
        },
        target: binding.target.clone(),
        issue_id,
        before: None,
        comment: None,
        acknowledged: false,
        desired_fields_verified: false,
        atomic_revision_check: false,
        persistence: PersistenceState::Unrecorded,
        prior_record: None,
        latest_record: None,
        pending_record: None,
        pending_record_state: PersistenceState::Unrecorded,
        http_status: None,
        provider_request_id: None,
        retry_after: None,
        credential_cache_update: None,
        reconciliation_required: false,
        replay_authorized: false,
    }
}
fn permission(request: &CanonicalRequest) -> Result<Op, CraftError> {
    native::validate(request)?;
    let any = request
        .input
        .get("ownership")
        .and_then(serde_json::Value::as_str)
        == Some("any");
    Ok(match request.command.as_str() {
        native::CREATE => Op::CommentCreate,
        native::UPDATE => {
            if any {
                Op::CommentUpdateAny
            } else {
                Op::CommentUpdateOwn
            }
        }
        native::DELETE => {
            if any {
                Op::CommentDeleteAny
            } else {
                Op::CommentDeleteOwn
            }
        }
        _ => return Err(CraftError::field("/command", "expected_comment_write")),
    })
}
fn step(command: &str) -> &'static str {
    match command {
        native::CREATE => native::CREATE_ROUTE,
        native::UPDATE => native::UPDATE_ROUTE,
        _ => native::DELETE_ROUTE,
    }
}
pub struct PreparedCommentWrite {
    registry: Registry,
    write: PreparedOperation,
    read: PreparedOperation,
    binding: OperationBinding,
    policy: IssueMutationPolicyRecord,
    permission: Op,
    issue_id: String,
}
pub struct CommentWriteEnvironment<'a, 'credentials> {
    pub store: &'a ProfileStore,
    pub client: &'a ServiceOAuthClient,
    pub locked: &'a LockedCredentials<'credentials>,
    pub journal: &'a NativeJournal,
    pub tls: TlsPolicy,
    pub control: InvocationControl,
}
struct Guard<'a> {
    native: &'a mut dyn OperationContext,
    check: &'a dyn Fn() -> Result<(), CraftError>,
}
impl OperationContext for Guard<'_> {
    fn checkpoint(&self) -> Result<(), CraftError> {
        (self.check)()?;
        self.native.checkpoint()
    }
    fn verify_operation(&self, op: &PreparedOperation) -> Result<(), CraftError> {
        self.checkpoint()?;
        self.native.verify_operation(op)
    }
    fn invoke(&mut self, call: &NativeCall) -> Result<NativeResponse, CraftError> {
        self.checkpoint()?;
        let response = self.native.invoke(call)?;
        self.checkpoint()?;
        Ok(response)
    }
}
impl PreparedCommentWrite {
    pub fn prepare(
        request: CanonicalRequest,
        operation_id: &str,
        apply: bool,
        profile: &str,
        store: &ProfileStore,
        control: &InvocationControl,
    ) -> Result<Self, CraftError> {
        if !apply {
            return Err(CraftError::new(
                ErrorCode::ApplyRequired,
                "Comment writes require explicit apply.",
            ));
        }
        if !(MINIMUM_OUTPUT..=16_777_216).contains(&(request.selection.max_bytes as usize)) {
            return Err(CraftError::new(
                ErrorCode::OutputBudgetTooSmall,
                "Reserve --max-bytes 8388608 for complete comment journal and credential receipts; normal output omits ADF bodies.",
            ));
        }
        let permission = permission(&request)?;
        let registry = registry()?;
        let read = registry.prepare(CanonicalRequest {
            command: native::context_command(&request.command)?.into(),
            target: request.target.clone(),
            input: request.input.clone(),
            selection: Selection::default(),
            preconditions: None,
        })?;
        let write = registry.prepare(request)?;
        let policy = store
            .issue_mutation_policy(profile, &write.request().target.id, control)?
            .ok_or_else(|| {
                denied("The exact project requires an explicit comment mutation allowance.")
            })?;
        store.verify_issue_mutation_policy(&policy, permission, control)?;
        if policy.target != write.request().target {
            return Err(denied(
                "Comment target differs from the captured project allowance.",
            ));
        }
        let binding =
            write.mutation_binding(operation_id, &policy.grant, &policy.expected_actor)?;
        let issue_id = write.request().input["issue_id"]
            .as_str()
            .ok_or_else(|| denied("Missing validated issue identity."))?
            .to_owned();
        Ok(Self {
            registry,
            write,
            read,
            binding,
            policy,
            permission,
            issue_id,
        })
    }
    pub fn grant(&self) -> &GrantRef {
        &self.binding.grant
    }
    pub fn binding(&self) -> &OperationBinding {
        &self.binding
    }
    fn prior(
        &self,
        journal: &NativeJournal,
        data: &mut CommentWriteData,
    ) -> Result<(), CraftError> {
        let record = journal_observation(journal.get(&self.binding.operation_id), data)?;
        if data.prior_record.is_none() {
            data.prior_record = record.clone();
        } else {
            data.latest_record = record.clone();
        }
        if record.is_some() {
            data.persistence = PersistenceState::Recorded;
        }
        if let Some(record) = record
            && (record.binding != self.binding
                || record.steps.iter().any(|(name, s)| {
                    name != step(&self.write.request().command)
                        || s.state != SubmissionState::Prepared
                }))
        {
            return Err(CraftError::new(
                ErrorCode::OperationIdReused,
                "Comment operation changed or already started; retain its journal and do not replay it.",
            ));
        }
        Ok(())
    }
    pub fn execute(
        self,
        environment: CommentWriteEnvironment<'_, '_>,
        extra: impl Fn() -> Result<(), CraftError>,
    ) -> (CommentWriteData, Option<CraftError>) {
        let CommentWriteEnvironment {
            store,
            client,
            locked,
            journal,
            tls,
            control,
        } = environment;
        let journal = journal.for_invocation(&control);
        let mut data = empty(&self.binding, self.issue_id.clone());
        let mut claim_attempted = false;
        let result = (|| {
            let policy_check = || {
                control.check()?;
                extra()?;
                store.verify_issue_mutation_policy(&self.policy, self.permission, &control)?;
                extra()?;
                control.check()
            };
            self.prior(&journal, &mut data)?;
            policy_check()?;
            if locked.grant() != self.binding.grant {
                return Err(denied(
                    "Locked comment grant differs from the captured allowance.",
                ));
            }
            let client = client.for_invocation(control.clone())?;
            let access = match client.open_service_access(locked, &self.binding.target.tenant_ref())
            {
                Ok(access) => access,
                Err(failure) => {
                    data.credential_cache_update = failure
                        .credential_update
                        .map(|r| CredentialCacheUpdate {
                            grant: self.binding.grant.clone(),
                            write_state: RequestState::Confirmed,
                            cleanup_pending: Some(r.cleanup_pending),
                        })
                        .or_else(|| {
                            failure
                                .credential_write_state
                                .map(|state| CredentialCacheUpdate {
                                    grant: self.binding.grant.clone(),
                                    write_state: state,
                                    cleanup_pending: None,
                                })
                        });
                    let mut error = failure.error;
                    error.details.write_state = RequestState::NotSent;
                    return Err(error);
                }
            };
            data.credential_cache_update = access.cache_update().map(|r| CredentialCacheUpdate {
                grant: access.grant().clone(),
                write_state: RequestState::Confirmed,
                cleanup_pending: Some(r.cleanup_pending),
            });
            if access.tenant() != &self.binding.target.tenant_ref()
                || access.grant() != &self.binding.grant
                || access.identity().actor() != self.binding.actor
            {
                return Err(denied(
                    "Authenticated comment site, grant or actor differs from captured policy.",
                ));
            }
            let check = || {
                access.check_operation(&self.registry, &self.write)?;
                access.check_operation(&self.registry, &self.read)?;
                policy_check()
            };
            check()?;
            self.prior(&journal, &mut data)?;
            let principal = MetadataPrincipal {
                grant: access.grant().clone(),
                actor: access.identity().actor().into(),
            };
            let mut context =
                access.read_context(&self.registry, &self.read, tls.clone(), control.clone())?;
            let mut guard = Guard {
                native: &mut context,
                check: &check,
            };
            let frozen = native::prepare(
                &self.read,
                &self.write,
                &mut guard,
                principal.clone(),
                access.clock(),
            )?;
            data.before = frozen.before().cloned().map(metadata);
            frozen.verify(&self.write, &principal)?;
            check()?;
            let mutation = access.prepare_mutation(
                &self.registry,
                &self.write,
                tls,
                control.clone(),
                frozen.call(),
            )?;
            claim_attempted = true;
            let attempt = mutation.send_checked(
                &journal,
                &MutationAuthority {
                    apply: true,
                    operation_id: &self.binding.operation_id,
                    grant: access.grant(),
                    actor: access.identity().actor(),
                },
                step(&self.write.request().command),
                || {
                    check()?;
                    frozen.verify(&self.write, &principal)?;
                    check()
                },
            )?;
            finish_with(
                &self.write,
                &self.binding,
                CompletionView {
                    binding: attempt.binding(),
                    command: attempt.command(),
                    route: attempt.route(),
                    step: attempt.step(),
                    response: attempt.response.as_ref(),
                    pending: attempt.pending_evidence.as_ref(),
                },
                &mut data,
                |outcome| attempt.record_outcome(outcome),
            )
        })();
        let error=result.err().map(|mut error| {
            if data.latest_record.is_none() && claim_attempted {
                data.latest_record=journal.get(&self.binding.operation_id).ok().flatten();
                if data.latest_record.is_some() {data.persistence=PersistenceState::Recorded;}
                else if error.details.write_state==RequestState::SentUnknown {data.persistence=PersistenceState::Uncertain;}
            }
            if data.http_status.is_none() {
                if let Some(record)=data.latest_record.as_ref().or(data.prior_record.as_ref())
                    && record.binding==self.binding && record.validate().is_ok()
                    && let Some(saved)=record.steps.get(step(&self.write.request().command)) {
                    data.receipt.request_state=request_state(saved.state);data.receipt.confirmed_resources=saved.confirmed_resources.clone();data.receipt.job=saved.job.clone();
                }
                if error.details.write_state==RequestState::SentUnknown && data.receipt.request_state==RequestState::NotSent {data.receipt.request_state=RequestState::SentUnknown;}
            }
            data.reconciliation_required=data.reconciliation_required || data.receipt.request_state!=RequestState::NotSent || data.prior_record.is_some() || data.latest_record.is_some();
            error.details.write_state=data.receipt.request_state;
            error.next_action="Retain all comment receipts and inspect this operation ID; no automatic retry or replay is authorized.".into();error
        });
        (data, error)
    }
}
struct CompletionView<'a> {
    binding: &'a OperationBinding,
    command: &'a str,
    route: &'a str,
    step: &'a str,
    response: Result<&'a NativeResponse, &'a CraftError>,
    pending: Result<&'a JournalRecord, &'a StoreError>,
}
fn finish_with(
    operation: &PreparedOperation,
    binding: &OperationBinding,
    view: CompletionView<'_>,
    data: &mut CommentWriteData,
    record: impl FnOnce(&StepOutcome) -> Result<JournalRecord, StoreError>,
) -> Result<(), CraftError> {
    let command = &operation.request().command;
    let step = step(command);
    let recognized = view.binding == binding
        && view.command == command
        && view.route == step
        && view.step == step;
    let mut observation = match view.response {
        Ok(response) if recognized => native::interpret(operation, response)?,
        _ => native::CommentObservation {
            state: SubmissionState::Unknown,
            resources: vec![],
            comment: None,
            acknowledged: false,
            desired_fields_verified: false,
            error: Some(CraftError::new(
                view.response
                    .err()
                    .map_or(ErrorCode::PolicyDenied, |e| e.code),
                "Comment attempt lacks qualified response evidence.",
            )),
        },
    };
    let expected = observation.resources.clone();
    data.pending_record = view.pending.ok().cloned();
    data.pending_record_state = match view.pending {
        Ok(_) => PersistenceState::Recorded,
        Err(StoreError::OutcomeUnknown) => PersistenceState::Uncertain,
        Err(_) => PersistenceState::Unrecorded,
    };
    if let Some(pending) = &data.pending_record {
        if pending.binding == *binding && pending.validate().is_ok() {
            merge(pending, step, &mut observation, &mut data.receipt);
        } else {
            observation.error = Some(CraftError::new(
                ErrorCode::PartialApply,
                "Initial comment journal evidence has a different or invalid binding.",
            ));
        }
    }
    match record(&StepOutcome {
        state: observation.state,
        confirmed_resources: observation.resources.clone(),
        job: data.receipt.job.clone(),
    }) {
        Ok(saved) => {
            let valid = saved.binding == *binding && saved.validate().is_ok();
            let consistent = valid
                && saved.steps.get(step).is_some_and(|s| {
                    s.state == observation.state
                        && s.job == data.receipt.job
                        && observation
                            .resources
                            .iter()
                            .all(|r| s.confirmed_resources.contains(r))
                });
            if valid {
                merge(&saved, step, &mut observation, &mut data.receipt);
            }
            data.persistence = if consistent {
                PersistenceState::Recorded
            } else {
                PersistenceState::Uncertain
            };
            data.latest_record = Some(saved);
            if !consistent {
                observation.error = Some(CraftError::new(
                    ErrorCode::PartialApply,
                    "Comment journal acknowledgment conflicts with observed native effects.",
                ));
            }
        }
        Err(error) => {
            data.persistence = if error == StoreError::OutcomeUnknown {
                PersistenceState::Uncertain
            } else {
                PersistenceState::Unrecorded
            };
            observation.error.get_or_insert_with(|| {
                CraftError::new(
                    ErrorCode::Internal,
                    "Observed comment effects could not be durably acknowledged locally.",
                )
            });
        }
    }
    if let Ok(response) = view.response {
        data.http_status = Some(response.status);
        data.provider_request_id = response.request_id.clone();
        data.retry_after = response.retry_after;
    }
    if observation.resources.iter().any(|r| !expected.contains(r))
        || data.receipt.job.is_some()
        || data
            .latest_record
            .as_ref()
            .is_some_and(|r| r.steps.len() != 1)
        || data
            .pending_record
            .as_ref()
            .is_some_and(|r| r.steps.len() != 1)
    {
        observation.error = Some(CraftError::new(
            ErrorCode::PartialApply,
            "Additional recorded comment effects require inspection.",
        ));
    }
    data.receipt.request_state = request_state(observation.state);
    data.receipt.confirmed_resources = observation.resources;
    data.acknowledged = observation.acknowledged;
    data.desired_fields_verified = observation.desired_fields_verified;
    data.comment = observation.comment.map(metadata);
    data.reconciliation_required = observation.error.is_some();
    observation.error.map_or(Ok(()), Err)
}
fn merge(
    record: &JournalRecord,
    step: &str,
    observation: &mut native::CommentObservation,
    receipt: &mut OperationReceipt,
) {
    if let Some(saved) = record.steps.get(step) {
        for resource in &saved.confirmed_resources {
            if !observation.resources.contains(resource) {
                observation.resources.push(resource.clone());
            }
        }
        receipt.job = saved.job.clone().or(receipt.job.take());
        if saved.state == SubmissionState::Confirmed {
            observation.state = SubmissionState::Confirmed;
        }
        if observation.state == SubmissionState::Failed
            && (!observation.resources.is_empty() || receipt.job.is_some())
        {
            observation.state = SubmissionState::Unknown;
        }
    }
}
