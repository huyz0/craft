use std::collections::BTreeMap;
use std::sync::Mutex;

use craft_core::storage::{CredentialStore, SecretBytes, SecretKey, StoreError};

use super::*;

#[derive(Default)]
struct MemoryVault(Mutex<BTreeMap<SecretKey, SecretBytes>>);
impl CredentialStore for MemoryVault {
    fn read(&self, key: &SecretKey) -> Result<Option<SecretBytes>, StoreError> {
        key.validate()?;
        self.0
            .lock()
            .map_err(|_| StoreError::Unavailable)?
            .get(key)
            .map(|value| SecretBytes::new(value.expose().to_vec()))
            .transpose()
    }
    fn write(&self, key: &SecretKey, secret: &SecretBytes) -> Result<(), StoreError> {
        key.validate()?;
        let mut values = self.0.lock().map_err(|_| StoreError::Unavailable)?;
        let bytes: usize = values.values().map(|value| value.expose().len()).sum();
        if values.len() >= 8192
            || bytes.saturating_add(secret.expose().len())
                > 8 * craft_core::storage::MAX_SECRET_BYTES
        {
            return Err(StoreError::TooLarge);
        }
        values.insert(key.clone(), SecretBytes::new(secret.expose().to_vec())?);
        Ok(())
    }
    fn delete(&self, key: &SecretKey) -> Result<(), StoreError> {
        key.validate()?;
        self.0
            .lock()
            .map_err(|_| StoreError::Unavailable)?
            .remove(key);
        Ok(())
    }
}

pub(crate) struct SyntheticCredentials {
    locks: GrantLocks,
    vault: MemoryVault,
}
impl SyntheticCredentials {
    pub(crate) fn new(profiles: &ProfileStore) -> Result<Self, CraftError> {
        Ok(Self {
            locks: GrantLocks::open(profiles.path(), &profiles.settings().installation_id)?,
            vault: MemoryVault::default(),
        })
    }
    pub(crate) fn lock(
        &self,
        grant: &GrantRef,
        control: &InvocationControl,
    ) -> Result<LockedCredentials<'_>, CraftError> {
        Ok(LockedCredentials::new(
            &self.vault,
            self.locks.acquire(grant, control)?,
            control.clone(),
        ))
    }
}
