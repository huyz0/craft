//! Exact Confluence space write authority, independent of all read and Jira permissions.
use crate::profiles::{ProfileStore, begin, commit, invalid_state, state_error};
use craft_core::contracts::{AuthMode, GrantRef, ResourceRef, validate_identity};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::input::parse_object;
use craft_core::profiles::Profile;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

pub(crate) const TABLE: &str = "CREATE TABLE confluence_write_policies (profile TEXT NOT NULL COLLATE BINARY REFERENCES profiles(name), space_id TEXT NOT NULL COLLATE BINARY, revision INTEGER NOT NULL CHECK(revision>=1), payload TEXT NOT NULL CHECK(length(CAST(payload AS BLOB))<=65536), PRIMARY KEY(profile,space_id))";
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ConfluenceWriteOperation {
    #[serde(rename = "confluence.page.create")]
    Create,
    #[serde(rename = "confluence.page.update")]
    Update,
    #[serde(rename = "confluence.page.trash")]
    Trash,
    #[serde(rename = "confluence.comment.create")]
    CommentCreate,
    #[serde(rename = "confluence.comment.update")]
    CommentUpdate,
    #[serde(rename = "confluence.comment.delete")]
    CommentDelete,
    #[serde(rename = "confluence.content-state.set")]
    ContentStateSet,
    #[serde(rename = "confluence.content-state.clear")]
    ContentStateClear,
    #[serde(rename = "confluence.attachment.trash")]
    AttachmentTrash,
    #[serde(rename = "confluence.attachment.upload")]
    AttachmentUpload,
    #[serde(rename = "confluence.attachment.replace")]
    AttachmentReplace,
    #[serde(rename = "confluence.folder.create")]
    FolderCreate,
    #[serde(rename = "confluence.folder.trash")]
    FolderTrash,
    #[serde(rename = "confluence.whiteboard.create")]
    WhiteboardCreate,
    #[serde(rename = "confluence.whiteboard.trash")]
    WhiteboardTrash,
}
impl ConfluenceWriteOperation {
    pub fn command(self) -> &'static str {
        match self {
            Self::Create => "confluence.page.create",
            Self::Update => "confluence.page.update",
            Self::Trash => "confluence.page.trash",
            Self::CommentCreate => "confluence.comment.create",
            Self::CommentUpdate => "confluence.comment.update",
            Self::CommentDelete => "confluence.comment.delete",
            Self::ContentStateSet => "confluence.content-state.set",
            Self::ContentStateClear => "confluence.content-state.clear",
            Self::AttachmentTrash => "confluence.attachment.trash",
            Self::AttachmentUpload => "confluence.attachment.upload",
            Self::AttachmentReplace => "confluence.attachment.replace",
            Self::FolderCreate => "confluence.folder.create",
            Self::FolderTrash => "confluence.folder.trash",
            Self::WhiteboardCreate => "confluence.whiteboard.create",
            Self::WhiteboardTrash => "confluence.whiteboard.trash",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfluenceWritePolicyRecord {
    pub format: u8,
    pub installation: String,
    pub revision: u64,
    pub profile: String,
    pub grant: GrantRef,
    pub target: ResourceRef,
    pub expected_actor: String,
    pub enabled: bool,
    pub operations: Vec<ConfluenceWriteOperation>,
}
pub struct ConfluenceWritePolicyUpdate {
    pub target: ResourceRef,
    pub expected_actor: String,
    pub enabled: bool,
    pub operations: Vec<ConfluenceWriteOperation>,
    pub expected_revision: Option<u64>,
}
fn numeric(id: &str) -> bool {
    id.parse::<i64>()
        .is_ok_and(|n| n > 0 && n.to_string() == id)
}

fn validate(
    profile: &Profile,
    target: &ResourceRef,
    actor: &str,
    operations: &[ConfluenceWriteOperation],
) -> Result<(), CraftError> {
    profile.validate_target("space", target)?;
    crate::transport::product_identity_url(&target.tenant_ref())?;
    validate_identity("/expected_actor", actor)?;
    if profile.provider != "confluence"
        || profile.deployment != "cloud"
        || profile.auth_mode != AuthMode::Service
        || !numeric(&target.id)
        || matches!(actor, "unknown" | "anonymous")
    {
        return Err(CraftError::field(
            "/policy",
            "expected_exact_confluence_service_space_and_actor",
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
impl ConfluenceWritePolicyRecord {
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
    space: &str,
) -> Result<Option<ConfluenceWritePolicyRecord>, CraftError> {
    if !numeric(space) {
        return Err(CraftError::field("/space_id", "expected_numeric_space_id"));
    }
    let record=store.connection.query_row("SELECT revision,payload FROM confluence_write_policies WHERE profile=?1 AND space_id=?2",params![profile.name,space],|row|{
        let revision:i64=row.get(0)?;let payload=row.get_ref(1)?.as_str().map_err(|_|rusqlite::Error::InvalidQuery)?;
        if payload.len()>65_536{return Err(rusqlite::Error::InvalidQuery);}
        let value=parse_object(payload.as_bytes()).map_err(|_|rusqlite::Error::InvalidQuery)?;
        let record:ConfluenceWritePolicyRecord=serde_json::from_value(value).map_err(|_|rusqlite::Error::InvalidQuery)?;
        if revision<1||record.revision!=revision as u64||record.target.id!=space {return Err(rusqlite::Error::InvalidQuery);}
        Ok(record)
    }).optional().map_err(|_|invalid_state())?;
    if let Some(record) = &record {
        record.validate(store, profile)?;
    }
    Ok(record)
}
impl ProfileStore {
    pub fn confluence_write_policy(
        &self,
        profile: &str,
        space: &str,
        control: &InvocationControl,
    ) -> Result<Option<ConfluenceWritePolicyRecord>, CraftError> {
        control.check()?;
        let profile = self.get(profile, control)?;
        let record = read(self, &profile, space)?;
        control.check()?;
        Ok(record)
    }
    /// Trusted administration requires apply, receipt capacity and an exact row revision.
    pub fn set_confluence_write_policy(
        &self,
        profile: &str,
        mut update: ConfluenceWritePolicyUpdate,
        apply: bool,
        control: &InvocationControl,
        preflight: impl FnOnce(&ConfluenceWritePolicyRecord) -> Result<(), CraftError>,
    ) -> Result<ConfluenceWritePolicyRecord, CraftError> {
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
                "Mutation policy revision changed; inspect that exact space record.",
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
                    "SELECT count(*) FROM confluence_write_policies WHERE profile=?1",
                    [&profile.name],
                    |row| row.get(0),
                )
                .map_err(state_error)?;
            if count >= 256 {
                return Err(CraftError::field(
                    "/policy",
                    "too_many_space_mutation_records",
                ));
            }
        }
        let record = ConfluenceWritePolicyRecord {
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
            operations: update.operations,
        };
        record.validate(self, &profile)?;
        preflight(&record)?;
        let payload = serde_json::to_string(&record).map_err(|_| invalid_state())?;
        if payload.len() > 65_536 {
            return Err(invalid_state());
        }
        control.check()?;
        transaction.execute("INSERT INTO confluence_write_policies(profile,space_id,revision,payload) VALUES(?1,?2,?3,?4) ON CONFLICT(profile,space_id) DO UPDATE SET revision=excluded.revision,payload=excluded.payload",params![record.profile,record.target.id,record.revision as i64,payload]).map_err(state_error)?;
        commit(transaction,Some(control)).map_err(|mut error|{error.next_action="Read the exact space mutation record before retrying; preserve competing revisions.".into();error})?;
        Ok(record)
    }
    pub fn verify_confluence_write_policy(
        &self,
        expected: &ConfluenceWritePolicyRecord,
        operation: ConfluenceWriteOperation,
        control: &InvocationControl,
    ) -> Result<(), CraftError> {
        let current = self
            .confluence_write_policy(&expected.profile, &expected.target.id, control)?
            .ok_or_else(|| {
                CraftError::new(
                    ErrorCode::PolicyDenied,
                    "No explicit space mutation policy is configured.",
                )
            })?;
        if !current.enabled || !current.operations.contains(&operation) {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "This space mutation operation is not enabled.",
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
