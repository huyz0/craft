//! Internal read composition. No public parser/catalog registration or implicit provisioning.

pub mod command;

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
use craft_provider_jira::create_metadata::MetadataPrincipal;
use craft_provider_jira::permissions::{self, COMMAND, FieldPlan, JiraPermissions};
use craft_runtime::auth::{ServiceOAuthClient, VerifiedServiceAccess};
use craft_runtime::pagination::recovery_probe;
use craft_runtime::profiles::ProfileStore;
use craft_runtime::secrets::LockedCredentials;
use craft_runtime::transport::TlsPolicy;
use serde_json::json;

use crate::dispatch::CommandResult;
use crate::output::OutputPolicy;

/// Explicit host configuration, never loaded from provider links or repository hints.
pub struct PermissionReadPolicy {
    pub target: ResourceRef,
    pub grant: GrantRef,
    pub expected_actor: String,
}
pub struct PreparedPermissionRead {
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
        "Cannot encode qualified permission-observation output.",
    )
}
fn budget() -> CraftError {
    let mut error = CraftError::new(
        ErrorCode::OutputBudgetTooSmall,
        "Permission observations require room for complete paging and credential-cache recovery evidence.",
    );
    error.next_action = "Increase --max-bytes before authentication.".into();
    error
}

impl PreparedPermissionRead {
    fn validate_page(&self, page: &ListPage) -> Result<(), CraftError> {
        let input = permissions::input(self.operation.request())?;
        let corrupt = || {
            CraftError::new(
                ErrorCode::CursorCorrupt,
                "Permission page differs from its bound requested keys or decision states.",
            )
        };
        let mut previous = None;
        for item in &page.items {
            let result: permissions::PermissionResult =
                serde_json::from_value(item.clone()).map_err(|_| corrupt())?;
            let position = input
                .permissions
                .iter()
                .position(|key| key == &result.key)
                .ok_or_else(corrupt)?;
            if previous.is_some_and(|previous| position <= previous)
                || (!result.returned && result.have_permission.is_some())
            {
                return Err(corrupt());
            }
            let missing = result
                .fields_not_returned
                .iter()
                .any(|field| field == "have_permission");
            if result.have_permission.is_none() != missing {
                return Err(corrupt());
            }
            previous = Some(position);
        }
        Ok(())
    }
    /// Prepares closed filters, projection and full recovery budgets without opening state or auth.
    pub fn prepare(
        request: CanonicalRequest,
        request_id: &str,
        policy: PermissionReadPolicy,
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
                "Permission read differs from the exact configured site, grant or expected actor.",
            ));
        }
        let mut registry = Registry::default();
        registry.register_provider(&JiraPermissions)?;
        let operation = registry.prepare(request)?;
        if operation.request().command != COMMAND {
            return Err(CraftError::field("/command", "expected_permission_check"));
        }
        let command = operation.request().command.as_str();
        let fields = FieldPlan::compile(&operation.request().selection.fields)?;
        let projection = registry
            .projection(command, &operation.request().selection.fields)?
            .ok_or_else(internal)?;
        let descriptor = registry.prepared_descriptor(&operation)?;
        let input = serde_json::to_value(permissions::input(operation.request())?)
            .map_err(|_| internal())?;
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
            query: BTreeMap::from([("input".into(), input)]),
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
        let input = permissions::input(self.operation.request())?;
        let data = json!({"target":self.binding.target,"context":input.context()?,"requested_permissions":input.permissions,"items":page.map(|page|page.items.as_slice()).unwrap_or(&[]),"observation":page.map(|page|&page.observation),"cached":page.is_some_and(|page|page.cached),"credential_cache_update":cache,"atomic_snapshot":false,"mutation_authorized":false});
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
        let mut source = PermissionSource {
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
                self.validate_page(&page)?;
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
                    "Locked grant differs from the prepared permission-observation binding.",
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

struct PermissionSource<'a> {
    prepared: &'a PreparedPermissionRead,
    access: &'a VerifiedServiceAccess,
    tls: TlsPolicy,
    check_policy: &'a dyn Fn() -> Result<(), CraftError>,
}
impl PageSource for PermissionSource<'_> {
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
                "Verified permission access differs from the bound site, grant or actor.",
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
        let observed = permissions::read_context(
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
        Ok(observed.page().clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_core::contracts::Selection;
    fn request() -> CanonicalRequest {
        let cloud = "00000000-1111-2222-3333-444444444444";
        CanonicalRequest {
            command: COMMAND.into(),
            target: ResourceRef {
                provider: "jira".into(),
                deployment: "cloud".into(),
                tenant: cloud.into(),
                resource_type: "site".into(),
                id: cloud.into(),
            },
            input: json!({"permissions":["EDIT_ISSUES","BROWSE_PROJECTS"],"issue":"ED-1"}),
            selection: Selection::default(),
            preconditions: None,
        }
    }
    fn policy(request: &CanonicalRequest) -> PermissionReadPolicy {
        PermissionReadPolicy {
            target: request.target.clone(),
            grant: GrantRef {
                id: "gr_00000000000000000000000000000000".into(),
            },
            expected_actor: "actor-one".into(),
        }
    }
    #[test]
    fn permission_preparation_refuses_bad_authority_projection_and_complete_recovery_budget()
    -> Result<(), Box<dyn std::error::Error>> {
        for mode in ["target", "grant", "actor", "fields", "input", "budget"] {
            let mut request = request();
            let mut policy = policy(&request);
            match mode {
                "target" => policy.target.id = "other".into(),
                "grant" => policy.grant.id = "other".into(),
                "actor" => policy.expected_actor = "anonymous".into(),
                "fields" => request.selection.fields = vec!["unsupported".into()],
                "input" => request.input["account_id"] = json!("other"),
                _ => request.selection.max_bytes = 512,
            }
            assert!(
                PreparedPermissionRead::prepare(request, "request", policy).is_err(),
                "{mode}"
            );
        }
        Ok(())
    }
    #[test]
    fn cached_permission_items_require_bound_order_types_and_consistent_unknown_decisions()
    -> Result<(), Box<dyn std::error::Error>> {
        let request = request();
        let host = PreparedPermissionRead::prepare(request.clone(), "request", policy(&request))?;
        let mut page = recovery_probe();
        page.items = vec![
            json!({"key":"EDIT_ISSUES","returned":true,"have_permission":false}),
            json!({"key":"BROWSE_PROJECTS","returned":false,"have_permission":null,"fields_not_returned":["have_permission"]}),
        ];
        host.validate_page(&page)?;
        for mode in [
            "key",
            "order",
            "duplicate",
            "decision_type",
            "unreturned_decision",
            "missing_unknown",
        ] {
            let mut altered = page.clone();
            match mode {
                "key" => altered.items[0]["key"] = json!("ADMINISTER"),
                "order" => altered.items.reverse(),
                "duplicate" => altered.items.push(altered.items[0].clone()),
                "decision_type" => altered.items[0]["have_permission"] = json!("false"),
                "unreturned_decision" => altered.items[1]["have_permission"] = json!(true),
                _ => altered.items[1]["fields_not_returned"] = json!([]),
            }
            assert_eq!(
                host.validate_page(&altered)
                    .err()
                    .ok_or("Expected cache refusal")?
                    .code,
                ErrorCode::CursorCorrupt,
                "{mode}"
            );
        }
        Ok(())
    }
}
