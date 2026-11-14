//! Operator-managed reviewer intent, independent of resource-write permission and plan content.
use crate::profiles::{ProfileStore, begin, commit, invalid_state, state_error};
use craft_core::contracts::{AuthMode, GrantRef, ResourceRef, validate_identity};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::input::parse_object;
use craft_core::profiles::Profile;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub(crate) const TABLE: &str = "CREATE TABLE review_policies (profile TEXT NOT NULL COLLATE BINARY REFERENCES profiles(name), space_id TEXT NOT NULL COLLATE BINARY, revision INTEGER NOT NULL CHECK(revision>=1), payload TEXT NOT NULL CHECK(length(CAST(payload AS BLOB))<=65536), PRIMARY KEY(profile,space_id))";
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewPolicyRecord {
    pub format: u8,
    pub installation: String,
    pub revision: u64,
    pub profile: String,
    pub grant: GrantRef,
    pub target: ResourceRef,
    pub expected_actor: String,
    pub enabled: bool,
    pub reviewers: Vec<String>,
    pub excluded_actors: Vec<String>,
    pub quorum: u16,
}
pub struct ReviewPolicyUpdate {
    pub target: ResourceRef,
    pub expected_actor: String,
    pub enabled: bool,
    pub reviewers: Vec<String>,
    pub excluded_actors: Vec<String>,
    pub quorum: u16,
    pub expected_revision: Option<u64>,
}
pub fn numeric(id: &str) -> bool {
    id.parse::<i64>()
        .is_ok_and(|n| n > 0 && n.to_string() == id)
}
/// Validates administration input without opening state or consulting native credentials.
pub fn validate_eligibility(
    actor: &str,
    reviewers: &[String],
    excluded: &[String],
    quorum: u16,
) -> Result<(), CraftError> {
    validate_identity("/expected_actor", actor)?;
    if matches!(actor, "unknown" | "anonymous")
        || reviewers.is_empty()
        || reviewers.len() > 64
        || excluded.len() > 64
        || quorum == 0
        || usize::from(quorum) > reviewers.len()
    {
        return Err(CraftError::field(
            "/reviewers",
            "expected_bounded_explicit_reviewer_quorum",
        ));
    }
    for id in reviewers.iter().chain(excluded) {
        validate_identity("/reviewers", id)?;
        if matches!(id.as_str(), "unknown" | "anonymous") {
            return Err(CraftError::field(
                "/reviewers",
                "unestablished_reviewer_identity",
            ));
        }
    }
    let eligible: BTreeSet<_> = reviewers.iter().collect();
    let denied: BTreeSet<_> = excluded.iter().collect();
    if eligible.len() != reviewers.len()
        || denied.len() != excluded.len()
        || reviewers.iter().any(|id| id == actor)
        || !eligible.is_disjoint(&denied)
    {
        return Err(CraftError::field(
            "/reviewers",
            "reviewers_must_be_unique_and_exclude_worker_identities",
        ));
    }
    if serde_json::to_vec(&(actor, reviewers, excluded, quorum))
        .map_err(|_| CraftError::field("/reviewers", "invalid_reviewer_configuration"))?
        .len()
        > 61_440
    {
        return Err(CraftError::field(
            "/reviewers",
            "serialized_reviewer_configuration_limit_exceeded",
        ));
    }
    Ok(())
}
fn validate(
    profile: &Profile,
    target: &ResourceRef,
    actor: &str,
    reviewers: &[String],
    excluded: &[String],
    quorum: u16,
) -> Result<(), CraftError> {
    profile.validate_target("space", target)?;
    crate::transport::product_identity_url(&target.tenant_ref())?;
    if profile.provider != "confluence"
        || profile.deployment != "cloud"
        || profile.auth_mode != AuthMode::Service
        || !numeric(&target.id)
    {
        return Err(CraftError::field(
            "/target",
            "expected_exact_confluence_service_space",
        ));
    }
    validate_eligibility(actor, reviewers, excluded, quorum)
}
impl ReviewPolicyRecord {
    fn validate(&self, store: &ProfileStore, profile: &Profile) -> Result<(), CraftError> {
        if self.format != 1
            || self.installation != store.settings().installation_id
            || self.profile != profile.name
            || self.grant != profile.grant
            || self.revision == 0
            || self.revision > i64::MAX as u64
            || self.reviewers.windows(2).any(|p| p[0] >= p[1])
            || self.excluded_actors.windows(2).any(|p| p[0] >= p[1])
        {
            return Err(invalid_state());
        }
        validate(
            profile,
            &self.target,
            &self.expected_actor,
            &self.reviewers,
            &self.excluded_actors,
            self.quorum,
        )
        .map_err(|_| invalid_state())
    }
}
fn read(
    store: &ProfileStore,
    profile: &Profile,
    space: &str,
) -> Result<Option<ReviewPolicyRecord>, CraftError> {
    if !numeric(space) {
        return Err(CraftError::field("/space_id", "expected_numeric_space_id"));
    }
    let record = store
        .connection
        .query_row(
            "SELECT revision,payload FROM review_policies WHERE profile=?1 AND space_id=?2",
            params![profile.name, space],
            |row| {
                let revision: i64 = row.get(0)?;
                let payload = row
                    .get_ref(1)?
                    .as_str()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?;
                if payload.len() > 65_536 {
                    return Err(rusqlite::Error::InvalidQuery);
                }
                let value =
                    parse_object(payload.as_bytes()).map_err(|_| rusqlite::Error::InvalidQuery)?;
                let record: ReviewPolicyRecord =
                    serde_json::from_value(value).map_err(|_| rusqlite::Error::InvalidQuery)?;
                if revision < 1 || record.revision != revision as u64 || record.target.id != space {
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
    pub fn review_policy(
        &self,
        profile: &str,
        space: &str,
        control: &InvocationControl,
    ) -> Result<Option<ReviewPolicyRecord>, CraftError> {
        control.check()?;
        let profile = self.get(profile, control)?;
        let record = read(self, &profile, space)?;
        control.check()?;
        Ok(record)
    }
    /// Apply, CAS and the complete proposed-receipt preflight precede persistence.
    pub fn set_review_policy(
        &self,
        profile: &str,
        mut update: ReviewPolicyUpdate,
        apply: bool,
        control: &InvocationControl,
        preflight: impl FnOnce(&ReviewPolicyRecord) -> Result<(), CraftError>,
    ) -> Result<ReviewPolicyRecord, CraftError> {
        control.check()?;
        if !apply {
            return Err(CraftError::new(
                ErrorCode::ApplyRequired,
                "Reviewer policy changes require explicit apply.",
            ));
        }
        update.reviewers.sort();
        update.excluded_actors.sort();
        let transaction = begin(&self.connection, control)?;
        let profile = self.get(profile, control)?;
        validate(
            &profile,
            &update.target,
            &update.expected_actor,
            &update.reviewers,
            &update.excluded_actors,
            update.quorum,
        )?;
        let previous = read(self, &profile, &update.target.id)?;
        if previous.as_ref().map(|r| r.revision) != update.expected_revision {
            return Err(CraftError::new(
                ErrorCode::VersionConflict,
                "Reviewer policy revision changed; inspect the exact space record.",
            ));
        }
        if previous.as_ref().is_some_and(|r| {
            r.enabled && (r.expected_actor != update.expected_actor || r.target != update.target)
        }) {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Disable reviewer policy before transferring its actor or target.",
            ));
        }
        if previous.is_none() {
            let count: i64 = transaction
                .query_row(
                    "SELECT count(*) FROM review_policies WHERE profile=?1",
                    [&profile.name],
                    |row| row.get(0),
                )
                .map_err(state_error)?;
            if count >= 256 {
                return Err(CraftError::field(
                    "/policy",
                    "too_many_space_review_policies",
                ));
            }
        }
        let record = ReviewPolicyRecord {
            format: 1,
            installation: self.settings().installation_id.clone(),
            revision: update
                .expected_revision
                .unwrap_or(0)
                .checked_add(1)
                .filter(|r| *r <= i64::MAX as u64)
                .ok_or_else(invalid_state)?,
            profile: profile.name.clone(),
            grant: profile.grant.clone(),
            target: update.target,
            expected_actor: update.expected_actor,
            enabled: update.enabled,
            reviewers: update.reviewers,
            excluded_actors: update.excluded_actors,
            quorum: update.quorum,
        };
        record.validate(self, &profile)?;
        preflight(&record)?;
        let payload = serde_json::to_string(&record).map_err(|_| invalid_state())?;
        if payload.len() > 65_536 {
            return Err(invalid_state());
        }
        control.check()?;
        transaction.execute("INSERT INTO review_policies(profile,space_id,revision,payload) VALUES(?1,?2,?3,?4) ON CONFLICT(profile,space_id) DO UPDATE SET revision=excluded.revision,payload=excluded.payload",params![record.profile,record.target.id,record.revision as i64,payload]).map_err(state_error)?;
        commit(transaction,Some(control)).map_err(|mut e|{e.next_action="Read the same profile/space reviewer policy under the same state root before retrying; preserve competing revisions.".into();e})?;
        Ok(record)
    }
    pub fn verify_review_policy(
        &self,
        expected: &ReviewPolicyRecord,
        control: &InvocationControl,
    ) -> Result<(), CraftError> {
        let current = self
            .review_policy(&expected.profile, &expected.target.id, control)?
            .ok_or_else(|| {
                CraftError::new(
                    ErrorCode::PolicyDenied,
                    "No trusted reviewer configuration is present.",
                )
            })?;
        if !current.enabled {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Reviewer policy is disabled.",
            ));
        }
        if current != *expected {
            return Err(CraftError::new(
                ErrorCode::VersionConflict,
                "Captured reviewer policy changed during execution.",
            ));
        }
        Ok(())
    }
}
