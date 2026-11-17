use super::*;
use craft_core::input::{MAX_INPUT_BYTES, parse_object};
use craft_core::pagination::valid_digest;
use craft_core::presence::InputField;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PageRepresentation {
    Storage,
    AtlasDocFormat,
}
impl PageRepresentation {
    pub(super) fn native(self) -> &'static str {
        match self {
            Self::Storage => "storage",
            Self::AtlasDocFormat => "atlas_doc_format",
        }
    }
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageBody {
    pub representation: PageRepresentation,
    #[schemars(length(min = 1, max = 1048576))]
    pub value: String,
}
impl PageBody {
    pub(super) fn validate(&self) -> Result<(), CraftError> {
        if self.value.trim().is_empty()
            || self.value.len() > MAX_INPUT_BYTES
            || self.value.contains('\0')
        {
            return Err(invalid("/body/value", "expected_bounded_nonempty_body"));
        }
        if self.representation == PageRepresentation::AtlasDocFormat {
            let doc = parse_object(self.value.as_bytes())?;
            if doc.get("type").and_then(Value::as_str) != Some("doc")
                || doc.get("version").and_then(Value::as_u64) != Some(1)
                || !doc.get("content").is_some_and(Value::is_array)
            {
                return Err(invalid("/body/value", "expected_adf_document"));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PagePlacement {
    Root,
    Homepage,
    Parent { id: String },
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageCreateInput {
    pub space_id: String,
    pub title: String,
    pub placement: PagePlacement,
    pub body: PageBody,
}
impl PageCreateInput {
    pub(super) fn validate(&self) -> Result<(), CraftError> {
        if !numeric(&self.space_id) || !text(&self.title, 4096) {
            return Err(invalid(
                "/input",
                "expected_numeric_space_and_nonempty_title",
            ));
        }
        if let PagePlacement::Parent { id } = &self.placement
            && !numeric(id)
        {
            return Err(invalid(
                "/placement/id",
                "expected_canonical_positive_int64_id",
            ));
        }
        self.body.validate()
    }
}

/// Fingerprint of the exact published representation; it carries no approval or write authority.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageRevision {
    pub version: u32,
    pub digest: String,
    pub representation: PageRepresentation,
}

/// Explicit acknowledgment of the documented native publish behavior: Confluence reconciles the
/// published content into any open editor draft and may overwrite divergent unpublished edits.
/// It is bound into the operation digest but never sent as a native field, and grants no permission.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EditorDraftPolicy {
    NativeReconciliationMayOverwrite,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageUpdateInput {
    pub id: String,
    pub space_id: String,
    pub expected: PageRevision,
    pub editor_draft_policy: EditorDraftPolicy,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub title: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub body: InputField<PageBody>,
}
impl PageUpdateInput {
    pub(super) fn validate(&self) -> Result<(), CraftError> {
        if !numeric(&self.id) || !numeric(&self.space_id) {
            return Err(invalid(
                "/input",
                "expected_canonical_numeric_page_and_space",
            ));
        }
        if !(1..i32::MAX as u32).contains(&self.expected.version)
            || !valid_digest(&self.expected.digest)
        {
            return Err(invalid(
                "/expected",
                "expected_incrementable_int32_version_and_digest",
            ));
        }
        match &self.title {
            InputField::Omitted => {}
            InputField::Value(value) if text(value, 4096) => {}
            _ => return Err(invalid("/title", "expected_nonempty_title_or_omission")),
        }
        match &self.body {
            InputField::Omitted => {}
            InputField::Value(value) => value.validate()?,
            _ => return Err(invalid("/body", "expected_body_or_omission")),
        }
        if self.title.is_omitted() && self.body.is_omitted() {
            return Err(invalid("/input", "expected_at_least_one_page_change"));
        }
        Ok(())
    }
}

/// Moves one current page to the space trash after the published revision is rechecked. Trash is
/// recoverable by space administrators; this operation never purges and never touches drafts.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageTrashInput {
    pub id: String,
    pub space_id: String,
    pub expected: PageRevision,
}
impl PageTrashInput {
    pub(super) fn validate(&self) -> Result<(), CraftError> {
        if !numeric(&self.id) || !numeric(&self.space_id) {
            return Err(invalid(
                "/input",
                "expected_canonical_numeric_page_and_space",
            ));
        }
        if !(1..=i32::MAX as u32).contains(&self.expected.version)
            || !valid_digest(&self.expected.digest)
        {
            return Err(invalid("/expected", "expected_int32_version_and_digest"));
        }
        Ok(())
    }
}

pub(super) fn validate_request(request: &CanonicalRequest) -> Result<(), CraftError> {
    crate::registration::validate_target(&request.target)?;
    if request.preconditions.is_some()
        || request.selection.cursor.is_some()
        || request.selection.limit != craft_core::contracts::Selection::default().limit
        || !request.selection.fields.is_empty()
    {
        return Err(invalid(
            "/selection",
            "page_write_has_no_projection_paging_or_outer_preconditions",
        ));
    }
    match request.command.as_str() {
        PAGE_CREATE => parse::<PageCreateInput>(&request.input)?.validate(),
        PAGE_UPDATE => parse::<PageUpdateInput>(&request.input)?.validate(),
        PAGE_TRASH => parse::<PageTrashInput>(&request.input)?.validate(),
        COMMENT_CREATE => parse::<CommentCreateInput>(&request.input)?.validate(),
        COMMENT_UPDATE => parse::<CommentUpdateInput>(&request.input)?.validate(),
        COMMENT_DELETE => parse::<CommentDeleteInput>(&request.input)?.validate(),
        CONTENT_STATE_SET => parse::<ContentStateSetInput>(&request.input)?.validate(),
        CONTENT_STATE_CLEAR => parse::<ContentStateClearInput>(&request.input)?.validate(),
        ATTACHMENT_TRASH => parse::<AttachmentTrashInput>(&request.input)?.validate(),
        SPACE_CREATE => parse::<SpaceCreateInput>(&request.input)?.validate(),
        FOLDER_CREATE | WHITEBOARD_CREATE => parse::<ContentCreateInput>(&request.input)?
            .validate_for(request.command == WHITEBOARD_CREATE),
        FOLDER_TRASH | WHITEBOARD_TRASH => parse::<ContentTrashInput>(&request.input)?.validate(),
        PAGE_WIDTH_SET => parse::<PageWidthSetInput>(&request.input)?.validate(),
        _ => Err(invalid("/command", "unknown_confluence_page_write")),
    }
}
pub(super) fn parse<T: serde::de::DeserializeOwned>(value: &Value) -> Result<T, CraftError> {
    serde_json::from_value(value.clone())
        .map_err(|_| invalid("/input", "invalid_confluence_page_write"))
}
