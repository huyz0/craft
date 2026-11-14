//! Internal field-selected project reads through verified service access and bounded output.

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
use craft_provider_jira::project_context::{
    self, FieldPlan, GET, IdentityResolution, JiraProjectContext, ProjectContextInput, STATUSES,
};
use craft_runtime::auth::{ServiceOAuthClient, VerifiedServiceAccess};
use craft_runtime::pagination::recovery_probe;
use craft_runtime::profiles::ProfileStore;
use craft_runtime::secrets::LockedCredentials;
use craft_runtime::transport::TlsPolicy;
use serde_json::{Value, json};

use crate::dispatch::CommandResult;
use crate::output::OutputPolicy;

pub mod command;

/// Trusted host authority for these operations, independent of project-discovery records.
pub struct ProjectContextPolicy {
    pub target: ResourceRef,
    pub grant: GrantRef,
    pub expected_actor: String,
}
/// Purely prepared internal read; this type does not register a public command or provision access.
pub struct PreparedProjectContext {
    registry: Registry,
    operation: PreparedOperation,
    projection: CompiledProjection,
    fields: FieldPlan,
    binding: ListBinding,
    request_id: String,
}
enum ReadData<'a> {
    Get {
        project: Value,
        resolution: IdentityResolution,
        observed_at: u64,
    },
    Statuses(&'a ListPage),
}
fn internal() -> CraftError {
    CraftError::new(
        ErrorCode::Internal,
        "Cannot encode qualified project-context output.",
    )
}
fn budget() -> CraftError {
    let mut error = CraftError::new(
        ErrorCode::OutputBudgetTooSmall,
        "Project reads require room for complete error, paging and credential-cache recovery evidence.",
    );
    error.next_action = "Increase --max-bytes before authentication.".into();
    error
}
impl PreparedProjectContext {
    /// Validates the exact trusted policy, native inputs, field plan and complete recovery budget.
    /// Preparation performs no state, credential or network access and grants no new authority.
    pub fn prepare(
        request: CanonicalRequest,
        request_id: &str,
        policy: ProjectContextPolicy,
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
                "Project read differs from the exact configured project, grant or expected actor.",
            ));
        }
        let mut registry = Registry::default();
        registry.register_provider(&JiraProjectContext)?;
        let operation = registry.prepare(request)?;
        let command = &operation.request().command;
        let projection = registry
            .projection(command, &operation.request().selection.fields)?
            .ok_or_else(internal)?;
        let fields = FieldPlan::compile(command, &operation.request().selection.fields)?;
        let effective_input = serde_json::to_value(
            serde_json::from_value::<ProjectContextInput>(operation.request().input.clone())
                .map_err(|_| internal())?,
        )
        .map_err(|_| internal())?;
        let descriptor = registry.prepared_descriptor(&operation)?;
        let binding = ListBinding {
            operation: command.clone(),
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
            query: BTreeMap::from([("input".into(), effective_input)]),
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
        if prepared.binding.operation == STATUSES
            && !prepared.fits(&prepared.envelope(
                Some(ReadData::Statuses(&recovery_probe())),
                cache.clone(),
                None,
            )?)?
        {
            return Err(budget());
        }
        let mut worst = CraftError::new(ErrorCode::Internal, "x".repeat(500));
        worst.next_action = "x".repeat(1000);
        if !prepared.fits(&prepared.envelope(None, cache, Some(worst))?)? {
            return Err(budget());
        }
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
        result: Option<ReadData<'_>>,
        cache: Option<CredentialCacheUpdate>,
        error: Option<CraftError>,
    ) -> Result<Envelope, CraftError> {
        let mut page = None;
        let mut error = error;
        let data = if self.binding.operation == GET {
            let (project, resolution, observed_at) = match result {
                Some(ReadData::Get {
                    project,
                    resolution,
                    observed_at,
                }) => (Some(project), Some(resolution), Some(observed_at)),
                _ => (None, None, None),
            };
            json!({"target":self.binding.target,"project":project,"identity_resolution":resolution,"observed_at":observed_at,"credential_cache_update":cache})
        } else {
            let source = match result {
                Some(ReadData::Statuses(source)) => Some(source),
                _ => None,
            };
            page = source.map(|source| source.page.clone());
            error = error.or_else(|| source.and_then(|source| source.error.clone()));
            json!({"target":self.binding.target,"items":source.map(|source|source.items.as_slice()).unwrap_or(&[]),"observation":source.map(|source|&source.observation),"cached":source.is_some_and(|source|source.cached),"credential_cache_update":cache})
        };
        let data = self.projection.data(&data)?;
        let mut envelope = match error {
            Some(error) => Envelope::failure_with_data(
                &self.binding.operation,
                self.request_id.clone(),
                error,
                data,
            ),
            None => Envelope::success(&self.binding.operation, self.request_id.clone(), data),
        };
        envelope.page = page;
        Ok(envelope)
    }
    fn finish(
        &self,
        result: Option<ReadData<'_>>,
        cache: Option<CredentialCacheUpdate>,
        error: Option<CraftError>,
    ) -> CommandResult {
        let preserve = cache.is_some();
        let recovery = json!({"credential_cache_update":cache.clone()});
        let envelope = self.envelope(result, cache, error).unwrap_or_else(|error| {
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
    fn check_access(
        &self,
        access: &VerifiedServiceAccess,
        control: &InvocationControl,
        check_policy: &dyn Fn() -> Result<(), CraftError>,
    ) -> Result<(), CraftError> {
        control.check()?;
        check_policy()?;
        if access.tenant() != &self.binding.target.tenant_ref()
            || access.grant() != &self.binding.grant
            || access.identity().actor() != self.binding.actor
        {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Verified project access differs from the bound project, grant or actor.",
            ));
        }
        access.check_operation(&self.registry, &self.operation)
    }
    /// Uses existing sealed access for one live get or one bounded status read invocation.
    /// Cache receipts remain independent of the resource result and survive later failures.
    pub fn execute(
        &self,
        access: &VerifiedServiceAccess,
        store: &ProfileStore,
        tls: TlsPolicy,
        control: InvocationControl,
    ) -> CommandResult {
        self.execute_with_policy_check(access, store, tls, control, || Ok(()))
    }
    /// Rechecks trusted caller policy before native/cached access and before releasing data.
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
        if self.binding.operation == GET {
            let result = (|| {
                self.check_access(access, &control, &check_policy)?;
                let mut context =
                    access.read_context(&self.registry, &self.operation, tls, control.clone())?;
                let (project, resolution) =
                    project_context::read_get(&self.operation, &mut context, &self.fields)?;
                self.check_access(access, &control, &check_policy)?;
                let result = ReadData::Get {
                    project,
                    resolution,
                    observed_at: access.clock().unix_millis(),
                };
                let envelope = self.envelope(Some(result), cache.clone(), None)?;
                if !self.fits(&envelope)? {
                    let mut error = CraftError::new(
                        ErrorCode::ItemTooLarge,
                        "The selected project cannot fit in the complete output envelope.",
                    );
                    error.next_action =
                        "Select fewer fields or increase --max-bytes and perform a new live read."
                            .into();
                    return Err(error);
                }
                // Timestamp and complete-envelope work may outlive the earlier access check.
                check_policy()?;
                control.check()?;
                Ok(envelope)
            })();
            return match result {
                Ok(envelope) => CommandResult {
                    envelope,
                    output_policy: if cache.is_some() {
                        OutputPolicy::PreserveEvidence
                    } else {
                        OutputPolicy::Bounded
                    },
                },
                Err(error) => self.finish(None, cache, Some(error)),
            };
        }
        let mut source = StatusesSource {
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
            |page| {
                self.fits(&self.envelope(Some(ReadData::Statuses(page)), cache.clone(), None)?)
            },
        );
        match result {
            Ok(page) => match check_policy().and_then(|()| control.check()) {
                Ok(()) => self.finish(Some(ReadData::Statuses(&page)), cache, None),
                Err(error) => self.finish(None, cache, Some(error)),
            },
            Err(error) => self.finish(None, cache, Some(error)),
        }
    }
    /// Opens verified access under the supplied locked grant, then executes without resource retries.
    /// Invalid status continuation fails before credential acquisition.
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
    /// Adds dynamic policy checks before auth and throughout the native/cached read path.
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
                    "Locked grant differs from the prepared project-context binding.",
                ));
            }
            if self.binding.operation == STATUSES {
                store.check_provider_cursor(&self.request(), client.clock().as_ref(), &control)?;
            }
            Ok(())
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

struct StatusesSource<'a> {
    prepared: &'a PreparedProjectContext,
    access: &'a VerifiedServiceAccess,
    tls: TlsPolicy,
    check_policy: &'a dyn Fn() -> Result<(), CraftError>,
}
impl PageSource for StatusesSource<'_> {
    fn check_access(
        &mut self,
        binding: &ListBinding,
        control: &InvocationControl,
    ) -> Result<(), CraftError> {
        if binding != &self.prepared.binding {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Project status traversal binding changed.",
            ));
        }
        self.prepared
            .check_access(self.access, control, self.check_policy)
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
        project_context::read_statuses(
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
    fn project_context_preparation_refuses_wrong_authority_input_fields_and_recovery_budget()
    -> Result<(), Box<dyn std::error::Error>> {
        let target = ResourceRef {
            provider: "jira".into(),
            deployment: "cloud".into(),
            tenant: "00000000-1111-2222-3333-444444444444".into(),
            resource_type: "project".into(),
            id: "10000".into(),
        };
        for command in [GET, STATUSES] {
            for mode in [
                "target", "grant", "actor", "fields", "input", "budget", "cursor",
            ] {
                let mut request = CanonicalRequest {
                    command: command.into(),
                    target: target.clone(),
                    input: json!({}),
                    selection: Selection::default(),
                    preconditions: None,
                };
                let mut policy = ProjectContextPolicy {
                    target: target.clone(),
                    grant: GrantRef {
                        id: format!("gr_{}", "0".repeat(32)),
                    },
                    expected_actor: "actor-one".into(),
                };
                match mode {
                    "target" => request.target.id = "other".into(),
                    "grant" => policy.grant.id = "unissued".into(),
                    "actor" => policy.expected_actor = "unknown".into(),
                    "fields" => request.selection.fields = vec!["lead.email".into()],
                    "input" => request.input = json!({"expand":"lead"}),
                    "budget" => request.selection.max_bytes = 512,
                    _ => {
                        request.selection.cursor = Some("cur_opaque".into());
                        if command == STATUSES {
                            request.selection.fields = vec!["unknown".into()];
                        }
                    }
                }
                let error = PreparedProjectContext::prepare(request, "request", policy)
                    .err()
                    .ok_or("Expected pure refusal")?;
                if mode == "budget" {
                    assert_eq!(error.code, ErrorCode::OutputBudgetTooSmall);
                }
            }
        }
        Ok(())
    }
}
