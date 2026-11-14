use std::collections::BTreeMap;
use std::fmt;
use std::io::Write;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::contracts::{GrantRef, RemoteJob, ResourceRef, validate_identity};

pub trait Clock: Send + Sync {
    fn unix_millis(&self) -> u64;
    fn monotonic_millis(&self) -> u64;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("Storage is unavailable")]
    Unavailable,
    #[error("Credential store is locked")]
    Locked,
    #[error("Stored data is corrupt")]
    Corrupt,
    #[error("Storage revision or identity conflicts")]
    Conflict,
    #[error("Storage input is invalid")]
    Invalid,
    #[error("Storage budget exceeded")]
    TooLarge,
    #[error("Storage operation cancelled before publication")]
    Cancelled,
    #[error("Storage operation deadline exceeded before publication")]
    DeadlineExceeded,
    #[error("Storage commit outcome is unknown; reconcile before retry")]
    OutcomeUnknown,
}

pub const MAX_SECRET_BYTES: usize = 1_048_576;

pub struct SecretBytes(Zeroizing<Vec<u8>>);

impl SecretBytes {
    pub fn new(bytes: Vec<u8>) -> Result<Self, StoreError> {
        let bytes = Zeroizing::new(bytes);
        if bytes.is_empty() {
            return Err(StoreError::Invalid);
        }
        if bytes.len() > MAX_SECRET_BYTES {
            return Err(StoreError::TooLarge);
        }
        Ok(Self(bytes))
    }

    pub fn expose(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for SecretBytes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretBytes([REDACTED])")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SecretKey {
    pub service: String,
    pub account: String,
}

impl SecretKey {
    pub fn validate(&self) -> Result<(), StoreError> {
        validate_identity("/service", &self.service).map_err(|_| StoreError::Invalid)?;
        validate_identity("/account", &self.account).map_err(|_| StoreError::Invalid)
    }
}

pub trait CredentialStore: Send + Sync {
    fn read(&self, key: &SecretKey) -> Result<Option<SecretBytes>, StoreError>;
    fn write(&self, key: &SecretKey, secret: &SecretBytes) -> Result<(), StoreError>;
    fn delete(&self, key: &SecretKey) -> Result<(), StoreError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationBinding {
    pub operation_id: String,
    pub request_digest: String,
    pub descriptor_digest: String,
    pub target: ResourceRef,
    pub grant: GrantRef,
    /// Host-verified effective actor under the selected provider/tenant authority.
    pub actor: String,
}

fn valid_digest(digest: &str) -> bool {
    digest.strip_prefix("sha256:").is_some_and(|hash| {
        hash.len() == 64
            && hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

impl OperationBinding {
    pub fn validate(&self) -> Result<(), StoreError> {
        validate_identity("/operation_id", &self.operation_id).map_err(|_| StoreError::Invalid)?;
        self.target.validate().map_err(|_| StoreError::Invalid)?;
        self.grant.validate().map_err(|_| StoreError::Invalid)?;
        validate_identity("/actor", &self.actor).map_err(|_| StoreError::Invalid)?;
        if !valid_digest(&self.request_digest) || !valid_digest(&self.descriptor_digest) {
            return Err(StoreError::Invalid);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubmissionState {
    Prepared,
    SendStarted,
    Confirmed,
    Failed,
    Unknown,
}

impl SubmissionState {
    pub fn requires_reconciliation(self) -> bool {
        matches!(self, Self::SendStarted | Self::Unknown)
    }

    pub fn allows(self, next: Self) -> bool {
        self == next
            || matches!(
                (self, next),
                (Self::Prepared, Self::SendStarted | Self::Failed)
                    | (
                        Self::SendStarted,
                        Self::Confirmed | Self::Failed | Self::Unknown
                    )
                    | (Self::Unknown, Self::Confirmed | Self::Failed)
            )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JournalStep {
    pub request_digest: Option<String>,
    pub state: SubmissionState,
    pub confirmed_resources: Vec<ResourceRef>,
    pub job: Option<RemoteJob>,
}

impl Default for JournalStep {
    fn default() -> Self {
        Self {
            request_digest: None,
            state: SubmissionState::Prepared,
            confirmed_resources: Vec::new(),
            job: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JournalRecord {
    pub binding: OperationBinding,
    pub revision: u64,
    pub steps: BTreeMap<String, JournalStep>,
}

impl JournalRecord {
    pub fn initial(binding: OperationBinding) -> Result<Self, StoreError> {
        binding.validate()?;
        Ok(Self {
            binding,
            revision: 0,
            steps: BTreeMap::new(),
        })
    }

    pub fn validate(&self) -> Result<(), StoreError> {
        self.binding.validate()?;
        if self.revision == 0 && !self.steps.is_empty() {
            return Err(StoreError::Conflict);
        }
        if self.steps.len() > 1_000 {
            return Err(StoreError::TooLarge);
        }
        for (id, step) in &self.steps {
            validate_identity("/step", id).map_err(|_| StoreError::Invalid)?;
            if step
                .request_digest
                .as_ref()
                .is_some_and(|digest| !valid_digest(digest))
            {
                return Err(StoreError::Invalid);
            }
            if matches!(
                step.state,
                SubmissionState::SendStarted
                    | SubmissionState::Confirmed
                    | SubmissionState::Unknown
            ) && step.request_digest.is_none()
            {
                return Err(StoreError::Conflict);
            }
            if step.confirmed_resources.len() > 1_000 {
                return Err(StoreError::TooLarge);
            }
            for resource in &step.confirmed_resources {
                resource.validate().map_err(|_| StoreError::Invalid)?;
            }
            if let Some(job) = &step.job {
                job.resource.validate().map_err(|_| StoreError::Invalid)?;
            }
            if matches!(
                step.state,
                SubmissionState::Prepared | SubmissionState::SendStarted
            ) && (!step.confirmed_resources.is_empty() || step.job.is_some())
            {
                return Err(StoreError::Conflict);
            }
        }
        Ok(())
    }

    /// Rejects changes that could erase effect evidence or reopen a terminal send for replay.
    pub fn validate_successor(&self, next: &Self) -> Result<(), StoreError> {
        if self.binding != next.binding || self.revision.checked_add(1) != Some(next.revision) {
            return Err(StoreError::Conflict);
        }
        next.validate()?;
        for (id, step) in &next.steps {
            if let Some(previous) = self.steps.get(id) {
                if previous
                    .request_digest
                    .as_ref()
                    .is_some_and(|digest| step.request_digest.as_ref() != Some(digest))
                    || !previous.state.allows(step.state)
                    || previous
                        .confirmed_resources
                        .iter()
                        .any(|resource| !step.confirmed_resources.contains(resource))
                {
                    return Err(StoreError::Conflict);
                }
                if let Some(job) = &previous.job {
                    let next_job = step.job.as_ref().ok_or(StoreError::Conflict)?;
                    if next_job.resource != job.resource {
                        return Err(StoreError::Conflict);
                    }
                    if !job.state.allows(next_job.state) {
                        return Err(StoreError::Conflict);
                    }
                }
            } else if step.state != SubmissionState::Prepared
                || !step.confirmed_resources.is_empty()
                || step.job.is_some()
            {
                return Err(StoreError::Conflict);
            }
        }
        if self.steps.keys().any(|id| !next.steps.contains_key(id)) {
            return Err(StoreError::Conflict);
        }
        Ok(())
    }
}

pub trait OperationJournal: Send + Sync {
    fn get(&self, operation_id: &str) -> Result<Option<JournalRecord>, StoreError>;
    /// Reusing the same binding returns its record; changed bindings must conflict.
    fn begin(&self, binding: &OperationBinding) -> Result<JournalRecord, StoreError>;
    fn compare_and_swap(
        &self,
        previous_revision: u64,
        next: &JournalRecord,
    ) -> Result<(), StoreError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactSpec {
    pub name: String,
    pub media_type: String,
    pub max_bytes: u64,
    pub source: Option<ResourceRef>,
}

impl ArtifactSpec {
    pub fn validate(&self) -> Result<(), StoreError> {
        validate_artifact_name(&self.name)?;
        if self.max_bytes == 0
            || self.media_type.is_empty()
            || self.media_type.len() > 256
            || self.media_type.chars().any(char::is_control)
        {
            return Err(StoreError::Invalid);
        }
        if let Some(source) = &self.source {
            source.validate().map_err(|_| StoreError::Invalid)?;
        }
        Ok(())
    }
}

pub fn validate_artifact_name(name: &str) -> Result<(), StoreError> {
    if name.is_empty()
        || name.len() > 128
        || matches!(name, "." | "..")
        || !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
    {
        return Err(StoreError::Invalid);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactReceipt {
    pub name: String,
    pub media_type: String,
    pub digest: String,
    pub bytes: u64,
    pub source: Option<ResourceRef>,
    pub local_path: Option<String>,
}

pub struct ArtifactBlob {
    pub receipt: ArtifactReceipt,
    pub bytes: Vec<u8>,
}

pub trait ArtifactWriter: Write + Send {
    fn finish(self: Box<Self>) -> Result<ArtifactReceipt, StoreError>;
}

pub trait ArtifactStore: Send + Sync {
    fn begin(&self, spec: ArtifactSpec) -> Result<Box<dyn ArtifactWriter>, StoreError>;
    fn receipt(&self, name: &str) -> Result<Option<ArtifactReceipt>, StoreError>;
    fn read(&self, name: &str, max_bytes: u64) -> Result<Option<ArtifactBlob>, StoreError>;
}
