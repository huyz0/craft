//! Explicit read-only investigation authority, independent of resource write and read policies.
use crate::profiles::{ProfileStore, begin, commit, invalid_state, state_error};
use craft_core::contracts::{AuthMode, GrantRef, ResourceRef, validate_identity};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::input::parse_object;
use craft_core::profiles::Profile;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

pub(crate) const TABLE: &str = "CREATE TABLE issue_recovery_policies (profile TEXT NOT NULL COLLATE BINARY REFERENCES profiles(name), project_id TEXT NOT NULL COLLATE BINARY, revision INTEGER NOT NULL CHECK(revision>=1), payload TEXT NOT NULL CHECK(length(CAST(payload AS BLOB))<=65536), PRIMARY KEY(profile,project_id))";
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum IssueRecoveryOperation {
    #[serde(rename = "jira.issue.create")]
    Create,
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
    #[serde(rename = "jira.link.create")]
    LinkCreate,
    #[serde(rename = "jira.link.delete")]
    LinkDelete,
}
impl IssueRecoveryOperation {
    /// Original write identity, used only to authorize read-only investigation.
    pub fn command(self) -> &'static str {
        match self {
            Self::Create => "jira.issue.create",
            Self::Edit => "jira.issue.update",
            Self::Transition => "jira.transition.apply",
            Self::Assign => "jira.issue.assign",
            Self::Unassign => "jira.issue.unassign",
            Self::Delete => "jira.issue.delete",
            Self::LinkCreate => "jira.link.create",
            Self::LinkDelete => "jira.link.delete",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IssueRecoveryPolicyRecord {
    pub format: u8,
    pub installation: String,
    pub revision: u64,
    pub profile: String,
    pub grant: GrantRef,
    pub target: ResourceRef,
    pub expected_actor: String,
    pub enabled: bool,
    pub operations: Vec<IssueRecoveryOperation>,
}
pub struct IssueRecoveryPolicyUpdate {
    pub target: ResourceRef,
    pub expected_actor: String,
    pub enabled: bool,
    pub operations: Vec<IssueRecoveryOperation>,
    pub expected_revision: Option<u64>,
}
fn numeric(id: &str) -> bool {
    !id.is_empty() && id.len() <= 512 && id.bytes().all(|byte| byte.is_ascii_digit())
}
fn validate(
    profile: &Profile,
    target: &ResourceRef,
    actor: &str,
    operations: &[IssueRecoveryOperation],
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
        || operations.len() > 8
        || operations.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err(CraftError::field(
            "/operations",
            "expected_unique_explicit_recovery_operations",
        ));
    }
    Ok(())
}
impl IssueRecoveryPolicyRecord {
    fn validate(&self, store: &ProfileStore, profile: &Profile) -> Result<(), CraftError> {
        if !matches!(self.format, 1 | 2)
            || (self.format == 1
                && self.operations.iter().any(|op| {
                    matches!(
                        op,
                        IssueRecoveryOperation::LinkCreate | IssueRecoveryOperation::LinkDelete
                    )
                }))
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
) -> Result<Option<IssueRecoveryPolicyRecord>, CraftError> {
    if !numeric(project) {
        return Err(CraftError::field(
            "/project_id",
            "expected_numeric_project_id",
        ));
    }
    let record=store.connection.query_row("SELECT revision,payload FROM issue_recovery_policies WHERE profile=?1 AND project_id=?2",params![profile.name,project],|row|{
        let revision:i64=row.get(0)?;let payload=row.get_ref(1)?.as_str().map_err(|_|rusqlite::Error::InvalidQuery)?;
        if payload.len()>65_536{return Err(rusqlite::Error::InvalidQuery);}
        let value=parse_object(payload.as_bytes()).map_err(|_|rusqlite::Error::InvalidQuery)?;
        let record:IssueRecoveryPolicyRecord=serde_json::from_value(value).map_err(|_|rusqlite::Error::InvalidQuery)?;
        if revision<1||record.revision!=revision as u64||record.target.id!=project {return Err(rusqlite::Error::InvalidQuery);}
        Ok(record)
    }).optional().map_err(|_|invalid_state())?;
    if let Some(record) = &record {
        record.validate(store, profile)?;
    }
    Ok(record)
}
impl ProfileStore {
    pub fn issue_recovery_policy(
        &self,
        profile: &str,
        project: &str,
        control: &InvocationControl,
    ) -> Result<Option<IssueRecoveryPolicyRecord>, CraftError> {
        control.check()?;
        let profile = self.get(profile, control)?;
        let record = read(self, &profile, project)?;
        control.check()?;
        Ok(record)
    }
    /// Trusted administration requires apply, receipt capacity and an exact row revision.
    pub fn set_issue_recovery_policy(
        &self,
        profile: &str,
        mut update: IssueRecoveryPolicyUpdate,
        apply: bool,
        control: &InvocationControl,
        preflight: impl FnOnce(&IssueRecoveryPolicyRecord) -> Result<(), CraftError>,
    ) -> Result<IssueRecoveryPolicyRecord, CraftError> {
        control.check()?;
        if !apply {
            return Err(CraftError::new(
                ErrorCode::ApplyRequired,
                "Recovery policy changes require explicit apply.",
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
                "Recovery policy revision changed; inspect that exact project record.",
            ));
        }
        if previous.as_ref().is_some_and(|record| {
            record.enabled
                && (record.expected_actor != update.expected_actor
                    || record.target != update.target)
        }) {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Disable the recovery record before transferring its actor or target.",
            ));
        }
        if previous.is_none() {
            let count: i64 = transaction
                .query_row(
                    "SELECT count(*) FROM issue_recovery_policies WHERE profile=?1",
                    [&profile.name],
                    |row| row.get(0),
                )
                .map_err(state_error)?;
            if count >= 256 {
                return Err(CraftError::field(
                    "/policy",
                    "too_many_project_recovery_records",
                ));
            }
        }
        let record = IssueRecoveryPolicyRecord {
            format: if update.operations.iter().any(|op| {
                matches!(
                    op,
                    IssueRecoveryOperation::LinkCreate | IssueRecoveryOperation::LinkDelete
                )
            }) {
                2
            } else {
                1
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
        transaction.execute("INSERT INTO issue_recovery_policies(profile,project_id,revision,payload) VALUES(?1,?2,?3,?4) ON CONFLICT(profile,project_id) DO UPDATE SET revision=excluded.revision,payload=excluded.payload",params![record.profile,record.target.id,record.revision as i64,payload]).map_err(state_error)?;
        commit(transaction,Some(control)).map_err(|mut error|{error.next_action="Read the exact project recovery record before retrying; preserve competing revisions.".into();error})?;
        Ok(record)
    }
    pub fn verify_issue_recovery_policy(
        &self,
        expected: &IssueRecoveryPolicyRecord,
        operation: IssueRecoveryOperation,
        control: &InvocationControl,
    ) -> Result<(), CraftError> {
        let current = self
            .issue_recovery_policy(&expected.profile, &expected.target.id, control)?
            .ok_or_else(|| {
                CraftError::new(
                    ErrorCode::PolicyDenied,
                    "No explicit project recovery policy is configured.",
                )
            })?;
        if !current.enabled || !current.operations.contains(&operation) {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "This project recovery operation is not enabled.",
            ));
        }
        if current != *expected {
            return Err(CraftError::new(
                ErrorCode::VersionConflict,
                "Captured recovery policy changed during execution.",
            ));
        }
        Ok(())
    }
}
