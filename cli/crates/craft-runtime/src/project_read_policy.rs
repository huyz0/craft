//! Exact private read configuration. It supplies local intent, never remote permission or review.

use craft_core::contracts::{AuthMode, GrantRef, ResourceRef, validate_identity};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::input::parse_object;
use craft_core::profiles::Profile;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::profiles::{ProfileStore, begin, commit, invalid_state, state_error};
use crate::transport::JiraProjectSite;

mod context;
pub use context::{
    ProjectContextOperation, ProjectContextPermission, ProjectContextPermissionUpdate,
};

pub(crate) const TABLE: &str = "CREATE TABLE project_read_policies (profile TEXT PRIMARY KEY COLLATE BINARY REFERENCES profiles(name), revision INTEGER NOT NULL CHECK(revision>=1), payload TEXT NOT NULL CHECK(length(CAST(payload AS BLOB))<=65536))";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectReadPolicyRecord {
    pub format: u8,
    pub installation: String,
    pub revision: u64,
    pub profile: String,
    pub grant: GrantRef,
    pub target: ResourceRef,
    pub expected_actor: String,
    pub continuation_site: String,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_reads: Option<Vec<ProjectContextPermission>>,
}
pub struct ProjectReadPolicyUpdate {
    pub target: ResourceRef,
    pub expected_actor: String,
    pub continuation_site: String,
    pub enabled: bool,
    pub expected_revision: Option<u64>,
}
fn validate(
    profile: &Profile,
    target: &ResourceRef,
    actor: &str,
    site: &str,
) -> Result<(), CraftError> {
    profile.validate_target("site", target)?;
    crate::transport::product_identity_url(&target.tenant_ref())?;
    validate_identity("/expected_actor", actor)?;
    if profile.provider != "jira"
        || profile.deployment != "cloud"
        || profile.auth_mode != AuthMode::Service
        || target.id != target.tenant
        || matches!(actor, "anonymous" | "unknown")
    {
        return Err(CraftError::field(
            "/policy",
            "expected_exact_jira_service_site_and_actor",
        ));
    }
    if JiraProjectSite::parse(site)?.as_str() != site {
        return Err(CraftError::field(
            "/continuation_site",
            "expected_canonical_jira_site_origin",
        ));
    }
    Ok(())
}
impl ProjectReadPolicyRecord {
    fn validate(&self, store: &ProfileStore, profile: &Profile) -> Result<(), CraftError> {
        if !matches!(self.format, 1 | 2)
            || self.installation != store.settings().installation_id
            || self.profile != profile.name
            || self.grant != profile.grant
            || self.revision == 0
            || self.revision > i64::MAX as u64
        {
            return Err(invalid_state());
        }
        match (self.format, &self.context_reads) {
            (1, None) => {}
            (2, Some(permissions)) => {
                context::validate_permissions(profile, permissions).map_err(|_| invalid_state())?
            }
            _ => return Err(invalid_state()),
        }
        validate(
            profile,
            &self.target,
            &self.expected_actor,
            &self.continuation_site,
        )
        .map_err(|_| invalid_state())
    }
}
fn persist(
    transaction: rusqlite::Transaction<'_>,
    record: &ProjectReadPolicyRecord,
    control: &InvocationControl,
) -> Result<(), CraftError> {
    let payload = serde_json::to_string(record).map_err(|_| invalid_state())?;
    if payload.len() > 65_536 {
        return Err(invalid_state());
    }
    control.check()?;
    transaction.execute("INSERT INTO project_read_policies(profile,revision,payload) VALUES(?1,?2,?3) ON CONFLICT(profile) DO UPDATE SET revision=excluded.revision,payload=excluded.payload",params![record.profile,record.revision as i64,payload]).map_err(state_error)?;
    commit(transaction,Some(control)).map_err(|mut error|{error.next_action="Read the named project-read configuration under the same state root before retrying; preserve competing revisions.".into();error})
}
fn read(
    store: &ProfileStore,
    profile: &Profile,
) -> Result<Option<ProjectReadPolicyRecord>, CraftError> {
    let record = store
        .connection
        .query_row(
            "SELECT revision,payload FROM project_read_policies WHERE profile=?1",
            [&profile.name],
            |row| {
                let revision: i64 = row.get(0)?;
                let payload = row
                    .get_ref(1)?
                    .as_str()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?;
                if payload.len() > 65_536 {
                    return Err(rusqlite::Error::InvalidQuery);
                }
                let raw =
                    parse_object(payload.as_bytes()).map_err(|_| rusqlite::Error::InvalidQuery)?;
                let record: ProjectReadPolicyRecord =
                    serde_json::from_value(raw).map_err(|_| rusqlite::Error::InvalidQuery)?;
                if revision < 1 || record.revision != revision as u64 {
                    return Err(rusqlite::Error::InvalidQuery);
                }
                Ok(record)
            },
        )
        .optional()
        .map_err(|_| invalid_state())?;
    if let Some(record) = &record {
        record.validate(store, profile)?;
    }
    Ok(record)
}
impl ProfileStore {
    pub fn project_read_policy(
        &self,
        profile: &str,
        control: &InvocationControl,
    ) -> Result<Option<ProjectReadPolicyRecord>, CraftError> {
        control.check()?;
        let profile = self.get(profile, control)?;
        let record = read(self, &profile)?;
        control.check()?;
        Ok(record)
    }
    /// Trusted-host configuration only; absent revision creates, an exact revision updates.
    pub fn set_project_read_policy(
        &self,
        profile: &str,
        mut update: ProjectReadPolicyUpdate,
        apply: bool,
        control: &InvocationControl,
        preflight: impl FnOnce(&ProjectReadPolicyRecord) -> Result<(), CraftError>,
    ) -> Result<ProjectReadPolicyRecord, CraftError> {
        control.check()?;
        if !apply {
            return Err(CraftError::new(
                ErrorCode::ApplyRequired,
                "Local project-read configuration changes require explicit apply.",
            ));
        }
        update.continuation_site = JiraProjectSite::parse(&update.continuation_site)?
            .as_str()
            .into();
        let transaction = begin(&self.connection, control)?;
        let profile = self.get(profile, control)?;
        validate(
            &profile,
            &update.target,
            &update.expected_actor,
            &update.continuation_site,
        )?;
        let previous = read(self, &profile)?;
        if previous.as_ref().map(|record| record.revision) != update.expected_revision {
            return Err(CraftError::new(
                ErrorCode::VersionConflict,
                "Project-read configuration revision changed; inspect it before updating.",
            ));
        }
        if previous.as_ref().is_some_and(|record| {
            record.expected_actor != update.expected_actor
                && record.context_reads.as_ref().is_some_and(|permissions| {
                    permissions.iter().any(|permission| permission.enabled)
                })
        }) {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Disable enabled project-context permissions before changing the shared actor.",
            ));
        }
        let record = ProjectReadPolicyRecord {
            format: previous.as_ref().map_or(1, |record| record.format),
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
            continuation_site: update.continuation_site,
            enabled: update.enabled,
            context_reads: previous.and_then(|record| record.context_reads),
        };
        record.validate(self, &profile)?;
        preflight(&record)?;
        persist(transaction, &record, control)?;
        Ok(record)
    }
    /// Rechecks listing enablement and the full captured record; context permissions are separate.
    pub fn verify_project_read_policy(
        &self,
        expected: &ProjectReadPolicyRecord,
        control: &InvocationControl,
    ) -> Result<(), CraftError> {
        let current = self
            .project_read_policy(&expected.profile, control)?
            .ok_or_else(|| {
                CraftError::new(
                    ErrorCode::PolicyDenied,
                    "No trusted project-read configuration is present.",
                )
            })?;
        if !current.enabled {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Project listing is disabled in trusted read configuration.",
            ));
        }
        if current != *expected {
            return Err(CraftError::new(
                ErrorCode::VersionConflict,
                "Trusted project-read configuration changed during execution.",
            ));
        }
        Ok(())
    }
}
