//! Current comment read authority and opaque buffers preserve native privacy evidence.
pub mod command;
use crate::dispatch::CommandResult;
use crate::issue_reads::IssueReadPolicy;
use crate::output::OutputPolicy;
use craft_core::contracts::{CanonicalRequest, CredentialCacheUpdate, Envelope, RequestState};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{NativeCall, NativeResponse, OperationContext, PreparedOperation};
use craft_core::input::{canonical_digest, parse_object};
use craft_core::pagination::{
    ListBinding, ListPage, ListRequest, NativePage, NativePosition, PageObservation, PageSource,
};
use craft_core::projection::CompiledProjection;
use craft_core::registry::Registry;
use craft_provider_jira::comments as native;
use craft_runtime::auth::{ServiceOAuthClient, VerifiedServiceAccess};
use craft_runtime::pagination::recovery_probe;
use craft_runtime::profiles::ProfileStore;
use craft_runtime::secrets::LockedCredentials;
use craft_runtime::transport::TlsPolicy;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
fn denied(message: &str) -> CraftError {
    CraftError::new(ErrorCode::PolicyDenied, message)
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
    fn normalize_next(&self, call: &NativeCall, link: &str) -> Result<NativeCall, CraftError> {
        self.checkpoint()?;
        self.native.normalize_next(call, link)
    }
    fn invoke(&mut self, call: &NativeCall) -> Result<NativeResponse, CraftError> {
        self.checkpoint()?;
        let result = self.native.invoke(call)?;
        self.checkpoint()?;
        Ok(result)
    }
}
pub fn registry() -> Result<Registry, CraftError> {
    let mut registry = Registry::default();
    native::register(&mut registry)?;
    Ok(registry)
}
fn invalid() -> CraftError {
    CraftError::new(
        ErrorCode::RemoteUnavailable,
        "Comment read evidence is malformed or inconsistent.",
    )
}
fn budget() -> CraftError {
    CraftError::new(
        ErrorCode::OutputBudgetTooSmall,
        "Increase --max-bytes before authentication to retain link paging and cache receipts.",
    )
}
pub struct PreparedCommentRead {
    registry: Registry,
    operation: PreparedOperation,
    projection: CompiledProjection,
    binding: ListBinding,
    request_id: String,
}
pub struct ReadEnvironment<'a, 'credentials> {
    pub store: &'a ProfileStore,
    pub client: &'a ServiceOAuthClient,
    pub locked: &'a LockedCredentials<'credentials>,
    pub tls: TlsPolicy,
    pub control: InvocationControl,
}
impl PreparedCommentRead {
    pub fn prepare(
        request: CanonicalRequest,
        request_id: &str,
        policy: IssueReadPolicy,
    ) -> Result<Self, CraftError> {
        parse_object(&serde_json::to_vec(&request.input).map_err(|_| invalid())?)?;
        craft_core::contracts::validate_identity("/request_id", request_id)?;
        craft_core::contracts::validate_identity("/actor", &policy.expected_actor)?;
        policy.grant.validate()?;
        if request_id.len() > 80
            || policy.target != request.target
            || matches!(policy.expected_actor.as_str(), "anonymous" | "unknown")
        {
            return Err(denied(
                "Comment read differs from its trusted site, grant or actor.",
            ));
        }
        let registry = registry()?;
        let operation = registry.prepare(request)?;
        let projection = registry
            .projection(
                &operation.request().command,
                &operation.request().selection.fields,
            )?
            .ok_or_else(invalid)?;
        let descriptor = registry.prepared_descriptor(&operation)?;
        let binding = ListBinding {
            operation: operation.request().command.clone(),
            descriptor_digest: canonical_digest(&json!(descriptor))?,
            schema_digest: registry.schema_digest(&operation.request().command, true)?,
            projection_digest: canonical_digest(
                &projection.response_schema(&descriptor.response_schema)?,
            )?,
            api_revision: descriptor.api_revision.clone(),
            target: operation.request().target.clone(),
            grant: policy.grant,
            actor: policy.expected_actor,
            query: BTreeMap::from([("input".into(), operation.request().input.clone())]),
        };
        binding.digest()?;
        let host = Self {
            registry,
            operation,
            projection,
            binding,
            request_id: request_id.into(),
        };
        let cache = Some(CredentialCacheUpdate {
            grant: host.binding.grant.clone(),
            write_state: RequestState::SentUnknown,
            cleanup_pending: Some(false),
        });
        let mut error = CraftError::new(ErrorCode::Internal, "x".repeat(500));
        error.next_action = "x".repeat(1000);
        if !host.fits(&host.envelope(None, None, None, cache.clone(), Some(error))?)? {
            return Err(budget());
        }
        if host.binding.operation != native::GET
            && !host.fits(&host.envelope(None, Some(&recovery_probe()), None, cache, None)?)?
        {
            return Err(budget());
        }
        Ok(host)
    }
    fn list_request(&self) -> ListRequest<'_> {
        ListRequest {
            binding: &self.binding,
            cursor: self.operation.request().selection.cursor.as_deref(),
            limit: self.operation.request().selection.limit,
        }
    }
    fn fits(&self, envelope: &Envelope) -> Result<bool, CraftError> {
        Ok(serde_json::to_vec(envelope).map_err(|_| invalid())?.len()
            < self.operation.request().selection.max_bytes as usize)
    }
    fn validate_items(&self, items: &[Value]) -> Result<(), CraftError> {
        let mut ids = BTreeSet::new();
        for value in items {
            if [
                "id",
                "issue_id",
                "visibility",
                "jsd_public",
                "jsd_author_can_see_request",
                "body_state",
                "revision",
            ]
            .iter()
            .any(|key| value.get(key).is_none())
            {
                return Err(invalid());
            }
            let record: native::CommentRecord =
                serde_json::from_value(value.clone()).map_err(|_| invalid())?;
            if !native::numeric(&record.id)
                || record.issue_id != self.operation.request().input["issue_id"]
                || !ids.insert(record.id.clone())
                || (self.binding.operation == native::GET
                    && record.id != self.operation.request().input["comment_id"])
                || (record.body_state == native::BodyState::Returned) != record.body.is_some()
                || record.revision.as_ref().is_some_and(|s| {
                    !s.strip_prefix("sha256:")
                        .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
                })
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
    fn envelope(
        &self,
        item: Option<Value>,
        page: Option<&ListPage>,
        observation: Option<PageObservation>,
        cache: Option<CredentialCacheUpdate>,
        error: Option<CraftError>,
    ) -> Result<Envelope, CraftError> {
        let command = self.binding.operation.as_str();
        if let Some(item) = &item {
            self.validate_items(std::slice::from_ref(item))?;
        }
        if let Some(page) = page {
            self.validate_items(&page.items)?;
            let expected = if command == native::LIST {
                Some(
                    json!({"source_issue_id":self.operation.request().input["issue_id"],"coverage":"visible_comment_page"}),
                )
            } else {
                None
            };
            if page.context != expected && !(page.items.is_empty() && page.error.is_some()) {
                return Err(invalid());
            }
        }
        let mut data = if command == native::GET {
            json!({"target":self.binding.target,"comment":item,"observation":observation,"credential_cache_update":cache})
        } else {
            json!({"target":self.binding.target,"items":page.map(|p|p.items.as_slice()).unwrap_or(&[]),"observation":page.map(|p|&p.observation),"cached":page.is_some_and(|p|p.cached),"credential_cache_update":cache})
        };
        if command == native::LIST {
            data["source_issue_id"] = self.operation.request().input["issue_id"].clone();
        }
        let data = self.projection.data(&data)?;
        let mut envelope = match error.or_else(|| page.and_then(|p| p.error.clone())) {
            Some(error) => {
                Envelope::failure_with_data(command, self.request_id.clone(), error, data)
            }
            None => Envelope::success(command, self.request_id.clone(), data),
        };
        envelope.page = page.map(|p| p.page.clone());
        Ok(envelope)
    }
    fn finish(
        &self,
        envelope: Result<Envelope, CraftError>,
        cache: Option<CredentialCacheUpdate>,
    ) -> CommandResult {
        let envelope = envelope.unwrap_or_else(|error| {
            self.envelope(None, None, None, cache.clone(), Some(error.clone()))
                .unwrap_or_else(|_| {
                    Envelope::failure_with_data(
                        &self.binding.operation,
                        self.request_id.clone(),
                        error,
                        json!({"credential_cache_update":cache}),
                    )
                })
        });
        CommandResult {
            envelope,
            output_policy: if cache.is_some() {
                OutputPolicy::PreserveEvidence
            } else {
                OutputPolicy::Bounded
            },
        }
    }
    fn check(
        &self,
        access: &VerifiedServiceAccess,
        control: &InvocationControl,
        policy: &dyn Fn() -> Result<(), CraftError>,
    ) -> Result<(), CraftError> {
        control.check()?;
        policy()?;
        if access.tenant() != &self.binding.target.tenant_ref()
            || access.grant() != &self.binding.grant
            || access.identity().actor() != self.binding.actor
        {
            return Err(denied(
                "Comment read access differs from the exact site, grant or actor.",
            ));
        }
        access.check_operation(&self.registry, &self.operation)
    }
    pub fn execute(
        &self,
        environment: ReadEnvironment<'_, '_>,
        policy: impl Fn() -> Result<(), CraftError>,
    ) -> CommandResult {
        let ReadEnvironment {
            store,
            client,
            locked,
            tls,
            control,
        } = environment;
        let before = (|| {
            control.check()?;
            policy()?;
            if locked.grant() != self.binding.grant {
                return Err(denied("Comment read locked grant mismatch."));
            }
            if self.binding.operation != native::GET {
                store.check_provider_cursor(
                    &self.list_request(),
                    client.clock().as_ref(),
                    &control,
                )?;
            }
            policy()?;
            control.check()
        })();
        if let Err(error) = before {
            return self.finish(Err(error), None);
        }
        let client = match client.for_invocation(control.clone()) {
            Ok(client) => client,
            Err(error) => return self.finish(Err(error), None),
        };
        let access = match client.open_service_access(locked, &self.binding.target.tenant_ref()) {
            Ok(access) => access,
            Err(failure) => {
                let cache = failure
                    .credential_update
                    .map(|r| CredentialCacheUpdate {
                        grant: locked.grant(),
                        write_state: RequestState::Confirmed,
                        cleanup_pending: Some(r.cleanup_pending),
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
                return self.finish(Err(error), cache);
            }
        };
        let cache = access.cache_update().map(|r| CredentialCacheUpdate {
            grant: access.grant().clone(),
            write_state: RequestState::Confirmed,
            cleanup_pending: Some(r.cleanup_pending),
        });
        let result = (|| {
            let check = || self.check(&access, &control, &policy);
            check()?;
            if self.binding.operation == native::GET {
                let mut context =
                    access.read_context(&self.registry, &self.operation, tls, control.clone())?;
                let mut guard = Guard {
                    native: &mut context,
                    check: &check,
                };
                let page = native::read_page(&self.operation, &mut guard, None)?;
                let observation = PageObservation {
                    observed_at: access.clock().unix_millis(),
                    page_digest: canonical_digest(&json!(page))?,
                };
                let item = page.items.into_iter().next().ok_or_else(invalid)?;
                let envelope =
                    self.envelope(Some(item), None, Some(observation), cache.clone(), None)?;
                if !self.fits(&envelope)? {
                    return Err(CraftError::new(
                        ErrorCode::ItemTooLarge,
                        "Selected comment cannot fit; select fewer fields or increase --max-bytes for a new live read.",
                    ));
                }
                check()?;
                return Ok(envelope);
            }
            let mut source = Source {
                host: self,
                access: &access,
                tls,
                policy: &policy,
            };
            let page = store.provider_page(
                self.list_request(),
                access.clock().as_ref(),
                &control,
                &mut source,
                |p| self.fits(&self.envelope(None, Some(p), None, cache.clone(), None)?),
            )?;
            let envelope = self.envelope(None, Some(&page), None, cache.clone(), None)?;
            check()?;
            Ok(envelope)
        })();
        self.finish(result, cache)
    }
}
struct Source<'a> {
    host: &'a PreparedCommentRead,
    access: &'a VerifiedServiceAccess,
    tls: TlsPolicy,
    policy: &'a dyn Fn() -> Result<(), CraftError>,
}
impl PageSource for Source<'_> {
    fn check_access(
        &mut self,
        binding: &ListBinding,
        control: &InvocationControl,
    ) -> Result<(), CraftError> {
        if binding != &self.host.binding {
            return Err(denied("Comment cursor binding changed."));
        }
        self.host.check(self.access, control, self.policy)
    }
    fn fetch(
        &mut self,
        binding: &ListBinding,
        position: Option<&NativePosition>,
        control: &InvocationControl,
    ) -> Result<NativePage, CraftError> {
        self.check_access(binding, control)?;
        let check = || self.host.check(self.access, control, self.policy);
        let mut context = self.access.read_context(
            &self.host.registry,
            &self.host.operation,
            self.tls.clone(),
            control.clone(),
        )?;
        let mut guard = Guard {
            native: &mut context,
            check: &check,
        };
        native::read_page(&self.host.operation, &mut guard, position)
    }
}
