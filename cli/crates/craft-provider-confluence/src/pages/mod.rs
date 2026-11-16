//! Pure page publication preparation. The caller must supply verified fresh evidence and a journaled host.
mod attachment_writes;
mod comment_writes;
mod content_state_writes;
mod content_writes;
mod input;
mod registration;
mod result;
mod space_writes;
use crate::models::{numeric, text};
pub use attachment_writes::{
    ATTACHMENT_TRASH, ATTACHMENT_TRASH_ROUTE, AttachmentRevision, AttachmentTrashInput,
    prepare_attachment_trash,
};
pub use comment_writes::{
    COMMENT_CREATE, COMMENT_CREATE_ROUTE, COMMENT_DELETE, COMMENT_DELETE_ROUTE, COMMENT_UPDATE,
    COMMENT_UPDATE_ROUTE, CommentCreateInput, CommentDeleteInput, CommentUpdateInput,
    prepare_comment_create, prepare_comment_delete, prepare_comment_update,
};
pub use content_state_writes::{
    CONTENT_STATE_CLEAR, CONTENT_STATE_CLEAR_ROUTE, CONTENT_STATE_SET, CONTENT_STATE_SET_ROUTE,
    ContentStateClearInput, ContentStateSetInput, prepare_content_state_clear,
    prepare_content_state_set,
};
pub use content_writes::{
    ContentCreateInput, ContentPlacement, ContentRevision, ContentTrashInput, FOLDER_CREATE,
    FOLDER_CREATE_ROUTE, FOLDER_TRASH, FOLDER_TRASH_ROUTE, WHITEBOARD_CREATE,
    WHITEBOARD_CREATE_ROUTE, WHITEBOARD_TRASH, WHITEBOARD_TRASH_ROUTE, content_kind,
    prepare_content_create, prepare_content_trash,
};
use craft_core::contracts::{CanonicalRequest, EffectClass, ResourceRef};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{NativeCall, NativeResponse, PreparedOperation};
use craft_core::input::canonical_digest;
use craft_core::presence::InputField;
pub use input::{
    EditorDraftPolicy, PageBody, PageCreateInput, PagePlacement, PageRepresentation, PageRevision,
    PageTrashInput, PageUpdateInput,
};
pub use registration::ConfluencePages;
pub use result::PageWriteObservation;
use serde_json::{Value, json};
pub use space_writes::{
    SPACE_CREATE, SPACE_CREATE_ROUTE, SpaceCreateInput, SpaceVisibility, prepare_space_create,
};
use std::collections::BTreeMap;

pub const PAGE_CREATE: &str = "confluence.page.create";
pub const PAGE_UPDATE: &str = "confluence.page.update";
pub const PAGE_TRASH: &str = "confluence.page.trash";
pub const CREATE_ROUTE: &str = "confluence-page-create";
pub const UPDATE_ROUTE: &str = "confluence-page-update";
pub const TRASH_ROUTE: &str = "confluence-page-trash";

/// The compiled native effect on a page or one of its footer comments; interpretation depends on
/// it rather than on optional identity.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PageWriteKind {
    Create,
    Update,
    Trash,
    CommentCreate,
    CommentUpdate,
    CommentDelete,
    ContentStateSet,
    ContentStateClear,
    AttachmentTrash,
    SpaceCreate,
    FolderCreate,
    FolderTrash,
    WhiteboardCreate,
    WhiteboardTrash,
}
impl PageWriteKind {
    pub fn command(self) -> &'static str {
        match self {
            Self::Create => PAGE_CREATE,
            Self::Update => PAGE_UPDATE,
            Self::Trash => PAGE_TRASH,
            Self::CommentCreate => COMMENT_CREATE,
            Self::CommentUpdate => COMMENT_UPDATE,
            Self::CommentDelete => COMMENT_DELETE,
            Self::ContentStateSet => CONTENT_STATE_SET,
            Self::ContentStateClear => CONTENT_STATE_CLEAR,
            Self::AttachmentTrash => ATTACHMENT_TRASH,
            Self::SpaceCreate => SPACE_CREATE,
            Self::FolderCreate => FOLDER_CREATE,
            Self::FolderTrash => FOLDER_TRASH,
            Self::WhiteboardCreate => WHITEBOARD_CREATE,
            Self::WhiteboardTrash => WHITEBOARD_TRASH,
        }
    }
    pub fn route(self) -> &'static str {
        match self {
            Self::Create => CREATE_ROUTE,
            Self::Update => UPDATE_ROUTE,
            Self::Trash => TRASH_ROUTE,
            Self::CommentCreate => COMMENT_CREATE_ROUTE,
            Self::CommentUpdate => COMMENT_UPDATE_ROUTE,
            Self::CommentDelete => COMMENT_DELETE_ROUTE,
            Self::ContentStateSet => CONTENT_STATE_SET_ROUTE,
            Self::ContentStateClear => CONTENT_STATE_CLEAR_ROUTE,
            Self::AttachmentTrash => ATTACHMENT_TRASH_ROUTE,
            Self::SpaceCreate => SPACE_CREATE_ROUTE,
            Self::FolderCreate => FOLDER_CREATE_ROUTE,
            Self::FolderTrash => FOLDER_TRASH_ROUTE,
            Self::WhiteboardCreate => WHITEBOARD_CREATE_ROUTE,
            Self::WhiteboardTrash => WHITEBOARD_TRASH_ROUTE,
        }
    }
    pub const ALL: [Self; 14] = [
        Self::Create,
        Self::Update,
        Self::Trash,
        Self::CommentCreate,
        Self::CommentUpdate,
        Self::CommentDelete,
        Self::ContentStateSet,
        Self::ContentStateClear,
        Self::AttachmentTrash,
        Self::SpaceCreate,
        Self::FolderCreate,
        Self::FolderTrash,
        Self::WhiteboardCreate,
        Self::WhiteboardTrash,
    ];
    pub fn effect(self) -> EffectClass {
        match self {
            Self::Trash | Self::AttachmentTrash | Self::FolderTrash | Self::WhiteboardTrash => {
                EffectClass::Trash
            }
            Self::CommentDelete => EffectClass::Permanent,
            _ => EffectClass::Write,
        }
    }
    pub fn resource_type(self) -> &'static str {
        match self {
            Self::CommentCreate | Self::CommentUpdate | Self::CommentDelete => "comment",
            Self::AttachmentTrash => "attachment",
            Self::SpaceCreate => "space",
            Self::FolderCreate | Self::FolderTrash => "folder",
            Self::WhiteboardCreate | Self::WhiteboardTrash => "whiteboard",
            _ => "page",
        }
    }
    pub fn from_command(command: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.command() == command)
    }
}

fn invalid(path: &str, rule: &str) -> CraftError {
    CraftError::field(path, rule)
}
fn malformed() -> CraftError {
    CraftError::new(
        ErrorCode::RemoteUnavailable,
        "Confluence did not return the required current page or space evidence.",
    )
}
fn conflict() -> CraftError {
    CraftError::new(
        ErrorCode::VersionConflict,
        "The current published page differs from the expected revision. Read and review it before preparing another update.",
    )
}
fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, CraftError> {
    value.get(key).and_then(Value::as_str).ok_or_else(malformed)
}
fn current(response: &NativeResponse) -> Result<&Value, CraftError> {
    if response.status != 200
        || response.body.get("status").and_then(Value::as_str) != Some("current")
        || response.body.get("subtype").is_some_and(|v| !v.is_null())
    {
        return Err(malformed());
    }
    Ok(&response.body)
}

/// Exact published page data. Obtaining this value does not establish service identity or freshness.
pub struct PageSnapshot {
    id: String,
    space_id: String,
    title: String,
    version: u32,
    body: PageBody,
}
impl PageSnapshot {
    pub fn revision(&self) -> Result<PageRevision, CraftError> {
        Ok(PageRevision {
            version: self.version,
            representation: self.body.representation,
            digest: canonical_digest(
                &json!({"id":self.id,"space_id":self.space_id,"title":self.title,"version":self.version,"body":self.body}),
            )?,
        })
    }
}

/// Reads one complete representation from a native published-page response without following links.
pub fn observe_page(
    response: &NativeResponse,
    representation: PageRepresentation,
) -> Result<PageSnapshot, CraftError> {
    let raw = current(response)?;
    let id = string(raw, "id")?;
    let space_id = string(raw, "spaceId")?;
    let title = string(raw, "title")?;
    let version = raw
        .pointer("/version/number")
        .and_then(Value::as_u64)
        .filter(|v| (1..=i32::MAX as u64).contains(v))
        .ok_or_else(malformed)?;
    let body = raw
        .get("body")
        .and_then(|b| b.get(representation.native()))
        .ok_or_else(malformed)?;
    if !numeric(id)
        || !numeric(space_id)
        || !text(title, 4096)
        || string(body, "representation")? != representation.native()
    {
        return Err(malformed());
    }
    let body = PageBody {
        representation,
        value: string(body, "value")?.into(),
    };
    body.validate().map_err(|_| malformed())?;
    Ok(PageSnapshot {
        id: id.into(),
        space_id: space_id.into(),
        title: title.into(),
        version: version as u32,
        body,
    })
}

/// A bound native payload, with no transport/replay authority. Keep the service grant lock and
/// original evidence freshness checks through the host's final claimed-send callback.
pub struct PreparedPageWrite {
    kind: PageWriteKind,
    operation_digest: String,
    site: ResourceRef,
    space_id: String,
    page_id: Option<String>,
    /// A reply's parent comment, or the requested key for a space create.
    parent_id: Option<String>,
    id: Option<String>,
    title: String,
    version: u32,
    call: NativeCall,
}
impl PreparedPageWrite {
    pub fn call(&self) -> &NativeCall {
        &self.call
    }
    pub fn operation_digest(&self) -> &str {
        &self.operation_digest
    }
    pub fn space_id(&self) -> &str {
        &self.space_id
    }
    pub fn kind(&self) -> PageWriteKind {
        self.kind
    }
}
fn validate_operation(operation: &PreparedOperation, command: &str) -> Result<(), CraftError> {
    input::validate_request(operation.request())?;
    let effect = PageWriteKind::from_command(command)
        .map(PageWriteKind::effect)
        .ok_or_else(|| CraftError::field("/command", "unknown_confluence_page_write"))?;
    if operation.request().command != command || operation.effect() != effect {
        return Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "The prepared operation is not the requested page write.",
        ));
    }
    Ok(())
}

/// Checks explicit placement against observed space/parent data and compiles one normal page POST.
/// Evidence must have been fetched under the host's exact site, actor, grant and freshness policy.
pub fn prepare_create(
    operation: &PreparedOperation,
    space: &NativeResponse,
    parent: Option<&NativeResponse>,
) -> Result<PreparedPageWrite, CraftError> {
    validate_operation(operation, PAGE_CREATE)?;
    let input: PageCreateInput = input::parse(&operation.request().input)?;
    let s = current(space)?;
    if string(s, "id")? != input.space_id {
        return Err(malformed());
    }
    let parent_id = match &input.placement {
        PagePlacement::Root => {
            if parent.is_some() {
                return Err(invalid("/placement", "root_placement_has_no_parent"));
            }
            None
        }
        PagePlacement::Homepage => Some(string(s, "homepageId")?),
        PagePlacement::Parent { id } => Some(id.as_str()),
    };
    if let Some(id) = parent_id {
        let p = current(parent.ok_or_else(malformed)?)?;
        if !numeric(id) || string(p, "id")? != id || string(p, "spaceId")? != input.space_id {
            return Err(malformed());
        }
    }
    let mut body =
        json!({"spaceId":input.space_id,"status":"current","title":input.title,"body":input.body});
    if let Some(id) = parent_id {
        body["parentId"] = json!(id);
    }
    let call = NativeCall {
        route: CREATE_ROUTE.into(),
        path_parameters: BTreeMap::new(),
        query: BTreeMap::from([("root-level".into(), parent_id.is_none().to_string())]),
        repeated_query: BTreeMap::new(),
        body: Some(body),
    };
    registration::route(PageWriteKind::Create).validate_call(&call)?;
    Ok(PreparedPageWrite {
        kind: PageWriteKind::Create,
        operation_digest: operation.digest().into(),
        site: operation.request().target.clone(),
        space_id: input.space_id,
        page_id: None,
        parent_id: None,
        id: None,
        title: input.title,
        version: 1,
        call,
    })
}

/// Compares both version and exact content fingerprint, preserving omitted title/body values.
/// Published CAS cannot establish safety around other users' unpublished editor drafts.
pub fn prepare_update(
    operation: &PreparedOperation,
    page: &NativeResponse,
) -> Result<PreparedPageWrite, CraftError> {
    validate_operation(operation, PAGE_UPDATE)?;
    let input: PageUpdateInput = input::parse(&operation.request().input)?;
    let observed = observe_page(page, input.expected.representation)?;
    if observed.id != input.id
        || observed.space_id != input.space_id
        || observed.version != input.expected.version
        || observed.revision()?.digest != input.expected.digest
    {
        return Err(conflict());
    }
    let title = match input.title {
        InputField::Value(value) => value,
        _ => observed.title,
    };
    let body = match input.body {
        InputField::Value(value) => value,
        _ => observed.body,
    };
    let version = observed
        .version
        .checked_add(1)
        .filter(|v| *v <= i32::MAX as u32)
        .ok_or_else(conflict)?;
    let call = NativeCall {
        route: UPDATE_ROUTE.into(),
        path_parameters: BTreeMap::from([("id".into(), input.id.clone())]),
        query: BTreeMap::new(),
        repeated_query: BTreeMap::new(),
        body: Some(
            json!({"id":input.id,"status":"current","title":title,"body":body,"version":{"number":version}}),
        ),
    };
    registration::route(PageWriteKind::Update).validate_call(&call)?;
    Ok(PreparedPageWrite {
        kind: PageWriteKind::Update,
        operation_digest: operation.digest().into(),
        site: operation.request().target.clone(),
        space_id: input.space_id,
        page_id: None,
        parent_id: None,
        id: Some(input.id),
        title,
        version,
        call,
    })
}

/// Rechecks the reviewed published revision, then compiles one DELETE without `purge` or draft
/// selectors, which moves the current page to the space trash.
pub fn prepare_trash(
    operation: &PreparedOperation,
    page: &NativeResponse,
) -> Result<PreparedPageWrite, CraftError> {
    validate_operation(operation, PAGE_TRASH)?;
    let input: PageTrashInput = input::parse(&operation.request().input)?;
    let observed = observe_page(page, input.expected.representation)?;
    if observed.id != input.id
        || observed.space_id != input.space_id
        || observed.version != input.expected.version
        || observed.revision()?.digest != input.expected.digest
    {
        return Err(conflict());
    }
    let call = NativeCall {
        route: TRASH_ROUTE.into(),
        path_parameters: BTreeMap::from([("id".into(), input.id.clone())]),
        query: BTreeMap::new(),
        repeated_query: BTreeMap::new(),
        body: None,
    };
    registration::route(PageWriteKind::Trash).validate_call(&call)?;
    Ok(PreparedPageWrite {
        kind: PageWriteKind::Trash,
        operation_digest: operation.digest().into(),
        site: operation.request().target.clone(),
        space_id: input.space_id,
        page_id: None,
        parent_id: None,
        id: Some(input.id),
        title: observed.title,
        version: observed.version,
        call,
    })
}
