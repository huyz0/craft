//! Selected Confluence reads through sealed service access and the existing buffered paging engine.

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
use craft_provider_confluence::{
    self as provider, ConfluenceContext, GetInput, PAGE_GET, PAGE_LIST, PAGE_SEARCH, PageListInput,
    SPACE_GET, SPACE_LIST, SearchInput, SpaceListInput,
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
pub mod inventory;

/// Trusted host authority for these operations, independent of project-discovery records.
pub struct ConfluenceReadPolicy {
    pub target: ResourceRef,
    pub grant: GrantRef,
    pub expected_actor: String,
    pub enabled: bool,
    pub allowed_commands: Vec<String>,
}
/// Purely prepared internal read; this type does not register a public command or provision access.
pub struct PreparedConfluenceRead {
    registry: Registry,
    operation: PreparedOperation,
    projection: CompiledProjection,
    binding: ListBinding,
    request_id: String,
}
enum ReadData<'a> {
    Get { item: Value, observed_at: u64 },
    List(&'a ListPage),
}
fn internal() -> CraftError {
    CraftError::new(
        ErrorCode::Internal,
        "Cannot encode qualified Confluence-read output.",
    )
}
fn budget() -> CraftError {
    let mut error = CraftError::new(
        ErrorCode::OutputBudgetTooSmall,
        "Confluence reads require room for complete error, paging and credential-cache recovery evidence.",
    );
    error.next_action = "Increase --max-bytes before authentication.".into();
    error
}
impl PreparedConfluenceRead {
    /// Validates the exact trusted policy, native inputs, field plan and complete recovery budget.
    /// Preparation performs no state, credential or network access and grants no new authority.
    pub fn prepare(
        request: CanonicalRequest,
        request_id: &str,
        policy: ConfluenceReadPolicy,
    ) -> Result<Self, CraftError> {
        parse_object(&serde_json::to_vec(&request.input).map_err(|_| internal())?)?;
        validate_identity("/request_id", request_id)?;
        validate_identity("/expected_actor", &policy.expected_actor)?;
        policy.grant.validate()?;
        if !policy.enabled
            || !policy
                .allowed_commands
                .iter()
                .any(|c| c == &request.command)
            || policy.allowed_commands.is_empty()
            || policy.allowed_commands.len() > 16
            || policy
                .allowed_commands
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != policy.allowed_commands.len()
            || policy.allowed_commands.iter().any(|c| {
                !matches!(
                    c.as_str(),
                    PAGE_GET
                        | PAGE_LIST
                        | PAGE_SEARCH
                        | SPACE_GET
                        | SPACE_LIST
                        | craft_provider_confluence::comments::COMMENT_GET
                        | craft_provider_confluence::comments::paging::COMMENT_LIST
                        | craft_provider_confluence::comments::paging::COMMENT_REPLIES
                        | craft_provider_confluence::content_state::CONTENT_STATE_GET
                        | craft_provider_confluence::content_state::CONTENT_STATE_AVAILABLE
                        | craft_provider_confluence::attachments::ATTACHMENT_GET
                        | craft_provider_confluence::attachments::ATTACHMENT_LIST
                        | craft_provider_confluence::attachment_download::DOWNLOAD
                        | craft_provider_confluence::content::FOLDER_GET
                        | craft_provider_confluence::content::WHITEBOARD_GET
                )
            })
            || !policy.grant.id.strip_prefix("gr_").is_some_and(|suffix| {
                suffix.len() == 32
                    && suffix
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
            || matches!(policy.expected_actor.as_str(), "anonymous" | "unknown")
            || policy.target != request.target
        {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Confluence read differs from the exact configured site, grant or expected actor.",
            ));
        }
        let mut registry = Registry::default();
        registry.register_provider(&ConfluenceContext)?;
        let operation = registry.prepare(request)?;
        let command = &operation.request().command;
        let projection = registry
            .projection(command, &operation.request().selection.fields)?
            .ok_or_else(internal)?;
        let effective_input =
            match command.as_str() {
                craft_provider_confluence::comments::paging::COMMENT_LIST => serde_json::to_value(
                    serde_json::from_value::<
                        craft_provider_confluence::comments::paging::CommentListInput,
                    >(operation.request().input.clone())
                    .map_err(|_| internal())?,
                ),
                craft_provider_confluence::comments::paging::COMMENT_REPLIES => {
                    serde_json::to_value(
                        serde_json::from_value::<
                            craft_provider_confluence::comments::paging::CommentRepliesInput,
                        >(operation.request().input.clone())
                        .map_err(|_| internal())?,
                    )
                }
                craft_provider_confluence::comments::COMMENT_GET => serde_json::to_value(
                    serde_json::from_value::<craft_provider_confluence::comments::CommentGetInput>(
                        operation.request().input.clone(),
                    )
                    .map_err(|_| internal())?,
                ),
                craft_provider_confluence::content_state::CONTENT_STATE_GET
                | craft_provider_confluence::content_state::CONTENT_STATE_AVAILABLE => {
                    serde_json::to_value(
                        serde_json::from_value::<
                            craft_provider_confluence::content_state::ContentStateInput,
                        >(operation.request().input.clone())
                        .map_err(|_| internal())?,
                    )
                }
                craft_provider_confluence::attachments::ATTACHMENT_GET => serde_json::to_value(
                    serde_json::from_value::<
                        craft_provider_confluence::attachments::AttachmentGetInput,
                    >(operation.request().input.clone())
                    .map_err(|_| internal())?,
                ),
                craft_provider_confluence::attachments::ATTACHMENT_LIST => serde_json::to_value(
                    serde_json::from_value::<
                        craft_provider_confluence::attachments::AttachmentListInput,
                    >(operation.request().input.clone())
                    .map_err(|_| internal())?,
                ),
                craft_provider_confluence::content::FOLDER_GET
                | craft_provider_confluence::content::WHITEBOARD_GET => serde_json::to_value(
                    serde_json::from_value::<craft_provider_confluence::content::ContentGetInput>(
                        operation.request().input.clone(),
                    )
                    .map_err(|_| internal())?,
                ),
                PAGE_GET | SPACE_GET => serde_json::to_value(
                    serde_json::from_value::<GetInput>(operation.request().input.clone())
                        .map_err(|_| internal())?,
                ),
                SPACE_LIST => serde_json::to_value(
                    serde_json::from_value::<SpaceListInput>(operation.request().input.clone())
                        .map_err(|_| internal())?,
                ),
                PAGE_LIST => serde_json::to_value(
                    serde_json::from_value::<PageListInput>(operation.request().input.clone())
                        .map_err(|_| internal())?,
                ),
                _ => serde_json::to_value(
                    serde_json::from_value::<SearchInput>(operation.request().input.clone())
                        .map_err(|_| internal())?,
                ),
            }
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
            binding,
            request_id: request_id.into(),
        };
        let cache = Some(CredentialCacheUpdate {
            grant: prepared.binding.grant.clone(),
            write_state: RequestState::SentUnknown,
            cleanup_pending: Some(false),
        });
        if !is_get(&prepared.binding.operation)
            && !prepared.fits(&prepared.envelope(
                Some(ReadData::List(&recovery_probe())),
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
        let data = if is_get(&self.binding.operation) {
            let (item, observed_at) = match result {
                Some(ReadData::Get { item, observed_at }) => (Some(item), Some(observed_at)),
                _ => (None, None),
            };
            json!({"target":self.binding.target,"item":item,"observed_at":observed_at,"credential_cache_update":cache})
        } else {
            let source = match result {
                Some(ReadData::List(source)) => Some(source),
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
                "Verified Confluence access differs from the bound site, grant or actor.",
            ));
        }
        access.check_operation(&self.registry, &self.operation)
    }
    /// Uses existing sealed access for one live get or one bounded search invocation.
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
        if is_get(&self.binding.operation) {
            let result = (|| {
                self.check_access(access, &control, &check_policy)?;
                let mut context =
                    access.read_context(&self.registry, &self.operation, tls, control.clone())?;
                let mut context = PolicyContext {
                    native: &mut context,
                    check: &check_policy,
                };
                let item = provider::read_get(&self.operation, &mut context)?;
                self.check_access(access, &control, &check_policy)?;
                let result = ReadData::Get {
                    item,
                    observed_at: access.clock().unix_millis(),
                };
                let envelope = self.envelope(Some(result), cache.clone(), None)?;
                if !self.fits(&envelope)? {
                    let mut error = CraftError::new(
                        ErrorCode::ItemTooLarge,
                        "The selected Confluence item cannot fit in the complete output envelope.",
                    );
                    error.next_action =
                        "Select fewer fields or increase --max-bytes and perform a new live read."
                            .into();
                    return Err(error);
                }
                // Timestamp and complete-envelope work may outlive the earlier access check.
                self.check_access(access, &control, &check_policy)?;
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
        let mut source = ConfluenceSource {
            prepared: self,
            access,
            tls,
            check_policy: &check_policy,
            advice: None,
        };
        let result = store.provider_page(
            self.request(),
            access.clock().as_ref(),
            &control,
            &mut source,
            |page| self.fits(&self.envelope(Some(ReadData::List(page)), cache.clone(), None)?),
        );
        match result {
            Ok(page) => match check_policy().and_then(|()| control.check()) {
                Ok(()) => self.finish(Some(ReadData::List(&page)), cache, None),
                Err(error) => self.finish(None, cache, Some(error)),
            },
            Err(error) => self.finish(None, cache, Some(error)),
        }
    }
    /// Opens verified access under the supplied locked grant, then executes without resource retries.
    /// Invalid search continuation fails before credential acquisition.
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
                    "Locked grant differs from the prepared Confluence-read binding.",
                ));
            }
            if !is_get(&self.binding.operation) {
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

impl PreparedConfluenceRead {
    pub fn binding(&self) -> &ListBinding {
        &self.binding
    }
    fn project_list(&self, items: &[Value]) -> Result<Vec<Value>, CraftError> {
        let page = ListPage {
            context: None,
            items: items.to_vec(),
            page: craft_core::contracts::Page {
                returned: items.len(),
                has_more: false,
                next_cursor: None,
                truncated: false,
                stop_reason: "exhausted".into(),
                consistency: "live".into(),
            },
            observation: craft_core::pagination::PageObservation {
                observed_at: 0,
                page_digest: format!("sha256:{}", "0".repeat(64)),
            },
            cached: false,
            error: None,
        };
        let envelope = self.envelope(Some(ReadData::List(&page)), None, None)?;
        envelope
            .data
            .as_ref()
            .and_then(|data| data["items"].as_array())
            .cloned()
            .ok_or_else(internal)
    }
    /// Lends the scan driver this listing's own source after opening verified access.
    pub fn scan_cached(
        &self,
        client: &ServiceOAuthClient,
        locked: &LockedCredentials<'_>,
        tls: TlsPolicy,
        control: InvocationControl,
        check_policy: impl Fn() -> Result<(), CraftError>,
        run: crate::scans::ScanRunner<'_>,
    ) -> (
        Result<craft_runtime::scan::Advanced, CraftError>,
        Option<CredentialCacheUpdate>,
    ) {
        if is_get(&self.binding.operation) {
            return (Err(internal()), None);
        }
        let before = (|| {
            control.check()?;
            check_policy()?;
            if locked.grant() != self.binding.grant {
                return Err(CraftError::new(
                    ErrorCode::PolicyDenied,
                    "Locked grant differs from the prepared Confluence read binding.",
                ));
            }
            Ok(())
        })();
        if let Err(error) = before {
            return (Err(error), None);
        }
        let (access, cache) = match crate::scans::open_access(
            client,
            locked,
            &self.binding.target.tenant_ref(),
            &control,
        ) {
            Ok(opened) => opened,
            Err((error, cache)) => return (Err(error), cache),
        };
        let advice = std::cell::Cell::new(None);
        let mut source = ConfluenceSource {
            prepared: self,
            access: &access,
            tls,
            check_policy: &check_policy,
            advice: Some(&advice),
        };
        let project = |items: &[Value]| self.project_list(items);
        let read_advice = || advice.get();
        let result = run(crate::scans::ScanSession {
            source: &mut source,
            binding: &self.binding,
            project: &project,
            rate_advice: &read_advice,
        });
        (result, cache)
    }
}

struct ConfluenceSource<'a> {
    prepared: &'a PreparedConfluenceRead,
    access: &'a VerifiedServiceAccess,
    tls: TlsPolicy,
    check_policy: &'a dyn Fn() -> Result<(), CraftError>,
    advice: Option<&'a std::cell::Cell<Option<craft_core::execution::RetryAfter>>>,
}
impl PageSource for ConfluenceSource<'_> {
    fn check_access(
        &mut self,
        binding: &ListBinding,
        control: &InvocationControl,
    ) -> Result<(), CraftError> {
        if binding != &self.prepared.binding {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Confluence traversal binding changed.",
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
        let mut context = PolicyContext {
            native: &mut context,
            check: self.check_policy,
        };
        match self.advice {
            Some(last) => provider::read_page(
                &self.prepared.operation,
                &mut crate::scans::AdviceRecorder {
                    inner: &mut context,
                    last,
                },
                position,
            ),
            None => provider::read_page(&self.prepared.operation, &mut context, position),
        }
    }
}

fn is_get(command: &str) -> bool {
    matches!(
        command,
        PAGE_GET
            | SPACE_GET
            | craft_provider_confluence::comments::COMMENT_GET
            | craft_provider_confluence::content_state::CONTENT_STATE_GET
            | craft_provider_confluence::content_state::CONTENT_STATE_AVAILABLE
            | craft_provider_confluence::attachments::ATTACHMENT_GET
            | craft_provider_confluence::content::FOLDER_GET
            | craft_provider_confluence::content::WHITEBOARD_GET
    )
}

struct PolicyContext<'a> {
    native: &'a mut dyn craft_core::execution::OperationContext,
    check: &'a dyn Fn() -> Result<(), CraftError>,
}
impl craft_core::execution::OperationContext for PolicyContext<'_> {
    fn checkpoint(&self) -> Result<(), CraftError> {
        (self.check)()?;
        self.native.checkpoint()
    }
    fn verify_operation(&self, op: &PreparedOperation) -> Result<(), CraftError> {
        self.checkpoint()?;
        self.native.verify_operation(op)
    }
    fn invoke(
        &mut self,
        call: &craft_core::execution::NativeCall,
    ) -> Result<craft_core::execution::NativeResponse, CraftError> {
        self.checkpoint()?;
        self.native.invoke(call)
    }
    fn normalize_next(
        &self,
        call: &craft_core::execution::NativeCall,
        link: &str,
    ) -> Result<craft_core::execution::NativeCall, CraftError> {
        self.checkpoint()?;
        self.native.normalize_next(call, link)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_core::contracts::Selection;
    fn target() -> ResourceRef {
        ResourceRef {
            provider: "confluence".into(),
            deployment: "cloud".into(),
            tenant: "00000000-1111-2222-3333-444444444444".into(),
            resource_type: "site".into(),
            id: "00000000-1111-2222-3333-444444444444".into(),
        }
    }
    fn policy(command: &str) -> ConfluenceReadPolicy {
        ConfluenceReadPolicy {
            target: target(),
            grant: GrantRef {
                id: format!("gr_{}", "0".repeat(32)),
            },
            expected_actor: "actor-one".into(),
            enabled: true,
            allowed_commands: vec![command.into()],
        }
    }
    fn request(command: &str, input: Value) -> CanonicalRequest {
        CanonicalRequest {
            command: command.into(),
            target: target(),
            input,
            selection: Selection::default(),
            preconditions: None,
        }
    }
    #[test]
    fn confluence_preparation_rejects_untrusted_authority_input_fields_and_recovery_budget()
    -> Result<(), Box<dyn std::error::Error>> {
        for command in [SPACE_GET, SPACE_LIST, PAGE_GET, PAGE_LIST, PAGE_SEARCH] {
            let input = match command {
                SPACE_GET | PAGE_GET => json!({"id":"100"}),
                PAGE_SEARCH => json!({"cql":"type = page"}),
                _ => json!({}),
            };
            for mode in [
                "disabled", "excluded", "actor", "grant", "target", "fields", "input", "budget",
            ] {
                let mut policy = policy(command);
                let mut request = request(command, input.clone());
                match mode {
                    "disabled" => policy.enabled = false,
                    "excluded" => policy.allowed_commands.clear(),
                    "actor" => policy.expected_actor = "anonymous".into(),
                    "grant" => policy.grant.id = "unissued".into(),
                    "target" => request.target.provider = "jira".into(),
                    "fields" => request.selection.fields = vec!["ownerId".into()],
                    "input" => request.input["expand"] = json!("*all"),
                    _ => request.selection.max_bytes = 512,
                };
                let error = PreparedConfluenceRead::prepare(request, "request", policy)
                    .err()
                    .ok_or("expected refusal")?;
                if mode == "budget" {
                    assert_eq!(error.code, ErrorCode::OutputBudgetTooSmall);
                }
            }
        }
        Ok(())
    }
    #[test]
    fn confluence_cursor_binding_uses_effective_defaults_and_equivalent_field_order()
    -> Result<(), Box<dyn std::error::Error>> {
        for command in [SPACE_LIST, PAGE_LIST, PAGE_SEARCH] {
            let input = if command == PAGE_SEARCH {
                json!({"cql":"type = page"})
            } else {
                json!({})
            };
            let minimal = PreparedConfluenceRead::prepare(
                request(command, input.clone()),
                "first",
                policy(command),
            )?;
            let mut explicit = input;
            explicit["page_size"] = json!(25);
            if command == PAGE_LIST {
                explicit["status"] = json!("current");
            }
            let explicit = PreparedConfluenceRead::prepare(
                request(command, explicit),
                "second",
                policy(command),
            )?;
            assert_eq!(minimal.binding, explicit.binding);
        }
        let mut first = request(PAGE_GET, json!({"id":"100"}));
        first.selection.fields = vec!["title".into(), "version".into()];
        let mut second = first.clone();
        second.selection.fields.reverse();
        assert_eq!(
            PreparedConfluenceRead::prepare(first, "first", policy(PAGE_GET))?.binding,
            PreparedConfluenceRead::prepare(second, "second", policy(PAGE_GET))?.binding
        );
        Ok(())
    }
}
