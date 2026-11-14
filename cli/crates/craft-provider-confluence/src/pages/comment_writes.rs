//! Footer-comment writes bound to one page in one space. Comment text and authorship are content,
//! never approval; deletion is permanent in Confluence and is declared as such.
use super::*;
use crate::comments::CommentRevision;
use craft_core::pagination::valid_digest;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const COMMENT_CREATE: &str = "confluence.comment.create";
pub const COMMENT_UPDATE: &str = "confluence.comment.update";
pub const COMMENT_DELETE: &str = "confluence.comment.delete";
pub const COMMENT_CREATE_ROUTE: &str = "confluence-footer-comment-create";
pub const COMMENT_UPDATE_ROUTE: &str = "confluence-footer-comment-update";
pub const COMMENT_DELETE_ROUTE: &str = "confluence-footer-comment-delete";

/// A new top-level footer comment, or a reply when `parent_comment_id` names a current comment
/// on the same page.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentCreateInput {
    pub space_id: String,
    pub page_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_comment_id: Option<String>,
    pub body: PageBody,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentUpdateInput {
    pub space_id: String,
    pub page_id: String,
    pub id: String,
    pub expected: CommentRevision,
    pub body: PageBody,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentDeleteInput {
    pub space_id: String,
    pub page_id: String,
    pub id: String,
    pub expected: CommentRevision,
}

fn ids(values: &[(&str, &str)]) -> Result<(), CraftError> {
    for (path, value) in values {
        if !numeric(value) {
            return Err(invalid(path, "expected_canonical_positive_int64_id"));
        }
    }
    Ok(())
}
fn revision_valid(expected: &CommentRevision) -> Result<(), CraftError> {
    if !(1..i32::MAX as u32).contains(&expected.version)
        || !valid_digest(&expected.digest)
        || !matches!(
            expected.representation.as_str(),
            "storage" | "atlas_doc_format"
        )
    {
        return Err(invalid(
            "/expected",
            "expected_incrementable_int32_version_digest_and_representation",
        ));
    }
    Ok(())
}
impl CommentCreateInput {
    pub(super) fn validate(&self) -> Result<(), CraftError> {
        ids(&[("/space_id", &self.space_id), ("/page_id", &self.page_id)])?;
        if let Some(parent) = &self.parent_comment_id {
            ids(&[("/parent_comment_id", parent)])?;
        }
        self.body.validate()
    }
}
impl CommentUpdateInput {
    pub(super) fn validate(&self) -> Result<(), CraftError> {
        ids(&[
            ("/space_id", &self.space_id),
            ("/page_id", &self.page_id),
            ("/id", &self.id),
        ])?;
        revision_valid(&self.expected)?;
        self.body.validate()
    }
}
impl CommentDeleteInput {
    pub(super) fn validate(&self) -> Result<(), CraftError> {
        ids(&[
            ("/space_id", &self.space_id),
            ("/page_id", &self.page_id),
            ("/id", &self.id),
        ])?;
        revision_valid(&self.expected)
    }
}

/// The selected space must contain the current published page that hosts the comment.
fn check_page(page: &NativeResponse, page_id: &str, space_id: &str) -> Result<(), CraftError> {
    let raw = current(page)?;
    if string(raw, "id")? != page_id || string(raw, "spaceId")? != space_id {
        return Err(malformed());
    }
    Ok(())
}
fn comment_conflict() -> CraftError {
    CraftError::new(
        ErrorCode::VersionConflict,
        "The current footer comment differs from the expected revision. Read it again before preparing another write.",
    )
}
fn observed_revision(
    comment: &NativeResponse,
    id: &str,
    page_id: &str,
    representation: &str,
) -> Result<CommentRevision, CraftError> {
    if comment.status != 200 {
        return Err(malformed());
    }
    crate::comments::observe_revision(&comment.body, id, page_id, representation)
}

/// Checks the page (and a reply's parent comment) and compiles one footer-comment POST.
pub fn prepare_comment_create(
    operation: &PreparedOperation,
    page: &NativeResponse,
    parent: Option<&NativeResponse>,
) -> Result<PreparedPageWrite, CraftError> {
    validate_operation(operation, COMMENT_CREATE)?;
    let input: CommentCreateInput = input::parse(&operation.request().input)?;
    check_page(page, &input.page_id, &input.space_id)?;
    let body = match (&input.parent_comment_id, parent) {
        (None, None) => json!({"pageId":input.page_id,"body":input.body}),
        (Some(parent_id), Some(parent)) => {
            // Any current revision of the parent proves it exists on this exact page.
            observed_revision(parent, parent_id, &input.page_id, "storage")?;
            json!({"parentCommentId":parent_id,"body":input.body})
        }
        _ => return Err(malformed()),
    };
    let call = NativeCall {
        route: COMMENT_CREATE_ROUTE.into(),
        path_parameters: BTreeMap::new(),
        query: BTreeMap::new(),
        repeated_query: BTreeMap::new(),
        body: Some(body),
    };
    registration::route(PageWriteKind::CommentCreate).validate_call(&call)?;
    Ok(PreparedPageWrite {
        kind: PageWriteKind::CommentCreate,
        operation_digest: operation.digest().into(),
        site: operation.request().target.clone(),
        space_id: input.space_id,
        page_id: Some(input.page_id),
        parent_id: input.parent_comment_id,
        id: None,
        title: String::new(),
        version: 1,
        call,
    })
}

/// Compares the reviewed comment revision and compiles one PUT with the next version number.
pub fn prepare_comment_update(
    operation: &PreparedOperation,
    page: &NativeResponse,
    comment: &NativeResponse,
) -> Result<PreparedPageWrite, CraftError> {
    validate_operation(operation, COMMENT_UPDATE)?;
    let input: CommentUpdateInput = input::parse(&operation.request().input)?;
    check_page(page, &input.page_id, &input.space_id)?;
    let observed = observed_revision(
        comment,
        &input.id,
        &input.page_id,
        &input.expected.representation,
    )?;
    if observed.version != input.expected.version || observed.digest != input.expected.digest {
        return Err(comment_conflict());
    }
    let version = observed
        .version
        .checked_add(1)
        .filter(|v| *v <= i32::MAX as u32)
        .ok_or_else(comment_conflict)?;
    let call = NativeCall {
        route: COMMENT_UPDATE_ROUTE.into(),
        path_parameters: BTreeMap::from([("comment-id".into(), input.id.clone())]),
        query: BTreeMap::new(),
        repeated_query: BTreeMap::new(),
        body: Some(json!({"version":{"number":version},"body":input.body})),
    };
    registration::route(PageWriteKind::CommentUpdate).validate_call(&call)?;
    Ok(PreparedPageWrite {
        kind: PageWriteKind::CommentUpdate,
        operation_digest: operation.digest().into(),
        site: operation.request().target.clone(),
        space_id: input.space_id,
        page_id: Some(input.page_id),
        parent_id: None,
        id: Some(input.id),
        title: String::new(),
        version,
        call,
    })
}

/// Compares the reviewed comment revision and compiles one permanent DELETE.
pub fn prepare_comment_delete(
    operation: &PreparedOperation,
    page: &NativeResponse,
    comment: &NativeResponse,
) -> Result<PreparedPageWrite, CraftError> {
    validate_operation(operation, COMMENT_DELETE)?;
    let input: CommentDeleteInput = input::parse(&operation.request().input)?;
    check_page(page, &input.page_id, &input.space_id)?;
    let observed = observed_revision(
        comment,
        &input.id,
        &input.page_id,
        &input.expected.representation,
    )?;
    if observed.version != input.expected.version || observed.digest != input.expected.digest {
        return Err(comment_conflict());
    }
    let call = NativeCall {
        route: COMMENT_DELETE_ROUTE.into(),
        path_parameters: BTreeMap::from([("comment-id".into(), input.id.clone())]),
        query: BTreeMap::new(),
        repeated_query: BTreeMap::new(),
        body: None,
    };
    registration::route(PageWriteKind::CommentDelete).validate_call(&call)?;
    Ok(PreparedPageWrite {
        kind: PageWriteKind::CommentDelete,
        operation_digest: operation.digest().into(),
        site: operation.request().target.clone(),
        space_id: input.space_id,
        page_id: Some(input.page_id),
        parent_id: None,
        id: Some(input.id),
        title: String::new(),
        version: observed.version,
        call,
    })
}
