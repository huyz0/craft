use crate::fields::FieldSchema;
use crate::issue_reads::IdentityResolution;
use craft_core::contracts::{CredentialCacheUpdate, ResourceRef};
use craft_core::pagination::PageObservation;
use craft_core::presence::InputField;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueState {
    pub id: String,
    pub key: String,
    pub project_id: String,
    pub issue_type_id: String,
    pub status_id: String,
    pub updated: String,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueObservation {
    pub requested_issue: String,
    pub resolved_issue: ResourceRef,
    pub resolution: IdentityResolution,
    pub state: IssueState,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditableField {
    pub id: String,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub key: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub name: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub required: InputField<bool>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub operations: InputField<Vec<String>>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub schema: InputField<FieldSchema>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub has_default: InputField<bool>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub allowed_values: InputField<Vec<Value>>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub default_value: InputField<Value>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub configuration: InputField<BTreeMap<String, Value>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields_not_returned: Vec<String>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StatusCategory {
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub id: InputField<i64>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub key: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub name: InputField<String>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DestinationStatus {
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub id: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub name: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub description: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub category: InputField<StatusCategory>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Transition {
    pub id: String,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub name: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub to: InputField<DestinationStatus>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub has_screen: InputField<bool>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub is_available: InputField<bool>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub is_conditional: InputField<bool>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub is_global: InputField<bool>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub is_initial: InputField<bool>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub looped: InputField<bool>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub fields: InputField<Vec<EditableField>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields_not_returned: Vec<String>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextData<T> {
    pub target: ResourceRef,
    pub items: Vec<T>,
    pub issue_observation: Option<IssueObservation>,
    pub observation: Option<PageObservation>,
    pub cached: bool,
    pub credential_cache_update: Option<CredentialCacheUpdate>,
    #[schemars(extend("const"=false))]
    pub atomic_snapshot: bool,
    #[schemars(extend("const"=false))]
    pub mutation_authorized: bool,
}
