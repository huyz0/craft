use super::*;
use craft_runtime::confluence_read_policy::{ConfluenceReadOperation, ConfluenceReadPolicyUpdate};
use craft_runtime::confluence_write_policy::{
    ConfluenceWriteOperation, ConfluenceWritePolicyUpdate,
};
use craft_runtime::create_policy::CreatePolicyUpdate;
use craft_runtime::field_read_policy::{FieldReadOperation, FieldReadPolicyUpdate};
use craft_runtime::issue_mutation_policy::{IssueMutationOperation, IssueMutationPolicyUpdate};
use craft_runtime::issue_read_policy::{IssueReadOperation, IssueReadPolicyUpdate};
use craft_runtime::issue_recovery_policy::{IssueRecoveryOperation, IssueRecoveryPolicyUpdate};
use craft_runtime::project_read_policy::{
    ProjectContextOperation, ProjectContextPermissionUpdate, ProjectReadPolicyUpdate,
};

fn operations<T: serde::de::DeserializeOwned>(values: &[String]) -> Result<Vec<T>, CraftError> {
    serde_json::from_value(json!(values))
        .map_err(|_| invalid("/operations", "operation_not_supported_by_policy_kind"))
}
pub(super) fn validate_operations(kind: PolicyKind, values: &[String]) -> Result<(), CraftError> {
    let unique: std::collections::BTreeSet<_> = values.iter().collect();
    if values.is_empty()
        || values.len()
            > if kind == PolicyKind::JiraMutation {
                16
            } else if kind == PolicyKind::JiraRead {
                14
            } else if matches!(kind, PolicyKind::ConfluenceRead | PolicyKind::JiraRecovery) {
                8
            } else {
                6
            }
        || unique.len() != values.len()
    {
        return Err(invalid(
            "/operations",
            "expected_bounded_unique_explicit_operations",
        ));
    }
    match kind {
        PolicyKind::ConfluenceRead => {
            operations::<ConfluenceReadOperation>(values)?;
        }
        PolicyKind::ConfluenceWrite => {
            operations::<ConfluenceWriteOperation>(values)?;
        }
        PolicyKind::JiraRead => {
            operations::<IssueReadOperation>(values)?;
        }
        PolicyKind::JiraProjectRead if values == ["jira.project.list"] => {}
        PolicyKind::JiraProjectContext => {
            operations::<ProjectContextOperation>(values)?;
        }
        PolicyKind::JiraFieldRead => {
            operations::<FieldReadOperation>(values)?;
        }
        PolicyKind::JiraMutation => {
            operations::<IssueMutationOperation>(values)?;
        }
        PolicyKind::JiraRecovery => {
            operations::<IssueRecoveryOperation>(values)?;
        }
        PolicyKind::JiraCreate if values == ["jira.issue.create"] => {}
        _ => {
            return Err(invalid(
                "/operations",
                "create_policy_only_allows_issue_create",
            ));
        }
    }
    Ok(())
}
fn snapshot(record: &impl Serialize, kind: PolicyKind) -> Result<PolicySnapshot, CraftError> {
    let mut raw = serde_json::to_value(record)
        .map_err(|_| invalid("/policy", "cannot_encode_saved_policy"))?;
    let object = raw
        .as_object_mut()
        .ok_or_else(|| invalid("/policy", "invalid_saved_policy"))?;
    for name in ["format", "installation", "profile"] {
        object.remove(name);
    }
    if kind == PolicyKind::JiraCreate {
        object.insert("operations".into(), json!(["jira.issue.create"]));
    }
    if matches!(
        kind,
        PolicyKind::JiraProjectRead | PolicyKind::JiraProjectContext
    ) {
        object.insert("operations".into(), json!(["jira.project.list"]));
    }
    serde_json::from_value(raw).map_err(|_| invalid("/policy", "unsupported_saved_policy_shape"))
}
pub(super) fn get(
    store: &ProfileStore,
    name: &str,
    kind: PolicyKind,
    target: &ResourceRef,
    control: &InvocationControl,
) -> Result<Option<PolicySnapshot>, CraftError> {
    match kind {
        PolicyKind::ConfluenceRead => store
            .confluence_read_policy(name, control)?
            .map(|r| snapshot(&r, kind))
            .transpose(),
        PolicyKind::ConfluenceWrite => store
            .confluence_write_policy(name, &target.id, control)?
            .map(|r| snapshot(&r, kind))
            .transpose(),
        PolicyKind::JiraCreate => store
            .create_policy(name, control)?
            .map(|r| snapshot(&r, kind))
            .transpose(),
        PolicyKind::JiraRead => store
            .issue_read_policy(name, control)?
            .map(|r| snapshot(&r, kind))
            .transpose(),
        PolicyKind::JiraProjectRead | PolicyKind::JiraProjectContext => store
            .project_read_policy(name, control)?
            .map(|r| snapshot(&r, kind))
            .transpose(),
        PolicyKind::JiraFieldRead => store
            .field_read_policy(name, control)?
            .map(|r| snapshot(&r, kind))
            .transpose(),
        PolicyKind::JiraMutation => store
            .issue_mutation_policy(name, &target.id, control)?
            .map(|r| snapshot(&r, kind))
            .transpose(),
        PolicyKind::JiraRecovery => store
            .issue_recovery_policy(name, &target.id, control)?
            .map(|r| snapshot(&r, kind))
            .transpose(),
    }
}
pub(super) fn set(
    store: &ProfileStore,
    name: &str,
    args: &PolicySetArgs,
    control: &InvocationControl,
    before: impl FnOnce(PolicySnapshot) -> Result<(), CraftError>,
) -> Result<PolicySnapshot, CraftError> {
    let kind = args.kind;
    let target = args.target.clone();
    let expected_actor = args.expected_actor.clone();
    let enabled = args.enabled == Some(true);
    let expected_revision = args.expected_revision;
    match kind {
        PolicyKind::JiraProjectRead => {
            let r = store.set_project_read_policy(
                name,
                ProjectReadPolicyUpdate {
                    target,
                    expected_actor,
                    enabled,
                    expected_revision,
                    continuation_site: args
                        .continuation_site
                        .clone()
                        .ok_or_else(|| invalid("/continuation_site", "required"))?,
                },
                args.apply,
                control,
                |r| before(snapshot(r, kind)?),
            )?;
            snapshot(&r, kind)
        }
        PolicyKind::JiraProjectContext => {
            let current = store.project_read_policy(name, control)?.ok_or_else(|| {
                CraftError::new(
                    ErrorCode::PolicyDenied,
                    "Configure the project-read site and actor first.",
                )
            })?;
            if current.expected_actor != expected_actor || current.target.tenant != target.tenant {
                return Err(CraftError::new(
                    ErrorCode::PolicyDenied,
                    "Project-context updates must retain the configured site actor.",
                ));
            }
            let r = store.set_project_context_permission(
                name,
                ProjectContextPermissionUpdate {
                    target,
                    enabled,
                    operations: operations(&args.operations)?,
                    expected_revision: expected_revision
                        .ok_or_else(|| invalid("/expected_revision", "required"))?,
                },
                args.apply,
                control,
                |r| before(snapshot(r, kind)?),
            )?;
            snapshot(&r, kind)
        }
        PolicyKind::JiraFieldRead => {
            let r = store.set_field_read_policy(
                name,
                FieldReadPolicyUpdate {
                    target,
                    expected_actor,
                    enabled,
                    expected_revision,
                    operations: operations(&args.operations)?,
                    continuation_site: args
                        .continuation_site
                        .clone()
                        .ok_or_else(|| invalid("/continuation_site", "required"))?,
                },
                args.apply,
                control,
                |r| before(snapshot(r, kind)?),
            )?;
            snapshot(&r, kind)
        }
        PolicyKind::ConfluenceRead => {
            let r = store.set_confluence_read_policy(
                name,
                ConfluenceReadPolicyUpdate {
                    target,
                    expected_actor,
                    enabled,
                    expected_revision,
                    operations: operations(&args.operations)?,
                },
                args.apply,
                control,
                |r| before(snapshot(r, kind)?),
            )?;
            snapshot(&r, kind)
        }
        PolicyKind::ConfluenceWrite => {
            let r = store.set_confluence_write_policy(
                name,
                ConfluenceWritePolicyUpdate {
                    target,
                    expected_actor,
                    enabled,
                    expected_revision,
                    operations: operations(&args.operations)?,
                },
                args.apply,
                control,
                |r| before(snapshot(r, kind)?),
            )?;
            snapshot(&r, kind)
        }
        PolicyKind::JiraCreate => {
            let r = store.set_create_policy(
                name,
                CreatePolicyUpdate {
                    target,
                    expected_actor,
                    enabled,
                    expected_revision,
                },
                args.apply,
                control,
                |r| before(snapshot(r, kind)?),
            )?;
            snapshot(&r, kind)
        }
        PolicyKind::JiraRead => {
            let r = store.set_issue_read_policy(
                name,
                IssueReadPolicyUpdate {
                    target,
                    expected_actor,
                    enabled,
                    expected_revision,
                    operations: operations(&args.operations)?,
                },
                args.apply,
                control,
                |r| before(snapshot(r, kind)?),
            )?;
            snapshot(&r, kind)
        }
        PolicyKind::JiraMutation => {
            let r = store.set_issue_mutation_policy(
                name,
                IssueMutationPolicyUpdate {
                    target,
                    expected_actor,
                    enabled,
                    expected_revision,
                    operations: operations(&args.operations)?,
                },
                args.apply,
                control,
                |r| before(snapshot(r, kind)?),
            )?;
            snapshot(&r, kind)
        }
        PolicyKind::JiraRecovery => {
            let r = store.set_issue_recovery_policy(
                name,
                IssueRecoveryPolicyUpdate {
                    target,
                    expected_actor,
                    enabled,
                    expected_revision,
                    operations: operations(&args.operations)?,
                },
                args.apply,
                control,
                |r| before(snapshot(r, kind)?),
            )?;
            snapshot(&r, kind)
        }
    }
}
