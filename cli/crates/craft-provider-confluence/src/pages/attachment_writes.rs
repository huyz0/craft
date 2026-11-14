//! Attachment trash on a page in an allowed space. The reviewed version and file identity must
//! still match, so a replaced binary is never trashed by an older review. Purge is not offered.
use super::*;
use crate::attachments::{observe, valid_attachment_id};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const ATTACHMENT_TRASH: &str = "confluence.attachment.trash";
pub const ATTACHMENT_TRASH_ROUTE: &str = "confluence-attachment-trash";

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttachmentRevision {
    pub version: u32,
    pub file_id: String,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttachmentTrashInput {
    pub space_id: String,
    pub page_id: String,
    pub id: String,
    pub expected: AttachmentRevision,
}
impl AttachmentTrashInput {
    pub(super) fn validate(&self) -> Result<(), CraftError> {
        if !numeric(&self.space_id) || !numeric(&self.page_id) || !valid_attachment_id(&self.id) {
            return Err(invalid(
                "/input",
                "expected_numeric_space_page_and_attachment_id",
            ));
        }
        if !(1..=i32::MAX as u32).contains(&self.expected.version)
            || self.expected.file_id.is_empty()
            || self.expected.file_id.len() > 128
            || self.expected.file_id.chars().any(char::is_control)
        {
            return Err(invalid("/expected", "expected_attachment_version_and_file"));
        }
        Ok(())
    }
}

pub fn prepare_attachment_trash(
    operation: &PreparedOperation,
    page: &NativeResponse,
    attachment: &NativeResponse,
) -> Result<PreparedPageWrite, CraftError> {
    validate_operation(operation, ATTACHMENT_TRASH)?;
    let input: AttachmentTrashInput = input::parse(&operation.request().input)?;
    let raw = current(page)?;
    if string(raw, "id")? != input.page_id || string(raw, "spaceId")? != input.space_id {
        return Err(malformed());
    }
    if attachment.status != 200 {
        return Err(malformed());
    }
    let observed = observe(&attachment.body, &input.page_id)?;
    if observed.id != input.id
        || observed.version != input.expected.version
        || observed.file_id != input.expected.file_id
    {
        return Err(CraftError::new(
            ErrorCode::VersionConflict,
            "The attachment changed since it was reviewed. Read it again before trashing it.",
        ));
    }
    let call = NativeCall {
        route: ATTACHMENT_TRASH_ROUTE.into(),
        path_parameters: BTreeMap::from([("id".into(), input.id.clone())]),
        query: BTreeMap::new(),
        repeated_query: BTreeMap::new(),
        body: None,
    };
    registration::route(PageWriteKind::AttachmentTrash).validate_call(&call)?;
    Ok(PreparedPageWrite {
        kind: PageWriteKind::AttachmentTrash,
        operation_digest: operation.digest().into(),
        site: operation.request().target.clone(),
        space_id: input.space_id,
        page_id: Some(input.page_id),
        parent_id: None,
        id: Some(input.id),
        title: observed.title,
        version: observed.version,
        call,
    })
}
