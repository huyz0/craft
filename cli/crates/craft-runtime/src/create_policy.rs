//! Private local create policy. Records are host intent, not proof of remote permission or human review.

use craft_core::contracts::{AuthMode, GrantRef, ResourceRef, validate_identity};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::input::parse_object;
use craft_core::profiles::Profile;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::profiles::{ProfileStore, begin, commit, invalid_state, state_error};

pub(crate) const TABLE: &str = "CREATE TABLE create_policies (profile TEXT PRIMARY KEY COLLATE BINARY REFERENCES profiles(name), revision INTEGER NOT NULL CHECK(revision>=1), payload TEXT NOT NULL CHECK(length(CAST(payload AS BLOB))<=65536))";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatePolicyRecord {
    pub format: u8,
    pub installation: String,
    pub revision: u64,
    pub profile: String,
    pub grant: GrantRef,
    pub target: ResourceRef,
    pub expected_actor: String,
    pub enabled: bool,
}

pub struct CreatePolicyUpdate {
    pub target: ResourceRef,
    pub expected_actor: String,
    pub enabled: bool,
    pub expected_revision: Option<u64>,
}

fn validate_target(profile: &Profile, target: &ResourceRef, actor: &str) -> Result<(), CraftError> {
    profile.validate_target("project", target)?;
    crate::transport::product_identity_url(&target.tenant_ref())?;
    validate_identity("/expected_actor", actor)?;
    if profile.provider != "jira"
        || profile.deployment != "cloud"
        || profile.auth_mode != AuthMode::Service
        || matches!(actor, "unknown" | "anonymous")
        || target.id.is_empty()
        || target.id.len() > 512
        || !target.id.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(CraftError::field(
            "/policy",
            "expected_exact_jira_service_project_and_actor",
        ));
    }
    Ok(())
}

impl CreatePolicyRecord {
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
        validate_target(profile, &self.target, &self.expected_actor).map_err(|_| invalid_state())
    }
}

fn read(store: &ProfileStore, profile: &Profile) -> Result<Option<CreatePolicyRecord>, CraftError> {
    let record = store
        .connection
        .query_row(
            "SELECT revision,payload FROM create_policies WHERE profile=?1",
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
                let record: CreatePolicyRecord =
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
    /// Reads only this installation's policy; absence never supplies an enabled default.
    pub fn create_policy(
        &self,
        profile: &str,
        control: &InvocationControl,
    ) -> Result<Option<CreatePolicyRecord>, CraftError> {
        control.check()?;
        let profile = self.get(profile, control)?;
        let record = read(self, &profile)?;
        control.check()?;
        Ok(record)
    }

    /// Trusted-host administration only. Public CLI has no policy-write command.
    /// None creates only; Some revision updates only that exact existing revision.
    pub fn set_create_policy(
        &self,
        profile: &str,
        update: CreatePolicyUpdate,
        apply: bool,
        control: &InvocationControl,
        preflight: impl FnOnce(&CreatePolicyRecord) -> Result<(), CraftError>,
    ) -> Result<CreatePolicyRecord, CraftError> {
        control.check()?;
        if !apply {
            return Err(CraftError::new(
                ErrorCode::ApplyRequired,
                "Local create policy changes require explicit apply.",
            ));
        }
        let transaction = begin(&self.connection, control)?;
        let profile = self.get(profile, control)?;
        validate_target(&profile, &update.target, &update.expected_actor)?;
        let prior = read(self, &profile)?;
        if prior.as_ref().map(|record| record.revision) != update.expected_revision {
            return Err(CraftError::new(
                ErrorCode::VersionConflict,
                "Create policy revision changed; read the same policy before another update.",
            ));
        }
        let record = CreatePolicyRecord {
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
            enabled: update.enabled,
        };
        record.validate(self, &profile)?;
        preflight(&record)?;
        let payload = serde_json::to_string(&record).map_err(|_| invalid_state())?;
        if payload.len() > 65_536 {
            return Err(invalid_state());
        }
        control.check()?;
        transaction.execute("INSERT INTO create_policies(profile,revision,payload) VALUES(?1,?2,?3) ON CONFLICT(profile) DO UPDATE SET revision=excluded.revision,payload=excluded.payload",params![record.profile,record.revision as i64,payload]).map_err(state_error)?;
        commit(transaction,Some(control)).map_err(|mut error|{error.next_action="Read the named create policy under the same state root before retrying; do not overwrite a changed revision.".into();error})?;
        Ok(record)
    }

    /// Refuses missing, disabled or changed policy without changing the recorded revision.
    pub fn verify_create_policy(
        &self,
        expected: &CreatePolicyRecord,
        control: &InvocationControl,
    ) -> Result<(), CraftError> {
        let current = self
            .create_policy(&expected.profile, control)?
            .ok_or_else(|| {
                CraftError::new(
                    ErrorCode::PolicyDenied,
                    "No trusted create policy is configured.",
                )
            })?;
        if !current.enabled {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Trusted create policy is disabled.",
            ));
        }
        if current != *expected {
            return Err(CraftError::new(
                ErrorCode::VersionConflict,
                "Trusted create policy changed during this invocation.",
            ));
        }
        Ok(())
    }
}
