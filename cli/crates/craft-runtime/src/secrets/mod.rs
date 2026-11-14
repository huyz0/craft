//! Keyring-only generations protected by installation/grant-scoped OS locks.

mod generations;
mod locks;
#[cfg(feature = "synthetic-acceptance")]
pub(crate) mod synthetic;
#[cfg(test)]
pub(crate) mod tests;

use craft_core::contracts::GrantRef;
use craft_core::control::InvocationControl;
use craft_core::error::CraftError;

use crate::profiles::ProfileStore;
use crate::vault::NativeVault;
use locks::GrantLocks;

pub use generations::{
    CredentialSnapshot, CredentialState, CredentialVersion, GenerationReceipt, IncompletePolicy,
    LockedCredentials, RecoveryReport,
};

pub struct NativeCredentials {
    locks: GrantLocks,
    vault: NativeVault,
}

impl NativeCredentials {
    /// Uses only the validated installation root and its explicitly selected native vault.
    pub fn open(profiles: &ProfileStore) -> Result<Self, CraftError> {
        let settings = profiles.settings();
        let locks = GrantLocks::open(profiles.path(), &settings.installation_id)?;
        let vault = NativeVault::open(settings.credential_backend, &settings.installation_id)
            .map_err(|error| generations::storage_error(error, false))?;
        Ok(Self { locks, vault })
    }

    /// Retains exclusive local grant ownership until the returned session is dropped.
    pub fn lock(
        &self,
        grant: &GrantRef,
        control: &InvocationControl,
    ) -> Result<LockedCredentials<'_>, CraftError> {
        let guard = self.locks.acquire(grant, control)?;
        Ok(LockedCredentials::new(&self.vault, guard, control.clone()))
    }
}
