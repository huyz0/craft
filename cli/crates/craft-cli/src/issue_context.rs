//! Internal read composition. No public parser/catalog registration or implicit provisioning.

pub mod command;

use std::collections::BTreeMap;

use craft_core::contracts::{
    CanonicalRequest, CredentialCacheUpdate, Envelope, GrantRef, RequestState, ResourceRef,
    validate_identity,
};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{NativeCall, NativeResponse, OperationContext, PreparedOperation};
use craft_core::input::{canonical_digest, parse_object};
use craft_core::pagination::{
    ListBinding, ListPage, ListRequest, NativePage, NativePosition, PageSource,
};
use craft_core::projection::CompiledProjection;
use craft_core::registry::Registry;
use craft_provider_jira::create_metadata::MetadataPrincipal;
use craft_provider_jira::issue_context::{
    self, EDIT, FieldPlan, IssueObservation, JiraIssueContext, TRANSITIONS,
};
use craft_runtime::auth::{ServiceOAuthClient, VerifiedServiceAccess};
use craft_runtime::pagination::recovery_probe;
use craft_runtime::profiles::ProfileStore;
use craft_runtime::secrets::LockedCredentials;
use craft_runtime::transport::TlsPolicy;
use serde_json::json;

use crate::dispatch::CommandResult;
use crate::output::OutputPolicy;

/// Explicit host configuration, never loaded from provider links or repository hints.
pub struct IssueContextPolicy {
    pub target: ResourceRef,
    pub grant: GrantRef,
    pub expected_actor: String,
}
pub struct PreparedIssueContext {
    registry: Registry,
    operation: PreparedOperation,
    projection: CompiledProjection,
    fields: FieldPlan,
    binding: ListBinding,
    request_id: String,
}
fn internal() -> CraftError {
    CraftError::new(
        ErrorCode::Internal,
        "Cannot encode qualified issue-metadata output.",
    )
}
fn budget() -> CraftError {
    let mut error = CraftError::new(
        ErrorCode::OutputBudgetTooSmall,
        "Issue metadata requires room for complete paging and credential-cache recovery evidence.",
    );
    error.next_action = "Increase --max-bytes before authentication.".into();
    error
}

impl PreparedIssueContext {
    /// Prepares closed filters, projection and full recovery budgets without opening state or auth.
    pub fn prepare(
        request: CanonicalRequest,
        request_id: &str,
        policy: IssueContextPolicy,
    ) -> Result<Self, CraftError> {
        parse_object(&serde_json::to_vec(&request.input).map_err(|_| internal())?)?;
        validate_identity("/request_id", request_id)?;
        validate_identity("/expected_actor", &policy.expected_actor)?;
        policy.grant.validate()?;
        if !policy.grant.id.strip_prefix("gr_").is_some_and(|suffix| {
            suffix.len() == 32
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        }) || matches!(policy.expected_actor.as_str(), "anonymous" | "unknown")
            || policy.target != request.target
        {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Issue metadata read differs from the exact configured site, grant or expected actor.",
            ));
        }
        let mut registry = Registry::default();
        registry.register_provider(&JiraIssueContext)?;
        let operation = registry.prepare(request)?;
        if !matches!(operation.request().command.as_str(), EDIT | TRANSITIONS) {
            return Err(CraftError::field(
                "/command",
                "expected_issue_metadata_read",
            ));
        }
        let command = operation.request().command.as_str();
        let fields = FieldPlan::compile(command, &operation.request().selection.fields)?;
        let projection = registry
            .projection(command, &operation.request().selection.fields)?
            .ok_or_else(internal)?;
        let descriptor = registry.prepared_descriptor(&operation)?;
        let input = issue_context::effective_input(operation.request())?;
        let binding = ListBinding {
            operation: command.into(),
            descriptor_digest: canonical_digest(
                &serde_json::to_value(descriptor).map_err(|_| internal())?,
            )?,
            schema_digest: registry.schema_digest(command, true)?,
            projection_digest: canonical_digest(
                &projection.response_schema(&descriptor.response_schema)?,
            )?,
            api_revision: descriptor.api_revision.clone(),
            target: operation.request().target.clone(),
            grant: policy.grant,
            actor: policy.expected_actor,
            query: BTreeMap::from([(
                "input".into(),
                serde_json::to_value(input).map_err(|_| internal())?,
            )]),
        };
        binding.digest()?;
        let prepared = Self {
            registry,
            operation,
            projection,
            fields,
            binding,
            request_id: request_id.into(),
        };
        let cache = Some(CredentialCacheUpdate {
            grant: prepared.binding.grant.clone(),
            write_state: RequestState::SentUnknown,
            cleanup_pending: Some(false),
        });
        let mut probe = recovery_probe();
        // Opaque update evidence can require JSON escaping; reserve the longest resolution label.
        probe.context = Some(
            json!({"requested_issue":prepared.binding.target.id,"resolved_issue":{"provider":"jira","deployment":"cloud","tenant":prepared.binding.target.tenant,"resource_type":"issue","id":"9".repeat(512)},"resolution":"resolved_key","state":{"id":"9".repeat(512),"key":format!("{}-1","A".repeat(510)),"project_id":"9".repeat(512),"issue_type_id":"9".repeat(512),"status_id":"9".repeat(512),"updated":"\\".repeat(128)}}),
        );
        prepared
            .fits(&prepared.envelope(Some(&probe), cache.clone(), None)?)?
            .then_some(())
            .ok_or_else(budget)?;
        let mut worst = CraftError::new(ErrorCode::Internal, "x".repeat(500));
        worst.next_action = "x".repeat(1000);
        prepared
            .fits(&prepared.envelope(None, cache, Some(worst))?)?
            .then_some(())
            .ok_or_else(budget)?;
        Ok(prepared)
    }
    fn request(&self) -> ListRequest<'_> {
        ListRequest {
            binding: &self.binding,
            cursor: self.operation.request().selection.cursor.as_deref(),
            limit: self.operation.request().selection.limit,
        }
    }
    fn fits(&self, envelope: &Envelope) -> Result<bool, CraftError> {
        Ok(serde_json::to_vec(envelope)
            .map_err(|_| internal())?
            .len()
            .saturating_add(1)
            <= self.operation.request().selection.max_bytes as usize)
    }
    fn envelope(
        &self,
        page: Option<&ListPage>,
        cache: Option<CredentialCacheUpdate>,
        error: Option<CraftError>,
    ) -> Result<Envelope, CraftError> {
        let issue = page
            .and_then(|page| page.context.as_ref())
            .map(|value| {
                serde_json::from_value::<IssueObservation>(value.clone()).map_err(|_| internal())
            })
            .transpose()?;
        let data = json!({"target":self.binding.target,"items":page.map(|page|page.items.as_slice()).unwrap_or(&[]),"issue_observation":issue,"observation":page.map(|page|&page.observation),"cached":page.is_some_and(|page|page.cached),"credential_cache_update":cache,"atomic_snapshot":false,"mutation_authorized":false});
        let data = self.projection.data(&data)?;
        let error = error.or_else(|| page.and_then(|page| page.error.clone()));
        let mut envelope = match error {
            Some(error) => Envelope::failure_with_data(
                &self.binding.operation,
                self.request_id.clone(),
                error,
                data,
            ),
            None => Envelope::success(&self.binding.operation, self.request_id.clone(), data),
        };
        envelope.page = page.map(|page| page.page.clone());
        Ok(envelope)
    }
    fn finish(
        &self,
        page: Option<&ListPage>,
        cache: Option<CredentialCacheUpdate>,
        error: Option<CraftError>,
    ) -> CommandResult {
        let preserve = cache.is_some();
        let recovery = json!({"credential_cache_update":cache.clone()});
        let envelope = self.envelope(page, cache, error).unwrap_or_else(|error| {
            Envelope::failure_with_data(
                &self.binding.operation,
                self.request_id.clone(),
                error,
                recovery,
            )
        });
        CommandResult {
            envelope,
            output_policy: if preserve {
                OutputPolicy::PreserveEvidence
            } else {
                OutputPolicy::Bounded
            },
        }
    }
    pub fn execute(
        &self,
        access: &VerifiedServiceAccess,
        store: &ProfileStore,
        tls: TlsPolicy,
        control: InvocationControl,
    ) -> CommandResult {
        self.execute_with_policy_check(access, store, tls, control, || Ok(()))
    }
    pub fn execute_with_policy_check(
        &self,
        access: &VerifiedServiceAccess,
        store: &ProfileStore,
        tls: TlsPolicy,
        control: InvocationControl,
        check_policy: impl Fn() -> Result<(), CraftError>,
    ) -> CommandResult {
        let cache = access.cache_update().map(|update| CredentialCacheUpdate {
            grant: access.grant().clone(),
            write_state: RequestState::Confirmed,
            cleanup_pending: Some(update.cleanup_pending),
        });
        let mut source = IssueSource {
            prepared: self,
            access,
            tls,
            check_policy: &check_policy,
        };
        let result = store
            .provider_page(
                self.request(),
                access.clock().as_ref(),
                &control,
                &mut source,
                |page| self.fits(&self.envelope(Some(page), cache.clone(), None)?),
            )
            .and_then(|page| {
                let issue: IssueObservation =
                    serde_json::from_value(page.context.clone().ok_or_else(|| {
                        CraftError::new(
                            ErrorCode::CursorCorrupt,
                            "Issue metadata observation is missing.",
                        )
                    })?)
                    .map_err(|_| {
                        CraftError::new(
                            ErrorCode::CursorCorrupt,
                            "Issue metadata observation is invalid.",
                        )
                    })?;
                issue
                    .validate_for_target(&self.binding.target)
                    .map_err(|_| {
                        CraftError::new(
                            ErrorCode::CursorCorrupt,
                            "Issue metadata observation differs from the bound issue.",
                        )
                    })?;
                source.check_access(&self.binding, &control)?;
                Ok(page)
            });
        match result {
            Ok(page) => self.finish(Some(&page), cache, None),
            Err(error) => self.finish(None, cache, Some(error)),
        }
    }
    pub fn execute_cached(
        &self,
        client: &ServiceOAuthClient,
        locked: &LockedCredentials<'_>,
        store: &ProfileStore,
        tls: TlsPolicy,
        control: InvocationControl,
    ) -> CommandResult {
        self.execute_cached_with_policy_check(client, locked, store, tls, control, || Ok(()))
    }
    pub fn execute_cached_with_policy_check(
        &self,
        client: &ServiceOAuthClient,
        locked: &LockedCredentials<'_>,
        store: &ProfileStore,
        tls: TlsPolicy,
        control: InvocationControl,
        check_policy: impl Fn() -> Result<(), CraftError>,
    ) -> CommandResult {
        let before = (|| {
            control.check()?;
            check_policy()?;
            if locked.grant() != self.binding.grant {
                return Err(CraftError::new(
                    ErrorCode::PolicyDenied,
                    "Locked grant differs from the prepared issue-metadata binding.",
                ));
            }
            store.check_provider_cursor(&self.request(), client.clock().as_ref(), &control)
        })();
        if let Err(error) = before {
            return self.finish(None, None, Some(error));
        }
        let client = match client.for_invocation(control.clone()) {
            Ok(client) => client,
            Err(error) => return self.finish(None, None, Some(error)),
        };
        match client.open_service_access(locked, &self.binding.target.tenant_ref()) {
            Ok(access) => {
                self.execute_with_policy_check(&access, store, tls, control, check_policy)
            }
            Err(failure) => {
                let cache = failure
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
                let mut error = failure.error;
                error.details.write_state = RequestState::NotSent;
                self.finish(None, cache, Some(error))
            }
        }
    }
}

struct IssueSource<'a> {
    prepared: &'a PreparedIssueContext,
    access: &'a VerifiedServiceAccess,
    tls: TlsPolicy,
    check_policy: &'a dyn Fn() -> Result<(), CraftError>,
}

struct PolicyCheckedContext<'a> {
    inner: &'a mut dyn OperationContext,
    check: &'a dyn Fn() -> Result<(), CraftError>,
}
impl OperationContext for PolicyCheckedContext<'_> {
    fn checkpoint(&self) -> Result<(), CraftError> {
        self.inner.checkpoint()
    }
    fn verify_operation(&self, operation: &PreparedOperation) -> Result<(), CraftError> {
        (self.check)()?;
        self.inner.verify_operation(operation)
    }
    fn invoke(&mut self, call: &NativeCall) -> Result<NativeResponse, CraftError> {
        self.inner.checkpoint()?;
        (self.check)()?;
        self.inner.invoke(call)
    }
    fn normalize_next(&self, call: &NativeCall, next: &str) -> Result<NativeCall, CraftError> {
        (self.check)()?;
        self.inner.normalize_next(call, next)
    }
}
impl PageSource for IssueSource<'_> {
    fn check_access(
        &mut self,
        binding: &ListBinding,
        control: &InvocationControl,
    ) -> Result<(), CraftError> {
        control.check()?;
        (self.check_policy)()?;
        if binding != &self.prepared.binding
            || self.access.tenant() != &binding.target.tenant_ref()
            || self.access.grant() != &binding.grant
            || self.access.identity().actor() != binding.actor
        {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Verified issue metadata access differs from the bound site, grant or actor.",
            ));
        }
        self.access
            .check_operation(&self.prepared.registry, &self.prepared.operation)?;
        (self.check_policy)()?;
        control.check()
    }
    fn fetch(
        &mut self,
        binding: &ListBinding,
        position: Option<&NativePosition>,
        control: &InvocationControl,
    ) -> Result<NativePage, CraftError> {
        self.check_access(binding, control)?;
        let mut context = self.access.read_context(
            &self.prepared.registry,
            &self.prepared.operation,
            self.tls.clone(),
            control.clone(),
        )?;
        let check = || {
            control.check()?;
            self.access
                .check_operation(&self.prepared.registry, &self.prepared.operation)?;
            (self.check_policy)()?;
            control.check()
        };
        // Bracket reads share one sealed context, but each exchange needs current policy.
        let mut context = PolicyCheckedContext {
            inner: &mut context,
            check: &check,
        };
        let metadata = issue_context::read_context(
            &self.prepared.operation,
            &mut context,
            &self.prepared.fields,
            position,
            &MetadataPrincipal {
                grant: binding.grant.clone(),
                actor: binding.actor.clone(),
            },
            self.access.clock().as_ref(),
        )?;
        Ok(metadata.page().clone())
    }
}
