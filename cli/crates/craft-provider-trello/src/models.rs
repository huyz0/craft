use craft_core::contracts::ResourceRef;
use craft_core::error::CraftError;
use craft_core::pagination::PageObservation;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub(crate) fn page_size() -> u16 {
    25
}

/// Trello ids are 24 lowercase hexadecimal characters.
pub fn trello_id(value: &str) -> bool {
    value.len() == 24
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

pub(crate) fn id_field(path: &str, value: &str) -> Result<(), CraftError> {
    if trello_id(value) {
        Ok(())
    } else {
        Err(CraftError::field(path, "expected_24_character_trello_id"))
    }
}

pub(crate) fn page_size_field(value: u16) -> Result<(), CraftError> {
    if (1..=100).contains(&value) {
        Ok(())
    } else {
        Err(CraftError::field(
            "/page_size",
            "expected_page_size_1_to_100",
        ))
    }
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoardListInput {
    #[serde(default = "page_size")]
    #[schemars(range(min = 1, max = 100))]
    pub page_size: u16,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdInput {
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub id: String,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoardScopedInput {
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub board_id: String,
    #[serde(default = "page_size")]
    #[schemars(range(min = 1, max = 100))]
    pub page_size: u16,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListScopedInput {
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub list_id: String,
    #[serde(default = "page_size")]
    #[schemars(range(min = 1, max = 100))]
    pub page_size: u16,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CardScopedInput {
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub card_id: String,
    #[serde(default = "page_size")]
    #[schemars(range(min = 1, max = 100))]
    pub page_size: u16,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchInput {
    #[schemars(length(min = 1, max = 16384))]
    pub query: String,
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub board_id: Option<String>,
    #[serde(default = "page_size")]
    #[schemars(range(min = 1, max = 100))]
    pub page_size: u16,
}

pub(crate) fn validate_search(value: &SearchInput) -> Result<(), CraftError> {
    page_size_field(value.page_size)?;
    if value.query.trim().is_empty()
        || value.query.len() > 16_384
        || value.query.chars().any(char::is_control)
    {
        return Err(CraftError::field("/query", "expected_bounded_search_text"));
    }
    if let Some(board) = &value.board_id {
        id_field("/board_id", board)?;
    }
    Ok(())
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoardSummary {
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub closed: Option<bool>,
    pub url: Option<String>,
    pub last_activity: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListSummary {
    pub id: String,
    pub name: Option<String>,
    pub closed: Option<bool>,
    pub position: Option<f64>,
    pub board_id: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CardBadges {
    pub comments: Option<u32>,
    pub attachments: Option<u32>,
    pub checklist_items: Option<u32>,
    pub checklist_items_checked: Option<u32>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CardSummary {
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub closed: Option<bool>,
    pub due: Option<String>,
    pub due_complete: Option<bool>,
    pub list_id: Option<String>,
    pub board_id: Option<String>,
    pub label_ids: Option<Vec<String>>,
    pub member_ids: Option<Vec<String>>,
    pub url: Option<String>,
    pub short_link: Option<String>,
    pub last_activity: Option<String>,
    pub position: Option<f64>,
    pub badges: Option<CardBadges>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentSummary {
    pub id: String,
    pub text: Option<String>,
    pub date: Option<String>,
    pub author_id: Option<String>,
    pub author_username: Option<String>,
    pub card_id: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LabelSummary {
    pub id: String,
    pub name: Option<String>,
    pub color: Option<String>,
    pub board_id: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckItemSummary {
    pub id: String,
    pub name: Option<String>,
    pub complete: bool,
    pub position: Option<f64>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChecklistSummary {
    pub id: String,
    pub name: Option<String>,
    pub card_id: Option<String>,
    pub items: Vec<CheckItemSummary>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttachmentSummary {
    pub id: String,
    pub name: Option<String>,
    pub url: Option<String>,
    pub bytes: Option<u64>,
    pub date: Option<String>,
    pub mime_type: Option<String>,
    pub is_upload: Option<bool>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemberSummary {
    pub id: String,
    pub username: Option<String>,
    pub full_name: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetData<T> {
    pub target: ResourceRef,
    pub item: Option<T>,
    pub observed_at: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListData<T> {
    pub target: ResourceRef,
    pub items: Vec<T>,
    pub observation: Option<PageObservation>,
    pub cached: bool,
}
