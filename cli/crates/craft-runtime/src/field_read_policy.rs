//! Exact field discovery authority is independent of issue and project permissions.

use craft_core::contracts::{AuthMode, GrantRef, ResourceRef, validate_identity};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::input::parse_object;
use craft_core::profiles::Profile;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::profiles::{ProfileStore, begin, commit, invalid_state, state_error};

pub(crate) const TABLE: &str = "CREATE TABLE field_read_policies (profile TEXT PRIMARY KEY COLLATE BINARY REFERENCES profiles(name), revision INTEGER NOT NULL CHECK(revision>=1), payload TEXT NOT NULL CHECK(length(CAST(payload AS BLOB))<=65536))";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum FieldReadOperation {
    #[serde(rename = "jira.field.list")]
    List,
    #[serde(rename = "jira.field.search")]
    Search,
}
impl FieldReadOperation {
    pub fn command(self) -> &'static str {
        match self {
            Self::List => "jira.field.list",
            Self::Search => "jira.field.search",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldReadPolicyRecord {
    pub format: u8,
    pub installation: String,
    pub revision: u64,
    pub profile: String,
    pub grant: GrantRef,
    pub target: ResourceRef,
    pub expected_actor: String,
    pub continuation_site: String,
    pub operations: Vec<FieldReadOperation>,
    pub enabled: bool,
}
pub struct FieldReadPolicyUpdate {
    pub target: ResourceRef,
    pub expected_actor: String,
    pub continuation_site: String,
    pub operations: Vec<FieldReadOperation>,
    pub enabled: bool,
    pub expected_revision: Option<u64>,
}
fn validate(
    profile: &Profile,
    target: &ResourceRef,
    actor: &str,
    origin: &str,
    operations: &[FieldReadOperation],
) -> Result<(), CraftError> {
    profile.validate_target("site", target)?;
    crate::transport::product_identity_url(&target.tenant_ref())?;
    validate_identity("/expected_actor", actor)?;
    let site = crate::transport::JiraFieldSite::parse(origin)?;
    if site.as_str() != origin {
        return Err(CraftError::field(
            "/continuation_site",
            "expected_canonical_origin",
        ));
    }
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
    if operations.is_empty()
        || operations.len() > 2
        || operations.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err(CraftError::field(
            "/operations",
            "expected_nonempty_unique_canonical_field_reads",
        ));
    }
    Ok(())
}
impl FieldReadPolicyRecord {
    fn validate(&self, store: &ProfileStore, profile: &Profile) -> Result<(), CraftError> {
        if self.format != 1
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
            &self.continuation_site,
            &self.operations,
        )
        .map_err(|_| invalid_state())
    }
}
fn read(
    store: &ProfileStore,
    profile: &Profile,
) -> Result<Option<FieldReadPolicyRecord>, CraftError> {
    let record = store
        .connection
        .query_row(
            "SELECT revision,payload FROM field_read_policies WHERE profile=?1",
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
                let record: FieldReadPolicyRecord =
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
    /// Reads strictly validated non-secret configuration; missing records grant no field access.
    pub fn field_read_policy(
        &self,
        profile: &str,
        control: &InvocationControl,
    ) -> Result<Option<FieldReadPolicyRecord>, CraftError> {
        control.check()?;
        let profile = self.get(profile, control)?;
        let record = read(self, &profile)?;
        control.check()?;
        Ok(record)
    }
    /// Trusted-host setter with apply, exact expected revision and output preflight before commit.
    /// An absent expected revision only creates; uncertain commits must be inspected before retry.
    pub fn set_field_read_policy(
        &self,
        profile: &str,
        mut update: FieldReadPolicyUpdate,
        apply: bool,
        control: &InvocationControl,
        preflight: impl FnOnce(&FieldReadPolicyRecord) -> Result<(), CraftError>,
    ) -> Result<FieldReadPolicyRecord, CraftError> {
        control.check()?;
        if !apply {
            return Err(CraftError::new(
                ErrorCode::ApplyRequired,
                "Local field-read configuration changes require explicit apply.",
            ));
        }
        update.operations.sort();
        update.continuation_site =
            crate::transport::JiraFieldSite::parse(&update.continuation_site)?
                .as_str()
                .into();
        let transaction = begin(&self.connection, control)?;
        let profile = self.get(profile, control)?;
        validate(
            &profile,
            &update.target,
            &update.expected_actor,
            &update.continuation_site,
            &update.operations,
        )?;
        let previous = read(self, &profile)?;
        if previous.as_ref().map(|record| record.revision) != update.expected_revision {
            return Err(CraftError::new(
                ErrorCode::VersionConflict,
                "Issue-read configuration revision changed; inspect it before updating.",
            ));
        }
        if previous.as_ref().is_some_and(|record| {
            record.enabled
                && (record.expected_actor != update.expected_actor
                    || record.target != update.target
                    || record.continuation_site != update.continuation_site)
        }) {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Disable the field-read record before transferring actor, target or continuation origin.",
            ));
        }
        let record = FieldReadPolicyRecord {
            format: 1,
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
            operations: update.operations,
            enabled: update.enabled,
        };
        record.validate(self, &profile)?;
        preflight(&record)?;
        let payload = serde_json::to_string(&record).map_err(|_| invalid_state())?;
        if payload.len() > 65_536 {
            return Err(invalid_state());
        }
        control.check()?;
        transaction.execute("INSERT INTO field_read_policies(profile,revision,payload) VALUES(?1,?2,?3) ON CONFLICT(profile) DO UPDATE SET revision=excluded.revision,payload=excluded.payload",params![record.profile,record.revision as i64,payload]).map_err(state_error)?;
        commit(transaction,Some(control)).map_err(|mut error|{error.next_action="Read the named field-read configuration under the same state root before retrying; preserve competing revisions.".into();error})?;
        Ok(record)
    }
    /// Rechecks operation allowance and the full captured record, including enabled/revision state.
    pub fn verify_field_read_policy(
        &self,
        expected: &FieldReadPolicyRecord,
        operation: FieldReadOperation,
        control: &InvocationControl,
    ) -> Result<(), CraftError> {
        let current = self
            .field_read_policy(&expected.profile, control)?
            .ok_or_else(|| {
                CraftError::new(
                    ErrorCode::PolicyDenied,
                    "No trusted field-read configuration is present.",
                )
            })?;
        if !current.enabled || !current.operations.contains(&operation) {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Trusted field-read configuration does not enable this operation.",
            ));
        }
        if current != *expected {
            return Err(CraftError::new(
                ErrorCode::VersionConflict,
                "Trusted field-read configuration changed during execution.",
            ));
        }
        Ok(())
    }
}
