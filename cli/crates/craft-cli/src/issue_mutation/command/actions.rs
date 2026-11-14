use super::*;
use craft_provider_jira::issue_context::mutation::actions::{Assignee, SubtaskDeletion};

fn assignee(text: &str) -> Result<Assignee, String> {
    let value = craft_core::input::parse_object(text.as_bytes())
        .map_err(|_| "invalid expected-assignee JSON")?;
    serde_json::from_value(value).map_err(|_| "invalid expected-assignee fields".into())
}
fn subtasks(text: &str) -> Result<SubtaskDeletion, String> {
    let value = craft_core::input::parse_object(text.as_bytes())
        .map_err(|_| "invalid subtask-intent JSON")?;
    serde_json::from_value(value).map_err(|_| "invalid subtask-intent fields".into())
}
// A missing flag may await structured input, but serde must reject missing or null intent.
fn required<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueAssignArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 512))]
    pub operation_id: String,
    #[arg(long)]
    #[serde(default)]
    pub apply: bool,
    #[arg(long)]
    #[schemars(length(min = 36, max = 36))]
    pub cloud_id: Option<String>,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 512), regex(pattern = "^[0-9]+$"))]
    pub issue: String,
    #[arg(long,value_parser=expected,default_value="{}")]
    pub expected: IssueState,
    #[arg(long,value_parser=assignee)]
    #[serde(deserialize_with = "required")]
    #[schemars(required, with = "Assignee")]
    pub expected_assignee: Option<Assignee>,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 128), regex(pattern = "^[A-Za-z0-9_:-]+$"))]
    pub account_id: String,
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueUnassignArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 512))]
    pub operation_id: String,
    #[arg(long)]
    #[serde(default)]
    pub apply: bool,
    #[arg(long)]
    #[schemars(length(min = 36, max = 36))]
    pub cloud_id: Option<String>,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 512), regex(pattern = "^[0-9]+$"))]
    pub issue: String,
    #[arg(long,value_parser=expected,default_value="{}")]
    pub expected: IssueState,
    #[arg(long,value_parser=assignee)]
    #[serde(deserialize_with = "required")]
    #[schemars(required, with = "Assignee")]
    pub expected_assignee: Option<Assignee>,
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueDeleteArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 512))]
    pub operation_id: String,
    #[arg(long)]
    #[serde(default)]
    pub apply: bool,
    #[arg(long)]
    #[schemars(length(min = 36, max = 36))]
    pub cloud_id: Option<String>,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 512), regex(pattern = "^[0-9]+$"))]
    pub issue: String,
    #[arg(long,value_parser=expected,default_value="{}")]
    pub expected: IssueState,
    #[arg(long,value_parser=subtasks)]
    #[serde(deserialize_with = "required")]
    #[schemars(required, with = "SubtaskDeletion")]
    pub subtasks: Option<SubtaskDeletion>,
}
