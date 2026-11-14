use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use craft_core::storage::{CredentialStore, SecretBytes, SecretKey, StoreError};

use super::{FaultPoint, Faults};

#[derive(Clone, Copy)]
pub enum VaultMode {
    Available,
    Locked,
    Unavailable,
}

struct VaultState {
    mode: VaultMode,
    entries: BTreeMap<SecretKey, SecretBytes>,
}

#[derive(Clone)]
pub struct MemoryVault {
    state: Arc<Mutex<VaultState>>,
    faults: Faults,
}

impl MemoryVault {
    pub fn new(faults: Faults) -> Self {
        Self {
            state: Arc::new(Mutex::new(VaultState {
                mode: VaultMode::Available,
                entries: BTreeMap::new(),
            })),
            faults,
        }
    }

    pub fn set_mode(&self, mode: VaultMode) -> Result<(), StoreError> {
        self.state.lock().map_err(|_| StoreError::Unavailable)?.mode = mode;
        Ok(())
    }
}

fn check_mode(mode: VaultMode) -> Result<(), StoreError> {
    match mode {
        VaultMode::Available => Ok(()),
        VaultMode::Locked => Err(StoreError::Locked),
        VaultMode::Unavailable => Err(StoreError::Unavailable),
    }
}

impl CredentialStore for MemoryVault {
    fn read(&self, key: &SecretKey) -> Result<Option<SecretBytes>, StoreError> {
        key.validate()?;
        if self.faults.hit(FaultPoint::VaultRead)? {
            return Err(StoreError::Unavailable);
        }
        let state = self.state.lock().map_err(|_| StoreError::Unavailable)?;
        check_mode(state.mode)?;
        state
            .entries
            .get(key)
            .map(|secret| SecretBytes::new(secret.expose().to_vec()))
            .transpose()
    }

    fn write(&self, key: &SecretKey, secret: &SecretBytes) -> Result<(), StoreError> {
        key.validate()?;
        let mut state = self.state.lock().map_err(|_| StoreError::Unavailable)?;
        check_mode(state.mode)?;
        if self.faults.hit(FaultPoint::VaultWriteBeforeCommit)? {
            return Err(StoreError::Unavailable);
        }
        state
            .entries
            .insert(key.clone(), SecretBytes::new(secret.expose().to_vec())?);
        if self.faults.hit(FaultPoint::VaultWriteAfterCommit)? {
            return Err(StoreError::OutcomeUnknown);
        }
        Ok(())
    }

    fn delete(&self, key: &SecretKey) -> Result<(), StoreError> {
        key.validate()?;
        let mut state = self.state.lock().map_err(|_| StoreError::Unavailable)?;
        check_mode(state.mode)?;
        if self.faults.hit(FaultPoint::VaultDeleteBeforeCommit)? {
            return Err(StoreError::Unavailable);
        }
        state.entries.remove(key);
        if self.faults.hit(FaultPoint::VaultDeleteAfterCommit)? {
            return Err(StoreError::OutcomeUnknown);
        }
        Ok(())
    }
}
