use craft_core::contracts::{AuthMode, ResourceRef};
use craft_core::control::{CancellationToken, InvocationControl};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::profiles::ProfileInput;
use craft_runtime::control::SystemClock;
use craft_runtime::profiles::ProfileStore;
use craft_runtime::review_policy::{ReviewPolicyUpdate, validate_eligibility};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};
type TestResult = Result<(), Box<dyn std::error::Error>>;
fn control() -> Result<InvocationControl, CraftError> {
    InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        15_000,
    )
}
fn target() -> ResourceRef {
    ResourceRef {
        provider: "confluence".into(),
        deployment: "cloud".into(),
        tenant: "00000000-1111-2222-3333-444444444444".into(),
        resource_type: "space".into(),
        id: "100".into(),
    }
}
fn update(revision: Option<u64>, enabled: bool) -> ReviewPolicyUpdate {
    ReviewPolicyUpdate {
        target: target(),
        expected_actor: "worker-agent".into(),
        enabled,
        reviewers: vec!["bob".into(), "alice".into()],
        excluded_actors: vec!["other-agent".into()],
        quorum: 2,
        expected_revision: revision,
    }
}
struct Files {
    root: PathBuf,
    parent: PathBuf,
}
impl Files {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        std::fs::create_dir_all(&parent)?;
        let parent = parent.canonicalize()?;
        let root = craft_test_support::state::fixture_root(
            &parent,
            "reviewer-policy",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos(),
        );
        Ok(Self { root, parent })
    }
    fn store(&self) -> Result<ProfileStore, CraftError> {
        let initialized = self.root.exists();
        let store = ProfileStore::open(&self.root, &control()?)?;
        if !initialized {
            store.create(
                &ProfileInput {
                    name: "factory".into(),
                    provider: "confluence".into(),
                    deployment: "cloud".into(),
                    auth_mode: AuthMode::Service,
                    grant: None,
                    tenant: Some(target().tenant),
                    defaults: BTreeMap::new(),
                },
                &control()?,
                |_| Ok(()),
            )?;
        }
        Ok(store)
    }
    fn raw(&self) -> Result<rusqlite::Connection, rusqlite::Error> {
        let raw = rusqlite::Connection::open(self.root.join("state.sqlite3"))?;
        raw.execute_batch("PRAGMA journal_mode=PERSIST;")?;
        Ok(raw)
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        if let Ok(root) = self.root.canonicalize()
            && root.starts_with(&self.parent)
            && root != self.parent
        {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

#[test]
fn reviewer_policy_requires_apply_complete_preflight_cas_and_disabled_transfer() -> TestResult {
    let files = Files::new()?;
    let store = files.store()?;
    let c = control()?;
    assert!(store.review_policy("factory", "100", &c)?.is_none());
    assert_eq!(
        store
            .set_review_policy("factory", update(None, true), false, &c, |_| Ok(()))
            .err()
            .ok_or("apply")?
            .code,
        ErrorCode::ApplyRequired
    );
    assert!(
        store
            .set_review_policy("factory", update(None, true), true, &c, |_| Err(
                CraftError::new(ErrorCode::OutputBudgetTooSmall, "No receipt capacity.")
            ))
            .is_err()
    );
    assert!(store.review_policy("factory", "100", &c)?.is_none());
    let first = store.set_review_policy("factory", update(None, true), true, &c, |_| Ok(()))?;
    assert_eq!(first.reviewers, vec!["alice", "bob"]);
    assert_eq!(first.revision, 1);
    assert_eq!(first.grant, store.get("factory", &c)?.grant);
    store.verify_review_policy(&first, &c)?;
    assert_eq!(
        files.store()?.review_policy("factory", "100", &c)?,
        Some(first.clone())
    );
    assert_eq!(
        store
            .set_review_policy("factory", update(None, true), true, &c, |_| Ok(()))
            .err()
            .ok_or("cas")?
            .code,
        ErrorCode::VersionConflict
    );
    let mut transfer = update(Some(1), true);
    transfer.expected_actor = "new-worker".into();
    assert_eq!(
        store
            .set_review_policy("factory", transfer, true, &c, |_| Ok(()))
            .err()
            .ok_or("transfer")?
            .code,
        ErrorCode::PolicyDenied
    );
    let disabled =
        store.set_review_policy("factory", update(Some(1), false), true, &c, |_| Ok(()))?;
    assert_eq!(
        store
            .verify_review_policy(&disabled, &c)
            .err()
            .ok_or("disabled")?
            .code,
        ErrorCode::PolicyDenied
    );
    let mut transfer = update(Some(2), true);
    transfer.expected_actor = "new-worker".into();
    let changed = store.set_review_policy("factory", transfer, true, &c, |_| Ok(()))?;
    assert_eq!(
        store
            .verify_review_policy(&first, &c)
            .err()
            .ok_or("changed")?
            .code,
        ErrorCode::VersionConflict
    );
    assert_eq!(changed.revision, 3);
    Ok(())
}
#[test]
fn self_approval_duplicates_quorum_and_foreign_targets_never_create_eligibility() -> TestResult {
    let files = Files::new()?;
    let store = files.store()?;
    let c = control()?;
    for mode in [
        "self",
        "duplicate",
        "overlap",
        "unknown",
        "quorum",
        "excluded_duplicate",
        "foreign",
    ] {
        let mut u = update(None, true);
        match mode {
            "self" => u.reviewers[0] = "worker-agent".into(),
            "duplicate" => u.reviewers[0] = "alice".into(),
            "overlap" => u.excluded_actors = vec!["alice".into()],
            "unknown" => u.reviewers[0] = "unknown".into(),
            "quorum" => u.quorum = 3,
            "excluded_duplicate" => {
                u.excluded_actors = vec!["other-agent".into(), "other-agent".into()]
            }
            _ => u.target.tenant = "00000000-1111-2222-3333-555555555555".into(),
        }
        assert!(
            store
                .set_review_policy("factory", u, true, &c, |_| Ok(()))
                .is_err(),
            "{mode}"
        );
        assert!(store.review_policy("factory", "100", &c)?.is_none());
    }
    let reviewers: Vec<_> = (0..64)
        .map(|i| format!("r{i:02}{}", "a".repeat(509)))
        .collect();
    let excluded: Vec<_> = (0..64)
        .map(|i| format!("x{i:02}{}", "b".repeat(509)))
        .collect();
    assert!(validate_eligibility("worker-agent", &reviewers, &excluded, 1).is_err());
    Ok(())
}
#[test]
fn reviewer_schema_migration_preserves_existing_configuration_and_refuses_conflicts() -> TestResult
{
    let files = Files::new()?;
    let store = files.store()?;
    let c = control()?;
    let profile = store.get("factory", &c)?;
    let settings = store.settings().clone();
    let write = store.set_confluence_write_policy(
        "factory",
        craft_runtime::confluence_write_policy::ConfluenceWritePolicyUpdate {
            target: target(),
            expected_actor: "worker-agent".into(),
            enabled: true,
            operations: vec![
                craft_runtime::confluence_write_policy::ConfluenceWriteOperation::Create,
            ],
            expected_revision: None,
        },
        true,
        &c,
        |_| Ok(()),
    )?;
    drop(store);
    let raw = files.raw()?;
    raw.execute_batch("DROP TABLE IF EXISTS confluence_admin_policies; DROP TABLE review_policies; PRAGMA user_version=11;")?;
    let migrated = files.store()?;
    assert_eq!(migrated.settings(), &settings);
    assert_eq!(migrated.get("factory", &c)?, profile);
    assert_eq!(
        migrated.confluence_write_policy("factory", "100", &c)?,
        Some(write)
    );
    assert!(migrated.review_policy("factory", "100", &c)?.is_none());
    drop(migrated);
    assert_eq!(
        raw.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
        13
    );
    raw.execute_batch("DROP TABLE IF EXISTS confluence_admin_policies; DROP TABLE review_policies; CREATE TABLE review_policies(foreign_data TEXT); INSERT INTO review_policies VALUES('retain-owned-fixture'); PRAGMA user_version=11;")?;
    assert!(files.store().is_err());
    assert_eq!(
        raw.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
        11
    );
    assert_eq!(
        raw.query_row("SELECT foreign_data FROM review_policies", [], |r| r
            .get::<_, String>(0))?,
        "retain-owned-fixture"
    );
    raw.execute_batch(
        "DROP TABLE IF EXISTS confluence_admin_policies; DROP TABLE review_policies;",
    )?;
    drop(files.store()?);
    raw.execute_batch(
        "CREATE TRIGGER review_trigger AFTER INSERT ON review_policies BEGIN SELECT 1; END;",
    )?;
    assert!(files.store().is_err());
    Ok(())
}
#[test]
fn tampered_foreign_or_unknown_reviewer_records_are_not_repaired_or_echoed() -> TestResult {
    for key in ["installation", "grant", "reviewers", "extra"] {
        let files = Files::new()?;
        let store = files.store()?;
        let c = control()?;
        let record =
            store.set_review_policy("factory", update(None, true), true, &c, |_| Ok(()))?;
        drop(store);
        let mut payload = serde_json::to_value(&record)?;
        match key {
            "installation" => payload["installation"] = serde_json::json!("private-sentinel"),
            "grant" => {
                payload["grant"]["id"] = serde_json::json!("gr_11111111111111111111111111111111")
            }
            "reviewers" => payload["reviewers"] = serde_json::json!(["worker-agent"]),
            _ => payload["private-sentinel"] = serde_json::json!(true),
        }
        files.raw()?.execute(
            "UPDATE review_policies SET payload=?1 WHERE profile='factory'",
            [serde_json::to_string(&payload)?],
        )?;
        let reopened = files.store()?;
        let error = reopened
            .review_policy("factory", "100", &c)
            .err()
            .ok_or("tampered record")?;
        assert!(!serde_json::to_string(&error)?.contains("private-sentinel"));
    }
    Ok(())
}
#[test]
fn concurrent_reviewer_updates_have_one_revision_winner() -> TestResult {
    let files = Files::new()?;
    let store = files.store()?;
    store.set_review_policy("factory", update(None, true), true, &control()?, |_| Ok(()))?;
    drop(store);
    let barrier = Arc::new(Barrier::new(3));
    let mut handles = Vec::new();
    for _ in 0..2 {
        let root = files.root.clone();
        let barrier = barrier.clone();
        handles.push(std::thread::spawn(move || -> Result<(), CraftError> {
            let opened =
                control().and_then(|c| ProfileStore::open(&root, &c).map(|store| (c, store)));
            barrier.wait();
            let (c, store) = opened?;
            store
                .set_review_policy("factory", update(Some(1), true), true, &c, |_| Ok(()))
                .map(|_| ())
        }));
    }
    barrier.wait();
    let mut wins = 0;
    let mut conflicts = 0;
    for h in handles {
        match h.join().map_err(|_| "review policy worker panic")? {
            Ok(()) => wins += 1,
            Err(e) if e.code == ErrorCode::VersionConflict => conflicts += 1,
            Err(e) => return Err(e.into()),
        }
    }
    assert_eq!((wins, conflicts), (1, 1));
    assert_eq!(
        files
            .store()?
            .review_policy("factory", "100", &control()?)?
            .ok_or("record")?
            .revision,
        2
    );
    Ok(())
}
