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
use craft_provider_confluence::attachments::ATTACHMENT_GET;
use craft_provider_confluence::comments::COMMENT_GET;
use craft_provider_confluence::content::{ContentKind, FOLDER_GET, WHITEBOARD_GET};
use craft_provider_confluence::content_state::CONTENT_STATE_AVAILABLE;
use craft_provider_confluence::pages::{
    self, ATTACHMENT_TRASH, AttachmentTrashInput, COMMENT_CREATE, COMMENT_DELETE, COMMENT_UPDATE,
    CONTENT_STATE_CLEAR, CONTENT_STATE_SET, CommentCreateInput, CommentDeleteInput,
    CommentUpdateInput, ConfluencePages, ContentCreateInput, ContentPlacement,
    ContentStateClearInput, ContentStateSetInput, ContentTrashInput, FOLDER_CREATE, FOLDER_TRASH,
    PAGE_CREATE, PAGE_TRASH, PAGE_UPDATE, PAGE_WIDTH_OBSERVE, PAGE_WIDTH_SET, PageCreateInput,
    PagePlacement, PageTrashInput, PageUpdateInput, PageWidthSetInput, PageWriteKind,
    PreparedPageWrite, SPACE_CREATE, SpaceCreateInput, WHITEBOARD_CREATE, WHITEBOARD_TRASH,
    WidthProperty,
};
use craft_provider_confluence::{ConfluenceContext, PAGE_GET, SPACE_GET, SPACE_LIST};
use craft_runtime::auth::{ServiceOAuthClient, VerifiedServiceAccess};
use craft_runtime::journal::{MutationAuthority, NativeJournal};
use craft_runtime::secrets::LockedCredentials;
use craft_runtime::transport::TlsPolicy;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Trusted caller authority. Read permissions grant no write operation; each write is listed.
pub struct PageWritePolicy {
    pub target: ResourceRef,
    pub space_id: String,
    pub grant: GrantRef,
    pub expected_actor: String,
    pub enabled: bool,
    pub allowed: Vec<PageWriteKind>,
}
enum Intent {
    Create(PageCreateInput),
    Update(PageUpdateInput),
    Trash(PageTrashInput),
    CommentCreate(CommentCreateInput),
    CommentUpdate(CommentUpdateInput),
    CommentDelete(CommentDeleteInput),
    ContentStateSet(ContentStateSetInput),
    ContentStateClear(ContentStateClearInput),
    AttachmentTrash(AttachmentTrashInput),
    SpaceCreate(SpaceCreateInput),
    /// Folder or whiteboard create; the kind selects the native type.
    ContentCreate(PageWriteKind, ContentCreateInput),
    ContentTrash(PageWriteKind, ContentTrashInput),
    WidthSet(PageWidthSetInput),
}
fn content_kind(kind: PageWriteKind) -> Result<ContentKind, CraftError> {
    pages::content_kind(kind).ok_or_else(denied)
}
impl Intent {
    fn decode(kind: PageWriteKind, input: &Value) -> Result<Self, CraftError> {
        Ok(match kind {
            PageWriteKind::Create => Self::Create(decode(input)?),
            PageWriteKind::Update => Self::Update(decode(input)?),
            PageWriteKind::Trash => Self::Trash(decode(input)?),
            PageWriteKind::CommentCreate => Self::CommentCreate(decode(input)?),
            PageWriteKind::CommentUpdate => Self::CommentUpdate(decode(input)?),
            PageWriteKind::CommentDelete => Self::CommentDelete(decode(input)?),
            PageWriteKind::ContentStateSet => Self::ContentStateSet(decode(input)?),
            PageWriteKind::ContentStateClear => Self::ContentStateClear(decode(input)?),
            PageWriteKind::AttachmentTrash => Self::AttachmentTrash(decode(input)?),
            PageWriteKind::SpaceCreate => Self::SpaceCreate(decode(input)?),
            PageWriteKind::FolderCreate | PageWriteKind::WhiteboardCreate => {
                Self::ContentCreate(kind, decode(input)?)
            }
            PageWriteKind::FolderTrash | PageWriteKind::WhiteboardTrash => {
                Self::ContentTrash(kind, decode(input)?)
            }
            PageWriteKind::WidthSet => Self::WidthSet(decode(input)?),
        })
    }
    fn kind(&self) -> PageWriteKind {
        match self {
            Self::Create(_) => PageWriteKind::Create,
            Self::Update(_) => PageWriteKind::Update,
            Self::Trash(_) => PageWriteKind::Trash,
            Self::CommentCreate(_) => PageWriteKind::CommentCreate,
            Self::CommentUpdate(_) => PageWriteKind::CommentUpdate,
            Self::CommentDelete(_) => PageWriteKind::CommentDelete,
            Self::ContentStateSet(_) => PageWriteKind::ContentStateSet,
            Self::ContentStateClear(_) => PageWriteKind::ContentStateClear,
            Self::AttachmentTrash(_) => PageWriteKind::AttachmentTrash,
            Self::SpaceCreate(_) => PageWriteKind::SpaceCreate,
            Self::ContentCreate(kind, _) | Self::ContentTrash(kind, _) => *kind,
            Self::WidthSet(_) => PageWriteKind::WidthSet,
        }
    }
    fn space_id(&self) -> &str {
        match self {
            Self::Create(i) => &i.space_id,
            Self::Update(i) => &i.space_id,
            Self::Trash(i) => &i.space_id,
            Self::CommentCreate(i) => &i.space_id,
            Self::CommentUpdate(i) => &i.space_id,
            Self::CommentDelete(i) => &i.space_id,
            Self::ContentStateSet(i) => &i.space_id,
            Self::ContentStateClear(i) => &i.space_id,
            Self::AttachmentTrash(i) => &i.space_id,
            Self::ContentCreate(_, i) => &i.space_id,
            Self::ContentTrash(_, i) => &i.space_id,
            Self::WidthSet(i) => &i.space_id,
            // Site-level: no existing space binds this write; authority is the site policy.
            Self::SpaceCreate(_) => "",
        }
    }
}
fn native_representation(value: pages::PageRepresentation) -> &'static str {
    match value {
        pages::PageRepresentation::Storage => "storage",
        pages::PageRepresentation::AtlasDocFormat => "atlas_doc_format",
    }
}
struct ReadSpec<'a> {
    command: &'a str,
    id: &'a str,
    /// The hosting page, required for footer-comment reads.
    page_id: Option<&'a str>,
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
        let kind = PageWriteKind::from_command(&operation.request().command)
            .ok_or_else(|| CraftError::field("/command", "expected_page_or_comment_write"))?;
        let intent = Intent::decode(kind, &operation.request().input)?;
        if !apply {
            return Err(CraftError::new(
                ErrorCode::ApplyRequired,
                "Page publication requires explicit apply.",
            ));
        }
        let space_id = intent.space_id();
        let allowed = policy.allowed.contains(&kind);
        validate_identity("/expected_actor", &policy.expected_actor)?;
        if !policy.enabled
            || !allowed
            || policy.target != operation.request().target
            || policy.space_id != space_id
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
        page_id: Option<&str>,
        representation: Option<&str>,
    ) -> Result<PreparedOperation, CraftError> {
        self.registry.prepare(CanonicalRequest {
            command: command.into(),
            target: self.binding.target.clone(),
            input: match page_id {
                Some(page_id) => json!({"id":id,"page_id":page_id}),
                None if command == CONTENT_STATE_AVAILABLE => json!({"page_id":id}),
                None if command == SPACE_LIST => json!({"key":id,"page_size":2}),
                None => json!({"id":id}),
            },
            selection: Selection {
                fields: representation
                    .map(|r| vec![format!("body_{r}")])
                    .unwrap_or_default(),
                ..Selection::default()
            },
            preconditions: None,
        })
    }
    fn width_operation(
        &self,
        page_id: &str,
        property: WidthProperty,
    ) -> Result<PreparedOperation, CraftError> {
        self.registry.prepare(CanonicalRequest {
            command: PAGE_WIDTH_OBSERVE.into(),
            target: self.binding.target.clone(),
            input: json!({"page_id":page_id,"key":property}),
            selection: Selection::default(),
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
                    &self.read_operation(SPACE_GET, &input.space_id, None, None)?,
                )?;
                if !matches!(input.placement, PagePlacement::Root) {
                    access.check_operation(
                        &self.registry,
                        &self.read_operation(PAGE_GET, "1", None, None)?,
                    )?;
                }
            }
            Intent::Update(PageUpdateInput { id, .. })
            | Intent::Trash(PageTrashInput { id, .. }) => access.check_operation(
                &self.registry,
                &self.read_operation(PAGE_GET, id, None, None)?,
            )?,
            Intent::CommentCreate(CommentCreateInput {
                page_id,
                parent_comment_id,
                ..
            }) => {
                access.check_operation(
                    &self.registry,
                    &self.read_operation(PAGE_GET, page_id, None, None)?,
                )?;
                if let Some(parent) = parent_comment_id {
                    access.check_operation(
                        &self.registry,
                        &self.read_operation(COMMENT_GET, parent, Some(page_id), None)?,
                    )?;
                }
            }
            Intent::ContentStateSet(ContentStateSetInput { page_id, .. }) => {
                access.check_operation(
                    &self.registry,
                    &self.read_operation(PAGE_GET, page_id, None, None)?,
                )?;
                access.check_operation(
                    &self.registry,
                    &self.read_operation(CONTENT_STATE_AVAILABLE, page_id, None, None)?,
                )?;
            }
            Intent::ContentStateClear(ContentStateClearInput { page_id, .. }) => {
                access.check_operation(
                    &self.registry,
                    &self.read_operation(PAGE_GET, page_id, None, None)?,
                )?;
            }
            Intent::SpaceCreate(SpaceCreateInput { key, .. }) => {
                access.check_operation(
                    &self.registry,
                    &self.read_operation(SPACE_LIST, key, None, None)?,
                )?;
            }
            Intent::AttachmentTrash(AttachmentTrashInput { page_id, id, .. }) => {
                access.check_operation(
                    &self.registry,
                    &self.read_operation(PAGE_GET, page_id, None, None)?,
                )?;
                access.check_operation(
                    &self.registry,
                    &self.read_operation(ATTACHMENT_GET, id, Some(page_id), None)?,
                )?;
            }
            Intent::CommentUpdate(CommentUpdateInput { page_id, id, .. })
            | Intent::CommentDelete(CommentDeleteInput { page_id, id, .. }) => {
                access.check_operation(
                    &self.registry,
                    &self.read_operation(PAGE_GET, page_id, None, None)?,
                )?;
                access.check_operation(
                    &self.registry,
                    &self.read_operation(COMMENT_GET, id, Some(page_id), None)?,
                )?;
            }
            Intent::ContentCreate(_, input) => {
                access.check_operation(
                    &self.registry,
                    &self.read_operation(SPACE_GET, &input.space_id, None, None)?,
                )?;
                let parent = match &input.placement {
                    ContentPlacement::Root => None,
                    ContentPlacement::Page { id } => Some((PAGE_GET, id)),
                    ContentPlacement::Folder { id } => Some((FOLDER_GET, id)),
                };
                if let Some((command, id)) = parent {
                    access.check_operation(
                        &self.registry,
                        &self.read_operation(command, id, None, None)?,
                    )?;
                }
            }
            Intent::ContentTrash(kind, input) => access.check_operation(
                &self.registry,
                &self.read_operation(content_kind(*kind)?.get_command(), &input.id, None, None)?,
            )?,
            Intent::WidthSet(PageWidthSetInput { page_id, .. }) => {
                access.check_operation(
                    &self.registry,
                    &self.read_operation(PAGE_GET, page_id, None, None)?,
                )?;
                for property in WidthProperty::ORDER {
                    access.check_operation(
                        &self.registry,
                        &self.width_operation(page_id, property)?,
                    )?;
                }
            }
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
        let steps = self.intent.kind().journal_steps();
        if prior.is_some_and(|p| {
            p.binding != self.binding
                || p.steps.iter().any(|(step, s)| {
                    !steps.contains(&step.as_str()) || s.state != SubmissionState::Prepared
                })
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
            page_id,
            representation,
        } = spec;
        check_policy()?;
        self.check_access(access)?;
        let operation = self.read_operation(command, id, page_id, representation)?;
        let mut context =
            access.read_context(&self.registry, &operation, tls.clone(), control.clone())?;
        let mut query = BTreeMap::new();
        if command == PAGE_GET {
            query.extend([
                ("status".into(), "current".into()),
                ("get-draft".into(), "false".into()),
                ("include-version".into(), "true".into()),
            ]);
        }
        if matches!(command, FOLDER_GET | WHITEBOARD_GET) {
            // Trash refuses non-empty items, so every content read lists direct children.
            query.insert("include-direct-children".into(), "true".into());
        }
        if command == COMMENT_GET || command == ATTACHMENT_GET {
            query.insert("include-version".into(), "true".into());
        }
        if let Some(r) = representation {
            query.insert("body-format".into(), r.into());
        }
        if command == SPACE_LIST {
            // Exact-key lookup: any visible space with this key stops the create.
            query.insert("keys".into(), id.into());
            query.insert("limit".into(), "2".into());
        }
        let (route, parameter) = match command {
            PAGE_GET => ("confluence-page-get", "id"),
            COMMENT_GET => ("confluence-footer-comment-get", "comment-id"),
            ATTACHMENT_GET => (craft_provider_confluence::attachments::GET_ROUTE, "id"),
            SPACE_LIST => ("confluence-space-list", "id"),
            FOLDER_GET => (ContentKind::Folder.get_route(), "id"),
            WHITEBOARD_GET => (ContentKind::Whiteboard.get_route(), "id"),
            CONTENT_STATE_AVAILABLE => (
                craft_provider_confluence::content_state::AVAILABLE_ROUTE,
                "id",
            ),
            _ => ("confluence-space-get", "id"),
        };
        let response = context.invoke(&NativeCall {
            route: route.into(),
            path_parameters: if command == SPACE_LIST {
                BTreeMap::new()
            } else {
                BTreeMap::from([(parameter.into(), id.into())])
            },
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
    /// Compiles the writes in send order: one for most intents, one per appearance property for
    /// a width write.
    fn qualify(
        &self,
        access: &VerifiedServiceAccess,
        tls: &TlsPolicy,
        control: &InvocationControl,
        check_policy: &impl Fn() -> Result<(), CraftError>,
    ) -> Result<Vec<PreparedPageWrite>, CraftError> {
        let Intent::WidthSet(PageWidthSetInput { page_id, .. }) = &self.intent else {
            return Ok(vec![self.qualify_one(
                access,
                tls,
                control,
                check_policy,
            )?]);
        };
        let page = self.page_evidence(access, tls, control, page_id, check_policy)?;
        let published = self.read_width(
            access,
            tls,
            control,
            page_id,
            WidthProperty::Published,
            check_policy,
        )?;
        let draft = self.read_width(
            access,
            tls,
            control,
            page_id,
            WidthProperty::Draft,
            check_policy,
        )?;
        pages::prepare_page_width_set(&self.operation, &page, &published, &draft)
    }
    /// Reads one appearance property by key under the same policy and access rechecks as `read`.
    fn read_width(
        &self,
        access: &VerifiedServiceAccess,
        tls: &TlsPolicy,
        control: &InvocationControl,
        page_id: &str,
        property: WidthProperty,
        check_policy: &impl Fn() -> Result<(), CraftError>,
    ) -> Result<NativeResponse, CraftError> {
        check_policy()?;
        self.check_access(access)?;
        let operation = self.width_operation(page_id, property)?;
        let mut context =
            access.read_context(&self.registry, &operation, tls.clone(), control.clone())?;
        let response = context.invoke(&pages::width_read_call(page_id, property))?;
        check_policy()?;
        self.check_access(access)?;
        if response.status != 200 {
            let mut error = CraftError::new(
                if response.status == 429 {
                    ErrorCode::RateLimited
                } else {
                    ErrorCode::RemoteUnavailable
                },
                "Required current page appearance evidence was not returned; no width write was sent.",
            );
            error.next_action =
                "Read the page with the same service identity before retrying preparation.".into();
            return Err(error);
        }
        Ok(response)
    }
    fn qualify_one(
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
                        page_id: None,
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
                                page_id: None,
                                representation: None,
                            },
                            check_policy,
                        )
                    })
                    .transpose()?;
                pages::prepare_create(&self.operation, &space, parent.as_ref())
            }
            Intent::Update(PageUpdateInput { id, expected, .. })
            | Intent::Trash(PageTrashInput { id, expected, .. }) => {
                let r = native_representation(expected.representation);
                let page = self.read(
                    access,
                    tls,
                    control,
                    ReadSpec {
                        command: PAGE_GET,
                        id,
                        page_id: None,
                        representation: Some(r),
                    },
                    check_policy,
                )?;
                if matches!(self.intent, Intent::Trash(_)) {
                    pages::prepare_trash(&self.operation, &page)
                } else {
                    pages::prepare_update(&self.operation, &page)
                }
            }
            Intent::CommentCreate(CommentCreateInput {
                page_id,
                parent_comment_id,
                ..
            }) => {
                let page = self.page_evidence(access, tls, control, page_id, check_policy)?;
                let parent = parent_comment_id
                    .as_deref()
                    .map(|id| {
                        self.read(
                            access,
                            tls,
                            control,
                            ReadSpec {
                                command: COMMENT_GET,
                                id,
                                page_id: Some(page_id),
                                representation: Some("storage"),
                            },
                            check_policy,
                        )
                    })
                    .transpose()?;
                pages::prepare_comment_create(&self.operation, &page, parent.as_ref())
            }
            Intent::CommentUpdate(CommentUpdateInput {
                page_id,
                id,
                expected,
                ..
            })
            | Intent::CommentDelete(CommentDeleteInput {
                page_id,
                id,
                expected,
                ..
            }) => {
                let page = self.page_evidence(access, tls, control, page_id, check_policy)?;
                let comment = self.read(
                    access,
                    tls,
                    control,
                    ReadSpec {
                        command: COMMENT_GET,
                        id,
                        page_id: Some(page_id),
                        representation: Some(&expected.representation),
                    },
                    check_policy,
                )?;
                if matches!(self.intent, Intent::CommentDelete(_)) {
                    pages::prepare_comment_delete(&self.operation, &page, &comment)
                } else {
                    pages::prepare_comment_update(&self.operation, &page, &comment)
                }
            }
            Intent::ContentStateSet(ContentStateSetInput { page_id, .. }) => {
                let page = self.page_evidence(access, tls, control, page_id, check_policy)?;
                let available = self.read(
                    access,
                    tls,
                    control,
                    ReadSpec {
                        command: CONTENT_STATE_AVAILABLE,
                        id: page_id,
                        page_id: None,
                        representation: None,
                    },
                    check_policy,
                )?;
                pages::prepare_content_state_set(&self.operation, &page, &available)
            }
            Intent::ContentStateClear(ContentStateClearInput { page_id, .. }) => {
                let page = self.page_evidence(access, tls, control, page_id, check_policy)?;
                pages::prepare_content_state_clear(&self.operation, &page)
            }
            Intent::SpaceCreate(SpaceCreateInput { key, .. }) => {
                let lookup = self.read(
                    access,
                    tls,
                    control,
                    ReadSpec {
                        command: SPACE_LIST,
                        id: key,
                        page_id: None,
                        representation: None,
                    },
                    check_policy,
                )?;
                pages::prepare_space_create(&self.operation, &lookup)
            }
            Intent::AttachmentTrash(AttachmentTrashInput { page_id, id, .. }) => {
                let page = self.page_evidence(access, tls, control, page_id, check_policy)?;
                let attachment = self.read(
                    access,
                    tls,
                    control,
                    ReadSpec {
                        command: ATTACHMENT_GET,
                        id,
                        page_id: Some(page_id),
                        representation: None,
                    },
                    check_policy,
                )?;
                pages::prepare_attachment_trash(&self.operation, &page, &attachment)
            }
            Intent::ContentCreate(_, input) => {
                let space = self.read(
                    access,
                    tls,
                    control,
                    ReadSpec {
                        command: SPACE_GET,
                        id: &input.space_id,
                        page_id: None,
                        representation: None,
                    },
                    check_policy,
                )?;
                let parent = match &input.placement {
                    ContentPlacement::Root => None,
                    ContentPlacement::Page { id } => Some((PAGE_GET, id.as_str())),
                    ContentPlacement::Folder { id } => Some((FOLDER_GET, id.as_str())),
                }
                .map(|(command, id)| {
                    self.read(
                        access,
                        tls,
                        control,
                        ReadSpec {
                            command,
                            id,
                            page_id: None,
                            representation: None,
                        },
                        check_policy,
                    )
                })
                .transpose()?;
                pages::prepare_content_create(&self.operation, &space, parent.as_ref())
            }
            Intent::ContentTrash(kind, input) => {
                let item = self.read(
                    access,
                    tls,
                    control,
                    ReadSpec {
                        command: content_kind(*kind)?.get_command(),
                        id: &input.id,
                        page_id: None,
                        representation: None,
                    },
                    check_policy,
                )?;
                pages::prepare_content_trash(&self.operation, &item)
            }
            Intent::WidthSet(_) => Err(denied()),
        }
    }
    fn page_evidence(
        &self,
        access: &VerifiedServiceAccess,
        tls: &TlsPolicy,
        control: &InvocationControl,
        page_id: &str,
        check_policy: &impl Fn() -> Result<(), CraftError>,
    ) -> Result<NativeResponse, CraftError> {
        self.read(
            access,
            tls,
            control,
            ReadSpec {
                command: PAGE_GET,
                id: page_id,
                page_id: None,
                representation: None,
            },
            check_policy,
        )
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
        let qualified = (|| {
            control.check()?;
            self.check_access(&access)?;
            check_policy()?;
            // Preserve the original observation age across every read and every journal claim
            // wait, including a width write's second property step.
            let freshness = Freshness::capture(access.clock());
            let writes = self.qualify(&access, &tls, &control, &check_policy)?;
            Ok((freshness, writes))
        })();
        let (freshness, writes) = match qualified {
            Ok(qualified) => qualified,
            Err(error) => return self.completion.claim_failure(error, prior.as_ref()),
        };
        // Claims and sends one compiled write under its own journal step.
        let send = |write: &PreparedPageWrite, step: &str| {
            freshness.check()?;
            check_policy()?;
            let mutation = access.prepare_mutation(
                &self.registry,
                &self.operation,
                tls.clone(),
                control.clone(),
                write.call(),
            )?;
            mutation.send_checked(
                &journal,
                &MutationAuthority {
                    apply: true,
                    operation_id: &self.binding.operation_id,
                    grant: access.grant(),
                    actor: access.identity().actor(),
                },
                step,
                || {
                    self.check_access(&access)?;
                    if write.operation_digest() != self.operation.digest() {
                        return Err(denied());
                    }
                    freshness.check()?;
                    check_policy()
                },
            )
        };
        if !matches!(self.intent, Intent::WidthSet(_)) {
            let Ok([write]) = <[PreparedPageWrite; 1]>::try_from(writes) else {
                return self.completion.before_send(denied(), prior.as_ref());
            };
            return match send(&write, &write.call().route) {
                Ok(attempt) => self.completion.finish(&attempt, &write),
                Err(error) => self.completion.claim_failure(error, prior.as_ref()),
            };
        }
        // Each property step is claimed only after the previous one is confirmed and recorded;
        // an uncertain or refused step stops the operation and nothing is ever resent.
        let mut evidence = Vec::new();
        let mut stopped = None;
        for write in &writes {
            let Some(step) = write.width_property() else {
                stopped = Some(denied());
                break;
            };
            match send(write, step) {
                Ok(attempt) => {
                    let recorded = self.completion.record_step(&attempt, write);
                    let next = recorded.allows_next();
                    evidence.push(recorded);
                    if !next {
                        break;
                    }
                }
                Err(error) => {
                    stopped = Some(error);
                    break;
                }
            }
        }
        if evidence.is_empty() {
            return self
                .completion
                .claim_failure(stopped.unwrap_or_else(denied), prior.as_ref());
        }
        self.completion.finish_steps(evidence, stopped)
    }
}
