//! Bound native issue creation retains complete receipts and original metadata freshness.

use std::sync::Arc;

use craft_core::contracts::{CanonicalRequest, EffectClass, SCHEMA_VERSION, Selection};
use craft_core::control::InvocationControl;
use craft_core::execution::{
    HttpMethod, NativeRoute, OperationContext, OperationHandler, PreparedOperation, RouteSegment,
};
use craft_core::presence::InputField;
use craft_core::registry::{
    OperationDescriptor, ReconciliationClass, Registry, RetryClass, schema_for,
};
use craft_core::storage::{Clock, OperationJournal};
use craft_provider_jira::create_metadata::{
    self, CreateInput, FrozenCreate, JiraMetadata, MetadataPrincipal,
};
use craft_runtime::auth::{ServiceOAuthClient, VerifiedServiceAccess};
use craft_runtime::control::SystemClock;
use craft_runtime::journal::{MutationAuthority, NativeJournal};
use craft_runtime::secrets::LockedCredentials;
use craft_runtime::transport::TlsPolicy;

use super::*;

#[cfg(test)]
mod tests;

/// Trusted host policy, never loaded from repository content or provider responses.
pub struct CreatePolicy {
    pub enabled: bool,
    pub target: ResourceRef,
    pub grant: GrantRef,
    pub expected_actor: String,
}

pub struct PreparedCreate {
    registry: Registry,
    create: PreparedOperation,
    metadata: PreparedOperation,
    assignee: Option<PreparedOperation>,
    binding: OperationBinding,
    completion: CreateCompletion,
    reviewed_material: Option<ReviewedMaterial>,
}

struct ReviewedMaterial {
    payload_digest: String,
    metadata_digest: String,
}

struct CreateHandler;
impl OperationHandler for CreateHandler {
    fn validate(&self, request: &CanonicalRequest) -> Result<(), CraftError> {
        created_issue::validate_target(&request.target)?;
        if request.preconditions.is_some() || request.selection.cursor.is_some() {
            return Err(CraftError::field(
                "/request",
                "create_preconditions_and_cursors_unsupported",
            ));
        }
        let input: CreateInput = serde_json::from_value(request.input.clone())
            .map_err(|_| CraftError::field("/input", "invalid_typed_create_input"))?;
        input.validate_basic()
    }

    fn execute(
        &self,
        _: &PreparedOperation,
        _: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "Create requires the journaled host workflow.",
        ))
    }
}

pub(crate) fn registry() -> Result<Registry, CraftError> {
    let mut registry = Registry::default();
    registry.register_provider(&JiraMetadata::new(Arc::new(SystemClock::default())))?;
    craft_provider_jira::issue_links::register(&mut registry)?;
    registry.register_typed_handler::<CreateInput>(
        OperationDescriptor {
            id: COMMAND.into(),
            provider: "jira".into(),
            contract_version: SCHEMA_VERSION.into(),
            api_revision: "jira-cloud-v3".into(),
            description: "Create one issue through the bound journaled host.".into(),
            effect: EffectClass::Write,
            input_schema: schema_for::<CreateInput>()?,
            response_schema: schema()?,
            example: json!({"issue_type_id":"10001","summary":"Example"}),
            response_example: json!(Envelope::success(COMMAND, "example".into(), json!({
                "receipt":{"operation_id":"create-report","request_state":"confirmed","confirmed_resources":[{"provider":"jira","deployment":"cloud","tenant":"00000000-1111-2222-3333-444444444444","resource_type":"issue","id":"10002"}]},
                "target":{"provider":"jira","deployment":"cloud","tenant":"00000000-1111-2222-3333-444444444444","resource_type":"project","id":"10000"},
                "step":ROUTE,"issue_key":"ED-24","secondary":{"transition":null,"watchers":null},"persistence":"recorded","journal_revision":3,"http_status":201,"provider_request_id":null,"retry_after":null,"reconciliation_required":false,"replay_authorized":false,"credential_cache_update":null
            }))),
            projection: Some(projection()),
            scope_alternatives: vec![vec!["write:jira-work".into()]],
            permissions: vec!["Browse projects".into(), "Create issues".into()],
            tenant_conditions: vec![
                "Fresh exact project/type metadata and optional parent/account evidence.".into(),
            ],
            routes: vec![NativeRoute {
repeated_query_parameters: Vec::new(),
                id: ROUTE.into(),
                method: HttpMethod::Post,
                path: ["rest", "api", "3", "issue"]
                    .map(|part| RouteSegment::Literal(part.into()))
                    .into(),
                query_parameters: vec![],
                continuation_parameters: vec![],
                response_max_bytes: 1_048_576,
            }],
            retry_class: RetryClass::ReconcileBeforeReplay,
            reconciliation: ReconciliationClass::KnownResource,
        },
        Arc::new(CreateHandler),
    )?;
    Ok(registry)
}

pub(super) fn descriptor() -> Result<OperationDescriptor, CraftError> {
    Ok(registry()?.descriptor(COMMAND)?.clone())
}

pub(crate) fn prepare_original(request: CanonicalRequest) -> Result<PreparedOperation, CraftError> {
    if request.command != COMMAND {
        return Err(CraftError::field("/command", "expected_issue_create"));
    }
    registry()?.prepare(request)
}

impl PreparedCreate {
    pub(crate) fn reject_retained_source(
        self,
        error: CraftError,
        receipt: Option<crate::saved_requests::SavedReceipt>,
        journal: &NativeJournal,
        control: &InvocationControl,
    ) -> CommandResult {
        let (prior, unavailable) =
            crate::saved_requests::failure_prior(journal, self.binding(), control);
        crate::saved_requests::attach_failure(
            self.reject_capture(error, prior.as_ref()),
            receipt,
            unavailable,
        )
    }
    pub(crate) fn retain_source(
        self,
        store: &craft_runtime::profiles::ProfileStore,
        journal: &NativeJournal,
        control: &InvocationControl,
    ) -> Result<(Self, Option<crate::saved_requests::SavedReceipt>), Box<CommandResult>> {
        let (original, binding) = self.saved_source();
        if let Err(error) = self.preflight_saved() {
            let (prior, unavailable) =
                crate::saved_requests::failure_prior(journal, binding, control);
            return Err(Box::new(crate::saved_requests::attach_failure(
                self.reject_capture(error, prior.as_ref()),
                None,
                unavailable,
            )));
        }
        match crate::saved_requests::capture(store, journal, original, binding, ROUTE, control) {
            Ok(receipt) => Ok((self, receipt)),
            Err(failure) => Err(Box::new(crate::saved_requests::attach_failure(
                self.reject_capture(failure.error, failure.prior.as_ref()),
                failure.receipt,
                failure.evidence_unavailable,
            ))),
        }
    }
    pub(crate) fn saved_source(&self) -> (&CanonicalRequest, &OperationBinding) {
        (self.create.request(), &self.binding)
    }
    pub(crate) fn preflight_saved(&self) -> Result<(), CraftError> {
        let request = self.create.request();
        CreateCompletion::prepare(
            self.binding.clone(),
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
        self.completion
            .before_send_failure_with_record(error, prior)
    }
    pub fn binding(&self) -> &OperationBinding {
        &self.binding
    }
    /// Pure preparation. Invalid input, policy, apply or output fails before state/auth access.
    pub fn prepare(
        request: CanonicalRequest,
        operation_id: &str,
        apply: bool,
        request_id: &str,
        policy: CreatePolicy,
    ) -> Result<Self, CraftError> {
        let encoded = serde_json::to_vec(&request.input)
            .map_err(|_| CraftError::field("/input", "cannot_encode_create_input"))?;
        craft_core::input::parse_object(&encoded)?;
        let registry = registry()?;
        let create = registry.prepare(request)?;
        if create.request().command != COMMAND {
            return Err(CraftError::field("/command", "expected_issue_create"));
        }
        if !apply {
            return Err(CraftError::new(
                ErrorCode::ApplyRequired,
                "Issue creation requires explicit apply.",
            ));
        }
        if !policy.enabled || policy.target != create.request().target {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Issue create is disabled or outside the exact trusted project policy.",
            ));
        }
        craft_core::contracts::validate_identity("/expected_actor", &policy.expected_actor)?;
        if matches!(policy.expected_actor.as_str(), "unknown" | "anonymous") {
            return Err(CraftError::field(
                "/expected_actor",
                "expected_service_actor",
            ));
        }
        let binding =
            create.mutation_binding(operation_id, &policy.grant, &policy.expected_actor)?;
        let completion = CreateCompletion::prepare(
            binding.clone(),
            request_id,
            &create.request().selection.fields,
            create.request().selection.max_bytes as usize,
        )?;
        let input: CreateInput = serde_json::from_value(create.request().input.clone())
            .map_err(|_| CraftError::field("/input", "invalid_typed_create_input"))?;
        let read = |command: &str, input: Value| {
            registry.prepare(CanonicalRequest {
                command: command.into(),
                target: create.request().target.clone(),
                input,
                selection: Selection::default(),
                preconditions: None,
            })
        };
        let metadata = read(
            create_metadata::COMMAND,
            json!({"issue_type_id":input.issue_type_id,"parent_issue_id":match input.parent_issue_id {InputField::Value(id)=>Some(id),_=>None},"page_size":50}),
        )?;
        let assignee = match input.assignee_account_id {
            InputField::Value(id) => Some(read(
                create_metadata::ASSIGNEE_COMMAND,
                json!({"account_id":id}),
            )?),
            _ => None,
        };
        Ok(Self {
            registry,
            create,
            metadata,
            assignee,
            binding,
            completion,
            reviewed_material: None,
        })
    }

    /// Adds equality constraints only. The caller must separately establish live human approval.
    pub fn constrain_reviewed_material(
        mut self,
        payload: Value,
        metadata_digest: String,
    ) -> Result<Self, CraftError> {
        let encoded = serde_json::to_vec(&payload)
            .map_err(|_| CraftError::field("/reviewed_payload", "invalid_native_payload"))?;
        craft_core::input::parse_object(&encoded)?;
        if self.reviewed_material.is_some()
            || !payload
                .as_object()
                .is_some_and(|value| value.len() == 1 && value["fields"].is_object())
            || !metadata_digest
                .strip_prefix("sha256:")
                .is_some_and(|value| {
                    value.len() == 64
                        && value
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                })
        {
            return Err(CraftError::field(
                "/reviewed_material",
                "expected_one_native_payload_and_metadata_digest",
            ));
        }
        self.reviewed_material = Some(ReviewedMaterial {
            payload_digest: craft_core::input::canonical_digest(&payload)?,
            metadata_digest,
        });
        Ok(self)
    }

    fn check_reviewed_material(
        &self,
        frozen: &FrozenCreate,
        metadata_digest: &str,
    ) -> Result<(), CraftError> {
        if let Some(expected) = &self.reviewed_material {
            let payload = frozen
                .call()
                .body
                .as_ref()
                .ok_or_else(|| CraftError::field("/payload", "missing_frozen_native_body"))?;
            if expected.payload_digest != craft_core::input::canonical_digest(payload)?
                || expected.metadata_digest != metadata_digest
            {
                return Err(CraftError::new(
                    ErrorCode::VersionConflict,
                    "Fresh native payload or material metadata differs from the approved ticket manifest; obtain a new review before applying changed material.",
                ));
            }
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
                "Verified service access differs from the trusted create policy.",
            ));
        }
        // Qualify every required scope before the first resource request, including optional account reads.
        for operation in [&self.create, &self.metadata]
            .into_iter()
            .chain(self.assignee.iter())
        {
            access.check_operation(&self.registry, operation)?;
        }
        Ok(())
    }

    fn prior(&self, journal: &NativeJournal) -> Result<Option<JournalRecord>, CraftError> {
        journal.get(&self.binding.operation_id).map_err(|_| {
            CraftError::new(
                ErrorCode::Internal,
                "Cannot inspect prior create authority; no resource request was sent.",
            )
        })
    }

    fn check_prior(&self, prior: Option<&JournalRecord>) -> Result<(), CraftError> {
        if let Some(prior) = prior
            && (prior.binding != self.binding
                || prior
                    .steps
                    .iter()
                    .any(|(step, value)| step != ROUTE || value.state != SubmissionState::Prepared))
        {
            let mut error = CraftError::new(
                ErrorCode::OperationIdReused,
                "Operation identity is changed or already started; inspect and reconcile it before any further create.",
            );
            error.next_action = "Inspect the same operation ID under the same state root; this invocation grants no replay authority.".into();
            return Err(error);
        }
        Ok(())
    }

    fn qualify(
        &self,
        principal: MetadataPrincipal,
        clock: Arc<dyn Clock>,
        mut read: impl FnMut(&PreparedOperation) -> Result<Box<dyn OperationContext>, CraftError>,
        check_policy: &dyn Fn() -> Result<(), CraftError>,
    ) -> Result<(FrozenCreate, String), CraftError> {
        let mut context = read(&self.metadata)?;
        let mut details = std::collections::BTreeMap::new();
        let metadata = {
            let mut guard = super::material::MetadataGuard {
                native: context.as_mut(),
                check: check_policy,
                details: &mut details,
            };
            create_metadata::read_context(
                &self.metadata,
                &mut guard,
                principal.clone(),
                clock.clone(),
            )?
        };
        let assignee = self
            .assignee
            .as_ref()
            .map(|operation| {
                let mut context = read(operation)?;
                let mut guard = super::material::MetadataGuard {
                    native: context.as_mut(),
                    check: check_policy,
                    details: &mut details,
                };
                create_metadata::read_assignable(
                    operation,
                    &mut guard,
                    principal.clone(),
                    clock.clone(),
                )
            })
            .transpose()?;
        let frozen =
            metadata.prepare_create_with_assignee(&self.create, &principal, assignee.as_ref())?;
        let metadata_digest =
            craft_core::input::canonical_digest(&super::material::snapshot(&metadata, &details))?;
        Ok((frozen, metadata_digest))
    }

    /// Uses an already verified invocation capability; no token refresh occurs during send.
    pub fn execute(
        self,
        access: &VerifiedServiceAccess,
        journal: &NativeJournal,
        tls: TlsPolicy,
        control: InvocationControl,
    ) -> CommandResult {
        self.execute_with_policy_check(access, journal, tls, control, || Ok(()))
    }

    /// Dynamic policy owners revalidate before reads and after a durable claim.
    pub fn execute_with_policy_check(
        mut self,
        access: &VerifiedServiceAccess,
        journal: &NativeJournal,
        tls: TlsPolicy,
        control: InvocationControl,
        check_policy: impl Fn() -> Result<(), CraftError>,
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
        let result = (|| {
            control.check()?;
            self.check_access(access)?;
            check_policy()?;
            prior = self.prior(&journal)?;
            self.check_prior(prior.as_ref())?;
            let principal = MetadataPrincipal {
                grant: access.grant().clone(),
                actor: access.identity().actor().into(),
            };
            let (frozen, metadata_digest) = self.qualify(
                principal.clone(),
                access.clock(),
                |operation| {
                    check_policy()?;
                    Ok(Box::new(access.read_context(
                        &self.registry,
                        operation,
                        tls.clone(),
                        control.clone(),
                    )?))
                },
                &check_policy,
            )?;
            frozen.verify(&self.create, &principal)?;
            self.check_reviewed_material(&frozen, &metadata_digest)?;
            let mutation = access.prepare_mutation(
                &self.registry,
                &self.create,
                tls,
                control,
                frozen.call(),
            )?;
            mutation.send_checked(
                &journal,
                &MutationAuthority {
                    apply: true,
                    operation_id: &self.binding.operation_id,
                    grant: access.grant(),
                    actor: access.identity().actor(),
                },
                ROUTE,
                || {
                    access.check_operation(&self.registry, &self.create)?;
                    frozen.verify(&self.create, &principal)?;
                    self.check_reviewed_material(&frozen, &metadata_digest)?;
                    check_policy()
                },
            )
        })();
        match result {
            Ok(attempt) => self.completion.finish(&attempt),
            Err(error) => self
                .completion
                .before_send_failure_with_record(error, prior.as_ref()),
        }
    }

    /// The caller's locked grant stays borrowed through authentication, qualification and completion.
    /// A cached-token replacement is reported independently from the issue's outcome.
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

    /// Keeps dynamic policy refusal separate from cache effects and claimed-send uncertainty.
    pub fn execute_cached_with_policy_check(
        mut self,
        client: &ServiceOAuthClient,
        locked: &LockedCredentials<'_>,
        journal: &NativeJournal,
        tls: TlsPolicy,
        control: InvocationControl,
        check_policy: impl Fn() -> Result<(), CraftError>,
    ) -> CommandResult {
        let journal = journal.for_invocation(&control);
        let mut prior = None;
        let result = (|| {
            control.check()?;
            check_policy()?;
            if locked.grant() != self.binding.grant {
                return Err(CraftError::new(
                    ErrorCode::PolicyDenied,
                    "Locked grant differs from the prepared create policy.",
                ));
            }
            prior = self.prior(&journal)?;
            self.check_prior(prior.as_ref())
        })();
        if let Err(error) = result {
            return self
                .completion
                .before_send_failure_with_record(error, prior.as_ref());
        }
        match client.open_service_access(locked, &self.binding.target.tenant_ref()) {
            Ok(access) => {
                self.execute_with_policy_check(&access, &journal, tls, control, check_policy)
            }
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
                // OAuth acquisition/cache uncertainty is distinct from a resource POST that never began.
                let mut error = failure.error;
                error.details.write_state = RequestState::NotSent;
                self.completion.before_send_failure(error)
            }
        }
    }
}
