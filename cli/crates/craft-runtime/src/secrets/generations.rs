use std::collections::BTreeSet;

use craft_core::contracts::RequestState;
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::input::{content_digest, parse_object};
use craft_core::storage::{CredentialStore, MAX_SECRET_BYTES, SecretBytes, SecretKey, StoreError};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::locks::GrantLock;
use crate::profiles::{is_opaque_id, random_id};

const CHUNK_BYTES: usize = 2_048;
const INDEX_BYTES: usize = 2_048;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Recovery policy for an indexed replacement whose chunks cannot be verified.
pub enum IncompletePolicy {
    /// Preserve the previous complete credential when the external protocol permits reuse.
    KeepPrevious,
    /// Invalidate the previous credential when its external authority may have changed.
    Reauthorize,
}

/// Locked snapshot identity; cleanup alone does not invalidate it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialVersion {
    installation: String,
    grant: String,
    epoch: u64,
    generation: Option<String>,
}

pub struct CredentialSnapshot {
    pub version: CredentialVersion,
    pub secret: Option<SecretBytes>,
}

#[derive(Debug)]
pub struct CredentialState {
    pub version: CredentialVersion,
    pub active: bool,
    pub recovery_required: bool,
    pub cleanup_pending: bool,
}

#[derive(Debug)]
pub struct GenerationReceipt {
    pub version: CredentialVersion,
    pub cleanup_pending: bool,
}

#[derive(Debug)]
pub struct RecoveryReport {
    pub version: CredentialVersion,
    pub recovered: bool,
    pub reauthorization_required: bool,
    pub cleanup_pending: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    id: String,
    length: usize,
    chunks: usize,
    digest: String,
}

impl Manifest {
    fn validate(&self) -> bool {
        is_opaque_id(&self.id, "gen_")
            && (1..=MAX_SECRET_BYTES).contains(&self.length)
            && self.chunks == self.length.div_ceil(CHUNK_BYTES)
            && self.digest.strip_prefix("sha256:").is_some_and(|digest| {
                digest.len() == 64
                    && digest
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    manifest: Manifest,
    incomplete: IncompletePolicy,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    format: u8,
    installation: String,
    grant: String,
    revision: u64,
    epoch: u64,
    active: Option<Manifest>,
    pending: Option<Pending>,
    retired: Vec<Manifest>,
}

impl Record {
    fn empty(guard: &GrantLock) -> Self {
        Self {
            format: 1,
            installation: guard.installation.clone(),
            grant: guard.grant.clone(),
            revision: 0,
            epoch: 0,
            active: None,
            pending: None,
            retired: Vec::new(),
        }
    }

    fn validate(&self, guard: &GrantLock) -> bool {
        if self.format != 1
            || self.installation != guard.installation
            || self.grant != guard.grant
            || self.revision < self.epoch
            || (self.pending.is_some() && !self.retired.is_empty())
        {
            return false;
        }
        let manifests: Vec<_> = self
            .active
            .iter()
            .chain(self.pending.iter().map(|pending| &pending.manifest))
            .chain(&self.retired)
            .collect();
        let mut ids = BTreeSet::new();
        manifests.len() <= 2
            && manifests
                .iter()
                .all(|manifest| manifest.validate() && ids.insert(&manifest.id))
    }

    fn version(&self) -> CredentialVersion {
        CredentialVersion {
            installation: self.installation.clone(),
            grant: self.grant.clone(),
            epoch: self.epoch,
            generation: self.active.as_ref().map(|manifest| manifest.id.clone()),
        }
    }
}

pub(super) fn storage_error(error: StoreError, dirty: bool) -> CraftError {
    let unknown = dirty || error == StoreError::OutcomeUnknown;
    let code = if unknown {
        ErrorCode::WriteOutcomeUnknown
    } else {
        match error {
            StoreError::Invalid | StoreError::TooLarge => ErrorCode::InputInvalid,
            StoreError::Conflict => ErrorCode::VersionConflict,
            StoreError::Cancelled => ErrorCode::Cancelled,
            StoreError::DeadlineExceeded => ErrorCode::DeadlineExceeded,
            _ => ErrorCode::KeyringUnavailable,
        }
    };
    let mut result = CraftError::new(
        code,
        if unknown {
            "Credential update requires reconciliation."
        } else {
            "Credential state is unavailable, invalid or conflicts with this request."
        },
    );
    result.details.write_state = if unknown {
        RequestState::SentUnknown
    } else {
        RequestState::NotSent
    };
    result.next_action = "Inspect and recover this grant under its lock; never retry an external token exchange blindly.".into();
    result
}

/// Exclusive access to one local installation/grant until this session is dropped.
/// The caller must persist external exchange intent separately before remote rotation.
pub struct LockedCredentials<'a> {
    store: &'a dyn CredentialStore,
    guard: GrantLock,
    control: InvocationControl,
}

impl<'a> LockedCredentials<'a> {
    /// Non-secret identity of the grant whose native lock this session holds.
    pub fn grant(&self) -> craft_core::contracts::GrantRef {
        craft_core::contracts::GrantRef {
            id: self.guard.grant.clone(),
        }
    }

    pub(super) fn new(
        store: &'a dyn CredentialStore,
        guard: GrantLock,
        control: InvocationControl,
    ) -> Self {
        Self {
            store,
            guard,
            control,
        }
    }

    fn check(&self, dirty: bool) -> Result<(), CraftError> {
        self.control.check().map_err(|mut error| {
            if dirty {
                error.details.write_state = RequestState::SentUnknown;
            }
            error
        })
    }

    fn index_key(&self) -> SecretKey {
        SecretKey {
            service: "credential-generations-v1".into(),
            account: format!("{}/index", self.guard.grant),
        }
    }
    fn chunk_key(&self, manifest: &Manifest, chunk: usize) -> SecretKey {
        SecretKey {
            service: "credential-generations-v1".into(),
            account: format!("{}/{}/{chunk:04}", self.guard.grant, manifest.id),
        }
    }

    fn record(&self, dirty: bool) -> Result<Record, CraftError> {
        self.check(dirty)?;
        let Some(bytes) = self
            .store
            .read(&self.index_key())
            .map_err(|error| storage_error(error, dirty))?
        else {
            return Ok(Record::empty(&self.guard));
        };
        if bytes.expose().len() > INDEX_BYTES {
            return Err(storage_error(StoreError::Corrupt, dirty));
        }
        let value =
            parse_object(bytes.expose()).map_err(|_| storage_error(StoreError::Corrupt, dirty))?;
        // Missing nullable fields are corruption, not an implicit inactive/default state.
        if ![
            "format",
            "installation",
            "grant",
            "revision",
            "epoch",
            "active",
            "pending",
            "retired",
        ]
        .iter()
        .all(|key| value.get(*key).is_some())
        {
            return Err(storage_error(StoreError::Corrupt, dirty));
        }
        let record: Record =
            serde_json::from_value(value).map_err(|_| storage_error(StoreError::Corrupt, dirty))?;
        if !record.validate(&self.guard) {
            return Err(storage_error(StoreError::Corrupt, dirty));
        }
        Ok(record)
    }

    fn verified_write(
        &self,
        key: &SecretKey,
        secret: &SecretBytes,
        dirty: &mut bool,
    ) -> Result<(), CraftError> {
        self.check(*dirty)?;
        *dirty = true;
        match self.store.write(key, secret) {
            Ok(()) | Err(StoreError::OutcomeUnknown) => match self.store.read(key) {
                Ok(Some(actual)) if actual.expose() == secret.expose() => Ok(()),
                _ => Err(storage_error(StoreError::OutcomeUnknown, true)),
            },
            Err(error) => Err(storage_error(error, true)),
        }
    }

    fn commit(&self, record: &mut Record, dirty: &mut bool) -> Result<(), CraftError> {
        record.revision = record
            .revision
            .checked_add(1)
            .ok_or_else(|| storage_error(StoreError::Corrupt, *dirty))?;
        if !record.validate(&self.guard) {
            return Err(storage_error(StoreError::Corrupt, *dirty));
        }
        let bytes =
            serde_json::to_vec(record).map_err(|_| storage_error(StoreError::Corrupt, *dirty))?;
        if bytes.len() > INDEX_BYTES {
            return Err(storage_error(StoreError::TooLarge, *dirty));
        }
        let bytes = SecretBytes::new(bytes).map_err(|error| storage_error(error, *dirty))?;
        self.verified_write(&self.index_key(), &bytes, dirty)
    }

    fn contents(&self, manifest: &Manifest, dirty: bool) -> Result<SecretBytes, CraftError> {
        let mut buffer = Zeroizing::new(Vec::with_capacity(manifest.length));
        for chunk in 0..manifest.chunks {
            self.check(dirty)?;
            let value = self
                .store
                .read(&self.chunk_key(manifest, chunk))
                .map_err(|error| storage_error(error, dirty))?
                .ok_or_else(|| storage_error(StoreError::Corrupt, dirty))?;
            if value.expose().len() != CHUNK_BYTES.min(manifest.length - chunk * CHUNK_BYTES) {
                return Err(storage_error(StoreError::Corrupt, dirty));
            }
            buffer.extend_from_slice(value.expose());
        }
        if buffer.len() != manifest.length || content_digest(&buffer) != manifest.digest {
            return Err(storage_error(StoreError::Corrupt, dirty));
        }
        SecretBytes::new(std::mem::take(&mut *buffer)).map_err(|error| storage_error(error, dirty))
    }

    fn cleanup(&self, record: &mut Record, dirty: &mut bool) -> Result<(), CraftError> {
        if record.retired.is_empty() {
            return Ok(());
        }
        for manifest in &record.retired {
            for chunk in 0..manifest.chunks {
                self.check(*dirty)?;
                *dirty = true;
                match self.store.delete(&self.chunk_key(manifest, chunk)) {
                    Ok(()) => {}
                    Err(StoreError::OutcomeUnknown)
                        if self
                            .store
                            .read(&self.chunk_key(manifest, chunk))
                            .is_ok_and(|value| value.is_none()) => {}
                    Err(error) => return Err(storage_error(error, true)),
                }
            }
        }
        record.retired.clear();
        self.commit(record, dirty)
    }

    fn expected(&self, record: &Record, expected: &CredentialVersion) -> Result<(), CraftError> {
        if &record.version() != expected {
            return Err(storage_error(StoreError::Conflict, false));
        }
        Ok(())
    }

    /// Reads index metadata without returning secrets or checking active chunk contents.
    pub fn inspect(&self) -> Result<CredentialState, CraftError> {
        let record = self.record(false)?;
        Ok(CredentialState {
            version: record.version(),
            active: record.active.is_some(),
            recovery_required: record.pending.is_some(),
            cleanup_pending: !record.retired.is_empty(),
        })
    }

    /// Never returns an old credential while an unresolved replacement intent exists.
    pub fn read(&self) -> Result<CredentialSnapshot, CraftError> {
        let record = self.record(false)?;
        if record.pending.is_some() {
            return Err(storage_error(StoreError::Conflict, false));
        }
        Ok(CredentialSnapshot {
            version: record.version(),
            secret: record
                .active
                .as_ref()
                .map(|manifest| self.contents(manifest, false))
                .transpose()?,
        })
    }

    /// Stages and verifies all keyring chunks before atomically selecting the generation.
    /// External OAuth exchange intent must be persisted by the auth protocol before exchange.
    /// Rejects stale versions and unresolved staging. Confirmed activation returns a receipt
    /// even if subsequent cleanup fails or is cancelled; earlier uncertain writes require recovery.
    pub fn replace(
        &self,
        expected: &CredentialVersion,
        secret: &SecretBytes,
        incomplete: IncompletePolicy,
    ) -> Result<GenerationReceipt, CraftError> {
        let mut record = self.record(false)?;
        self.expected(&record, expected)?;
        if record.pending.is_some() {
            return Err(storage_error(StoreError::Conflict, false));
        }
        let mut dirty = false;
        self.cleanup(&mut record, &mut dirty)?;
        let manifest = Manifest {
            id: random_id("gen_").map_err(|mut error| {
                if dirty {
                    error.details.write_state = RequestState::SentUnknown;
                }
                error
            })?,
            length: secret.expose().len(),
            chunks: secret.expose().len().div_ceil(CHUNK_BYTES),
            digest: content_digest(secret.expose()),
        };
        record.pending = Some(Pending {
            manifest: manifest.clone(),
            incomplete,
        });
        self.commit(&mut record, &mut dirty)?;
        for (chunk, bytes) in secret.expose().chunks(CHUNK_BYTES).enumerate() {
            let value =
                SecretBytes::new(bytes.to_vec()).map_err(|error| storage_error(error, true))?;
            self.verified_write(&self.chunk_key(&manifest, chunk), &value, &mut dirty)?;
        }
        self.contents(&manifest, true)?;
        self.activate(&mut record, &mut dirty)?;
        let version = record.version();
        let cleanup_pending = self.cleanup(&mut record, &mut dirty).is_err();
        Ok(GenerationReceipt {
            version,
            cleanup_pending,
        })
    }

    fn activate(&self, record: &mut Record, dirty: &mut bool) -> Result<(), CraftError> {
        let pending = record
            .pending
            .take()
            .ok_or_else(|| storage_error(StoreError::Corrupt, *dirty))?;
        if let Some(previous) = record.active.take() {
            record.retired.push(previous);
        }
        record.active = Some(pending.manifest);
        record.epoch = record
            .epoch
            .checked_add(1)
            .ok_or_else(|| storage_error(StoreError::Corrupt, *dirty))?;
        self.commit(record, dirty)
    }

    /// Resolves only indexed local intent; it never repeats a remote token exchange.
    /// Activates verified staging, or applies its recorded incomplete policy. An inaccessible
    /// vault cannot prove incomplete data and leaves the intent untouched.
    pub fn recover(&self) -> Result<RecoveryReport, CraftError> {
        let mut record = self.record(false)?;
        let mut dirty = false;
        let recovered = record.pending.is_some();
        let mut reauthorization_required = false;
        if let Some(pending) = record.pending.clone() {
            // Availability failures cannot establish whether a generation is incomplete.
            match self.contents(&pending.manifest, false) {
                Ok(_) => self.activate(&mut record, &mut dirty)?,
                Err(error) if error.code == ErrorCode::KeyringUnavailable => {
                    // Re-read individual items to distinguish missing/corrupt data from a locked vault.
                    let incomplete = self.incomplete(&pending.manifest)?;
                    if !incomplete {
                        return Err(error);
                    }
                    record.pending = None;
                    record.retired.push(pending.manifest);
                    if pending.incomplete == IncompletePolicy::Reauthorize {
                        if let Some(previous) = record.active.take() {
                            record.retired.push(previous);
                        }
                        record.epoch = record
                            .epoch
                            .checked_add(1)
                            .ok_or_else(|| storage_error(StoreError::Corrupt, false))?;
                        reauthorization_required = true;
                    }
                    self.commit(&mut record, &mut dirty)?;
                }
                Err(error) => return Err(error),
            }
        }
        if let Some(active) = &record.active {
            self.contents(active, dirty)?;
        }
        let version = record.version();
        let cleanup_pending = self.cleanup(&mut record, &mut dirty).is_err();
        Ok(RecoveryReport {
            version,
            recovered,
            reauthorization_required,
            cleanup_pending,
        })
    }

    fn incomplete(&self, manifest: &Manifest) -> Result<bool, CraftError> {
        let mut buffer = Zeroizing::new(Vec::with_capacity(manifest.length));
        for chunk in 0..manifest.chunks {
            self.check(false)?;
            let value = self
                .store
                .read(&self.chunk_key(manifest, chunk))
                .map_err(|error| storage_error(error, false))?;
            let Some(value) = value else {
                return Ok(true);
            };
            if value.expose().len() != CHUNK_BYTES.min(manifest.length - chunk * CHUNK_BYTES) {
                return Ok(true);
            }
            buffer.extend_from_slice(value.expose());
        }
        Ok(content_digest(&buffer) != manifest.digest)
    }

    /// Commits an inactive epoch before cleanup; stale snapshots cannot reactivate it.
    /// This removes local credential authority only; it does not revoke a provider token.
    pub fn logout(&self, expected: &CredentialVersion) -> Result<GenerationReceipt, CraftError> {
        let mut record = self.record(false)?;
        self.expected(&record, expected)?;
        if let Some(active) = record.active.take() {
            record.retired.push(active);
        }
        if let Some(pending) = record.pending.take() {
            record.retired.push(pending.manifest);
        }
        record.epoch = record
            .epoch
            .checked_add(1)
            .ok_or_else(|| storage_error(StoreError::Corrupt, false))?;
        let mut dirty = false;
        self.commit(&mut record, &mut dirty)?;
        let version = record.version();
        let cleanup_pending = self.cleanup(&mut record, &mut dirty).is_err();
        Ok(GenerationReceipt {
            version,
            cleanup_pending,
        })
    }
}
