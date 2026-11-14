use std::collections::BTreeSet;

use craft_core::error::CraftError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FieldListInput {}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    Custom,
    System,
}

impl std::str::FromStr for FieldType {
    type Err = CraftError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        serde_json::from_value(serde_json::Value::String(value.into()))
            .map_err(|_| CraftError::field("/types", "expected_custom_or_system"))
    }
}

#[derive(Clone, Copy, Default, Serialize, Deserialize, JsonSchema)]
pub enum FieldOrder {
    #[default]
    #[serde(rename = "name")]
    Name,
    #[serde(rename = "+name")]
    NameAscending,
    #[serde(rename = "-name")]
    NameDescending,
    #[serde(rename = "contextsCount")]
    ContextsCount,
    #[serde(rename = "+contextsCount")]
    ContextsCountAscending,
    #[serde(rename = "-contextsCount")]
    ContextsCountDescending,
    #[serde(rename = "lastUsed")]
    LastUsed,
    #[serde(rename = "+lastUsed")]
    LastUsedAscending,
    #[serde(rename = "-lastUsed")]
    LastUsedDescending,
    #[serde(rename = "screensCount")]
    ScreensCount,
    #[serde(rename = "+screensCount")]
    ScreensCountAscending,
    #[serde(rename = "-screensCount")]
    ScreensCountDescending,
    #[serde(rename = "projectsCount")]
    ProjectsCount,
    #[serde(rename = "+projectsCount")]
    ProjectsCountAscending,
    #[serde(rename = "-projectsCount")]
    ProjectsCountDescending,
}

impl std::str::FromStr for FieldOrder {
    type Err = CraftError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        serde_json::from_value(serde_json::Value::String(value.into()))
            .map_err(|_| CraftError::field("/order_by", "expected_native_field_ordering"))
    }
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FieldSearchInput {
    #[serde(default)]
    #[schemars(length(max = 100))]
    pub ids: Vec<String>,
    #[serde(default)]
    #[schemars(length(max = 2))]
    pub types: Vec<FieldType>,
    #[serde(default)]
    #[schemars(length(max = 100))]
    pub project_ids: Vec<u64>,
    #[schemars(length(min = 1, max = 256))]
    pub query: Option<String>,
    #[serde(default)]
    pub order_by: FieldOrder,
    #[serde(default = "page_size")]
    #[schemars(range(min = 1, max = 100))]
    pub page_size: u16,
}
fn page_size() -> u16 {
    50
}
pub(super) fn positive_id(value: &str) -> bool {
    value
        .parse::<i64>()
        .is_ok_and(|number| number > 0 && number.to_string() == value)
}
impl FieldSearchInput {
    pub fn validate(&self) -> Result<(), CraftError> {
        if self.ids.len() > 100
            || self
                .ids
                .iter()
                .any(|id| !id.strip_prefix("customfield_").is_some_and(positive_id))
            || self.ids.iter().collect::<BTreeSet<_>>().len() != self.ids.len()
        {
            return Err(CraftError::field(
                "/ids",
                "expected_unique_bounded_custom_field_ids",
            ));
        }
        if self.types.len() > 2
            || self.types.iter().collect::<BTreeSet<_>>().len() != self.types.len()
        {
            return Err(CraftError::field("/types", "expected_unique_field_types"));
        }
        if self.project_ids.len() > 100
            || self
                .project_ids
                .iter()
                .any(|id| *id == 0 || *id > i64::MAX as u64)
            || self.project_ids.iter().collect::<BTreeSet<_>>().len() != self.project_ids.len()
        {
            return Err(CraftError::field(
                "/project_ids",
                "expected_unique_positive_int64_project_ids",
            ));
        }
        if !(1..=100).contains(&self.page_size) {
            return Err(CraftError::field(
                "/page_size",
                "expected_integer_between_1_and_100",
            ));
        }
        if self.query.as_ref().is_some_and(|query| {
            query.is_empty()
                || query.chars().count() > 256
                || query.len() > 1024
                || query.chars().any(char::is_control)
        }) {
            return Err(CraftError::field(
                "/query",
                "expected_bounded_literal_query",
            ));
        }
        Ok(())
    }
}
