//! Explicit native item access. Credentials never fall back to local metadata or a global store.

use std::fmt;
use std::sync::Mutex;

use craft_core::input::canonical_digest;
use craft_core::storage::{CredentialStore, SecretBytes, SecretKey, StoreError};
use serde::Serialize;
use serde_json::json;

use crate::profiles::{NativeBackend, is_opaque_id};

#[cfg(any(windows, target_os = "macos"))]
use keyring_core::api::CredentialStoreApi;

#[cfg(target_os = "linux")]
mod linux;

pub const WINDOWS_ITEM_LIMIT: usize = 2_560;
/// Keychain and Secret Service accept larger items; generations still chunk above this.
pub const UNIX_ITEM_LIMIT: usize = 8_192;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VaultAccess {
    Available,
    Unavailable,
    Locked,
    Unknown,
}

#[derive(Debug, Serialize)]
pub struct VaultHealth {
    pub backend: NativeBackend,
    pub principal: Option<String>,
    pub persistence: &'static str,
    /// Accessibility of one namespaced read probe, not all entries or future writes.
    pub read_access: VaultAccess,
    pub write_access: VaultAccess,
    pub item_limit_bytes: usize,
}

pub struct NativeVault {
    backend: NativeBackend,
    installation: String,
    gate: Mutex<()>,
    #[cfg(windows)]
    store: std::sync::Arc<windows_native_keyring_store::Store>,
    #[cfg(target_os = "macos")]
    store: std::sync::Arc<apple_native_keyring_store::keychain::Store>,
}

impl fmt::Debug for NativeVault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativeVault")
            .field("backend", &self.backend)
            .finish_non_exhaustive()
    }
}

fn item_target(installation: &str, key: &SecretKey) -> Result<String, StoreError> {
    if !is_opaque_id(installation, "ins_") {
        return Err(StoreError::Invalid);
    }
    key.validate()?;
    // Structured hashing prevents service/account delimiter collisions and hides their text.
    let digest = canonical_digest(
        &json!({"installation":installation,"service":key.service,"account":key.account}),
    )
    .map_err(|_| StoreError::Invalid)?;
    Ok(format!("Craft/v1/{installation}/{digest}"))
}

#[cfg(any(windows, target_os = "macos"))]
fn native_error(error: keyring_core::Error, mutating: bool) -> StoreError {
    use keyring_core::Error;
    use zeroize::Zeroize;
    match error {
        Error::BadEncoding(mut bytes) | Error::BadDataFormat(mut bytes, _) => {
            bytes.zeroize();
            if mutating {
                StoreError::OutcomeUnknown
            } else {
                StoreError::Corrupt
            }
        }
        Error::TooLong(_, _) => StoreError::TooLarge,
        Error::Invalid(_, _) => StoreError::Invalid,
        Error::Ambiguous(_) => StoreError::Conflict,
        // This Windows backend uses NoStorageAccess for an absent credential logon session.
        Error::NoStorageAccess(_) => StoreError::Unavailable,
        _ if mutating => StoreError::OutcomeUnknown,
        _ => StoreError::Unavailable,
    }
}

impl NativeVault {
    /// Opens this host's one native store without writing an item: Windows Credential Manager
    /// with local persistence, the macOS login keychain, or the Linux Secret Service login
    /// collection. Unsupported backend/host combinations fail instead of selecting another store.
    pub fn open(backend: NativeBackend, installation: &str) -> Result<Self, StoreError> {
        if !is_opaque_id(installation, "ins_") {
            return Err(StoreError::Invalid);
        }
        #[cfg(windows)]
        {
            if backend != NativeBackend::WindowsCredentialManager {
                return Err(StoreError::Unavailable);
            }
            let store = windows_native_keyring_store::Store::new()
                .map_err(|error| native_error(error, false))?;
            Ok(Self {
                backend,
                installation: installation.into(),
                gate: Mutex::new(()),
                store,
            })
        }
        #[cfg(target_os = "macos")]
        {
            if backend != NativeBackend::MacosKeychain {
                return Err(StoreError::Unavailable);
            }
            let store = apple_native_keyring_store::keychain::Store::new()
                .map_err(|error| native_error(error, false))?;
            Ok(Self {
                backend,
                installation: installation.into(),
                gate: Mutex::new(()),
                store,
            })
        }
        #[cfg(target_os = "linux")]
        {
            // Connection happens per operation; opening never contacts or unlocks the service.
            if backend != NativeBackend::LinuxSecretService {
                return Err(StoreError::Unavailable);
            }
            Ok(Self {
                backend,
                installation: installation.into(),
                gate: Mutex::new(()),
            })
        }
        #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
        {
            let _ = backend;
            Err(StoreError::Unavailable)
        }
    }

    pub fn item_limit_bytes(&self) -> usize {
        match self.backend {
            NativeBackend::WindowsCredentialManager => WINDOWS_ITEM_LIMIT,
            NativeBackend::MacosKeychain | NativeBackend::LinuxSecretService => UNIX_ITEM_LIMIT,
        }
    }

    /// Probes only this installation's reserved item and never tests permission by writing.
    pub fn health(&self) -> VaultHealth {
        let principal = craft_platform::current_process_principal().ok();
        #[cfg(target_os = "linux")]
        let collection = linux::locked();
        #[cfg(not(target_os = "linux"))]
        let collection: Result<bool, StoreError> = Ok(false);
        let read_access = if principal.is_none() {
            VaultAccess::Unavailable
        } else if collection == Ok(true) || collection == Err(StoreError::Locked) {
            VaultAccess::Locked
        } else if collection.is_err() {
            VaultAccess::Unavailable
        } else {
            match self.read(&SecretKey {
                service: "native-health".into(),
                account: "read-probe".into(),
            }) {
                Ok(_) => VaultAccess::Available,
                Err(StoreError::Locked) => VaultAccess::Locked,
                Err(_) => VaultAccess::Unavailable,
            }
        };
        VaultHealth {
            backend: self.backend,
            principal,
            persistence: match self.backend {
                NativeBackend::WindowsCredentialManager => "local_machine",
                NativeBackend::MacosKeychain => "login_keychain",
                NativeBackend::LinuxSecretService => "login_collection",
            },
            read_access,
            write_access: VaultAccess::Unknown,
            item_limit_bytes: self.item_limit_bytes(),
        }
    }

    #[cfg(windows)]
    fn entry(&self, target: &str) -> Result<keyring_core::Entry, StoreError> {
        let modifiers =
            std::collections::HashMap::from([("target", target), ("persistence", "Local")]);
        self.store
            .build("Craft AI Factory", "credential", Some(&modifiers))
            .map_err(|error| native_error(error, false))
    }

    #[cfg(windows)]
    fn compatible(entry: &keyring_core::Entry, target: &str) -> Result<bool, StoreError> {
        match entry.get_attributes() {
            Ok(attributes)
                if attributes.get("persistence").map(String::as_str) == Some("Local")
                    && attributes.get("target_name").map(String::as_str) == Some(target) =>
            {
                Ok(true)
            }
            Ok(_) => Err(StoreError::Corrupt),
            Err(keyring_core::Error::NoEntry) => Ok(false),
            Err(error) => Err(native_error(error, false)),
        }
    }

    /// One generic password per hashed target in the user's login keychain.
    #[cfg(target_os = "macos")]
    fn entry(&self, target: &str) -> Result<keyring_core::Entry, StoreError> {
        self.store
            .build("Craft AI Factory", target, None)
            .map_err(|error| native_error(error, false))
    }
}

impl CredentialStore for NativeVault {
    fn read(&self, key: &SecretKey) -> Result<Option<SecretBytes>, StoreError> {
        let target = item_target(&self.installation, key)?;
        let _guard = self.gate.lock().map_err(|_| StoreError::Unavailable)?;
        #[cfg(windows)]
        {
            let entry = self.entry(&target)?;
            if !Self::compatible(&entry, &target)? {
                return Ok(None);
            }
            match entry.get_secret() {
                Ok(bytes) => {
                    let secret = SecretBytes::new(bytes).map_err(|_| StoreError::Corrupt)?;
                    if secret.expose().len() > self.item_limit_bytes() {
                        return Err(StoreError::Corrupt);
                    }
                    Ok(Some(secret))
                }
                Err(keyring_core::Error::NoEntry) => Ok(None),
                Err(error) => Err(native_error(error, false)),
            }
        }
        #[cfg(target_os = "macos")]
        {
            match self.entry(&target)?.get_secret() {
                Ok(bytes) => {
                    let secret = SecretBytes::new(bytes).map_err(|_| StoreError::Corrupt)?;
                    if secret.expose().len() > self.item_limit_bytes() {
                        return Err(StoreError::Corrupt);
                    }
                    Ok(Some(secret))
                }
                Err(keyring_core::Error::NoEntry) => Ok(None),
                Err(error) => Err(native_error(error, false)),
            }
        }
        #[cfg(target_os = "linux")]
        {
            linux::read(&target, self.item_limit_bytes())
        }
        #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
        {
            let _ = target;
            Err(StoreError::Unavailable)
        }
    }

    fn write(&self, key: &SecretKey, secret: &SecretBytes) -> Result<(), StoreError> {
        let target = item_target(&self.installation, key)?;
        if secret.expose().len() > self.item_limit_bytes() {
            return Err(StoreError::TooLarge);
        }
        let _guard = self.gate.lock().map_err(|_| StoreError::Unavailable)?;
        #[cfg(windows)]
        {
            let entry = self.entry(&target)?;
            Self::compatible(&entry, &target)?;
            entry
                .set_secret(secret.expose())
                .map_err(|error| native_error(error, true))
        }
        #[cfg(target_os = "macos")]
        {
            self.entry(&target)?
                .set_secret(secret.expose())
                .map_err(|error| native_error(error, true))
        }
        #[cfg(target_os = "linux")]
        {
            linux::write(&target, secret)
        }
        #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
        {
            let _ = target;
            Err(StoreError::Unavailable)
        }
    }

    fn delete(&self, key: &SecretKey) -> Result<(), StoreError> {
        let target = item_target(&self.installation, key)?;
        let _guard = self.gate.lock().map_err(|_| StoreError::Unavailable)?;
        #[cfg(windows)]
        {
            let entry = self.entry(&target)?;
            if !Self::compatible(&entry, &target)? {
                return Ok(());
            }
            match entry.delete_credential() {
                Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
                Err(error) => Err(native_error(error, true)),
            }
        }
        #[cfg(target_os = "macos")]
        {
            match self.entry(&target)?.delete_credential() {
                Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
                Err(error) => Err(native_error(error, true)),
            }
        }
        #[cfg(target_os = "linux")]
        {
            linux::delete(&target)
        }
        #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
        {
            let _ = target;
            Err(StoreError::Unavailable)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INSTALLATION: &str = "ins_00000000000000000000000000000000";

    #[test]
    fn structured_item_identity_is_stable_collision_resistant_and_installation_bound()
    -> Result<(), Box<dyn std::error::Error>> {
        let first = SecretKey {
            service: "a.b".into(),
            account: "c".into(),
        };
        let second = SecretKey {
            service: "b".into(),
            account: "c.a".into(),
        };
        let target = item_target(INSTALLATION, &first)?;
        assert_eq!(target, item_target(INSTALLATION, &first)?);
        assert_ne!(target, item_target(INSTALLATION, &second)?);
        assert_ne!(
            target,
            item_target("ins_10000000000000000000000000000000", &first)?
        );
        assert!(target.starts_with("Craft/v1/ins_"));
        assert!(!target.contains("a.b"));
        assert!(item_target("private-sentinel", &first).is_err());
        assert!(
            item_target(
                INSTALLATION,
                &SecretKey {
                    service: "".into(),
                    account: "x".into()
                }
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn unsupported_backend_selection_never_becomes_an_implicit_fallback() {
        let other_hosts: &[NativeBackend] = if cfg!(windows) {
            &[
                NativeBackend::MacosKeychain,
                NativeBackend::LinuxSecretService,
            ]
        } else if cfg!(target_os = "macos") {
            &[
                NativeBackend::WindowsCredentialManager,
                NativeBackend::LinuxSecretService,
            ]
        } else {
            &[
                NativeBackend::WindowsCredentialManager,
                NativeBackend::MacosKeychain,
            ]
        };
        for &backend in other_hosts {
            assert_eq!(
                NativeVault::open(backend, INSTALLATION).err(),
                Some(StoreError::Unavailable)
            );
        }
        for backend in [
            NativeBackend::WindowsCredentialManager,
            NativeBackend::MacosKeychain,
            NativeBackend::LinuxSecretService,
        ] {
            assert_eq!(
                NativeVault::open(backend, "bad").err(),
                Some(StoreError::Invalid)
            );
        }
    }

    #[test]
    #[cfg(windows)]
    fn native_errors_are_redacted_and_mutation_failures_preserve_uncertainty() {
        let unavailable = keyring_core::Error::NoStorageAccess(Box::new(std::io::Error::other(
            "private-sentinel",
        )));
        assert_eq!(native_error(unavailable, false), StoreError::Unavailable);
        let failure = keyring_core::Error::PlatformFailure(Box::new(std::io::Error::other(
            "private-sentinel",
        )));
        let error = native_error(failure, true);
        assert_eq!(error, StoreError::OutcomeUnknown);
        assert!(!format!("{error:?}: {error}").contains("private-sentinel"));
        assert_eq!(
            native_error(
                keyring_core::Error::BadEncoding(b"private-sentinel".to_vec()),
                false
            ),
            StoreError::Corrupt
        );
        assert_eq!(
            native_error(keyring_core::Error::TooLong("secret".into(), 2560), true),
            StoreError::TooLarge
        );
    }

    #[test]
    #[cfg(windows)]
    #[ignore = "Writes only randomly namespaced synthetic items to the real Windows vault; run explicitly with native access"]
    fn native_windows_vault_round_trip_restart_limits_and_persistence()
    -> Result<(), Box<dyn std::error::Error>> {
        use craft_test_support::process::{RunOptions, run};
        use std::ffi::OsString;

        fn fixture_secret() -> Result<SecretBytes, StoreError> {
            SecretBytes::new(
                (0..WINDOWS_ITEM_LIMIT)
                    .map(|index| (index % 251) as u8)
                    .collect(),
            )
        }
        fn fixture_key() -> SecretKey {
            SecretKey {
                service: "native-vault-acceptance".into(),
                account: "binary-sentinel".into(),
            }
        }
        if let Some(installation) = std::env::var_os("CRAFT_NATIVE_VAULT_READ") {
            let installation = installation
                .to_str()
                .ok_or("Invalid native test namespace")?;
            let vault = NativeVault::open(NativeBackend::WindowsCredentialManager, installation)?;
            let actual = vault
                .read(&fixture_key())?
                .ok_or("Native sentinel did not survive process restart")?;
            assert!(actual.expose() == fixture_secret()?.expose());
            return Ok(());
        }

        struct Cleanup {
            vault: NativeVault,
            key: SecretKey,
        }
        impl Drop for Cleanup {
            fn drop(&mut self) {
                if let Err(error) = self.vault.delete(&self.key) {
                    eprintln!(
                        "Native sentinel cleanup failed for {}: {error}",
                        self.vault.installation
                    );
                }
            }
        }
        struct EntryCleanup(keyring_core::Entry);
        impl Drop for EntryCleanup {
            fn drop(&mut self) {
                let _ = self.0.delete_credential();
            }
        }
        struct DefaultStoreReset(Option<std::sync::Arc<keyring_core::CredentialStore>>);
        impl Drop for DefaultStoreReset {
            fn drop(&mut self) {
                keyring_core::unset_default_store();
                if let Some(store) = self.0.take() {
                    keyring_core::set_default_store(store);
                }
            }
        }

        let installation = crate::profiles::random_id("ins_")?;
        let vault = NativeVault::open(NativeBackend::WindowsCredentialManager, &installation)?;
        let cleanup = Cleanup {
            vault,
            key: fixture_key(),
        };
        let vault = &cleanup.vault;
        let key = &cleanup.key;
        let health = vault.health();
        assert_eq!(health.read_access, VaultAccess::Available);
        assert_eq!(health.write_access, VaultAccess::Unknown);
        assert!(
            health
                .principal
                .as_deref()
                .is_some_and(|principal| principal.starts_with("S-1-"))
        );
        assert_eq!(health.persistence, "local_machine");
        assert!(vault.read(key)?.is_none());

        let _default_reset = DefaultStoreReset(keyring_core::get_default_store());
        keyring_core::set_default_store(keyring_core::mock::Store::new()?);
        vault.write(key, &fixture_secret()?)?;
        assert!(
            vault.read(key)?.ok_or("Missing native sentinel")?.expose()
                == fixture_secret()?.expose()
        );
        let target = item_target(&installation, key)?;
        assert_eq!(
            vault
                .entry(&target)?
                .get_attributes()?
                .get("persistence")
                .map(String::as_str),
            Some("Local")
        );
        let too_large = SecretBytes::new(vec![0; WINDOWS_ITEM_LIMIT + 1])?;
        assert_eq!(vault.write(key, &too_large), Err(StoreError::TooLarge));
        assert!(
            vault
                .read(key)?
                .ok_or("Oversize refusal lost existing sentinel")?
                .expose()
                == fixture_secret()?.expose()
        );
        let foreign = NativeVault::open(
            NativeBackend::WindowsCredentialManager,
            &crate::profiles::random_id("ins_")?,
        )?;
        assert!(foreign.read(key)?.is_none());

        let binary = std::env::current_exe()?;
        let output = run(
            &binary,
            &[
                "--ignored",
                "--exact",
                "vault::tests::native_windows_vault_round_trip_restart_limits_and_persistence",
            ],
            None,
            RunOptions {
                environment: vec![(
                    OsString::from("CRAFT_NATIVE_VAULT_READ"),
                    Some(installation.clone().into()),
                )],
                ..RunOptions::default()
            },
        )?;
        assert!(
            output.status.success(),
            "Native restart child failed with sanitized output: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(output.stderr.is_empty());

        let wrong_key = SecretKey {
            service: key.service.clone(),
            account: "session-only-sentinel".into(),
        };
        let wrong_target = item_target(&installation, &wrong_key)?;
        let session = vault.store.build(
            "Craft AI Factory",
            "credential",
            Some(&std::collections::HashMap::from([
                ("target", wrong_target.as_str()),
                ("persistence", "Session"),
            ])),
        )?;
        let session = EntryCleanup(session);
        session.0.set_secret(b"non-sensitive-session-sentinel")?;
        assert_eq!(vault.read(&wrong_key).err(), Some(StoreError::Corrupt));
        assert_eq!(
            vault.write(&wrong_key, &fixture_secret()?),
            Err(StoreError::Corrupt)
        );
        assert_eq!(vault.delete(&wrong_key), Err(StoreError::Corrupt));
        session.0.delete_credential()?;
        vault.delete(key)?;
        assert!(vault.read(key)?.is_none());
        vault.delete(key)?;
        Ok(())
    }

    #[test]
    #[cfg(target_os = "linux")]
    #[ignore = "Writes randomly namespaced synthetic items to a real Secret Service and locks its default collection; run explicitly in a disposable session"]
    fn native_linux_secret_service_round_trip_limits_and_locked_refusal()
    -> Result<(), Box<dyn std::error::Error>> {
        let installation = crate::profiles::random_id("ins_")?;
        let vault = NativeVault::open(NativeBackend::LinuxSecretService, &installation)?;
        let key = SecretKey {
            service: "native-vault-acceptance".into(),
            account: "binary-sentinel".into(),
        };
        let full = SecretBytes::new((0..UNIX_ITEM_LIMIT).map(|n| (n % 251) as u8).collect())?;
        assert!(vault.read(&key)?.is_none());
        vault.write(&key, &full)?;
        assert!(vault.read(&key)?.ok_or("missing")?.expose() == full.expose());
        let replaced = SecretBytes::new("日本語🦀".as_bytes().to_vec())?;
        vault.write(&key, &replaced)?;
        assert!(vault.read(&key)?.ok_or("missing")?.expose() == replaced.expose());
        let oversized = SecretBytes::new(vec![7; UNIX_ITEM_LIMIT + 1])?;
        assert_eq!(vault.write(&key, &oversized), Err(StoreError::TooLarge));
        assert_eq!(vault.health().read_access, VaultAccess::Available);
        vault.delete(&key)?;
        vault.delete(&key)?;
        assert!(vault.read(&key)?.is_none());

        // Lock without unlocking: every operation must refuse rather than prompt.
        vault.write(&key, &replaced)?;
        let service =
            secret_service::blocking::SecretService::connect(secret_service::EncryptionType::Dh)?;
        service.get_default_collection()?.lock()?;
        assert_eq!(vault.read(&key).err(), Some(StoreError::Locked));
        assert_eq!(vault.write(&key, &replaced), Err(StoreError::Locked));
        assert_eq!(vault.delete(&key), Err(StoreError::Locked));
        assert_eq!(vault.health().read_access, VaultAccess::Locked);
        Ok(())
    }
}
