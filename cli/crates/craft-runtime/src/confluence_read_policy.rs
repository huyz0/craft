//! Separate exact Confluence-read configuration; project discovery never supplies this authority.

use craft_core::contracts::{AuthMode, GrantRef, ResourceRef, validate_identity};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::input::parse_object;
use craft_core::profiles::Profile;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::profiles::{ProfileStore, begin, commit, invalid_state, state_error};

pub(crate) const TABLE: &str = "CREATE TABLE confluence_read_policies (profile TEXT PRIMARY KEY COLLATE BINARY REFERENCES profiles(name), revision INTEGER NOT NULL CHECK(revision>=1), payload TEXT NOT NULL CHECK(length(CAST(payload AS BLOB))<=65536))";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ConfluenceReadOperation {
    #[serde(rename = "confluence.space.get")]
    SpaceGet,
    #[serde(rename = "confluence.comment.get")]
    CommentGet,
    #[serde(rename = "confluence.space.list")]
    SpaceList,
    #[serde(rename = "confluence.page.get")]
    PageGet,
    #[serde(rename = "confluence.page.list")]
    PageList,
    #[serde(rename = "confluence.page.search")]
    PageSearch,
    #[serde(rename = "confluence.comment.list")]
    CommentList,
    #[serde(rename = "confluence.comment.replies")]
    CommentReplies,
    #[serde(rename = "confluence.content-state.get")]
    ContentStateGet,
    #[serde(rename = "confluence.content-state.available")]
    ContentStateAvailable,
}
impl ConfluenceReadOperation {
    pub fn command(self) -> &'static str {
        match self {
            Self::SpaceGet => "confluence.space.get",
            Self::CommentGet => "confluence.comment.get",
            Self::SpaceList => "confluence.space.list",
            Self::PageGet => "confluence.page.get",
            Self::PageList => "confluence.page.list",
            Self::PageSearch => "confluence.page.search",
            Self::CommentList => "confluence.comment.list",
            Self::CommentReplies => "confluence.comment.replies",
            Self::ContentStateGet => "confluence.content-state.get",
            Self::ContentStateAvailable => "confluence.content-state.available",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfluenceReadPolicyRecord {
    pub format: u8,
    pub installation: String,
    pub revision: u64,
    pub profile: String,
    pub grant: GrantRef,
    pub target: ResourceRef,
    pub expected_actor: String,
    pub operations: Vec<ConfluenceReadOperation>,
    pub enabled: bool,
}
pub struct ConfluenceReadPolicyUpdate {
    pub target: ResourceRef,
    pub expected_actor: String,
    pub operations: Vec<ConfluenceReadOperation>,
    pub enabled: bool,
    pub expected_revision: Option<u64>,
}
fn validate(
    profile: &Profile,
    target: &ResourceRef,
    actor: &str,
    operations: &[ConfluenceReadOperation],
) -> Result<(), CraftError> {
    profile.validate_target("site", target)?;
    crate::transport::product_identity_url(&target.tenant_ref())?;
    validate_identity("/expected_actor", actor)?;
    if profile.provider != "confluence"
        || profile.deployment != "cloud"
        || profile.auth_mode != AuthMode::Service
        || target.id != target.tenant
        || matches!(actor, "anonymous" | "unknown")
    {
        return Err(CraftError::field(
            "/policy",
            "expected_exact_confluence_service_site_and_actor",
        ));
    }
    if operations.is_empty()
        || operations.len() > 16
        || operations.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err(CraftError::field(
            "/operations",
            "expected_nonempty_unique_canonical_confluence_reads",
        ));
    }
    Ok(())
}
impl ConfluenceReadPolicyRecord {
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
            &self.operations,
        )
        .map_err(|_| invalid_state())
    }
}
fn read(
    store: &ProfileStore,
    profile: &Profile,
) -> Result<Option<ConfluenceReadPolicyRecord>, CraftError> {
    let record = store
        .connection
        .query_row(
            "SELECT revision,payload FROM confluence_read_policies WHERE profile=?1",
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
                let record: ConfluenceReadPolicyRecord =
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
    /// Reads strictly validated non-secret configuration; missing records grant no Confluence access.
    pub fn confluence_read_policy(
        &self,
        profile: &str,
        control: &InvocationControl,
    ) -> Result<Option<ConfluenceReadPolicyRecord>, CraftError> {
        control.check()?;
        let profile = self.get(profile, control)?;
        let record = read(self, &profile)?;
        control.check()?;
        Ok(record)
    }
    /// Trusted-host setter with apply, exact expected revision and output preflight before commit.
    /// An absent expected revision only creates; uncertain commits must be inspected before retry.
    pub fn set_confluence_read_policy(
        &self,
        profile: &str,
        mut update: ConfluenceReadPolicyUpdate,
        apply: bool,
        control: &InvocationControl,
        preflight: impl FnOnce(&ConfluenceReadPolicyRecord) -> Result<(), CraftError>,
    ) -> Result<ConfluenceReadPolicyRecord, CraftError> {
        control.check()?;
        if !apply {
            return Err(CraftError::new(
                ErrorCode::ApplyRequired,
                "Local Confluence-read configuration changes require explicit apply.",
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
                    || record.target != update.target)
        }) {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Disable the Confluence-read record before transferring actor or target.",
            ));
        }
        let record = ConfluenceReadPolicyRecord {
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
        transaction.execute("INSERT INTO confluence_read_policies(profile,revision,payload) VALUES(?1,?2,?3) ON CONFLICT(profile) DO UPDATE SET revision=excluded.revision,payload=excluded.payload",params![record.profile,record.revision as i64,payload]).map_err(state_error)?;
        commit(transaction,Some(control)).map_err(|mut error|{error.next_action="Read the named Confluence-read configuration under the same state root before retrying; preserve competing revisions.".into();error})?;
        Ok(record)
    }
    /// Rechecks operation allowance and the full captured record, including enabled/revision state.
    pub fn verify_confluence_read_policy(
        &self,
        expected: &ConfluenceReadPolicyRecord,
        operation: ConfluenceReadOperation,
        control: &InvocationControl,
    ) -> Result<(), CraftError> {
        let current = self
            .confluence_read_policy(&expected.profile, control)?
            .ok_or_else(|| {
                CraftError::new(
                    ErrorCode::PolicyDenied,
                    "No trusted Confluence-read configuration is present.",
                )
            })?;
        if !current.enabled || !current.operations.contains(&operation) {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Trusted Confluence-read configuration does not enable this operation.",
            ));
        }
        if current != *expected {
            return Err(CraftError::new(
                ErrorCode::VersionConflict,
                "Trusted Confluence-read configuration changed during execution.",
            ));
        }
        Ok(())
    }
}
