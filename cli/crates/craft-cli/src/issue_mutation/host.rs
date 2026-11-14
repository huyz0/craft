//! Sealed access and single-send journal composition for issue mutations.
use super::*;
use actions::{
    IssueAssignmentInput, IssueDeleteInput, IssueUnassignInput, JiraIssueActions, SubtaskDeletion,
};
use craft_core::contracts::{CanonicalRequest, GrantRef, Selection};
use craft_core::control::InvocationControl;
use craft_core::execution::{NativeCall, OperationContext, OperationHandler, PreparedOperation};
use craft_core::registry::{ProviderModule, Registry};
use craft_core::storage::OperationJournal;
use craft_provider_jira::create_metadata::MetadataPrincipal;
use craft_provider_jira::issue_context::mutation::{
    IssueEditInput, IssueTransitionInput, JiraIssueMutations,
};
use craft_provider_jira::issue_context::{self, FieldPlan, IssueState, JiraIssueContext};
use craft_runtime::auth::{ServiceOAuthClient, VerifiedServiceAccess};
use craft_runtime::journal::{MutationAuthority, NativeJournal};
use craft_runtime::secrets::LockedCredentials;
use craft_runtime::transport::TlsPolicy;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MutationPermission {
    Edit,
    Transition,
    Assign,
    Unassign,
    Delete,
    DeleteAllSubtasks,
}
fn permission(request: &CanonicalRequest) -> Result<MutationPermission, CraftError> {
    match request.command.as_str() {
        EDIT_ISSUE => Ok(MutationPermission::Edit),
        TRANSITION_ISSUE => Ok(MutationPermission::Transition),
        ASSIGN_ISSUE => Ok(MutationPermission::Assign),
        UNASSIGN_ISSUE => Ok(MutationPermission::Unassign),
        DELETE_ISSUE => {
            let input: IssueDeleteInput = serde_json::from_value(request.input.clone())
                .map_err(|_| CraftError::field("/input", "invalid_issue_deletion"))?;
            Ok(match input.subtasks {
                SubtaskDeletion::Refuse => MutationPermission::Delete,
                SubtaskDeletion::DeleteAll { .. } => MutationPermission::DeleteAllSubtasks,
            })
        }
        _ => Err(CraftError::field(
            "/command",
            "expected_qualified_issue_mutation",
        )),
    }
}
/// Trusted host configuration, never decoded from agent input or resource content.
pub struct IssueMutationPolicy {
    pub enabled: bool,
    pub project: ResourceRef,
    pub grant: GrantRef,
    pub expected_actor: String,
    pub permissions: Vec<MutationPermission>,
}
pub struct PreparedIssueMutation {
    registry: Registry,
    write: PreparedOperation,
    metadata: PreparedOperation,
    binding: OperationBinding,
    completion: MutationCompletion,
}
struct Handler;
impl OperationHandler for Handler {
    fn validate(&self, request: &CanonicalRequest) -> Result<(), CraftError> {
        if matches!(request.command.as_str(), EDIT_ISSUE | TRANSITION_ISSUE) {
            mutation::validate_request(request)
        } else {
            actions::validate_request(request)
        }
    }
    fn execute(
        &self,
        _: &PreparedOperation,
        _: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "Issue mutation execution requires the journaled host.",
        ))
    }
}
pub(super) fn registry() -> Result<Registry, CraftError> {
    let mut registry = Registry::default();
    registry.register_provider(&JiraIssueContext)?;
    let mut provider = Registry::default();
    JiraIssueMutations.register(&mut provider)?;
    JiraIssueActions.register(&mut provider)?;
    for command in [ASSIGN_ISSUE, UNASSIGN_ISSUE, DELETE_ISSUE] {
        let command = actions::metadata_command(command)?;
        registry.register_handler(provider.descriptor(command)?.clone(), Arc::new(Handler))?;
    }
    for command in [
        EDIT_ISSUE,
        TRANSITION_ISSUE,
        ASSIGN_ISSUE,
        UNASSIGN_ISSUE,
        DELETE_ISSUE,
    ] {
        let mut descriptor = provider.descriptor(command)?.clone();
        descriptor.response_schema = schema()?;
        descriptor.projection = Some(projection());
        descriptor.response_example["data"] = json!({
            "receipt":{"operation_id":"issue-change","request_state":"confirmed","confirmed_resources":[{"provider":"jira","deployment":"cloud","tenant":"00000000-1111-2222-3333-444444444444","resource_type":"issue","id":"10000"}]},
            "target":{"provider":"jira","deployment":"cloud","tenant":"00000000-1111-2222-3333-444444444444","resource_type":"issue","id":"10000"},
            "step":route(command)?,"persistence":"recorded","journal_revision":3,"http_status":204,"provider_request_id":null,"retry_after":null,"reconciliation_required":false,"replay_authorized":false,"credential_cache_update":null,"prior_record":null
        });
        match command {
            EDIT_ISSUE => {
                registry.register_typed_handler::<IssueEditInput>(descriptor, Arc::new(Handler))?
            }
            TRANSITION_ISSUE => registry
                .register_typed_handler::<IssueTransitionInput>(descriptor, Arc::new(Handler))?,
            ASSIGN_ISSUE => registry
                .register_typed_handler::<IssueAssignmentInput>(descriptor, Arc::new(Handler))?,
            UNASSIGN_ISSUE => registry
                .register_typed_handler::<IssueUnassignInput>(descriptor, Arc::new(Handler))?,
            _ => registry
                .register_typed_handler::<IssueDeleteInput>(descriptor, Arc::new(Handler))?,
        }
    }
    Ok(registry)
}
pub(crate) fn prepare_original(request: CanonicalRequest) -> Result<PreparedOperation, CraftError> {
    permission(&request)?;
    registry()?.prepare(request)
}
impl PreparedIssueMutation {
    pub(crate) fn saved_source(&self) -> (&CanonicalRequest, &OperationBinding) {
        (self.write.request(), &self.binding)
    }
    pub(crate) fn preflight_saved(&self) -> Result<(), CraftError> {
        let request = self.write.request();
        MutationCompletion::prepare(
            self.binding.clone(),
            &request.command,
            &self.completion.request_id,
            &request.selection.fields,
            crate::saved_requests::native_budget(request.selection.max_bytes as usize)?,
        )?;
        Ok(())
    }
    pub(crate) fn reject_capture(
        self,
        error: CraftError,
        prior: Option<&JournalRecord>,
    ) -> CommandResult {
        self.completion.before_send_failure(error, prior)
    }
    /// Pure input, apply, trusted project/operation policy and output validation.
    pub fn prepare(
        request: CanonicalRequest,
        operation_id: &str,
        apply: bool,
        request_id: &str,
        policy: IssueMutationPolicy,
    ) -> Result<Self, CraftError> {
        let registry = registry()?;
        let write = registry.prepare(request)?;
        if !apply {
            return Err(CraftError::new(
                ErrorCode::ApplyRequired,
                "Issue mutations require explicit apply.",
            ));
        }
        craft_provider_jira::created_issue::validate_target(&policy.project)?;
        craft_core::contracts::validate_identity("/expected_actor", &policy.expected_actor)?;
        let expected: IssueState =
            serde_json::from_value(write.request().input["expected"].clone())
                .map_err(|_| CraftError::field("/expected", "invalid_issue_state"))?;
        let permission = permission(write.request())?;
        if !policy.enabled
            || !policy.permissions.contains(&permission)
            || matches!(policy.expected_actor.as_str(), "unknown" | "anonymous")
            || policy.project.tenant_ref() != write.request().target.tenant_ref()
            || policy.project.id != expected.project_id
        {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Issue mutation is outside the trusted project/operation/actor policy.",
            ));
        }
        let binding =
            write.mutation_binding(operation_id, &policy.grant, &policy.expected_actor)?;
        let completion = MutationCompletion::prepare(
            binding.clone(),
            &write.request().command,
            request_id,
            &write.request().selection.fields,
            write.request().selection.max_bytes as usize,
        )?;
        let transition = write.request().command == TRANSITION_ISSUE;
        let metadata = if matches!(
            write.request().command.as_str(),
            ASSIGN_ISSUE | UNASSIGN_ISSUE | DELETE_ISSUE
        ) {
            registry.prepare(CanonicalRequest {
                command: actions::metadata_command(&write.request().command)?.into(),
                target: write.request().target.clone(),
                input: write.request().input.clone(),
                selection: Selection::default(),
                preconditions: None,
            })?
        } else {
            registry.prepare(CanonicalRequest {
                command: if transition {issue_context::TRANSITIONS} else {issue_context::EDIT}.into(),
                target: write.request().target.clone(),
                input: if transition {
                    json!({"issue":write.request().target.id,"transition_id":write.request().input["transition_id"]})
                } else {
                    json!({"issue":write.request().target.id})
                },
                selection: Selection {
                    fields: if transition {vec!["id".into(),"fields".into()]} else {vec![]},
                    ..Selection::default()
                },
                preconditions: None,
            })?
        };
        Ok(Self {
            registry,
            write,
            metadata,
            binding,
            completion,
        })
    }
    fn prior(&self, journal: &NativeJournal) -> Result<Option<JournalRecord>, CraftError> {
        journal.get(&self.binding.operation_id).map_err(|_| {
            CraftError::new(
                ErrorCode::Internal,
                "Cannot inspect prior mutation authority; no resource request was sent.",
            )
        })
    }
    fn check_prior(&self, prior: Option<&JournalRecord>) -> Result<(), CraftError> {
        let step = route(&self.write.request().command)?;
        if prior.is_some_and(|record| {
            record.binding != self.binding
                || record.steps.iter().any(|(id, step_state)| {
                    id != step || step_state.state != SubmissionState::Prepared
                })
        }) {
            return Err(CraftError::new(
                ErrorCode::OperationIdReused,
                "Operation identity differs or has already started; inspect its journal before recovery.",
            ));
        }
        Ok(())
    }
    fn check_access(&self, access: &VerifiedServiceAccess) -> Result<(), CraftError> {
        if access.tenant() != &self.binding.target.tenant_ref()
            || access.grant() != &self.binding.grant
            || access.identity().actor() != self.binding.actor
        {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Verified service access differs from the trusted mutation policy.",
            ));
        }
        for operation in [&self.write, &self.metadata] {
            access.check_operation(&self.registry, operation)?;
        }
        Ok(())
    }
    pub fn execute(
        self,
        access: &VerifiedServiceAccess,
        journal: &NativeJournal,
        tls: TlsPolicy,
        control: InvocationControl,
    ) -> CommandResult {
        self.execute_with_policy_check(access, journal, tls, control, || Ok(()))
    }
    pub fn execute_with_policy_check(
        mut self,
        access: &VerifiedServiceAccess,
        journal: &NativeJournal,
        tls: TlsPolicy,
        control: InvocationControl,
        check: impl Fn() -> Result<(), CraftError>,
    ) -> CommandResult {
        let journal = journal.for_invocation(&control);
        if let Some(update) = access.cache_update() {
            self.completion
                .set_cache_update(Some(CredentialCacheUpdate {
                    grant: access.grant().clone(),
                    write_state: RequestState::Confirmed,
                    cleanup_pending: Some(update.cleanup_pending),
                }));
        }
        let mut prior = None;
        let mut claim_attempted = false;
        let attempt = (|| {
            control.check()?;
            check()?;
            self.check_access(access)?;
            prior = self.prior(&journal)?;
            self.check_prior(prior.as_ref())?;
            let principal = MetadataPrincipal {
                grant: access.grant().clone(),
                actor: access.identity().actor().into(),
            };
            let clock = access.clock();
            let mut native = access.read_context(
                &self.registry,
                &self.metadata,
                tls.clone(),
                control.clone(),
            )?;
            let mut context = PolicyRead {
                native: &mut native,
                check: &check,
            };
            let frozen = if matches!(
                self.write.request().command.as_str(),
                ASSIGN_ISSUE | UNASSIGN_ISSUE | DELETE_ISSUE
            ) {
                actions::prepare_action(
                    &self.metadata,
                    &self.write,
                    &mut context,
                    &principal,
                    clock.as_ref(),
                )?
            } else {
                let plan = FieldPlan::compile(
                    &self.metadata.request().command,
                    &self.metadata.request().selection.fields,
                )?;
                let metadata = issue_context::read_context(
                    &self.metadata,
                    &mut context,
                    &plan,
                    None,
                    &principal,
                    clock.as_ref(),
                )?;
                metadata.prepare_mutation(
                    &self.metadata,
                    &self.write,
                    &mut context,
                    &principal,
                    clock.as_ref(),
                )?
            };
            check()?;
            control.check()?;
            let prepared = access.prepare_mutation(
                &self.registry,
                &self.write,
                tls,
                control,
                frozen.call(),
            )?;
            claim_attempted = true;
            prepared.send_checked(
                &journal,
                &MutationAuthority {
                    apply: true,
                    operation_id: &self.binding.operation_id,
                    grant: access.grant(),
                    actor: access.identity().actor(),
                },
                route(&self.write.request().command)?,
                || {
                    self.check_access(access)?;
                    frozen.verify(&self.write, &principal, clock.as_ref())?;
                    check()
                },
            )
        })();
        match attempt {
            Ok(attempt) => self.completion.finish(&attempt),
            Err(mut error) => {
                if claim_attempted {
                    error.details.write_state = RequestState::SentUnknown;
                    if let Ok(Some(latest)) = self.prior(&journal) {
                        prior = Some(latest);
                    }
                }
                self.completion.before_send_failure(error, prior.as_ref())
            }
        }
    }
    pub fn execute_cached(
        self,
        client: &ServiceOAuthClient,
        locked: &LockedCredentials<'_>,
        journal: &NativeJournal,
        tls: TlsPolicy,
        control: InvocationControl,
    ) -> CommandResult {
        self.execute_cached_with_policy_check(client, locked, journal, tls, control, || Ok(()))
    }
    pub fn execute_cached_with_policy_check(
        mut self,
        client: &ServiceOAuthClient,
        locked: &LockedCredentials<'_>,
        journal: &NativeJournal,
        tls: TlsPolicy,
        control: InvocationControl,
        check: impl Fn() -> Result<(), CraftError>,
    ) -> CommandResult {
        let journal = journal.for_invocation(&control);
        let mut prior = None;
        let ready = (|| {
            control.check()?;
            check()?;
            if locked.grant() != self.binding.grant {
                return Err(CraftError::new(
                    ErrorCode::PolicyDenied,
                    "Locked grant differs from the mutation policy.",
                ));
            }
            prior = self.prior(&journal)?;
            self.check_prior(prior.as_ref())
        })();
        if let Err(error) = ready {
            return self.completion.before_send_failure(error, prior.as_ref());
        }
        match client.open_service_access(locked, &self.binding.target.tenant_ref()) {
            Ok(access) => self.execute_with_policy_check(&access, &journal, tls, control, check),
            Err(failure) => {
                let update = failure
                    .credential_update
                    .as_ref()
                    .map(|receipt| CredentialCacheUpdate {
                        grant: locked.grant(),
                        write_state: RequestState::Confirmed,
                        cleanup_pending: Some(receipt.cleanup_pending),
                    })
                    .or_else(|| {
                        failure
                            .credential_write_state
                            .map(|state| CredentialCacheUpdate {
                                grant: locked.grant(),
                                write_state: state,
                                cleanup_pending: None,
                            })
                    });
                self.completion.set_cache_update(update);
                let mut error = failure.error;
                error.details.write_state = RequestState::NotSent;
                self.completion.before_send_failure(error, prior.as_ref())
            }
        }
    }
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
    fn normalize_next(&self, call: &NativeCall, next: &str) -> Result<NativeCall, CraftError> {
        self.checkpoint()?;
        self.native.normalize_next(call, next)
    }
}
