//! Per-profile, per-class operation allowances for providers that use a static credential.
//!
//! A stored grant is not authority to run any command. Each class (for example `trello.read` or
//! `bitbucket.write`) is enabled separately, bound to the profile's exact grant and tenant, and
//! rechecked before data is released or a mutation is sent.

use craft_core::contracts::{AuthMode, GrantRef, ResourceRef};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::input::parse_object;
use craft_core::profiles::Profile;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::profiles::{ProfileStore, begin, commit, invalid_state, state_error};

pub(crate) const TABLE: &str = "CREATE TABLE provider_policies (profile TEXT NOT NULL COLLATE BINARY REFERENCES profiles(name), class TEXT NOT NULL COLLATE BINARY, revision INTEGER NOT NULL CHECK(revision>=1), payload TEXT NOT NULL CHECK(length(CAST(payload AS BLOB))<=65536), PRIMARY KEY(profile,class))";

/// Classes a profile can enable. The provider is the prefix before the dot.
pub const CLASSES: [&str; 4] = [
    "trello.read",
    "trello.write",
    "bitbucket.read",
    "bitbucket.write",
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderPolicyRecord {
    pub format: u8,
    pub installation: String,
    pub revision: u64,
    pub profile: String,
    pub class: String,
    pub grant: GrantRef,
    pub target: ResourceRef,
    pub operations: Vec<String>,
    pub enabled: bool,
}

pub struct ProviderPolicyUpdate {
    pub class: String,
    pub target: ResourceRef,
    pub operations: Vec<String>,
    pub enabled: bool,
    pub expected_revision: Option<u64>,
}

fn provider_of(class: &str) -> Option<&str> {
    CLASSES
        .contains(&class)
        .then(|| class.split_once('.').map(|(provider, _)| provider))
        .flatten()
}

fn validate(
    profile: &Profile,
    class: &str,
    target: &ResourceRef,
    operations: &[String],
) -> Result<(), CraftError> {
    let provider = provider_of(class)
        .ok_or_else(|| CraftError::field("/class", "expected_known_provider_policy_class"))?;
    // A policy binds the whole account: the Trello member or the Bitbucket workspace the
    // credential was proven against. Board or repository narrowing is not a policy concern.
    let kind = if provider == "trello" {
        "account"
    } else {
        "workspace"
    };
    profile.validate_target(kind, target)?;
    if target.id != target.tenant {
        return Err(CraftError::field("/target", "expected_exact_account_target"));
    }
    crate::transport::product_identity_url(&target.tenant_ref())?;
    if profile.provider != provider
        || profile.deployment != "cloud"
        || profile.auth_mode != AuthMode::Service
    {
        return Err(CraftError::field(
            "/policy",
            "expected_matching_static_credential_service_profile",
        ));
    }
    if operations.is_empty()
        || operations.len() > 64
        || operations.windows(2).any(|pair| pair[0] >= pair[1])
        || operations.iter().any(|operation| {
            !operation.starts_with(&format!("{provider}."))
                || operation.len() > 64
                || !operation
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b == b'.' || b == b'-')
        })
    {
        return Err(CraftError::field(
            "/operations",
            "expected_nonempty_unique_sorted_provider_operations",
        ));
    }
    Ok(())
}

impl ProviderPolicyRecord {
    fn check(&self, store: &ProfileStore, profile: &Profile) -> Result<(), CraftError> {
        if self.format != 1
            || self.installation != store.settings().installation_id
            || self.profile != profile.name
            || self.grant != profile.grant
            || self.revision == 0
            || self.revision > i64::MAX as u64
        {
            return Err(invalid_state());
        }
        validate(profile, &self.class, &self.target, &self.operations).map_err(|_| invalid_state())
    }
}

fn read(
    store: &ProfileStore,
    profile: &Profile,
    class: &str,
) -> Result<Option<ProviderPolicyRecord>, CraftError> {
    let record = store
        .connection
        .query_row(
            "SELECT revision,payload FROM provider_policies WHERE profile=?1 AND class=?2",
            [&profile.name, class],
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
                let record: ProviderPolicyRecord =
                    serde_json::from_value(raw).map_err(|_| rusqlite::Error::InvalidQuery)?;
                if revision < 1 || record.revision != revision as u64 || record.class != class {
                    return Err(rusqlite::Error::InvalidQuery);
                }
                Ok(record)
            },
        )
        .optional()
        .map_err(|_| invalid_state())?;
    if let Some(record) = &record {
        record.check(store, profile)?;
    }
    Ok(record)
}

impl ProfileStore {
    /// Reads strictly validated configuration; a missing record grants no access.
    pub fn provider_policy(
        &self,
        profile: &str,
        class: &str,
        control: &InvocationControl,
    ) -> Result<Option<ProviderPolicyRecord>, CraftError> {
        control.check()?;
        provider_of(class)
            .ok_or_else(|| CraftError::field("/class", "expected_known_provider_policy_class"))?;
        let profile = self.get(profile, control)?;
        let record = read(self, &profile, class)?;
        control.check()?;
        Ok(record)
    }

    /// Requires explicit apply and the exact previous revision. An absent expected revision only
    /// creates. The preflight sees the complete record before anything commits.
    pub fn set_provider_policy(
        &self,
        profile: &str,
        mut update: ProviderPolicyUpdate,
        apply: bool,
        control: &InvocationControl,
        preflight: impl FnOnce(&ProviderPolicyRecord) -> Result<(), CraftError>,
    ) -> Result<ProviderPolicyRecord, CraftError> {
        control.check()?;
        if !apply {
            return Err(CraftError::new(
                ErrorCode::ApplyRequired,
                "Provider policy changes require explicit apply.",
            ));
        }
        update.operations.sort();
        let transaction = begin(&self.connection, control)?;
        let profile = self.get(profile, control)?;
        validate(&profile, &update.class, &update.target, &update.operations)?;
        let previous = read(self, &profile, &update.class)?;
        if previous.as_ref().map(|record| record.revision) != update.expected_revision {
            return Err(CraftError::new(
                ErrorCode::VersionConflict,
                "Provider policy revision changed; inspect it before updating.",
            ));
        }
        if previous
            .as_ref()
            .is_some_and(|record| record.enabled && record.target != update.target)
        {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Disable the provider policy before transferring its target.",
            ));
        }
        let record = ProviderPolicyRecord {
            format: 1,
            installation: self.settings().installation_id.clone(),
            revision: update
                .expected_revision
                .unwrap_or(0)
                .checked_add(1)
                .filter(|revision| *revision <= i64::MAX as u64)
                .ok_or_else(invalid_state)?,
            profile: profile.name.clone(),
            class: update.class,
            grant: profile.grant.clone(),
            target: update.target,
            operations: update.operations,
            enabled: update.enabled,
        };
        record.check(self, &profile)?;
        preflight(&record)?;
        let payload = serde_json::to_string(&record).map_err(|_| invalid_state())?;
        if payload.len() > 65_536 {
            return Err(invalid_state());
        }
        control.check()?;
        transaction
            .execute(
                "INSERT INTO provider_policies(profile,class,revision,payload) VALUES(?1,?2,?3,?4) ON CONFLICT(profile,class) DO UPDATE SET revision=excluded.revision,payload=excluded.payload",
                params![record.profile, record.class, record.revision as i64, payload],
            )
            .map_err(state_error)?;
        commit(transaction, Some(control)).map_err(|mut error| {
            error.next_action = "Read the named provider policy under the same state root before retrying; preserve competing revisions.".into();
            error
        })?;
        Ok(record)
    }

    /// Rechecks the allowance and the whole captured record, including enabled state and revision.
    pub fn verify_provider_policy(
        &self,
        expected: &ProviderPolicyRecord,
        operation: &str,
        control: &InvocationControl,
    ) -> Result<(), CraftError> {
        let current = self
            .provider_policy(&expected.profile, &expected.class, control)?
            .ok_or_else(|| {
                CraftError::new(
                    ErrorCode::PolicyDenied,
                    "No trusted provider policy is present.",
                )
            })?;
        if !current.enabled || !current.operations.iter().any(|name| name == operation) {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Trusted provider policy does not enable this operation.",
            ));
        }
        if current != *expected {
            return Err(CraftError::new(
                ErrorCode::VersionConflict,
                "Trusted provider policy changed during execution.",
            ));
        }
        Ok(())
    }
}
