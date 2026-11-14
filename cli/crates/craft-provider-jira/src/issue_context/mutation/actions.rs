//! Issue-specific assignment and hierarchy-aware deletion, without edit-screen authority.
use super::*;

pub const ASSIGN_ISSUE: &str = "jira.issue.assign";
pub const UNASSIGN_ISSUE: &str = "jira.issue.unassign";
pub const DELETE_ISSUE: &str = "jira.issue.delete";
pub const ASSIGN_ROUTE: &str = "jira-issue-assign";
pub const DELETE_ROUTE: &str = "jira-issue-delete";
const ASSIGN_CONTEXT: &str = "jira.metadata.issue-assignment";
const UNASSIGN_CONTEXT: &str = "jira.metadata.issue-unassignment";
const DELETE_CONTEXT: &str = "jira.metadata.issue-deletion";
const ACCOUNT_ROUTE: &str = "jira-issue-assignable-account";

mod input;
mod registration;
use input::{Action, intent, valid_account};
pub use input::{
    ActionData, ActionEvidence, Assignee, IssueAssignmentInput, IssueDeleteInput,
    IssueUnassignInput, SubtaskDeletion, metadata_command, validate_request,
};
pub use registration::JiraIssueActions;

fn observe(
    context: &mut dyn OperationContext,
    issue: &str,
    deletion: bool,
) -> Result<(IssueState, Value), CraftError> {
    let raw = invoke(
        context,
        NativeCall {
            route: IDENTITY_ROUTE.into(),
            path_parameters: BTreeMap::from([("issueIdOrKey".into(), issue.into())]),
            query: BTreeMap::from([
                (
                    "fields".into(),
                    if deletion {
                        "project,issuetype,status,updated,subtasks"
                    } else {
                        "project,issuetype,status,updated,assignee"
                    }
                    .into(),
                ),
                ("fieldsByKeys".into(), "false".into()),
                ("updateHistory".into(), "false".into()),
                ("failFast".into(), "true".into()),
            ]),
            repeated_query: BTreeMap::new(),
            body: None,
        },
    )?;
    let state = parse_state(&raw, issue)?;
    let value = raw
        .get("fields")
        .and_then(|v| v.get(if deletion { "subtasks" } else { "assignee" }))
        .cloned()
        .ok_or_else(malformed)?;
    Ok((state, value))
}
fn assignee(value: &Value) -> Result<Assignee, CraftError> {
    if value.is_null() {
        return Ok(Assignee::Unassigned);
    }
    let account = value
        .as_object()
        .and_then(|v| v.get("accountId"))
        .and_then(Value::as_str)
        .filter(|id| valid_account(id))
        .ok_or_else(malformed)?;
    Ok(Assignee::Account {
        account_id: account.into(),
    })
}
fn children(value: &Value, issue: &str) -> Result<Vec<String>, CraftError> {
    let values = value
        .as_array()
        .filter(|v| v.len() <= 256)
        .ok_or_else(malformed)?;
    let mut ids = BTreeSet::new();
    for value in values {
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| numeric_id(id) && *id != issue)
            .ok_or_else(malformed)?;
        if !ids.insert(id.to_owned()) {
            return Err(malformed());
        }
    }
    Ok(ids.into_iter().collect())
}
fn qualify_account(
    context: &mut dyn OperationContext,
    issue: &str,
    account: &str,
) -> Result<(), CraftError> {
    context.checkpoint()?;
    let response = context.invoke(&NativeCall {
        route: ACCOUNT_ROUTE.into(),
        path_parameters: BTreeMap::new(),
        query: BTreeMap::from([
            ("issueId".into(), issue.into()),
            ("accountId".into(), account.into()),
            ("startAt".into(), "0".into()),
            ("maxResults".into(), "2".into()),
        ]),
        repeated_query: BTreeMap::new(),
        body: None,
    })?;
    if response.status != 200 {
        return Err(CraftError::new(
            match response.status {
                401 => ErrorCode::AuthRequired,
                403 => ErrorCode::PermissionDenied,
                429 => ErrorCode::RateLimited,
                _ => ErrorCode::RemoteUnavailable,
            },
            "Issue-specific assignable account lookup did not qualify; no retry was performed.",
        ));
    }
    let body = craft_core::input::parse_json(
        &serde_json::to_vec(&response.body).map_err(|_| malformed())?,
        65_536,
    )
    .map_err(|_| malformed())?;
    let values = body
        .as_array()
        .filter(|v| v.len() <= 2)
        .ok_or_else(malformed)?;
    if values.len() != 1
        || values[0].get("accountId").and_then(Value::as_str) != Some(account)
        || values[0].get("active").and_then(Value::as_bool) != Some(true)
    {
        return Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "The exact active account was not positively qualified as assignable to this issue.",
        ));
    }
    Ok(())
}
/// Brackets live issue context and freezes a write, without sending any mutation.
pub fn prepare_action(
    read: &PreparedOperation,
    write: &PreparedOperation,
    context: &mut dyn OperationContext,
    principal: &MetadataPrincipal,
    clock: &dyn Clock,
) -> Result<FrozenIssueMutation, CraftError> {
    let intent = intent(write.request())?;
    intent_for_read(read, write)?;
    principal.grant.validate()?;
    validate_identity("/actor", &principal.actor)?;
    if matches!(principal.actor.as_str(), "unknown" | "anonymous") {
        return Err(invalid("/actor", "expected_exact_actor"));
    }
    context.verify_operation(read)?;
    context.checkpoint()?;
    let wall = clock.unix_millis();
    let monotonic = clock.monotonic_millis();
    let deletion = matches!(intent.action, Action::Delete(_));
    let (before, raw_before) = observe(context, &write.request().target.id, deletion)?;
    if before != intent.expected {
        return Err(changed());
    }
    let evidence = match &intent.action {
        Action::Assign(expected, _) | Action::Unassign(expected) => {
            let before = assignee(&raw_before)?;
            if &before != expected {
                return Err(changed());
            }
            let intended = if let Action::Assign(_, account) = &intent.action {
                context.verify_operation(read)?;
                check_age(wall, monotonic, clock)?;
                qualify_account(context, &write.request().target.id, account)?;
                Assignee::Account {
                    account_id: account.clone(),
                }
            } else {
                Assignee::Unassigned
            };
            ActionEvidence::Assignment { before, intended }
        }
        Action::Delete(subtasks) => {
            let observed = children(&raw_before, &before.id)?;
            let expected = match subtasks {
                SubtaskDeletion::Refuse => &[][..],
                SubtaskDeletion::DeleteAll {
                    expected_visible_ids,
                } => expected_visible_ids.as_slice(),
            };
            if observed != expected {
                return Err(changed());
            }
            let cascade = matches!(subtasks, SubtaskDeletion::DeleteAll { .. });
            ActionEvidence::Deletion {
                observed_subtasks: observed,
                delete_all_subtasks: cascade,
                includes_unobserved_subtasks: cascade,
                hierarchy_complete: false,
            }
        }
    };
    context.verify_operation(read)?;
    check_age(wall, monotonic, clock)?;
    let (after, raw_after) = observe(context, &before.id, deletion)?;
    if after != before {
        return Err(changed());
    }
    match &evidence {
        ActionEvidence::Assignment { before, .. } if assignee(&raw_after)? != *before => {
            return Err(changed());
        }
        ActionEvidence::Deletion {
            observed_subtasks, ..
        } if children(&raw_after, &after.id)? != *observed_subtasks => return Err(changed()),
        _ => {}
    }
    let (route, query, body) = match &intent.action {
        Action::Assign(_, account) => (
            ASSIGN_ROUTE,
            BTreeMap::new(),
            Some(json!({"accountId":account})),
        ),
        Action::Unassign(_) => (
            ASSIGN_ROUTE,
            BTreeMap::new(),
            Some(json!({"accountId":null})),
        ),
        Action::Delete(subtasks) => (
            DELETE_ROUTE,
            BTreeMap::from([(
                "deleteSubtasks".into(),
                matches!(subtasks, SubtaskDeletion::DeleteAll { .. }).to_string(),
            )]),
            None,
        ),
    };
    let frozen = FrozenIssueMutation {
        call: NativeCall {
            route: route.into(),
            path_parameters: BTreeMap::from([("issueIdOrKey".into(), before.id.clone())]),
            query,
            repeated_query: BTreeMap::new(),
            body,
        },
        operation: write.digest().into(),
        principal: principal.clone(),
        wall,
        monotonic,
        data: MutationData {
            target: write.request().target.clone(),
            issue_before: before,
            destination_status_id: None,
        },
        action: Some(evidence),
    };
    context.checkpoint()?;
    context.verify_operation(read)?;
    frozen.verify(write, principal, clock)?;
    Ok(frozen)
}
fn intent_for_read(read: &PreparedOperation, write: &PreparedOperation) -> Result<(), CraftError> {
    validate_request(read.request())?;
    let expected_effect = if write.request().command == DELETE_ISSUE {
        matches!(write.effect(), EffectClass::Permanent)
    } else {
        matches!(write.effect(), EffectClass::Write)
    };
    if !matches!(read.effect(), EffectClass::Read)
        || !expected_effect
        || read.request().command != metadata_command(&write.request().command)?
        || read.request().target != write.request().target
        || read.request().input != write.request().input
    {
        return Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "Issue action context differs from its write operation.",
        ));
    }
    Ok(())
}
