use std::fs;
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use craft_core::contracts::GrantRef;
use craft_core::control::{CancellationToken, InvocationControl};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::storage::{CredentialStore, MAX_SECRET_BYTES, SecretBytes, SecretKey, StoreError};
use craft_test_support::state::{FaultPoint, Faults, MemoryVault, VaultMode};

use super::locks::GrantLocks;
use super::*;
use crate::control::SystemClock;
use crate::profiles::random_id;

#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
pub(crate) mod native;

pub(crate) struct Fixture {
    pub(crate) root: PathBuf,
    locks: GrantLocks,
    pub(crate) vault: MemoryVault,
    pub(crate) faults: Faults,
    // Fields drop in declaration order; close pinned root handles before removal.
    _cleanup: FixtureCleanup,
}

struct FixtureCleanup {
    parent: PathBuf,
    root: PathBuf,
}

impl Fixture {
    pub(crate) fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        fs::create_dir_all(&parent)?;
        let parent = parent.canonicalize()?;
        let root = parent.join(random_id("credential-fixture-")?);
        let locks = GrantLocks::open(&root, INSTALLATION)?;
        let faults = Faults::default();
        let vault = MemoryVault::new(faults.clone());
        Ok(Self {
            _cleanup: FixtureCleanup {
                parent,
                root: root.clone(),
            },
            root,
            locks,
            vault,
            faults,
        })
    }
    pub(crate) fn lock(&self, grant: &GrantRef) -> Result<LockedCredentials<'_>, CraftError> {
        Ok(LockedCredentials::new(
            &self.vault,
            self.locks.acquire(grant, &control(10_000)?)?,
            control(10_000)?,
        ))
    }
}

impl Drop for FixtureCleanup {
    fn drop(&mut self) {
        if let Ok(path) = self.root.canonicalize()
            && path.starts_with(&self.parent)
            && path != self.parent
        {
            let _ = fs::remove_dir_all(path);
        }
    }
}

const INSTALLATION: &str = "ins_00000000000000000000000000000000";

#[test]
fn status_inspection_reads_only_the_index_without_secret_chunks_or_credential_writes()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let session = fixture.lock(&grant(1))?;
    session.replace(
        &session.read()?.version,
        &secret(b"synthetic-secret")?,
        IncompletePolicy::KeepPrevious,
    )?;
    let reads = fixture.faults.calls(FaultPoint::VaultRead)?;
    let writes = fixture.faults.calls(FaultPoint::VaultWriteBeforeCommit)?;
    fixture.faults.fail_on(FaultPoint::VaultRead, 2)?;
    let status = session.inspect()?;
    assert!(status.active);
    assert!(!status.recovery_required);
    assert_eq!(fixture.faults.calls(FaultPoint::VaultRead)?, reads + 1);
    assert_eq!(
        fixture.faults.calls(FaultPoint::VaultWriteBeforeCommit)?,
        writes
    );
    assert!(session.read().is_err());
    assert_eq!(
        fixture.faults.calls(FaultPoint::VaultWriteBeforeCommit)?,
        writes
    );
    Ok(())
}
fn grant(value: u32) -> GrantRef {
    GrantRef {
        id: format!("gr_{value:032x}"),
    }
}
fn control(timeout: u64) -> Result<InvocationControl, CraftError> {
    InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        timeout,
    )
}
fn secret(value: &[u8]) -> Result<SecretBytes, Box<dyn std::error::Error>> {
    Ok(SecretBytes::new(value.to_vec())?)
}
fn index_key(grant: &GrantRef) -> SecretKey {
    SecretKey {
        service: "credential-generations-v1".into(),
        account: format!("{}/index", grant.id),
    }
}

#[test]
fn kernel_locks_bound_waits_and_do_not_block_other_grants() -> Result<(), Box<dyn std::error::Error>>
{
    let fixture = Fixture::new()?;
    let first = fixture.locks.acquire(&grant(1), &control(1_000)?)?;
    let second_manager = GrantLocks::open(&fixture.root, INSTALLATION)?;
    assert_eq!(
        second_manager
            .acquire(&grant(1), &control(30)?)
            .err()
            .ok_or("Expected lock deadline")?
            .code,
        ErrorCode::DeadlineExceeded
    );
    let unrelated = second_manager.acquire(&grant(2), &control(1_000)?)?;
    drop(first);
    let reacquired = second_manager.acquire(&grant(1), &control(1_000)?)?;
    drop((unrelated, reacquired));
    let token = CancellationToken::default();
    token.cancel();
    let cancelled = InvocationControl::new(Arc::new(SystemClock::default()), token, 1_000)?;
    assert_eq!(
        second_manager
            .acquire(&grant(1), &cancelled)
            .err()
            .ok_or("Expected cancellation")?
            .code,
        ErrorCode::Cancelled
    );
    Ok(())
}

struct LockChild(Child);

impl Drop for LockChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn grant_lock_child() -> Result<(), Box<dyn std::error::Error>> {
    let Some(root) = std::env::var_os("CRAFT_TEST_GRANT_LOCK_ROOT") else {
        return Ok(());
    };
    let locks = GrantLocks::open(Path::new(&root), INSTALLATION)?;
    let _guard = locks.acquire(&grant(1), &control(5_000)?)?;
    writeln!(std::io::stdout(), "grant-lock-ready")?;
    std::io::stdout().flush()?;
    let mut signal = [0];
    std::io::stdin().read_exact(&mut signal)?;
    Ok(())
}

#[test]
fn cross_process_lock_excludes_same_grant_and_is_released_after_kill()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let mut child = LockChild(
        Command::new(std::env::current_exe()?)
            .args(["--exact", "secrets::tests::grant_lock_child", "--nocapture"])
            .env("CRAFT_TEST_GRANT_LOCK_ROOT", &fixture.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let stdout = child.0.stdout.take().ok_or("Missing child output")?;
    let (sender, receiver) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let output = std::io::BufReader::new(stdout.take(4_096));
        for line in output.lines().map_while(Result::ok) {
            if line == "grant-lock-ready" {
                let _ = sender.send(());
                break;
            }
        }
    });
    receiver.recv_timeout(Duration::from_secs(15))?;
    assert_eq!(
        fixture
            .locks
            .acquire(&grant(1), &control(50)?)
            .err()
            .ok_or("Child did not exclude parent")?
            .code,
        ErrorCode::DeadlineExceeded
    );
    let unrelated = fixture.locks.acquire(&grant(2), &control(1_000)?)?;
    drop(unrelated);
    child.0.kill()?;
    child.0.wait()?;
    reader.join().map_err(|_| "Child reader panicked")?;
    let released = fixture.locks.acquire(&grant(1), &control(1_000)?)?;
    drop(released);
    let root = fixture.root.clone();
    drop(fixture);
    assert!(
        !root.exists(),
        "Fixture left its owned lock directory behind"
    );
    Ok(())
}

struct CancelAfterActivation<'a> {
    vault: &'a MemoryVault,
    token: CancellationToken,
}

impl CredentialStore for CancelAfterActivation<'_> {
    fn read(&self, key: &SecretKey) -> Result<Option<SecretBytes>, StoreError> {
        self.vault.read(key)
    }

    fn write(&self, key: &SecretKey, secret: &SecretBytes) -> Result<(), StoreError> {
        self.vault.write(key, secret)?;
        if key.account.ends_with("/index") {
            let record: serde_json::Value =
                serde_json::from_slice(secret.expose()).map_err(|_| StoreError::Corrupt)?;
            if record["epoch"] == 2 && record["pending"].is_null() && record["active"].is_object() {
                self.token.cancel();
            }
        }
        Ok(())
    }

    fn delete(&self, key: &SecretKey) -> Result<(), StoreError> {
        self.vault.delete(key)
    }
}

#[test]
fn cancellation_after_activation_preserves_confirmed_receipt_and_defers_cleanup()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let initial = fixture.lock(&grant(1))?;
    let old = initial.replace(
        &initial.read()?.version,
        &secret(b"old-generation")?,
        IncompletePolicy::KeepPrevious,
    )?;
    drop(initial);
    let token = CancellationToken::default();
    let vault = CancelAfterActivation {
        vault: &fixture.vault,
        token: token.clone(),
    };
    let invocation = InvocationControl::new(Arc::new(SystemClock::default()), token, 10_000)?;
    let session = LockedCredentials::new(
        &vault,
        fixture.locks.acquire(&grant(1), &invocation)?,
        invocation,
    );
    let receipt = session.replace(
        &old.version,
        &secret(b"confirmed-generation")?,
        IncompletePolicy::KeepPrevious,
    )?;
    assert!(receipt.cleanup_pending);
    drop(session);
    let restarted = fixture.lock(&grant(1))?;
    let current = restarted.read()?;
    assert_eq!(current.version, receipt.version);
    assert_eq!(
        current.secret.ok_or("Lost confirmed generation")?.expose(),
        b"confirmed-generation"
    );
    assert!(restarted.inspect()?.cleanup_pending);
    assert!(!restarted.recover()?.cleanup_pending);
    Ok(())
}

#[test]
fn large_binary_and_unicode_values_are_exact_and_stale_versions_fail()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let session = fixture.lock(&grant(1))?;
    let empty = session.read()?;
    assert!(empty.secret.is_none());
    let unicode = "日本語🦀\n".repeat(1_000).into_bytes();
    let binary: Vec<u8> = (0..MAX_SECRET_BYTES)
        .map(|index| (index % 251) as u8)
        .collect();
    let first = session.replace(
        &empty.version,
        &secret(&unicode)?,
        IncompletePolicy::KeepPrevious,
    )?;
    assert!(!first.cleanup_pending);
    assert!(
        session
            .read()?
            .secret
            .ok_or("Missing Unicode secret")?
            .expose()
            == unicode
    );
    let second = session.replace(
        &first.version,
        &secret(&binary)?,
        IncompletePolicy::KeepPrevious,
    )?;
    assert!(
        session
            .read()?
            .secret
            .ok_or("Missing binary secret")?
            .expose()
            == binary
    );
    assert_eq!(
        session
            .replace(
                &first.version,
                &secret(b"stale")?,
                IncompletePolicy::KeepPrevious
            )
            .err()
            .ok_or("Expected stale refusal")?
            .code,
        ErrorCode::VersionConflict
    );
    let logout = session.logout(&second.version)?;
    assert!(!logout.cleanup_pending);
    assert!(session.read()?.secret.is_none());
    assert!(fixture.vault.read(&index_key(&grant(1)))?.is_some());
    assert_eq!(
        session
            .replace(
                &empty.version,
                &secret(b"old empty snapshot")?,
                IncompletePolicy::KeepPrevious
            )
            .err()
            .ok_or("Expected logout tombstone refusal")?
            .code,
        ErrorCode::VersionConflict
    );
    Ok(())
}

#[test]
fn incomplete_staging_is_blocked_then_recovered_by_its_recorded_policy()
-> Result<(), Box<dyn std::error::Error>> {
    for policy in [
        IncompletePolicy::KeepPrevious,
        IncompletePolicy::Reauthorize,
    ] {
        let fixture = Fixture::new()?;
        let session = fixture.lock(&grant(1))?;
        let initial = session.read()?.version;
        let receipt = session.replace(
            &initial,
            &secret(b"previous-complete")?,
            IncompletePolicy::KeepPrevious,
        )?;
        fixture
            .faults
            .fail_on(FaultPoint::VaultWriteBeforeCommit, 2)?;
        let error = session
            .replace(&receipt.version, &secret(&vec![b'x'; 5_000])?, policy)
            .err()
            .ok_or("Expected staged failure")?;
        assert_eq!(
            error.details.write_state,
            craft_core::contracts::RequestState::SentUnknown
        );
        assert!(session.inspect()?.recovery_required);
        assert!(session.read().is_err());
        drop(session);
        let restarted = fixture.lock(&grant(1))?;
        let report = restarted.recover()?;
        assert!(report.recovered);
        assert!(!report.cleanup_pending);
        assert_eq!(
            report.reauthorization_required,
            policy == IncompletePolicy::Reauthorize
        );
        let current = restarted.read()?;
        if policy == IncompletePolicy::KeepPrevious {
            assert!(current.secret.ok_or("Previous secret lost")?.expose() == b"previous-complete");
        } else {
            assert!(current.secret.is_none());
        }
    }
    Ok(())
}

#[test]
fn each_write_boundary_recovers_without_mixed_or_truncated_credentials()
-> Result<(), Box<dyn std::error::Error>> {
    let next = vec![0x9b; 6_200];
    // Stage, four chunks, activation, and the old-generation cleanup index commit.
    for point in [
        FaultPoint::VaultWriteBeforeCommit,
        FaultPoint::VaultWriteAfterCommit,
    ] {
        for boundary in 1..=7 {
            let fixture = Fixture::new()?;
            let session = fixture.lock(&grant(1))?;
            let old = session.replace(
                &session.read()?.version,
                &secret(b"complete-old")?,
                IncompletePolicy::KeepPrevious,
            )?;
            fixture.faults.fail_on(point, boundary)?;
            let before = fixture.faults.calls(point)?;
            let _outcome = session.replace(
                &old.version,
                &secret(&next)?,
                IncompletePolicy::KeepPrevious,
            );
            assert!(fixture.faults.calls(point)? - before >= boundary);
            drop(session);
            let restarted = fixture.lock(&grant(1))?;
            restarted.recover()?;
            let bytes = restarted
                .read()?
                .secret
                .ok_or("Lost both complete generations")?;
            assert!(bytes.expose() == b"complete-old" || bytes.expose() == next);
            assert!(!restarted.inspect()?.recovery_required);
            assert!(!restarted.inspect()?.cleanup_pending);
        }
    }
    Ok(())
}

#[test]
fn read_failures_and_cleanup_failures_never_hide_confirmed_activation()
-> Result<(), Box<dyn std::error::Error>> {
    for boundary in 1..=12 {
        let fixture = Fixture::new()?;
        let session = fixture.lock(&grant(1))?;
        let old = session.replace(
            &session.read()?.version,
            &secret(b"old")?,
            IncompletePolicy::KeepPrevious,
        )?;
        fixture.faults.fail_on(FaultPoint::VaultRead, boundary)?;
        let before = fixture.faults.calls(FaultPoint::VaultRead)?;
        let next = vec![0xab; 6_200];
        let _outcome = session.replace(
            &old.version,
            &secret(&next)?,
            IncompletePolicy::KeepPrevious,
        );
        assert!(fixture.faults.calls(FaultPoint::VaultRead)? - before >= boundary);
        session.recover()?;
        let current = session
            .read()?
            .secret
            .ok_or("No complete secret survived")?;
        assert!(current.expose() == b"old" || current.expose() == next);
    }
    for point in [
        FaultPoint::VaultDeleteBeforeCommit,
        FaultPoint::VaultDeleteAfterCommit,
    ] {
        let fixture = Fixture::new()?;
        let session = fixture.lock(&grant(1))?;
        let old = session.replace(
            &session.read()?.version,
            &secret(&vec![0; 5_000])?,
            IncompletePolicy::KeepPrevious,
        )?;
        fixture.faults.fail_on(point, 1)?;
        let receipt = session.replace(
            &old.version,
            &secret(b"new-confirmed")?,
            IncompletePolicy::KeepPrevious,
        )?;
        assert!(
            session
                .read()?
                .secret
                .ok_or("Confirmed activation hidden")?
                .expose()
                == b"new-confirmed"
        );
        if point == FaultPoint::VaultDeleteBeforeCommit {
            assert!(receipt.cleanup_pending);
        }
        session.recover()?;
        assert!(!session.inspect()?.cleanup_pending);
    }
    Ok(())
}

#[test]
fn corruption_and_locked_stores_do_not_trigger_fallback_or_cross_grant_deletion()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let session = fixture.lock(&grant(1))?;
    session.replace(
        &session.read()?.version,
        &secret(&vec![0x8a; 5_000])?,
        IncompletePolicy::KeepPrevious,
    )?;
    let index = fixture
        .vault
        .read(&index_key(&grant(1)))?
        .ok_or("Missing index")?;
    let mut record: serde_json::Value = serde_json::from_slice(index.expose())?;
    let generation = record["active"]["id"]
        .as_str()
        .ok_or("Missing generation")?;
    let key = SecretKey {
        service: "credential-generations-v1".into(),
        account: format!("{}/{generation}/0000", grant(1).id),
    };
    fixture.vault.write(&key, &secret(&vec![0x8b; 2_048])?)?;
    assert!(session.read().is_err());
    record["grant"] = serde_json::json!(grant(2).id);
    fixture.vault.write(
        &index_key(&grant(1)),
        &SecretBytes::new(serde_json::to_vec(&record)?)?,
    )?;
    let deletes = fixture.faults.calls(FaultPoint::VaultDeleteBeforeCommit)?;
    assert!(session.recover().is_err());
    assert_eq!(
        fixture.faults.calls(FaultPoint::VaultDeleteBeforeCommit)?,
        deletes
    );
    fixture.vault.set_mode(VaultMode::Locked)?;
    assert_eq!(
        session
            .read()
            .err()
            .ok_or("Expected locked-store refusal")?
            .code,
        ErrorCode::KeyringUnavailable
    );
    fixture.vault.set_mode(VaultMode::Unavailable)?;
    assert!(session.inspect().is_err());
    Ok(())
}

#[test]
fn logout_tombstone_survives_failed_cleanup_and_blocks_stale_cross_grant_writers()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let first = fixture.lock(&grant(1))?;
    let other = fixture.lock(&grant(2))?;
    let stale = first
        .replace(
            &first.read()?.version,
            &secret(b"old")?,
            IncompletePolicy::KeepPrevious,
        )?
        .version;
    let foreign = other
        .replace(
            &other.read()?.version,
            &secret(b"other")?,
            IncompletePolicy::KeepPrevious,
        )?
        .version;
    fixture
        .faults
        .fail_on(FaultPoint::VaultDeleteBeforeCommit, 1)?;
    let logout = first.logout(&stale)?;
    assert!(logout.cleanup_pending);
    assert!(first.read()?.secret.is_none());
    assert!(
        first
            .replace(
                &stale,
                &secret(b"stale-revival")?,
                IncompletePolicy::KeepPrevious
            )
            .is_err()
    );
    assert!(
        first
            .replace(
                &foreign,
                &secret(b"wrong-grant")?,
                IncompletePolicy::KeepPrevious
            )
            .is_err()
    );
    first.recover()?;
    assert!(!first.inspect()?.cleanup_pending);
    assert!(
        other
            .read()?
            .secret
            .ok_or("Unrelated grant deleted")?
            .expose()
            == b"other"
    );
    Ok(())
}

#[test]
fn malformed_indexes_fail_closed_before_any_write_or_delete()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let session = fixture.lock(&grant(1))?;
    session.replace(
        &session.read()?.version,
        &secret(b"intact")?,
        IncompletePolicy::KeepPrevious,
    )?;
    let original: serde_json::Value = serde_json::from_slice(
        fixture
            .vault
            .read(&index_key(&grant(1)))?
            .ok_or("Missing index")?
            .expose(),
    )?;
    let fields: Vec<_> = original
        .as_object()
        .ok_or("Expected object")?
        .keys()
        .cloned()
        .collect();
    let mut variants = Vec::new();
    for field in fields {
        let mut missing = original.clone();
        missing
            .as_object_mut()
            .ok_or("Expected object")?
            .remove(&field);
        variants.push(missing);
        if field != "pending" {
            let mut null = original.clone();
            null[&field] = serde_json::Value::Null;
            // An explicit inactive pointer is valid, unlike a missing pointer.
            if field != "active" {
                variants.push(null);
            }
        }
    }
    for (field, value) in [
        ("unknown", serde_json::json!(true)),
        ("format", serde_json::json!(2)),
        (
            "installation",
            serde_json::json!("ins_10000000000000000000000000000000"),
        ),
        ("grant", serde_json::json!(grant(2).id)),
        ("revision", serde_json::json!(0)),
        ("epoch", serde_json::json!(-1)),
    ] {
        let mut invalid = original.clone();
        invalid[field] = value;
        variants.push(invalid);
    }
    for (field, value) in [
        ("id", serde_json::json!("gen_../../foreign")),
        ("chunks", serde_json::json!(999)),
        ("length", serde_json::json!(0)),
        ("length", serde_json::json!(MAX_SECRET_BYTES + 1)),
        ("digest", serde_json::json!("sha256:INVALID")),
        ("extra", serde_json::json!(true)),
    ] {
        let mut invalid = original.clone();
        invalid["active"][field] = value;
        variants.push(invalid);
    }
    let mut duplicate = original.clone();
    duplicate["retired"] = serde_json::json!([original["active"].clone()]);
    variants.push(duplicate);
    for variant in variants {
        fixture.vault.write(
            &index_key(&grant(1)),
            &secret(&serde_json::to_vec(&variant)?)?,
        )?;
        let writes = fixture.faults.calls(FaultPoint::VaultWriteBeforeCommit)?;
        let deletes = fixture.faults.calls(FaultPoint::VaultDeleteBeforeCommit)?;
        assert!(session.inspect().is_err());
        assert!(session.read().is_err());
        assert!(session.recover().is_err());
        assert_eq!(
            fixture.faults.calls(FaultPoint::VaultWriteBeforeCommit)?,
            writes
        );
        assert_eq!(
            fixture.faults.calls(FaultPoint::VaultDeleteBeforeCommit)?,
            deletes
        );
    }
    Ok(())
}

#[test]
fn corrupt_pending_chunks_never_activate_and_unavailable_vaults_preserve_intent()
-> Result<(), Box<dyn std::error::Error>> {
    for corrupt_bytes in [vec![0x11; 2_048], vec![0x11; 2_047]] {
        let fixture = Fixture::new()?;
        let session = fixture.lock(&grant(1))?;
        let old = session.replace(
            &session.read()?.version,
            &secret(b"old")?,
            IncompletePolicy::KeepPrevious,
        )?;
        fixture
            .faults
            .fail_on(FaultPoint::VaultWriteBeforeCommit, 5)?;
        assert!(
            session
                .replace(
                    &old.version,
                    &secret(&vec![0x22; 5_000])?,
                    IncompletePolicy::Reauthorize,
                )
                .is_err()
        );
        let record: serde_json::Value = serde_json::from_slice(
            fixture
                .vault
                .read(&index_key(&grant(1)))?
                .ok_or("Missing staged index")?
                .expose(),
        )?;
        let generation = record["pending"]["manifest"]["id"]
            .as_str()
            .ok_or("Missing pending generation")?;
        fixture.vault.write(
            &SecretKey {
                service: "credential-generations-v1".into(),
                account: format!("{}/{generation}/0000", grant(1).id),
            },
            &secret(&corrupt_bytes)?,
        )?;
        let writes = fixture.faults.calls(FaultPoint::VaultWriteBeforeCommit)?;
        let deletes = fixture.faults.calls(FaultPoint::VaultDeleteBeforeCommit)?;
        for mode in [VaultMode::Locked, VaultMode::Unavailable] {
            fixture.vault.set_mode(mode)?;
            assert!(session.recover().is_err());
        }
        assert_eq!(
            fixture.faults.calls(FaultPoint::VaultWriteBeforeCommit)?,
            writes
        );
        assert_eq!(
            fixture.faults.calls(FaultPoint::VaultDeleteBeforeCommit)?,
            deletes
        );
        fixture.vault.set_mode(VaultMode::Available)?;
        assert!(session.inspect()?.recovery_required);
        let report = session.recover()?;
        assert!(report.reauthorization_required);
        assert!(session.read()?.secret.is_none());
        assert!(!session.inspect()?.cleanup_pending);
    }
    Ok(())
}

#[test]
fn every_retired_chunk_delete_is_idempotent_across_lost_acknowledgments()
-> Result<(), Box<dyn std::error::Error>> {
    for point in [
        FaultPoint::VaultDeleteBeforeCommit,
        FaultPoint::VaultDeleteAfterCommit,
    ] {
        for boundary in 1..=4 {
            let fixture = Fixture::new()?;
            let session = fixture.lock(&grant(1))?;
            let previous = session.replace(
                &session.read()?.version,
                &secret(&vec![0xaa; 6_200])?,
                IncompletePolicy::KeepPrevious,
            )?;
            fixture.faults.fail_on(point, boundary)?;
            let before = fixture.faults.calls(point)?;
            let receipt = session.replace(
                &previous.version,
                &secret(b"new")?,
                IncompletePolicy::KeepPrevious,
            )?;
            assert!(fixture.faults.calls(point)? - before >= boundary);
            assert_eq!(
                receipt.cleanup_pending,
                point == FaultPoint::VaultDeleteBeforeCommit
            );
            drop(session);
            let restarted = fixture.lock(&grant(1))?;
            assert_eq!(
                restarted
                    .read()?
                    .secret
                    .ok_or("Missing active secret")?
                    .expose(),
                b"new"
            );
            assert!(!restarted.recover()?.cleanup_pending);
            assert_eq!(restarted.read()?.version, receipt.version);
        }
    }
    Ok(())
}
