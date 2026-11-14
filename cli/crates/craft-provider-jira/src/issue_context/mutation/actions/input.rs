use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Assignee {
    Unassigned,
    Account {
        #[schemars(length(min = 1, max = 128), regex(pattern = "^[A-Za-z0-9_:-]+$"))]
        account_id: String,
    },
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueAssignmentInput {
    #[schemars(length(min = 1, max = 512), regex(pattern = "^[0-9]+$"))]
    pub issue: String,
    pub expected: IssueState,
    pub expected_assignee: Assignee,
    #[schemars(length(min = 1, max = 128), regex(pattern = "^[A-Za-z0-9_:-]+$"))]
    pub account_id: String,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueUnassignInput {
    #[schemars(length(min = 1, max = 512), regex(pattern = "^[0-9]+$"))]
    pub issue: String,
    pub expected: IssueState,
    pub expected_assignee: Assignee,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum SubtaskDeletion {
    Refuse,
    /// Explicitly includes children invisible to the caller or added after observation.
    DeleteAll {
        #[schemars(length(max = 256), extend("uniqueItems" = true))]
        expected_visible_ids: Vec<String>,
    },
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueDeleteInput {
    #[schemars(length(min = 1, max = 512), regex(pattern = "^[0-9]+$"))]
    pub issue: String,
    pub expected: IssueState,
    pub subtasks: SubtaskDeletion,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActionEvidence {
    Assignment {
        before: Assignee,
        intended: Assignee,
    },
    Deletion {
        observed_subtasks: Vec<String>,
        delete_all_subtasks: bool,
        includes_unobserved_subtasks: bool,
        #[schemars(extend("const" = false))]
        hierarchy_complete: bool,
    },
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActionData {
    pub target: ResourceRef,
    pub issue_before: IssueState,
    pub evidence: ActionEvidence,
    #[schemars(extend("const" = false))]
    pub atomic_snapshot: bool,
    #[schemars(extend("const" = false))]
    pub mutation_authorized: bool,
}
pub(super) enum Action {
    Assign(Assignee, String),
    Unassign(Assignee),
    Delete(SubtaskDeletion),
}
pub(super) struct Intent {
    pub(super) expected: IssueState,
    pub(super) action: Action,
}
/// Resolves only a supported action write to its separately scoped preparation read.
pub fn metadata_command(command: &str) -> Result<&'static str, CraftError> {
    match command {
        ASSIGN_ISSUE => Ok(ASSIGN_CONTEXT),
        UNASSIGN_ISSUE => Ok(UNASSIGN_CONTEXT),
        DELETE_ISSUE => Ok(DELETE_CONTEXT),
        _ => Err(invalid("/command", "expected_issue_assignment_or_deletion")),
    }
}
fn write_command(command: &str) -> Result<&'static str, CraftError> {
    match command {
        ASSIGN_ISSUE | ASSIGN_CONTEXT => Ok(ASSIGN_ISSUE),
        UNASSIGN_ISSUE | UNASSIGN_CONTEXT => Ok(UNASSIGN_ISSUE),
        DELETE_ISSUE | DELETE_CONTEXT => Ok(DELETE_ISSUE),
        _ => Err(invalid("/command", "expected_issue_assignment_or_deletion")),
    }
}
pub(super) fn valid_account(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !matches!(
            value,
            "-1" | "null" | "unassigned" | "unknown" | "anonymous"
        )
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':'))
}
fn assignee_valid(assignee: &Assignee) -> bool {
    match assignee {
        Assignee::Unassigned => true,
        Assignee::Account { account_id } => valid_account(account_id),
    }
}
pub(super) fn intent(request: &CanonicalRequest) -> Result<Intent, CraftError> {
    if request.preconditions.is_some() || request.selection.cursor.is_some() {
        return Err(invalid(
            "/input",
            "action_preconditions_and_cursor_unsupported",
        ));
    }
    craft_core::input::parse_json(
        &serde_json::to_vec(&request.input).map_err(|_| malformed())?,
        65_536,
    )?;
    let (issue, expected, action) = match write_command(&request.command)? {
        ASSIGN_ISSUE => {
            let input: IssueAssignmentInput = serde_json::from_value(request.input.clone())
                .map_err(|_| invalid("/input", "invalid_issue_assignment"))?;
            if !valid_account(&input.account_id) || !assignee_valid(&input.expected_assignee) {
                return Err(invalid("/account_id", "expected_exact_account_id"));
            }
            (
                input.issue,
                input.expected,
                Action::Assign(input.expected_assignee, input.account_id),
            )
        }
        UNASSIGN_ISSUE => {
            let input: IssueUnassignInput = serde_json::from_value(request.input.clone())
                .map_err(|_| invalid("/input", "invalid_issue_unassignment"))?;
            if !assignee_valid(&input.expected_assignee) {
                return Err(invalid("/expected_assignee", "expected_explicit_assignee"));
            }
            (
                input.issue,
                input.expected,
                Action::Unassign(input.expected_assignee),
            )
        }
        _ => {
            let input: IssueDeleteInput = serde_json::from_value(request.input.clone())
                .map_err(|_| invalid("/input", "invalid_issue_deletion"))?;
            if let SubtaskDeletion::DeleteAll {
                expected_visible_ids,
            } = &input.subtasks
                && (expected_visible_ids.len() > 256
                    || expected_visible_ids
                        .iter()
                        .any(|id| !numeric_id(id) || id == &input.issue)
                    || expected_visible_ids
                        .windows(2)
                        .any(|pair| pair[0] >= pair[1]))
            {
                return Err(invalid(
                    "/subtasks/expected_visible_ids",
                    "expected_sorted_unique_child_ids",
                ));
            }
            (input.issue, input.expected, Action::Delete(input.subtasks))
        }
    };
    if !numeric_id(&issue) || issue != expected.id {
        return Err(invalid("/issue", "expected_resolved_numeric_issue_id"));
    }
    validate_target(&request.target, &issue)?;
    IssueObservation {
        requested_issue: issue,
        resolved_issue: request.target.clone(),
        resolution: IdentityResolution::ExactId,
        state: expected.clone(),
    }
    .validate_for_target(&request.target)
    .map_err(|_| invalid("/expected", "invalid_expected_issue_state"))?;
    Ok(Intent { expected, action })
}
/// Pure strict validation for action writes and their matching preparation reads.
pub fn validate_request(request: &CanonicalRequest) -> Result<(), CraftError> {
    intent(request).map(|_| ())
}
