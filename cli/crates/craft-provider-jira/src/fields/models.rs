use std::collections::BTreeMap;

use craft_core::contracts::{CredentialCacheUpdate, ResourceRef};
use craft_core::pagination::PageObservation;
use craft_core::presence::InputField;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FieldSchema {
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub r#type: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub items: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub system: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub custom: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub custom_id: InputField<i64>,
    /// Provider configuration is bounded inert JSON, never a schema or credential destination.
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub configuration: InputField<BTreeMap<String, Value>>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScopeProject {
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub id: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub key: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub name: InputField<String>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FieldScope {
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub r#type: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub project: InputField<ScopeProject>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListedField {
    pub id: String,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub name: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub key: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub custom: InputField<bool>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub navigable: InputField<bool>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub orderable: InputField<bool>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub searchable: InputField<bool>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub clause_names: InputField<Vec<String>>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub schema: InputField<FieldSchema>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub scope: InputField<FieldScope>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields_not_returned: Vec<String>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LastUsed {
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub r#type: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub value: InputField<String>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchedField {
    pub id: String,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub name: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub schema: InputField<FieldSchema>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub description: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub key: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub stable_id: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub is_locked: InputField<bool>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub is_unscreenable: InputField<bool>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub searcher_key: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub screens_count: InputField<i64>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub contexts_count: InputField<i64>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub projects_count: InputField<i64>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub last_used: InputField<LastUsed>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub translated_name: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub translated_description: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub type_display_name: InputField<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields_not_returned: Vec<String>,
}
#[derive(Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FieldCoverage {
    VisibilityDependent,
    ClassicProjectFields,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FieldPageData<T> {
    pub target: ResourceRef,
    pub items: Vec<T>,
    pub coverage: FieldCoverage,
    pub observation: Option<PageObservation>,
    pub cached: bool,
    pub credential_cache_update: Option<CredentialCacheUpdate>,
}
