//! Internal read composition. No public parser/catalog registration or implicit provisioning.

use std::collections::BTreeMap;

use craft_core::contracts::{
    CanonicalRequest, CredentialCacheUpdate, Envelope, GrantRef, RequestState, ResourceRef,
    validate_identity,
};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::PreparedOperation;
use craft_core::input::{canonical_digest, parse_object};
use craft_core::pagination::{
    ListBinding, ListPage, ListRequest, NativePage, NativePosition, PageSource,
};
use craft_core::projection::CompiledProjection;
use craft_core::registry::Registry;
use craft_provider_jira::fields::{
    self, FieldListInput, FieldPlan, FieldSearchInput, JiraFields, LIST, SEARCH,
};
use craft_runtime::auth::{ServiceOAuthClient, VerifiedServiceAccess};
use craft_runtime::pagination::recovery_probe;
use craft_runtime::profiles::ProfileStore;
use craft_runtime::secrets::LockedCredentials;
use craft_runtime::transport::{JiraFieldSite, TlsPolicy};
use serde_json::json;

use crate::dispatch::CommandResult;
use crate::output::OutputPolicy;

pub mod command;

/// Explicit host configuration, never loaded from provider links or repository hints.
pub struct FieldReadPolicy {
    pub target: ResourceRef,
    pub grant: GrantRef,
    pub expected_actor: String,
    pub continuation_site: String,
}
pub struct PreparedFieldRead {
    registry: Registry,
    operation: PreparedOperation,
    projection: CompiledProjection,
    fields: FieldPlan,
    binding: ListBinding,
    site: JiraFieldSite,
    request_id: String,
}
fn internal() -> CraftError {
    CraftError::new(
        ErrorCode::Internal,
        "Cannot encode qualified field-discovery output.",
    )
}
fn budget() -> CraftError {
    let mut error = CraftError::new(
        ErrorCode::OutputBudgetTooSmall,
        "Field discovery requires room for complete paging and credential-cache recovery evidence.",
    );
    error.next_action = "Increase --max-bytes before authentication.".into();
    error
}

impl PreparedFieldRead {
    /// Prepares closed filters, projection and full recovery budgets without opening state or auth.
    pub fn prepare(
        request: CanonicalRequest,
        request_id: &str,
        policy: FieldReadPolicy,
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
                "Field read differs from the exact configured site, grant or expected actor.",
            ));
        }
        let site = JiraFieldSite::parse(&policy.continuation_site)?;
        let mut registry = Registry::default();
        registry.register_provider(&JiraFields)?;
        let operation = registry.prepare(request)?;
        if !matches!(operation.request().command.as_str(), LIST | SEARCH) {
            return Err(CraftError::field("/command", "expected_field_discovery"));
        }
        let command = operation.request().command.as_str();
        let fields = FieldPlan::compile(command, &operation.request().selection.fields)?;
        let projection = registry
            .projection(command, &operation.request().selection.fields)?
            .ok_or_else(internal)?;
        let descriptor = registry.prepared_descriptor(&operation)?;
        let input = if command == LIST {
            serde_json::to_value(
                serde_json::from_value::<FieldListInput>(operation.request().input.clone())
                    .map_err(|_| internal())?,
            )
            .map_err(|_| internal())?
        } else {
            serde_json::to_value(
                serde_json::from_value::<FieldSearchInput>(operation.request().input.clone())
                    .map_err(|_| internal())?,
            )
            .map_err(|_| internal())?
        };
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
            query: BTreeMap::from([
                (
                    "input".into(),
                    serde_json::to_value(input).map_err(|_| internal())?,
                ),
                ("continuation_site".into(), json!(site.as_str())),
                ("expansions".into(), json!(fields.expansions())),
            ]),
        };
        binding.digest()?;
        let prepared = Self {
            registry,
            operation,
            projection,
            fields,
            binding,
            site,
            request_id: request_id.into(),
        };
        let cache = Some(CredentialCacheUpdate {
            grant: prepared.binding.grant.clone(),
            write_state: RequestState::SentUnknown,
            cleanup_pending: Some(false),
        });
        prepared
            .fits(&prepared.envelope(Some(&recovery_probe()), cache.clone(), None)?)?
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
        let data = json!({"target":self.binding.target,"items":page.map(|page|page.items.as_slice()).unwrap_or(&[]),"observation":page.map(|page|&page.observation),"cached":page.is_some_and(|page|page.cached),"coverage":fields::coverage(&self.binding.operation)?,"credential_cache_update":cache});
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
        let mut source = FieldsSource {
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
                    "Locked grant differs from the prepared field-discovery binding.",
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

struct FieldsSource<'a> {
    prepared: &'a PreparedFieldRead,
    access: &'a VerifiedServiceAccess,
    tls: TlsPolicy,
    check_policy: &'a dyn Fn() -> Result<(), CraftError>,
}
impl PageSource for FieldsSource<'_> {
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
                "Verified field access differs from the bound site, grant or actor.",
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
        if self.prepared.binding.operation == SEARCH {
            context = context.with_jira_field_site(self.prepared.site.clone())?;
        }
        fields::read_page(
            &self.prepared.operation,
            &mut context,
            &self.prepared.fields,
            position,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_core::contracts::Selection;
    #[test]
    fn field_preparation_rejects_invalid_authority_filters_projection_and_recovery_budget()
    -> Result<(), Box<dyn std::error::Error>> {
        let target = ResourceRef {
            provider: "jira".into(),
            deployment: "cloud".into(),
            tenant: "00000000-1111-2222-3333-444444444444".into(),
            resource_type: "site".into(),
            id: "00000000-1111-2222-3333-444444444444".into(),
        };
        for command in [LIST, SEARCH] {
            for mode in [
                "target", "grant", "actor", "fields", "input", "budget", "site",
            ] {
                let mut request = CanonicalRequest {
                    command: command.into(),
                    target: target.clone(),
                    input: json!({}),
                    selection: Selection::default(),
                    preconditions: None,
                };
                let mut policy = FieldReadPolicy {
                    target: target.clone(),
                    grant: GrantRef {
                        id: "gr_00000000000000000000000000000000".into(),
                    },
                    expected_actor: "actor-one".into(),
                    continuation_site: "https://craft-test.atlassian.net".into(),
                };
                match mode {
                    "target" => request.target.id = "other".into(),
                    "grant" => policy.grant.id = "other".into(),
                    "actor" => policy.expected_actor = "anonymous".into(),
                    "fields" => request.selection.fields = vec!["unsupported".into()],
                    "input" => request.input = json!({"expand":"all"}),
                    "budget" => request.selection.max_bytes = 512,
                    _ => policy.continuation_site = "https://other.invalid".into(),
                }
                let error = PreparedFieldRead::prepare(request, "request", policy)
                    .err()
                    .ok_or("Expected local refusal")?;
                if mode == "budget" {
                    assert_eq!(error.code, ErrorCode::OutputBudgetTooSmall);
                }
            }
        }
        Ok(())
    }
}
