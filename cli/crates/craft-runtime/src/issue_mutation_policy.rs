//! Private per-project mutation intent, independent of create and read permissions.
use crate::profiles::{ProfileStore, begin, commit, invalid_state, state_error};
use craft_core::contracts::{AuthMode, GrantRef, ResourceRef, validate_identity};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::input::parse_object;
use craft_core::profiles::Profile;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

pub(crate) const TABLE: &str = "CREATE TABLE issue_mutation_policies (profile TEXT NOT NULL COLLATE BINARY REFERENCES profiles(name), project_id TEXT NOT NULL COLLATE BINARY, revision INTEGER NOT NULL CHECK(revision>=1), payload TEXT NOT NULL CHECK(length(CAST(payload AS BLOB))<=65536), PRIMARY KEY(profile,project_id))";
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum IssueMutationOperation {
    #[serde(rename = "jira.issue.update")]
    Edit,
    #[serde(rename = "jira.transition.apply")]
    Transition,
    #[serde(rename = "jira.issue.assign")]
    Assign,
    #[serde(rename = "jira.issue.unassign")]
    Unassign,
    #[serde(rename = "jira.issue.delete")]
    Delete,
    #[serde(rename = "jira.issue.delete-all-subtasks")]
    DeleteAllSubtasks,
    #[serde(rename = "jira.link.create")]
    Link,
    #[serde(rename = "jira.link.delete")]
    LinkDelete,
    #[serde(rename = "jira.comment.create")]
    CommentCreate,
    #[serde(rename = "jira.comment.update-own")]
    CommentUpdateOwn,
    #[serde(rename = "jira.comment.update-any")]
    CommentUpdateAny,
    #[serde(rename = "jira.comment.delete-own")]
    CommentDeleteOwn,
    #[serde(rename = "jira.comment.delete-any")]
    CommentDeleteAny,
    #[serde(rename = "jira.attachment.upload")]
    AttachmentUpload,
    #[serde(rename = "jira.attachment.delete-own")]
    AttachmentDeleteOwn,
    #[serde(rename = "jira.attachment.delete-any")]
    AttachmentDeleteAny,
}
impl IssueMutationOperation {
    /// Native command identity; ordinary and cascading deletion retain distinct policy values.
    pub fn command(self) -> &'static str {
        match self {
            Self::AttachmentUpload => "jira.attachment.upload",
            Self::AttachmentDeleteOwn | Self::AttachmentDeleteAny => "jira.attachment.delete",
            Self::Edit => "jira.issue.update",
            Self::Transition => "jira.transition.apply",
            Self::Assign => "jira.issue.assign",
            Self::Unassign => "jira.issue.unassign",
            Self::Delete | Self::DeleteAllSubtasks => "jira.issue.delete",
            Self::Link => "jira.link.create",
            Self::LinkDelete => "jira.link.delete",
            Self::CommentCreate => "jira.comment.create",
            Self::CommentUpdateOwn | Self::CommentUpdateAny => "jira.comment.update",
            Self::CommentDeleteOwn | Self::CommentDeleteAny => "jira.comment.delete",
        }
    }
    fn is_comment(self) -> bool {
        matches!(
            self,
            Self::CommentCreate
                | Self::CommentUpdateOwn
                | Self::CommentUpdateAny
                | Self::CommentDeleteOwn
                | Self::CommentDeleteAny
        )
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IssueMutationPolicyRecord {
    pub format: u8,
    pub installation: String,
    pub revision: u64,
    pub profile: String,
    pub grant: GrantRef,
    pub target: ResourceRef,
    pub expected_actor: String,
    pub enabled: bool,
    pub operations: Vec<IssueMutationOperation>,
}
pub struct IssueMutationPolicyUpdate {
    pub target: ResourceRef,
    pub expected_actor: String,
    pub enabled: bool,
    pub operations: Vec<IssueMutationOperation>,
    pub expected_revision: Option<u64>,
}
fn numeric(id: &str) -> bool {
    !id.is_empty() && id.len() <= 512 && id.bytes().all(|byte| byte.is_ascii_digit())
}
fn validate(
    profile: &Profile,
    target: &ResourceRef,
    actor: &str,
    operations: &[IssueMutationOperation],
) -> Result<(), CraftError> {
    profile.validate_target("project", target)?;
    crate::transport::product_identity_url(&target.tenant_ref())?;
    validate_identity("/expected_actor", actor)?;
    if profile.provider != "jira"
        || profile.deployment != "cloud"
        || profile.auth_mode != AuthMode::Service
        || !numeric(&target.id)
        || matches!(actor, "unknown" | "anonymous")
    {
        return Err(CraftError::field(
            "/policy",
            "expected_exact_jira_service_project_and_actor",
        ));
    }
    if operations.is_empty()
        || operations.len() > 16
        || operations.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err(CraftError::field(
            "/operations",
            "expected_unique_explicit_mutation_operations",
        ));
    }
    Ok(())
}
impl IssueMutationPolicyRecord {
    fn validate(&self, store: &ProfileStore, profile: &Profile) -> Result<(), CraftError> {
        // Recognizing a new operation name must not make it valid in a legacy payload.
        if !matches!(self.format, 1..=7)
            || (self.format < 7
                && self.operations.iter().any(|op| {
                    matches!(
                        op,
                        IssueMutationOperation::AttachmentDeleteOwn
                            | IssueMutationOperation::AttachmentDeleteAny
                    )
                }))
            || (self.format < 6
                && self
                    .operations
                    .contains(&IssueMutationOperation::AttachmentUpload))
            || (self.format == 1
                && self.operations.iter().any(|operation| {
                    !matches!(
                        operation,
                        IssueMutationOperation::Edit | IssueMutationOperation::Transition
                    )
                }))
            || (self.format < 3 && self.operations.contains(&IssueMutationOperation::Link))
            || (self.format < 4
                && self
                    .operations
                    .contains(&IssueMutationOperation::LinkDelete))
            || (self.format < 5 && self.operations.iter().any(|op| op.is_comment()))
            || self.installation != store.settings().installation_id
            || self.profile != profile.name
            || self.grant != profile.grant
            || self.revision == 0
            || self.revision > i64::MAX as u64
        {
            return Err(invalid_state());
        }
        validate(
            profile,
            &self.target,
            &self.expected_actor,
            &self.operations,
        )
        .map_err(|_| invalid_state())
    }
}
fn read(
    store: &ProfileStore,
    profile: &Profile,
    project: &str,
) -> Result<Option<IssueMutationPolicyRecord>, CraftError> {
    if !numeric(project) {
        return Err(CraftError::field(
            "/project_id",
            "expected_numeric_project_id",
        ));
    }
    let record=store.connection.query_row("SELECT revision,payload FROM issue_mutation_policies WHERE profile=?1 AND project_id=?2",params![profile.name,project],|row|{
        let revision:i64=row.get(0)?;let payload=row.get_ref(1)?.as_str().map_err(|_|rusqlite::Error::InvalidQuery)?;
        if payload.len()>65_536{return Err(rusqlite::Error::InvalidQuery);}
        let value=parse_object(payload.as_bytes()).map_err(|_|rusqlite::Error::InvalidQuery)?;
        let record:IssueMutationPolicyRecord=serde_json::from_value(value).map_err(|_|rusqlite::Error::InvalidQuery)?;
        if revision<1||record.revision!=revision as u64||record.target.id!=project {return Err(rusqlite::Error::InvalidQuery);}
        Ok(record)
    }).optional().map_err(|_|invalid_state())?;
    if let Some(record) = &record {
        record.validate(store, profile)?;
    }
    Ok(record)
}
impl ProfileStore {
    pub fn issue_mutation_policy(
        &self,
        profile: &str,
        project: &str,
        control: &InvocationControl,
    ) -> Result<Option<IssueMutationPolicyRecord>, CraftError> {
        control.check()?;
        let profile = self.get(profile, control)?;
        let record = read(self, &profile, project)?;
        control.check()?;
        Ok(record)
    }
    /// Trusted administration requires apply, receipt capacity and an exact row revision.
    pub fn set_issue_mutation_policy(
        &self,
        profile: &str,
        mut update: IssueMutationPolicyUpdate,
        apply: bool,
        control: &InvocationControl,
        preflight: impl FnOnce(&IssueMutationPolicyRecord) -> Result<(), CraftError>,
    ) -> Result<IssueMutationPolicyRecord, CraftError> {
        control.check()?;
        if !apply {
            return Err(CraftError::new(
                ErrorCode::ApplyRequired,
                "Mutation policy changes require explicit apply.",
            ));
        }
        update.operations.sort();
        let transaction = begin(&self.connection, control)?;
        let profile = self.get(profile, control)?;
        validate(
            &profile,
            &update.target,
            &update.expected_actor,
            &update.operations,
        )?;
        let previous = read(self, &profile, &update.target.id)?;
        if previous.as_ref().map(|record| record.revision) != update.expected_revision {
            return Err(CraftError::new(
                ErrorCode::VersionConflict,
                "Mutation policy revision changed; inspect that exact project record.",
            ));
        }
        if previous.as_ref().is_some_and(|record| {
            record.enabled
                && (record.expected_actor != update.expected_actor
                    || record.target != update.target)
        }) {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Disable the mutation record before transferring its actor or target.",
            ));
        }
        if previous.is_none() {
            let count: i64 = transaction
                .query_row(
                    "SELECT count(*) FROM issue_mutation_policies WHERE profile=?1",
                    [&profile.name],
                    |row| row.get(0),
                )
                .map_err(state_error)?;
            if count >= 256 {
                return Err(CraftError::field(
                    "/policy",
                    "too_many_project_mutation_records",
                ));
            }
        }
        let record = IssueMutationPolicyRecord {
            format: if update.operations.iter().any(|op| {
                matches!(
                    op,
                    IssueMutationOperation::AttachmentDeleteOwn
                        | IssueMutationOperation::AttachmentDeleteAny
                )
            }) {
                7
            } else if update
                .operations
                .contains(&IssueMutationOperation::AttachmentUpload)
            {
                6
            } else if update.operations.iter().any(|op| op.is_comment()) {
                5
            } else if update
                .operations
                .contains(&IssueMutationOperation::LinkDelete)
            {
                4
            } else if update.operations.contains(&IssueMutationOperation::Link) {
                3
            } else {
                2
            },
            installation: self.settings().installation_id.clone(),
            revision: update
                .expected_revision
                .unwrap_or(0)
                .checked_add(1)
                .filter(|revision| *revision <= i64::MAX as u64)
                .ok_or_else(invalid_state)?,
            profile: profile.name.clone(),
            grant: profile.grant.clone(),
            target: update.target,
            expected_actor: update.expected_actor,
            enabled: update.enabled,
            operations: update.operations,
        };
        record.validate(self, &profile)?;
        preflight(&record)?;
        let payload = serde_json::to_string(&record).map_err(|_| invalid_state())?;
        if payload.len() > 65_536 {
            return Err(invalid_state());
        }
        control.check()?;
        transaction.execute("INSERT INTO issue_mutation_policies(profile,project_id,revision,payload) VALUES(?1,?2,?3,?4) ON CONFLICT(profile,project_id) DO UPDATE SET revision=excluded.revision,payload=excluded.payload",params![record.profile,record.target.id,record.revision as i64,payload]).map_err(state_error)?;
        commit(transaction,Some(control)).map_err(|mut error|{error.next_action="Read the exact project mutation record before retrying; preserve competing revisions.".into();error})?;
        Ok(record)
    }
    pub fn verify_issue_mutation_policy(
        &self,
        expected: &IssueMutationPolicyRecord,
        operation: IssueMutationOperation,
        control: &InvocationControl,
    ) -> Result<(), CraftError> {
        let current = self
            .issue_mutation_policy(&expected.profile, &expected.target.id, control)?
            .ok_or_else(|| {
                CraftError::new(
                    ErrorCode::PolicyDenied,
                    "No explicit project mutation policy is configured.",
                )
            })?;
        if !current.enabled || !current.operations.contains(&operation) {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "This project mutation operation is not enabled.",
            ));
        }
        if current != *expected {
            return Err(CraftError::new(
                ErrorCode::VersionConflict,
                "Captured mutation policy changed during execution.",
            ));
        }
        Ok(())
    }
}
