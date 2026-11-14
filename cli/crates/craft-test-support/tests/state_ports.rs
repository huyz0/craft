use std::io::Write;
use std::sync::{Arc, Barrier};
use std::time::Duration;

use craft_core::contracts::{GrantRef, JobState, RemoteJob, ResourceRef};
use craft_core::input::content_digest;
use craft_core::storage::{
    ArtifactSpec, ArtifactStore, Clock, CredentialStore, JournalRecord, JournalStep,
    OperationBinding, OperationJournal, SecretBytes, SecretKey, StoreError, SubmissionState,
};
use craft_test_support::state::{
    FaultPoint, Faults, ManualClock, MemoryArtifacts, MemoryJournal, MemoryVault, VaultMode,
};

#[test]
fn fixture_roots_are_distinct_when_concurrent_wall_clock_samples_match() -> Result<(), &'static str>
{
    let roots: Vec<_> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..32)
            .map(|_| {
                scope.spawn(|| {
                    craft_test_support::state::fixture_root(
                        std::path::Path::new("synthetic-parent"),
                        "same-prefix",
                        100,
                    )
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().map_err(|_| "Fixture naming thread panicked."))
            .collect::<Result<Vec<_>, _>>()
    })?;
    assert_eq!(
        roots
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        32
    );
    assert!(
        roots
            .iter()
            .all(|root| root.parent() == Some(std::path::Path::new("synthetic-parent")))
    );
    Ok(())
}

fn target() -> ResourceRef {
    ResourceRef {
        provider: "sample".into(),
        deployment: "cloud".into(),
        tenant: "tenant-one".into(),
        resource_type: "record".into(),
        id: "record-one".into(),
    }
}

fn binding() -> OperationBinding {
    OperationBinding {
        operation_id: "operation-one".into(),
        request_digest: content_digest(b"request"),
        descriptor_digest: content_digest(b"descriptor"),
        target: target(),
        grant: GrantRef {
            id: "grant-one".into(),
        },
        actor: "actor-one".into(),
    }
}

fn next(record: &JournalRecord, state: SubmissionState) -> JournalRecord {
    let mut next = record.clone();
    next.revision += 1;
    let step = next.steps.entry("create".into()).or_default();
    step.state = state;
    step.request_digest = Some(content_digest(b"native-create"));
    next
}

#[test]
fn wall_adjustments_do_not_extend_monotonic_elapsed_time() -> Result<(), Box<dyn std::error::Error>>
{
    let clock = ManualClock::new(1_000);
    clock.advance(Duration::from_millis(60))?;
    let reader = clock.clone();
    clock.set_wall(100)?;
    clock.advance(Duration::from_millis(40))?;
    assert_eq!(reader.unix_millis(), 140);
    assert_eq!(reader.monotonic_millis(), 100);
    let overflow = ManualClock::new(u64::MAX);
    assert_eq!(
        overflow.advance(Duration::from_millis(1)),
        Err(StoreError::TooLarge)
    );
    assert_eq!(overflow.monotonic_millis(), 0);
    Ok(())
}

#[test]
fn vault_failures_distinguish_unchanged_and_committed_secrets()
-> Result<(), Box<dyn std::error::Error>> {
    let faults = Faults::default();
    let vault = MemoryVault::new(faults.clone());
    let key = SecretKey {
        service: "craft-fixture".into(),
        account: "grant-one".into(),
    };
    let secret = SecretBytes::new(b"private-sentinel".to_vec())?;
    assert!(!format!("{secret:?}").contains("private-sentinel"));
    faults.fail_on(FaultPoint::VaultWriteBeforeCommit, 1)?;
    assert_eq!(vault.write(&key, &secret), Err(StoreError::Unavailable));
    assert!(vault.read(&key)?.is_none());
    faults.fail_on(FaultPoint::VaultWriteAfterCommit, 1)?;
    assert_eq!(vault.write(&key, &secret), Err(StoreError::OutcomeUnknown));
    let reopened = vault.clone();
    assert_eq!(
        reopened
            .read(&key)?
            .ok_or("Missing committed credential")?
            .expose(),
        b"private-sentinel"
    );
    reopened.set_mode(VaultMode::Locked)?;
    assert!(matches!(vault.read(&key), Err(StoreError::Locked)));
    assert_eq!(vault.delete(&key), Err(StoreError::Locked));
    reopened.set_mode(VaultMode::Unavailable)?;
    assert_eq!(vault.write(&key, &secret), Err(StoreError::Unavailable));
    reopened.set_mode(VaultMode::Available)?;
    faults.fail_on(FaultPoint::VaultDeleteAfterCommit, 1)?;
    assert_eq!(vault.delete(&key), Err(StoreError::OutcomeUnknown));
    assert!(reopened.read(&key)?.is_none());
    Ok(())
}

#[test]
fn journal_preserves_intent_after_lost_commit_acknowledgments()
-> Result<(), Box<dyn std::error::Error>> {
    let faults = Faults::default();
    let journal = MemoryJournal::new(faults.clone());
    faults.fail_on(FaultPoint::JournalBeginBeforeCommit, 1)?;
    assert_eq!(journal.begin(&binding()), Err(StoreError::Unavailable));
    assert!(journal.get("operation-one")?.is_none());
    faults.fail_on(FaultPoint::JournalBeginAfterCommit, 1)?;
    assert_eq!(journal.begin(&binding()), Err(StoreError::OutcomeUnknown));
    let initial = journal
        .clone()
        .get("operation-one")?
        .ok_or("Missing committed intent")?;
    assert_eq!(journal.begin(&binding())?, initial);
    let prepared = next(&initial, SubmissionState::Prepared);
    journal.compare_and_swap(initial.revision, &prepared)?;
    let sending = next(&prepared, SubmissionState::SendStarted);
    faults.fail_on(FaultPoint::JournalUpdateBeforeCommit, 1)?;
    assert_eq!(
        journal.compare_and_swap(prepared.revision, &sending),
        Err(StoreError::Unavailable)
    );
    assert_eq!(
        journal.get("operation-one")?.ok_or("Missing record")?,
        prepared
    );
    faults.fail_on(FaultPoint::JournalUpdateAfterCommit, 1)?;
    assert_eq!(
        journal.compare_and_swap(prepared.revision, &sending),
        Err(StoreError::OutcomeUnknown)
    );
    let reopened = journal
        .clone()
        .get("operation-one")?
        .ok_or("Missing send marker")?;
    assert!(reopened.steps["create"].state.requires_reconciliation());
    assert_eq!(reopened, sending);
    assert_eq!(
        journal.compare_and_swap(prepared.revision, &sending),
        Err(StoreError::Conflict)
    );
    let mut changed_binding = binding();
    changed_binding.grant.id = "grant-two".into();
    assert_eq!(journal.begin(&changed_binding), Err(StoreError::Conflict));
    Ok(())
}

#[test]
fn journal_compare_and_swap_prevents_lost_concurrent_updates()
-> Result<(), Box<dyn std::error::Error>> {
    let journal = MemoryJournal::new(Faults::default());
    let record = journal.begin(&binding())?;
    let barrier = Arc::new(Barrier::new(3));
    let mut workers = Vec::new();
    for id in ["first", "second"] {
        let worker_journal = journal.clone();
        let ready = Arc::clone(&barrier);
        let mut changed = record.clone();
        changed.revision = 1;
        changed.steps.insert(id.into(), JournalStep::default());
        workers.push(std::thread::spawn(move || {
            ready.wait();
            worker_journal.compare_and_swap(0, &changed)
        }));
    }
    barrier.wait();
    let results: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().map_err(|_| "Journal worker failed"))
        .collect::<Result<_, _>>()?;
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| **result == Err(StoreError::Conflict))
            .count(),
        1
    );
    assert_eq!(
        journal
            .get("operation-one")?
            .ok_or("Missing journal")?
            .steps
            .len(),
        1
    );
    Ok(())
}

#[test]
fn journal_refuses_receipt_loss_terminal_regression_and_job_replacement()
-> Result<(), Box<dyn std::error::Error>> {
    let journal = MemoryJournal::new(Faults::default());
    let initial = journal.begin(&binding())?;
    let prepared = next(&initial, SubmissionState::Prepared);
    journal.compare_and_swap(initial.revision, &prepared)?;
    let sending = next(&prepared, SubmissionState::SendStarted);
    journal.compare_and_swap(prepared.revision, &sending)?;
    let mut confirmed = next(&sending, SubmissionState::Confirmed);
    let step = confirmed.steps.get_mut("create").ok_or("Missing step")?;
    step.confirmed_resources.push(target());
    step.job = Some(RemoteJob {
        resource: target(),
        state: JobState::Succeeded,
    });
    journal.compare_and_swap(sending.revision, &confirmed)?;
    let mut lost = next(&confirmed, SubmissionState::Confirmed);
    lost.steps
        .get_mut("create")
        .ok_or("Missing step")?
        .confirmed_resources
        .clear();
    assert_eq!(
        journal.compare_and_swap(confirmed.revision, &lost),
        Err(StoreError::Conflict)
    );
    assert_eq!(
        journal.compare_and_swap(
            confirmed.revision,
            &next(&confirmed, SubmissionState::Prepared)
        ),
        Err(StoreError::Conflict)
    );
    let mut regressed = next(&confirmed, SubmissionState::Confirmed);
    regressed
        .steps
        .get_mut("create")
        .and_then(|step| step.job.as_mut())
        .ok_or("Missing job")?
        .state = JobState::Running;
    assert_eq!(
        journal.compare_and_swap(confirmed.revision, &regressed),
        Err(StoreError::Conflict)
    );
    assert_eq!(journal.get("operation-one")?, Some(confirmed));
    Ok(())
}

fn artifact() -> ArtifactSpec {
    ArtifactSpec {
        name: "review.md".into(),
        media_type: "text/markdown".into(),
        max_bytes: 128,
        source: Some(target()),
    }
}

#[test]
fn artifact_faults_never_publish_partial_writes_or_replace_committed_content()
-> Result<(), Box<dyn std::error::Error>> {
    let faults = Faults::default();
    let store = MemoryArtifacts::new(faults.clone(), 1_024);
    let mut writer = store.begin(artifact())?;
    writer.write_all(b"partial")?;
    drop(writer);
    assert!(store.receipt("review.md")?.is_none());
    let mut writer = store.begin(artifact())?;
    faults.fail_on(FaultPoint::ArtifactWrite, 1)?;
    assert!(writer.write_all(b"failed").is_err());
    assert_eq!(writer.finish(), Err(StoreError::Unavailable));
    let mut writer = store.begin(artifact())?;
    writer.write_all(b"abc")?;
    faults.fail_on(FaultPoint::ArtifactBeforeCommit, 1)?;
    assert_eq!(writer.finish(), Err(StoreError::Unavailable));
    assert!(store.receipt("review.md")?.is_none());
    let mut writer = store.begin(artifact())?;
    writer.write_all(b"abc")?;
    faults.fail_on(FaultPoint::ArtifactAfterCommit, 1)?;
    assert_eq!(writer.finish(), Err(StoreError::OutcomeUnknown));
    let reopened = store.clone();
    let blob = reopened
        .read("review.md", 128)?
        .ok_or("Missing committed artifact")?;
    assert_eq!(blob.bytes, b"abc");
    assert_eq!(
        blob.receipt.digest,
        "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(blob.receipt.bytes, 3);
    assert_eq!(blob.receipt.source, Some(target()));
    assert!(matches!(
        reopened.begin(artifact()),
        Err(StoreError::Conflict)
    ));
    assert!(matches!(
        reopened.read("review.md", 2),
        Err(StoreError::TooLarge)
    ));
    Ok(())
}

#[test]
fn artifact_limits_and_competing_writers_preserve_the_first_complete_result()
-> Result<(), Box<dyn std::error::Error>> {
    let store = MemoryArtifacts::new(Faults::default(), 1_024);
    let mut invalid = artifact();
    invalid.name = "../review.md".into();
    assert!(matches!(store.begin(invalid), Err(StoreError::Invalid)));
    let mut oversized = store.begin(artifact())?;
    assert!(oversized.write_all(&[b'x'; 129]).is_err());
    assert!(oversized.finish().is_err());
    assert!(store.receipt("review.md")?.is_none());
    let mut first = store.begin(artifact())?;
    let mut second = store.begin(artifact())?;
    first.write_all(b"first")?;
    second.write_all(b"second")?;
    first.finish()?;
    assert_eq!(second.finish(), Err(StoreError::Conflict));
    assert_eq!(
        store
            .read("review.md", 128)?
            .ok_or("Missing artifact")?
            .bytes,
        b"first"
    );
    Ok(())
}
