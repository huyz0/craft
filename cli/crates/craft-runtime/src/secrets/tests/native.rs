use std::ffi::OsString;

use craft_core::contracts::AuthMode;
use craft_core::profiles::ProfileInput;
use craft_test_support::process::{RunOptions, run};

use super::*;
use crate::profiles::ProfileStore;
use crate::vault::NativeVault;

fn unicode_payload() -> Vec<u8> {
    "日本語🦀\n".repeat(700).into_bytes()
}

fn binary_payload() -> Vec<u8> {
    (0..12_345).map(|index| (index % 251) as u8).collect()
}

pub(crate) struct NativeCleanup {
    pub(crate) credentials: NativeCredentials,
    pub(crate) grant: GrantRef,
    cleaned: bool,
}

impl NativeCleanup {
    pub(crate) fn new(
        profiles: &ProfileStore,
        grant: GrantRef,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            credentials: NativeCredentials::open(profiles)?,
            grant,
            cleaned: false,
        })
    }

    pub(crate) fn cleanup(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let session = self.credentials.lock(&self.grant, &control(30_000)?)?;
        let receipt = session.logout(&session.inspect()?.version)?;
        if receipt.cleanup_pending {
            return Err("Synthetic native credential cleanup is pending".into());
        }
        // Production logout retains tombstones; only this owned test namespace is removed.
        let key = index_key(&self.grant);
        self.credentials.vault.delete(&key)?;
        if self.credentials.vault.read(&key)?.is_some() {
            return Err("Synthetic native index cleanup failed".into());
        }
        self.cleaned = true;
        Ok(())
    }
}

impl Drop for NativeCleanup {
    fn drop(&mut self) {
        if !self.cleaned && self.cleanup().is_err() {
            eprintln!(
                "Synthetic native generation cleanup failed for {}",
                self.grant.id
            );
        }
    }
}

struct RefuseChunks<'a>(&'a NativeVault);

impl CredentialStore for RefuseChunks<'_> {
    fn read(&self, key: &SecretKey) -> Result<Option<SecretBytes>, StoreError> {
        self.0.read(key)
    }

    fn write(&self, key: &SecretKey, secret: &SecretBytes) -> Result<(), StoreError> {
        if !key.account.ends_with("/index") {
            return Err(StoreError::Unavailable);
        }
        self.0.write(key, secret)
    }

    fn delete(&self, key: &SecretKey) -> Result<(), StoreError> {
        self.0.delete(key)
    }
}

fn native_child(root: &Path, mode: &str) -> Result<(), Box<dyn std::error::Error>> {
    let output = run(
        &std::env::current_exe()?,
        &[
            "--ignored",
            "--exact",
            "secrets::tests::native::native_generations_survive_restart_recover_and_logout",
        ],
        None,
        RunOptions {
            timeout: Duration::from_secs(30),
            max_stream_bytes: 16_384,
            environment: vec![
                (
                    OsString::from("CRAFT_NATIVE_GENERATION_ROOT"),
                    Some(root.into()),
                ),
                (
                    OsString::from("CRAFT_NATIVE_GENERATION_MODE"),
                    Some(mode.into()),
                ),
            ],
            ..RunOptions::default()
        },
    )?;
    assert!(
        output.status.success(),
        "Native generation child failed: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(output.stderr.is_empty());
    Ok(())
}

#[test]
#[ignore = "Writes owned synthetic credential generations to the real native vault; run explicitly with native access"]
fn native_generations_survive_restart_recover_and_logout() -> Result<(), Box<dyn std::error::Error>>
{
    if let Some(root) = std::env::var_os("CRAFT_NATIVE_GENERATION_ROOT") {
        let profiles = ProfileStore::open(Path::new(&root), &control(30_000)?)?;
        let profile = profiles.get("native-generation", &control(30_000)?)?;
        let credentials = NativeCredentials::open(&profiles)?;
        let session = credentials.lock(&profile.grant, &control(30_000)?)?;
        match std::env::var("CRAFT_NATIVE_GENERATION_MODE")?.as_str() {
            "unicode" => {
                assert!(
                    session
                        .read()?
                        .secret
                        .ok_or("Missing Unicode generation")?
                        .expose()
                        == unicode_payload()
                );
            }
            "binary-logout" => {
                let current = session.read()?;
                assert!(
                    current.secret.ok_or("Missing binary generation")?.expose() == binary_payload()
                );
                assert!(!session.logout(&current.version)?.cleanup_pending);
                assert!(session.read()?.secret.is_none());
            }
            "recover" => {
                assert!(session.read().is_err());
                let report = session.recover()?;
                assert!(
                    report.recovered && report.reauthorization_required && !report.cleanup_pending
                );
                assert!(session.read()?.secret.is_none());
            }
            _ => return Err("Unknown native generation child action".into()),
        }
        return Ok(());
    }

    let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
    fs::create_dir_all(&parent)?;
    let parent = parent.canonicalize()?;
    let root = parent.join(random_id("native-generation-")?);
    let _files = FixtureCleanup {
        parent,
        root: root.clone(),
    };
    let profiles = ProfileStore::open(&root, &control(30_000)?)?;
    let profile = profiles.create(
        &ProfileInput {
            name: "native-generation".into(),
            provider: "jira".into(),
            deployment: "cloud".into(),
            auth_mode: AuthMode::Delegated,
            grant: None,
            tenant: None,
            defaults: Default::default(),
        },
        &control(30_000)?,
        |_| Ok(()),
    )?;
    let mut cleanup = NativeCleanup {
        credentials: NativeCredentials::open(&profiles)?,
        grant: profile.grant,
        cleaned: false,
    };
    let session = cleanup
        .credentials
        .lock(&cleanup.grant, &control(30_000)?)?;
    let empty = session.read()?.version;
    let unicode = session.replace(
        &empty,
        &secret(&unicode_payload())?,
        IncompletePolicy::KeepPrevious,
    )?;
    assert!(!unicode.cleanup_pending);
    drop(session);
    native_child(&root, "unicode")?;
    let session = cleanup
        .credentials
        .lock(&cleanup.grant, &control(30_000)?)?;
    let binary = session.replace(
        &unicode.version,
        &secret(&binary_payload())?,
        IncompletePolicy::KeepPrevious,
    )?;
    assert!(!binary.cleanup_pending);
    assert!(
        session
            .replace(
                &unicode.version,
                &secret(b"stale")?,
                IncompletePolicy::KeepPrevious
            )
            .is_err()
    );
    drop(session);
    native_child(&root, "binary-logout")?;
    let session = cleanup
        .credentials
        .lock(&cleanup.grant, &control(30_000)?)?;
    assert!(
        session
            .replace(
                &binary.version,
                &secret(b"revival")?,
                IncompletePolicy::KeepPrevious
            )
            .is_err()
    );
    let previous = session.replace(
        &session.read()?.version,
        &secret(b"previous")?,
        IncompletePolicy::KeepPrevious,
    )?;
    drop(session);
    let refused = RefuseChunks(&cleanup.credentials.vault);
    let interrupted = LockedCredentials::new(
        &refused,
        cleanup
            .credentials
            .locks
            .acquire(&cleanup.grant, &control(30_000)?)?,
        control(30_000)?,
    );
    assert!(
        interrupted
            .replace(
                &previous.version,
                &secret(&unicode_payload())?,
                IncompletePolicy::Reauthorize
            )
            .is_err()
    );
    assert!(interrupted.inspect()?.recovery_required);
    drop(interrupted);
    native_child(&root, "recover")?;
    cleanup.cleanup()?;
    drop(cleanup);
    drop(profiles);
    drop(_files);
    assert!(!root.exists());
    Ok(())
}
