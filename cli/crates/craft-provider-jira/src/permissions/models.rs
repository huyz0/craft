use craft_core::contracts::{CredentialCacheUpdate, ResourceRef};
use craft_core::pagination::PageObservation;
use craft_core::presence::InputField;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextKind {
    Global,
    Project,
    Issue,
    Comment,
}
#[derive(Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProjectPermissionEvaluation {
    AnyProject,
    AnyQualifyingIssueOrProject,
    RequestedIssueOrContainingProject,
    CommentVisibilityAndParentIssue,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PermissionContext {
    pub kind: ContextKind,
    pub requested_selector: Option<String>,
    pub native_parameter: Option<String>,
    pub project_permission_evaluation: ProjectPermissionEvaluation,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PermissionKind {
    Global,
    Project,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PermissionResult {
    pub key: String,
    pub returned: bool,
    pub have_permission: Option<bool>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub kind: InputField<PermissionKind>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub id: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub name: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub description: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub deprecated_key: InputField<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields_not_returned: Vec<String>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PermissionData {
    pub target: ResourceRef,
    pub context: PermissionContext,
    pub requested_permissions: Vec<String>,
    pub items: Vec<PermissionResult>,
    pub observation: Option<PageObservation>,
    pub cached: bool,
    pub credential_cache_update: Option<CredentialCacheUpdate>,
    #[schemars(extend("const" = false))]
    pub atomic_snapshot: bool,
    #[schemars(extend("const" = false))]
    pub mutation_authorized: bool,
}
