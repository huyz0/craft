//! Exact directed link writes with independent endpoint policy and complete recovery evidence.
pub mod command;
pub mod reads;
pub mod recovery;
use crate::issue_create::PersistenceState;
#[cfg(test)]
mod tests;
use craft_core::contracts::{
    CanonicalRequest, CredentialCacheUpdate, OperationReceipt, RequestState, ResourceRef, Selection,
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
use craft_provider_jira::create_metadata::MetadataPrincipal;
use craft_provider_jira::issue_links::write::{self, CONTEXT, CREATE, CREATE_ROUTE, LinkInput};
use craft_runtime::auth::ServiceOAuthClient;
use craft_runtime::issue_mutation_policy::{
    IssueMutationOperation as Op, IssueMutationPolicyRecord,
};
use craft_runtime::journal::{MutationAuthority, NativeJournal, StepOutcome};
use craft_runtime::profiles::ProfileStore;
use craft_runtime::secrets::LockedCredentials;
use craft_runtime::transport::{MutationAttempt, TlsPolicy};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const MINIMUM_OUTPUT: usize = 8_388_608;
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LinkData {
    pub receipt: OperationReceipt,
    pub target: ResourceRef,
    pub relation: LinkInput,
    pub relation_confirmed: bool,
    pub relation_deleted: bool,
    #[schemars(extend("const"=null))]
    pub new_link_created: Option<bool>,
    pub link_id: Option<String>,
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
    #[schemars(extend("const"=false))]
    pub replay_authorized: bool,
}
pub fn registry() -> Result<Registry, CraftError> {
    let mut registry = Registry::default();
    write::register(&mut registry)?;
    Ok(registry)
}
fn denied(message: &str) -> CraftError {
    CraftError::new(ErrorCode::PolicyDenied, message)
}
fn storage(_: StoreError) -> CraftError {
    CraftError::new(
        ErrorCode::Internal,
        "Cannot inspect the exact link operation journal.",
    )
}
fn request_state(state: SubmissionState) -> RequestState {
    match state {
        SubmissionState::Prepared => RequestState::NotSent,
        SubmissionState::Confirmed => RequestState::Confirmed,
        SubmissionState::Failed => RequestState::Failed,
        _ => RequestState::SentUnknown,
    }
}
fn empty(binding: &OperationBinding, input: LinkInput) -> LinkData {
    LinkData {
        receipt: OperationReceipt {
            operation_id: binding.operation_id.clone(),
            request_state: RequestState::NotSent,
            confirmed_resources: vec![],
            job: None,
        },
        target: binding.target.clone(),
        relation: input,
        relation_confirmed: false,
        relation_deleted: false,
        new_link_created: None,
        link_id: None,
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
enum LinkIntent {
    Create(LinkInput),
    Delete(write::LinkDeleteInput),
}
pub struct PreparedLink {
    registry: Registry,
    write: PreparedOperation,
    read: PreparedOperation,
    binding: OperationBinding,
    input: LinkInput,
    deletion: Option<write::LinkDeleteInput>,
    policies: Vec<IssueMutationPolicyRecord>,
}
pub struct LinkEnvironment<'a, 'credentials> {
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
    fn verify_operation(&self, operation: &PreparedOperation) -> Result<(), CraftError> {
        self.checkpoint()?;
        self.native.verify_operation(operation)
    }
    fn normalize_next(&self, call: &NativeCall, link: &str) -> Result<NativeCall, CraftError> {
        self.checkpoint()?;
        self.native.normalize_next(call, link)
    }
    fn invoke(&mut self, call: &NativeCall) -> Result<NativeResponse, CraftError> {
        self.checkpoint()?;
        let response = self.native.invoke(call)?;
        self.checkpoint()?;
        Ok(response)
    }
}
impl PreparedLink {
    pub fn prepare(
        input: LinkInput,
        target: ResourceRef,
        profile: &str,
        operation_id: &str,
        apply: bool,
        store: &ProfileStore,
        control: &InvocationControl,
    ) -> Result<Self, CraftError> {
        Self::prepare_intent(
            LinkIntent::Create(input),
            target,
            profile,
            operation_id,
            apply,
            store,
            control,
        )
    }
    pub fn prepare_delete(
        input: write::LinkDeleteInput,
        target: ResourceRef,
        profile: &str,
        operation_id: &str,
        apply: bool,
        store: &ProfileStore,
        control: &InvocationControl,
    ) -> Result<Self, CraftError> {
        Self::prepare_intent(
            LinkIntent::Delete(input),
            target,
            profile,
            operation_id,
            apply,
            store,
            control,
        )
    }
    fn prepare_intent(
        intent: LinkIntent,
        target: ResourceRef,
        profile: &str,
        operation_id: &str,
        apply: bool,
        store: &ProfileStore,
        control: &InvocationControl,
    ) -> Result<Self, CraftError> {
        if !apply {
            return Err(CraftError::new(
                ErrorCode::ApplyRequired,
                "Link writes require explicit apply.",
            ));
        }
        let registry = registry()?;
        let (input, deletion) = match intent {
            LinkIntent::Create(input) => (input, None),
            LinkIntent::Delete(input) => (input.expected.clone(), Some(input)),
        };
        let (command, context, permission) = if deletion.is_some() {
            (write::DELETE, write::DELETE_CONTEXT, Op::LinkDelete)
        } else {
            (CREATE, CONTEXT, Op::Link)
        };
        let value = if let Some(deletion) = &deletion {
            json!(deletion)
        } else {
            json!(input)
        };
        let write = registry.prepare(CanonicalRequest {
            command: command.into(),
            target: target.clone(),
            input: value.clone(),
            selection: Selection::default(),
            preconditions: None,
        })?;
        let mut policies = Vec::new();
        for project in [&target.id, &input.inward_project_id] {
            let policy = store
                .issue_mutation_policy(profile, project, control)?
                .ok_or_else(|| {
                    denied("Both endpoint projects require an explicit link allowance.")
                })?;
            store.verify_issue_mutation_policy(&policy, permission, control)?;
            if policy.target.tenant != target.tenant
                || policy.target.provider != target.provider
                || policy.target.deployment != target.deployment
            {
                return Err(denied(
                    "Link endpoint policy differs from the exact Jira site.",
                ));
            }
            policies.push(policy);
        }
        if policies[0].grant != policies[1].grant
            || policies[0].expected_actor != policies[1].expected_actor
            || policies[0].installation != policies[1].installation
        {
            return Err(denied(
                "Both project link allowances must identify the same installation, grant and service actor.",
            ));
        }
        let binding = write.mutation_binding(
            operation_id,
            &policies[0].grant,
            &policies[0].expected_actor,
        )?;
        let read = registry.prepare(CanonicalRequest {
            command: context.into(),
            target,
            input: value,
            selection: Selection::default(),
            preconditions: None,
        })?;
        Ok(Self {
            registry,
            write,
            read,
            binding,
            input,
            deletion,
            policies,
        })
    }
    fn step(&self) -> &'static str {
        if self.deletion.is_some() {
            write::DELETE_ROUTE
        } else {
            CREATE_ROUTE
        }
    }
    fn permission(&self) -> Op {
        if self.deletion.is_some() {
            Op::LinkDelete
        } else {
            Op::Link
        }
    }
    pub fn grant(&self) -> &craft_core::contracts::GrantRef {
        &self.binding.grant
    }
    pub fn binding(&self) -> &OperationBinding {
        &self.binding
    }
    fn prior(&self, journal: &NativeJournal, data: &mut LinkData) -> Result<(), CraftError> {
        let prior = journal.get(&self.binding.operation_id).map_err(storage)?;
        if data.prior_record.is_none() {
            data.prior_record = prior.clone();
        } else {
            data.latest_record = prior.clone();
        }
        if prior.is_some() {
            data.persistence = PersistenceState::Recorded;
        }
        if let Some(prior) = prior
            && (prior.binding != self.binding
                || prior.steps.iter().any(|(name, step)| {
                    name != self.step() || step.state != SubmissionState::Prepared
                }))
        {
            return Err(CraftError::new(
                ErrorCode::OperationIdReused,
                "Link operation identity changed or already started; inspect its journal without replaying it.",
            ));
        }
        Ok(())
    }
    pub fn execute(
        self,
        environment: LinkEnvironment<'_, '_>,
        extra: impl Fn() -> Result<(), CraftError>,
    ) -> (LinkData, Option<CraftError>) {
        let LinkEnvironment {
            store,
            client,
            locked,
            journal,
            tls,
            control,
        } = environment;
        let journal = journal.for_invocation(&control);
        let mut data = empty(&self.binding, self.input.clone());
        data.link_id = self.deletion.as_ref().map(|d| d.link_id.clone());
        let mut claim_attempted = false;
        let result = (|| {
            let policy_check = || {
                control.check()?;
                extra()?;
                for policy in &self.policies {
                    store.verify_issue_mutation_policy(policy, self.permission(), &control)?;
                }
                extra()?;
                control.check()
            };
            self.prior(&journal, &mut data)?;
            policy_check()?;
            if locked.grant() != self.binding.grant {
                return Err(denied(
                    "Locked link grant differs from its captured project policies.",
                ));
            }
            let client = client.for_invocation(control.clone())?;
            let access = match client.open_service_access(locked, &self.binding.target.tenant_ref())
            {
                Ok(access) => access,
                Err(failure) => {
                    if let Some(receipt) = failure.credential_update {
                        data.credential_cache_update = Some(CredentialCacheUpdate {
                            grant: self.binding.grant.clone(),
                            write_state: RequestState::Confirmed,
                            cleanup_pending: Some(receipt.cleanup_pending),
                        });
                    } else if let Some(state) = failure.credential_write_state {
                        data.credential_cache_update = Some(CredentialCacheUpdate {
                            grant: self.binding.grant.clone(),
                            write_state: state,
                            cleanup_pending: None,
                        });
                    }
                    let mut error = failure.error;
                    error.details.write_state = RequestState::NotSent;
                    return Err(error);
                }
            };
            if let Some(receipt) = access.cache_update() {
                data.credential_cache_update = Some(CredentialCacheUpdate {
                    grant: access.grant().clone(),
                    write_state: RequestState::Confirmed,
                    cleanup_pending: Some(receipt.cleanup_pending),
                });
            }
            if access.tenant() != &self.binding.target.tenant_ref()
                || access.grant() != &self.binding.grant
                || access.identity().actor() != self.binding.actor
            {
                return Err(denied(
                    "Authenticated link actor, grant or site differs from captured policy.",
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
            let mut native =
                access.read_context(&self.registry, &self.read, tls.clone(), control.clone())?;
            let mut guard = Guard {
                native: &mut native,
                check: &check,
            };
            let frozen = if self.deletion.is_some() {
                write::prepare_delete(
                    &self.read,
                    &self.write,
                    &mut guard,
                    principal.clone(),
                    access.clock(),
                )?
            } else {
                write::prepare_link(
                    &self.read,
                    &self.write,
                    &mut guard,
                    principal.clone(),
                    access.clock(),
                )?
            };
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
                self.step(),
                || {
                    frozen.verify(&self.write, &principal)?;
                    check()
                },
            )?;
            finish(
                &self.binding,
                &self.input,
                self.deletion.as_ref(),
                &attempt,
                &mut data,
            )
        })();
        let error = result.err().map(|mut error| {
            if data.latest_record.is_none() && claim_attempted {
                data.latest_record = journal.get(&self.binding.operation_id).ok().flatten();
                if data.latest_record.is_some() { data.persistence = PersistenceState::Recorded; }
                else if error.details.write_state == RequestState::SentUnknown { data.persistence = PersistenceState::Uncertain; }
            }
            if data.http_status.is_none() {
                if let Some(record) = data.latest_record.as_ref().or(data.prior_record.as_ref())
                    && record.binding == self.binding && record.validate().is_ok()
                    && let Some(step) = record.steps.get(self.step()) {
                    data.receipt.request_state = request_state(step.state);
                    data.receipt.confirmed_resources = step.confirmed_resources.clone();
                    data.receipt.job = step.job.clone();
                }
                if error.details.write_state == RequestState::SentUnknown && data.receipt.request_state == RequestState::NotSent {
                    data.receipt.request_state = RequestState::SentUnknown;
                }
            }
            data.reconciliation_required = data.receipt.request_state != RequestState::NotSent || data.prior_record.is_some() || data.latest_record.is_some();
            error.details.write_state = data.receipt.request_state;
            error.next_action = "Retain all receipts and inspect this operation ID; no automatic retry or replay is authorized.".into();
            error
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
fn finish(
    binding: &OperationBinding,
    input: &LinkInput,
    deletion: Option<&write::LinkDeleteInput>,
    attempt: &MutationAttempt,
    data: &mut LinkData,
) -> Result<(), CraftError> {
    finish_with(
        binding,
        input,
        deletion,
        CompletionView {
            binding: attempt.binding(),
            command: attempt.command(),
            route: attempt.route(),
            step: attempt.step(),
            response: attempt.response.as_ref(),
            pending: attempt.pending_evidence.as_ref(),
        },
        data,
        |outcome| attempt.record_outcome(outcome),
    )
}
fn finish_with(
    binding: &OperationBinding,
    input: &LinkInput,
    deletion: Option<&write::LinkDeleteInput>,
    view: CompletionView<'_>,
    data: &mut LinkData,
    record: impl FnOnce(&StepOutcome) -> Result<JournalRecord, StoreError>,
) -> Result<(), CraftError> {
    let (command, step) = if deletion.is_some() {
        (write::DELETE, write::DELETE_ROUTE)
    } else {
        (CREATE, CREATE_ROUTE)
    };
    let recognized = view.binding == binding
        && view.command == command
        && view.route == step
        && view.step == step;
    let mut observation = match &view.response {
        Ok(response) if recognized => {
            if let Some(deletion) = deletion {
                write::interpret_delete(&binding.target, deletion, response)
            } else {
                write::interpret(&binding.target, input, response)
            }
        }
        _ => write::LinkObservation {
            state: SubmissionState::Unknown,
            resources: vec![],
            error: Some(CraftError::new(
                view.response
                    .err()
                    .map_or(ErrorCode::PolicyDenied, |error| error.code),
                "Link attempt lacks qualified response evidence.",
            )),
        },
    };
    data.pending_record = view.pending.ok().cloned();
    data.pending_record_state = match view.pending {
        Ok(_) => PersistenceState::Recorded,
        Err(StoreError::OutcomeUnknown) => PersistenceState::Uncertain,
        Err(_) => PersistenceState::Unrecorded,
    };
    if data
        .pending_record
        .as_ref()
        .is_some_and(|r| r.binding != *binding || r.validate().is_err())
    {
        observation.error = Some(CraftError::new(
            ErrorCode::PartialApply,
            "Initial link journal evidence has a different or invalid binding.",
        ));
    }
    if let Some(prior) = data
        .pending_record
        .as_ref()
        .filter(|p| p.binding == *binding && p.validate().is_ok())
        && let Some(saved) = prior.steps.get(step)
    {
        for resource in &saved.confirmed_resources {
            if !observation.resources.contains(resource) {
                observation.resources.push(resource.clone());
            }
        }
        data.receipt.job = saved.job.clone();
        if saved.state == SubmissionState::Confirmed {
            observation.state = SubmissionState::Confirmed;
        }
        if observation.state == SubmissionState::Failed
            && (!observation.resources.is_empty() || data.receipt.job.is_some())
        {
            observation.state = SubmissionState::Unknown;
        }
    }
    let persisted = record(&StepOutcome {
        state: observation.state,
        confirmed_resources: observation.resources.clone(),
        job: data.receipt.job.clone(),
    });
    match persisted {
        Ok(record) => {
            let consistent = record.binding == *binding
                && record.validate().is_ok()
                && record.steps.get(step).is_some_and(|step| {
                    step.state == observation.state
                        && observation
                            .resources
                            .iter()
                            .all(|r| step.confirmed_resources.contains(r))
                });
            if let Some(step) = record
                .steps
                .get(step)
                .filter(|_| record.binding == *binding && record.validate().is_ok())
            {
                for resource in &step.confirmed_resources {
                    if !observation.resources.contains(resource) {
                        observation.resources.push(resource.clone());
                    }
                }
                data.receipt.job = step.job.clone().or(data.receipt.job.take());
                if step.state == SubmissionState::Confirmed {
                    observation.state = SubmissionState::Confirmed;
                }
            }
            data.persistence = if consistent {
                PersistenceState::Recorded
            } else {
                PersistenceState::Uncertain
            };
            data.latest_record = Some(record);
            if !consistent {
                observation.error = Some(CraftError::new(
                    ErrorCode::PartialApply,
                    "Link journal acknowledgment conflicts with observed effect evidence.",
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
                    "Observed link outcome could not be durably acknowledged locally.",
                )
            });
        }
    }
    data.receipt.request_state = request_state(observation.state);
    data.receipt.confirmed_resources = observation.resources;
    data.relation_confirmed = recognized
        && deletion.is_none()
        && matches!(view.response.as_ref().map(|r| r.status), Ok(201));
    data.relation_deleted = recognized
        && deletion.is_some()
        && matches!(view.response.as_ref().map(|r| r.status), Ok(200 | 204));
    if let Ok(response) = &view.response {
        data.http_status = Some(response.status);
        data.provider_request_id = response.request_id.clone();
        data.retry_after = response.retry_after;
    }
    let mut expected: Vec<_> = [&input.outward_issue_id, &input.inward_issue_id]
        .into_iter()
        .map(|id| ResourceRef {
            resource_type: "issue".into(),
            id: id.clone(),
            ..binding.target.clone()
        })
        .collect();
    if let Some(deletion) = deletion {
        expected.push(ResourceRef {
            resource_type: "issue_link".into(),
            id: deletion.link_id.clone(),
            ..binding.target.clone()
        });
    }
    let unexpected_resources = data
        .receipt
        .confirmed_resources
        .iter()
        .any(|resource| !expected.contains(resource));
    let extra_effects = data
        .latest_record
        .as_ref()
        .is_some_and(|r| r.steps.len() != 1)
        || data.receipt.job.is_some();
    if extra_effects
        || unexpected_resources
        || data
            .pending_record
            .as_ref()
            .is_some_and(|r| r.steps.len() != 1)
    {
        observation.error = Some(CraftError::new(
            ErrorCode::PartialApply,
            "Additional recorded effects require inspection.",
        ));
    }
    data.reconciliation_required = observation.error.is_some();
    observation.error.map_or(Ok(()), Err)
}
