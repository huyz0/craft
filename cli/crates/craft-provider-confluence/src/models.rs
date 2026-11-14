use craft_core::contracts::{CredentialCacheUpdate, ResourceRef};
use craft_core::error::CraftError;
use craft_core::pagination::PageObservation;
use craft_core::presence::InputField;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub(crate) fn page_size() -> u16 {
    25
}
pub(crate) fn numeric(value: &str) -> bool {
    value
        .parse::<i64>()
        .is_ok_and(|id| id > 0 && id.to_string() == value)
}
pub(crate) fn text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetInput {
    #[schemars(length(min = 1, max = 19), regex(pattern = "^[1-9][0-9]*$"))]
    pub id: String,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpaceListInput {
    #[schemars(length(min = 1, max = 256))]
    pub key: Option<String>,
    #[serde(default = "page_size")]
    #[schemars(range(min = 1, max = 250))]
    pub page_size: u16,
}
#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ListStatus {
    #[default]
    Current,
    Archived,
    Deleted,
    Trashed,
}
impl ListStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Archived => "archived",
            Self::Deleted => "deleted",
            Self::Trashed => "trashed",
        }
    }
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageListInput {
    #[schemars(length(min = 1, max = 19), regex(pattern = "^[1-9][0-9]*$"))]
    pub space_id: Option<String>,
    #[schemars(length(min = 1, max = 4096))]
    pub title: Option<String>,
    #[serde(default)]
    pub status: ListStatus,
    #[serde(default = "page_size")]
    #[schemars(range(min = 1, max = 250))]
    pub page_size: u16,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchInput {
    #[schemars(length(min = 1, max = 16384))]
    pub cql: String,
    #[serde(default = "page_size")]
    #[schemars(range(min = 1, max = 100))]
    pub page_size: u16,
}
pub(crate) fn validate_get(value: &GetInput) -> Result<(), CraftError> {
    if numeric(&value.id) {
        Ok(())
    } else {
        Err(CraftError::field(
            "/id",
            "expected_canonical_positive_int64_id",
        ))
    }
}
pub(crate) fn validate_spaces(value: &SpaceListInput) -> Result<(), CraftError> {
    if !(1..=250).contains(&value.page_size)
        || value
            .key
            .as_ref()
            .is_some_and(|k| !text(k, 256) || k.contains(','))
    {
        Err(CraftError::field(
            "/input",
            "invalid_space_filter_or_page_size",
        ))
    } else {
        Ok(())
    }
}
pub(crate) fn validate_pages(value: &PageListInput) -> Result<(), CraftError> {
    if !(1..=250).contains(&value.page_size)
        || value.space_id.as_ref().is_some_and(|id| !numeric(id))
        || value.title.as_ref().is_some_and(|v| !text(v, 4096))
    {
        Err(CraftError::field(
            "/input",
            "invalid_page_filter_or_page_size",
        ))
    } else {
        Ok(())
    }
}
pub(crate) fn validate_search(value: &SearchInput) -> Result<(), CraftError> {
    if !(1..=100).contains(&value.page_size)
        || value.cql.trim().is_empty()
        || value.cql.len() > 16384
        || value
            .cql
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        Err(CraftError::field(
            "/input",
            "expected_bounded_literal_cql_and_page_size",
        ))
    } else {
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BodyState {
    NotRequested,
    NotReturned,
    Returned,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageSummary {
    pub id: String,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub space_id: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub title: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub status: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub version: InputField<u32>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub parent_id: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub body_storage: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub body_atlas_doc_format: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub revision: InputField<crate::pages::PageRevision>,
    pub body_state: BodyState,
    pub fields_not_returned: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpaceSummary {
    pub id: String,
    pub key: String,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub name: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub space_type: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub status: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub description_plain: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub description_view: InputField<String>,
    pub fields_not_returned: Vec<String>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetData<T> {
    pub target: ResourceRef,
    pub item: Option<T>,
    pub observed_at: Option<u64>,
    pub credential_cache_update: Option<CredentialCacheUpdate>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListData<T> {
    pub target: ResourceRef,
    pub items: Vec<T>,
    pub observation: Option<PageObservation>,
    pub cached: bool,
    pub credential_cache_update: Option<CredentialCacheUpdate>,
}
