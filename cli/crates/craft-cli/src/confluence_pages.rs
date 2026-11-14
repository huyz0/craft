//! Internal page publication through sealed service access and the existing durable send journal.
pub mod command;
mod completion;
#[cfg(test)]
mod tests;
use crate::dispatch::CommandResult;
use completion::Completion;
pub use completion::{PublicationData, schema};
use craft_core::contracts::{
    CanonicalRequest, GrantRef, ResourceRef, Selection, validate_identity,
};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{NativeCall, NativeResponse, OperationContext, PreparedOperation};
use craft_core::registry::{ProviderModule, Registry};
use craft_core::storage::{
    Clock, JournalRecord, OperationBinding, OperationJournal, SubmissionState,
};
use craft_provider_confluence::pages::{
    self, ConfluencePages, PAGE_CREATE, PAGE_UPDATE, PageCreateInput, PagePlacement,
    PageUpdateInput, PreparedPageWrite,
};
use craft_provider_confluence::{ConfluenceContext, PAGE_GET, SPACE_GET};
use craft_runtime::auth::{ServiceOAuthClient, VerifiedServiceAccess};
use craft_runtime::journal::{MutationAuthority, NativeJournal};
use craft_runtime::secrets::LockedCredentials;
use craft_runtime::transport::TlsPolicy;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Trusted caller authority. Read permissions do not grant either write operation.
pub struct PageWritePolicy {
    pub target: ResourceRef,
    pub space_id: String,
    pub grant: GrantRef,
    pub expected_actor: String,
    pub enabled: bool,
    pub allow_create: bool,
    pub allow_update: bool,
}
enum Intent {
    Create(PageCreateInput),
    Update(PageUpdateInput),
}
struct ReadSpec<'a> {
    command: &'a str,
    id: &'a str,
    representation: Option<&'a str>,
}
pub struct PreparedPagePublication {
    registry: Registry,
    operation: PreparedOperation,
    binding: OperationBinding,
    intent: Intent,
    completion: Completion,
}
fn denied() -> CraftError {
    CraftError::new(
        ErrorCode::PolicyDenied,
        "Page publication is outside the exact trusted site/space/grant/actor policy.",
    )
}
fn registry() -> Result<Registry, CraftError> {
    let mut registry = Registry::default();
    registry.register_provider(&ConfluenceContext)?;
    ConfluencePages.register(&mut registry)?;
    Ok(registry)
}
fn decode<T: serde::de::DeserializeOwned>(v: &Value) -> Result<T, CraftError> {
    serde_json::from_value(v.clone())
        .map_err(|_| CraftError::field("/input", "invalid_page_publication_input"))
}
struct Freshness {
    clock: Arc<dyn Clock>,
    wall: u64,
    monotonic: u64,
}
impl Freshness {
    fn capture(clock: Arc<dyn Clock>) -> Self {
        Self {
            wall: clock.unix_millis(),
            monotonic: clock.monotonic_millis(),
            clock,
        }
    }
    fn check(&self) -> Result<(), CraftError> {
        if self
            .clock
            .unix_millis()
            .checked_sub(self.wall)
            .is_none_or(|age| age >= 30_000)
            || self
                .clock
                .monotonic_millis()
                .checked_sub(self.monotonic)
                .is_none_or(|age| age >= 30_000)
        {
            return Err(CraftError::new(
                ErrorCode::VersionConflict,
                "Page evidence expired or its clock moved backward; fetch and review it again.",
            ));
        }
        Ok(())
    }
}
impl PreparedPagePublication {
    pub fn binding(&self) -> &OperationBinding {
        &self.binding
    }
    /// Validates local input, exact authority, apply and recovery-output capacity before state/auth.
    pub fn prepare(
        request: CanonicalRequest,
        operation_id: &str,
        apply: bool,
        request_id: &str,
        policy: PageWritePolicy,
    ) -> Result<Self, CraftError> {
        let encoded = serde_json::to_vec(&request.input)
            .map_err(|_| CraftError::field("/input", "invalid_page_input"))?;
        craft_core::input::parse_object(&encoded)?;
        let registry = registry()?;
        let operation = registry.prepare(request)?;
        let intent = match operation.request().command.as_str() {
            PAGE_CREATE => Intent::Create(decode(&operation.request().input)?),
            PAGE_UPDATE => Intent::Update(decode(&operation.request().input)?),
            _ => {
                return Err(CraftError::field(
                    "/command",
                    "expected_page_create_or_update",
                ));
            }
        };
        if !apply {
            return Err(CraftError::new(
                ErrorCode::ApplyRequired,
                "Page publication requires explicit apply.",
            ));
        }
        let (space_id, allowed) = match &intent {
            Intent::Create(i) => (&i.space_id, policy.allow_create),
            Intent::Update(i) => (&i.space_id, policy.allow_update),
        };
        validate_identity("/expected_actor", &policy.expected_actor)?;
        if !policy.enabled
            || !allowed
            || policy.target != operation.request().target
            || &policy.space_id != space_id
            || matches!(policy.expected_actor.as_str(), "unknown" | "anonymous")
        {
            return Err(denied());
        }
        let binding =
            operation.mutation_binding(operation_id, &policy.grant, &policy.expected_actor)?;
        let completion = Completion::prepare(&operation, binding.clone(), space_id, request_id)?;
        Ok(Self {
            registry,
            operation,
            binding,
            intent,
            completion,
        })
    }
    fn read_operation(
        &self,
        command: &str,
        id: &str,
        representation: Option<&str>,
    ) -> Result<PreparedOperation, CraftError> {
        self.registry.prepare(CanonicalRequest {
            command: command.into(),
            target: self.binding.target.clone(),
            input: json!({"id":id}),
            selection: Selection {
                fields: representation
                    .map(|r| vec![format!("body_{r}")])
                    .unwrap_or_default(),
                ..Selection::default()
            },
            preconditions: None,
        })
    }
    fn check_access(&self, access: &VerifiedServiceAccess) -> Result<(), CraftError> {
        if access.tenant() != &self.binding.target.tenant_ref()
            || access.grant() != &self.binding.grant
            || access.identity().actor() != self.binding.actor
        {
            return Err(denied());
        }
        access.check_operation(&self.registry, &self.operation)?;
        // Check every needed scope before any resource read, even when placement is resolved later.
        match &self.intent {
            Intent::Create(input) => {
                access.check_operation(
                    &self.registry,
                    &self.read_operation(SPACE_GET, &input.space_id, None)?,
                )?;
                if !matches!(input.placement, PagePlacement::Root) {
                    access.check_operation(
                        &self.registry,
                        &self.read_operation(PAGE_GET, "1", None)?,
                    )?;
                }
            }
            Intent::Update(input) => access.check_operation(
                &self.registry,
                &self.read_operation(PAGE_GET, &input.id, None)?,
            )?,
        }
        Ok(())
    }
    fn prior(&self, journal: &NativeJournal) -> Result<Option<JournalRecord>, CraftError> {
        journal.get(&self.binding.operation_id).map_err(|_| {
            CraftError::new(
                ErrorCode::Internal,
                "Cannot inspect prior page-write authority; no resource request was sent.",
            )
        })
    }
    fn check_prior(&self, prior: Option<&JournalRecord>) -> Result<(), CraftError> {
        let route = if matches!(self.intent, Intent::Create(_)) {
            pages::CREATE_ROUTE
        } else {
            pages::UPDATE_ROUTE
        };
        if prior.is_some_and(|p| {
            p.binding != self.binding
                || p.steps
                    .iter()
                    .any(|(step, s)| step != route || s.state != SubmissionState::Prepared)
        }) {
            return Err(CraftError::new(
                ErrorCode::OperationIdReused,
                "The operation identity changed or a page write already started. Inspect its receipt; this invocation grants no replay authority.",
            ));
        }
        Ok(())
    }
    fn read(
        &self,
        access: &VerifiedServiceAccess,
        tls: &TlsPolicy,
        control: &InvocationControl,
        spec: ReadSpec<'_>,
        check_policy: &impl Fn() -> Result<(), CraftError>,
    ) -> Result<NativeResponse, CraftError> {
        let ReadSpec {
            command,
            id,
            representation,
        } = spec;
        check_policy()?;
        self.check_access(access)?;
        let operation = self.read_operation(command, id, representation)?;
        let mut context =
            access.read_context(&self.registry, &operation, tls.clone(), control.clone())?;
        let mut query = BTreeMap::new();
        if command == PAGE_GET {
            query.extend([
                ("status".into(), "current".into()),
                ("get-draft".into(), "false".into()),
                ("include-version".into(), "true".into()),
            ]);
            if let Some(r) = representation {
                query.insert("body-format".into(), r.into());
            }
        }
        let response = context.invoke(&NativeCall {
            route: if command == PAGE_GET {
                "confluence-page-get"
            } else {
                "confluence-space-get"
            }
            .into(),
            path_parameters: BTreeMap::from([("id".into(), id.into())]),
            query,
            repeated_query: BTreeMap::new(),
            body: None,
        })?;
        check_policy()?;
        self.check_access(access)?;
        if response.status != 200 {
            let mut error = CraftError::new(
                if response.status == 429 {
                    ErrorCode::RateLimited
                } else {
                    ErrorCode::RemoteUnavailable
                },
                "Required current Confluence evidence was not returned; no page write was sent.",
            );
            error.next_action = "Read the selected space/page with the same service identity before retrying preparation.".into();
            return Err(error);
        }
        Ok(response)
    }
    fn qualify(
        &self,
        access: &VerifiedServiceAccess,
        tls: &TlsPolicy,
        control: &InvocationControl,
        check_policy: &impl Fn() -> Result<(), CraftError>,
    ) -> Result<PreparedPageWrite, CraftError> {
        match &self.intent {
            Intent::Create(input) => {
                let space = self.read(
                    access,
                    tls,
                    control,
                    ReadSpec {
                        command: SPACE_GET,
                        id: &input.space_id,
                        representation: None,
                    },
                    check_policy,
                )?;
                let parent_id = match &input.placement {
                    PagePlacement::Root => None,
                    PagePlacement::Parent { id } => Some(id.as_str()),
                    PagePlacement::Homepage => Some(
                        space
                            .body
                            .get("homepageId")
                            .and_then(Value::as_str)
                            .ok_or_else(|| {
                                CraftError::new(
                                    ErrorCode::RemoteUnavailable,
                                    "The selected space returned no homepage identity.",
                                )
                            })?,
                    ),
                };
                let parent = parent_id
                    .map(|id| {
                        self.read(
                            access,
                            tls,
                            control,
                            ReadSpec {
                                command: PAGE_GET,
                                id,
                                representation: None,
                            },
                            check_policy,
                        )
                    })
                    .transpose()?;
                pages::prepare_create(&self.operation, &space, parent.as_ref())
            }
            Intent::Update(input) => {
                let r = match input.expected.representation {
                    pages::PageRepresentation::Storage => "storage",
                    pages::PageRepresentation::AtlasDocFormat => "atlas_doc_format",
                };
                let page = self.read(
                    access,
                    tls,
                    control,
                    ReadSpec {
                        command: PAGE_GET,
                        id: &input.id,
                        representation: Some(r),
                    },
                    check_policy,
                )?;
                pages::prepare_update(&self.operation, &page)
            }
        }
    }
    /// Borrowing the locked grant keeps credentials serialized through auth, evidence and send.
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
        let before_auth = (|| {
            control.check()?;
            check_policy()?;
            if locked.grant() != self.binding.grant {
                return Err(denied());
            }
            prior = self.prior(&journal)?;
            self.check_prior(prior.as_ref())
        })();
        if let Err(error) = before_auth {
            return self.completion.before_send(error, prior.as_ref());
        }
        let access = match client.open_service_access(locked, &self.binding.target.tenant_ref()) {
            Ok(access) => access,
            Err(failure) => {
                self.completion.cache_failure(locked.grant(), &failure);
                let mut error = failure.error;
                error.details.write_state = craft_core::contracts::RequestState::NotSent;
                return self.completion.before_send(error, prior.as_ref());
            }
        };
        self.completion.cache_success(&access);
        let mut write = None;
        let result = (|| {
            control.check()?;
            self.check_access(&access)?;
            check_policy()?;
            // Preserve the original observation age across every read and the journal claim wait.
            let freshness = Freshness::capture(access.clock());
            write = Some(self.qualify(&access, &tls, &control, &check_policy)?);
            let frozen = write.as_ref().ok_or_else(denied)?;
            freshness.check()?;
            check_policy()?;
            let mutation = access.prepare_mutation(
                &self.registry,
                &self.operation,
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
                &frozen.call().route,
                || {
                    self.check_access(&access)?;
                    if frozen.operation_digest() != self.operation.digest() {
                        return Err(denied());
                    }
                    freshness.check()?;
                    check_policy()
                },
            )
        })();
        match (result, write) {
            (Ok(attempt), Some(write)) => self.completion.finish(&attempt, &write),
            (Err(error), _) => self.completion.claim_failure(error, prior.as_ref()),
            _ => self.completion.before_send(denied(), prior.as_ref()),
        }
    }
}
