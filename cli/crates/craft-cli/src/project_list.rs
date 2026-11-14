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
use craft_provider_jira::projects::{self, COMMAND, JiraProjects, ProjectListInput};
use craft_runtime::auth::{ServiceOAuthClient, VerifiedServiceAccess};
use craft_runtime::pagination::recovery_probe;
use craft_runtime::profiles::ProfileStore;
use craft_runtime::secrets::LockedCredentials;
use craft_runtime::transport::{JiraProjectSite, TlsPolicy};
use serde_json::json;

use crate::dispatch::CommandResult;
use crate::output::OutputPolicy;

pub mod command;

/// Explicit host configuration, never loaded from provider links or repository hints.
pub struct ProjectReadPolicy {
    pub target: ResourceRef,
    pub grant: GrantRef,
    pub expected_actor: String,
    pub continuation_site: String,
}
pub struct PreparedProjectList {
    registry: Registry,
    operation: PreparedOperation,
    projection: CompiledProjection,
    binding: ListBinding,
    site: JiraProjectSite,
    request_id: String,
}
fn internal() -> CraftError {
    CraftError::new(
        ErrorCode::Internal,
        "Cannot encode qualified project-list output.",
    )
}
fn budget() -> CraftError {
    let mut error = CraftError::new(
        ErrorCode::OutputBudgetTooSmall,
        "Project listing requires room for complete paging and credential-cache recovery evidence.",
    );
    error.next_action = "Increase --max-bytes before authentication.".into();
    error
}

impl PreparedProjectList {
    pub fn prepare(
        request: CanonicalRequest,
        request_id: &str,
        policy: ProjectReadPolicy,
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
                "Project read differs from the exact configured site, grant or expected actor.",
            ));
        }
        let site = JiraProjectSite::parse(&policy.continuation_site)?;
        let mut registry = Registry::default();
        registry.register_provider(&JiraProjects)?;
        let operation = registry.prepare(request)?;
        if operation.request().command != COMMAND {
            return Err(CraftError::field("/command", "expected_project_list"));
        }
        let projection = registry
            .projection(COMMAND, &operation.request().selection.fields)?
            .ok_or_else(internal)?;
        let descriptor = registry.prepared_descriptor(&operation)?;
        let input: ProjectListInput =
            serde_json::from_value(operation.request().input.clone()).map_err(|_| internal())?;
        let binding = ListBinding {
            operation: COMMAND.into(),
            descriptor_digest: canonical_digest(
                &serde_json::to_value(descriptor).map_err(|_| internal())?,
            )?,
            schema_digest: registry.schema_digest(COMMAND, true)?,
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
                ("action".into(), json!("view")),
                ("orderBy".into(), json!("key")),
            ]),
        };
        binding.digest()?;
        let prepared = Self {
            registry,
            operation,
            projection,
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
        let data = json!({"target":self.binding.target,"items":page.map(|page|page.items.as_slice()).unwrap_or(&[]),"observation":page.map(|page|&page.observation),"cached":page.is_some_and(|page|page.cached),"credential_cache_update":cache});
        let data = self.projection.data(&data)?;
        let error = error.or_else(|| page.and_then(|page| page.error.clone()));
        let mut envelope = match error {
            Some(error) => {
                Envelope::failure_with_data(COMMAND, self.request_id.clone(), error, data)
            }
            None => Envelope::success(COMMAND, self.request_id.clone(), data),
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
            Envelope::failure_with_data(COMMAND, self.request_id.clone(), error, recovery)
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
        let mut source = ProjectsSource {
            prepared: self,
            access,
            tls,
            check_policy: &check_policy,
        };
        let result = store.provider_page(
            self.request(),
            access.clock().as_ref(),
            &control,
            &mut source,
            |page| self.fits(&self.envelope(Some(page), cache.clone(), None)?),
        );
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
                    "Locked grant differs from the prepared project-list binding.",
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
struct ProjectsSource<'a> {
    prepared: &'a PreparedProjectList,
    access: &'a VerifiedServiceAccess,
    tls: TlsPolicy,
    check_policy: &'a dyn Fn() -> Result<(), CraftError>,
}
impl PageSource for ProjectsSource<'_> {
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
                "Verified project access differs from the bound site, grant or actor.",
            ));
        }
        self.access
            .check_operation(&self.prepared.registry, &self.prepared.operation)
    }
    fn fetch(
        &mut self,
        binding: &ListBinding,
        position: Option<&NativePosition>,
        control: &InvocationControl,
    ) -> Result<NativePage, CraftError> {
        self.check_access(binding, control)?;
        let mut context = self
            .access
            .read_context(
                &self.prepared.registry,
                &self.prepared.operation,
                self.tls.clone(),
                control.clone(),
            )?
            .with_jira_project_site(self.prepared.site.clone())?;
        projects::read_page(&self.prepared.operation, &mut context, position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_core::contracts::Selection;
    #[test]
    fn preparation_refuses_wrong_target_projection_query_and_recovery_budget()
    -> Result<(), Box<dyn std::error::Error>> {
        let target = ResourceRef {
            provider: "jira".into(),
            deployment: "cloud".into(),
            tenant: "00000000-1111-2222-3333-444444444444".into(),
            resource_type: "site".into(),
            id: "00000000-1111-2222-3333-444444444444".into(),
        };
        for mode in ["target", "fields", "query", "page_size", "budget", "site"] {
            let mut request = CanonicalRequest {
                command: COMMAND.into(),
                target: target.clone(),
                input: json!({}),
                selection: Selection::default(),
                preconditions: None,
            };
            let mut policy = ProjectReadPolicy {
                target: target.clone(),
                grant: GrantRef {
                    id: "gr_00000000000000000000000000000000".into(),
                },
                expected_actor: "actor-one".into(),
                continuation_site: "https://craft-test.atlassian.net".into(),
            };
            match mode {
                "target" => request.target.id = "other".into(),
                "fields" => request.selection.fields = vec!["description".into()],
                "query" => request.input = json!({"query":"x\n"}),
                "page_size" => request.input = json!({"page_size":101}),
                "budget" => request.selection.max_bytes = 512,
                _ => policy.continuation_site = "https://other.invalid".into(),
            }
            let error = PreparedProjectList::prepare(request, "request", policy)
                .err()
                .ok_or("Expected pure refusal")?;
            if mode == "budget" {
                assert_eq!(error.code, ErrorCode::OutputBudgetTooSmall);
            }
        }
        Ok(())
    }
}
