use craft_core::contracts::{AuthMode, ResourceRef};
use craft_core::control::{CancellationToken, InvocationControl};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::profiles::ProfileInput;
use craft_runtime::confluence_read_policy::{
    ConfluenceReadOperation as Op, ConfluenceReadPolicyUpdate,
};
use craft_runtime::control::SystemClock;
use craft_runtime::profiles::ProfileStore;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
type TestResult = Result<(), Box<dyn std::error::Error>>;
fn control() -> Result<InvocationControl, CraftError> {
    InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        15_000,
    )
}
fn target() -> ResourceRef {
    let cloud = "00000000-1111-2222-3333-444444444444";
    ResourceRef {
        provider: "confluence".into(),
        deployment: "cloud".into(),
        tenant: cloud.into(),
        resource_type: "site".into(),
        id: cloud.into(),
    }
}
fn update(revision: Option<u64>, enabled: bool) -> ConfluenceReadPolicyUpdate {
    ConfluenceReadPolicyUpdate {
        target: target(),
        expected_actor: "actor-one".into(),
        operations: vec![
            Op::PageSearch,
            Op::SpaceList,
            Op::PageGet,
            Op::SpaceGet,
            Op::PageList,
        ],
        enabled,
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
            "confluence-read-policy",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos(),
        );
        Ok(Self { root, parent })
    }
    fn store(&self) -> Result<ProfileStore, Box<dyn std::error::Error>> {
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
fn confluence_authority_requires_apply_preflight_revision_and_exact_operations() -> TestResult {
    let files = Files::new()?;
    let store = files.store()?;
    let control = control()?;
    assert!(store.confluence_read_policy("factory", &control)?.is_none());
    assert!(
        store
            .set_confluence_read_policy("factory", update(None, true), false, &control, |_| Ok(()))
            .is_err()
    );
    assert!(
        store
            .set_confluence_read_policy("factory", update(None, true), true, &control, |_| Err(
                CraftError::new(ErrorCode::OutputBudgetTooSmall, "No receipt capacity.")
            ))
            .is_err()
    );
    assert!(store.confluence_read_policy("factory", &control)?.is_none());
    let first =
        store.set_confluence_read_policy("factory", update(None, true), true, &control, |_| {
            Ok(())
        })?;
    assert_eq!(
        first.operations,
        vec![
            Op::SpaceGet,
            Op::SpaceList,
            Op::PageGet,
            Op::PageList,
            Op::PageSearch
        ]
    );
    for op in &first.operations {
        store.verify_confluence_read_policy(&first, *op, &control)?;
        assert_eq!(serde_json::to_value(op)?, json!(op.command()));
    }
    let mut next = update(Some(1), true);
    next.operations = vec![Op::PageGet];
    let second = store.set_confluence_read_policy("factory", next, true, &control, |_| Ok(()))?;
    assert_eq!(
        store
            .verify_confluence_read_policy(&first, Op::PageList, &control)
            .err()
            .ok_or("denied")?
            .code,
        ErrorCode::PolicyDenied
    );
    assert_eq!(
        store
            .verify_confluence_read_policy(&first, Op::PageGet, &control)
            .err()
            .ok_or("changed")?
            .code,
        ErrorCode::VersionConflict
    );
    assert!(
        store
            .set_confluence_read_policy("factory", update(Some(1), false), true, &control, |_| Ok(
                ()
            ))
            .is_err()
    );
    assert_eq!(
        files.store()?.confluence_read_policy("factory", &control)?,
        Some(second)
    );
    Ok(())
}
#[test]
fn confluence_actor_transfer_and_corrupt_foreign_records_fail_without_repair() -> TestResult {
    let files = Files::new()?;
    let store = files.store()?;
    let control = control()?;
    let first =
        store.set_confluence_read_policy("factory", update(None, true), true, &control, |_| {
            Ok(())
        })?;
    let mut transfer = update(Some(1), false);
    transfer.expected_actor = "actor-two".into();
    assert!(
        store
            .set_confluence_read_policy("factory", transfer, true, &control, |_| Ok(()))
            .is_err()
    );
    store.set_confluence_read_policy("factory", update(Some(1), false), true, &control, |_| {
        Ok(())
    })?;
    let mut transfer = update(Some(2), true);
    transfer.expected_actor = "actor-two".into();
    let current =
        store.set_confluence_read_policy("factory", transfer, true, &control, |_| Ok(()))?;
    assert_ne!(first.expected_actor, current.expected_actor);
    let raw = files.raw()?;
    for patch in [
        json!({"format":2}),
        json!({"installation":"foreign"}),
        json!({"profile":"other"}),
        json!({"revision":1}),
        json!({"expected_actor":"anonymous"}),
        json!({"grant":{"id":"gr_11111111111111111111111111111111"}}),
        json!({"operations":["jira.issue.get"]}),
        json!({"operations":["confluence.page.get","confluence.page.get"]}),
        json!({"operations":["confluence.page.list","confluence.page.get"]}),
    ] {
        let mut value = serde_json::to_value(&current)?;
        for (k, v) in patch.as_object().ok_or("patch")? {
            value[k] = v.clone();
        }
        let payload = serde_json::to_string(&value)?;
        raw.execute("UPDATE confluence_read_policies SET payload=?1", [&payload])?;
        assert!(store.confluence_read_policy("factory", &control).is_err());
        assert_eq!(
            raw.query_row("SELECT payload FROM confluence_read_policies", [], |r| {
                r.get::<_, String>(0)
            })?,
            payload
        );
    }
    Ok(())
}
#[test]
fn confluence_configuration_is_independent_of_jira_even_under_a_shared_grant() -> TestResult {
    use craft_runtime::issue_read_policy::{IssueReadOperation, IssueReadPolicyUpdate};
    let files = Files::new()?;
    let store = files.store()?;
    let control = control()?;
    let profile = store.get("factory", &control)?;
    store.create(
        &ProfileInput {
            name: "jira".into(),
            provider: "jira".into(),
            deployment: "cloud".into(),
            auth_mode: AuthMode::Service,
            grant: Some(profile.grant),
            tenant: Some(target().tenant),
            defaults: BTreeMap::new(),
        },
        &control,
        |_| Ok(()),
    )?;
    let jira = store.set_issue_read_policy(
        "jira",
        IssueReadPolicyUpdate {
            target: ResourceRef {
                provider: "jira".into(),
                ..target()
            },
            expected_actor: "actor-one".into(),
            enabled: true,
            operations: vec![IssueReadOperation::Get],
            expected_revision: None,
        },
        true,
        &control,
        |_| Ok(()),
    )?;
    assert!(store.confluence_read_policy("factory", &control)?.is_none());
    assert!(
        store
            .set_confluence_read_policy("jira", update(None, true), true, &control, |_| Ok(()))
            .is_err()
    );
    store.set_confluence_read_policy("factory", update(None, true), true, &control, |_| Ok(()))?;
    assert_eq!(store.issue_read_policy("jira", &control)?, Some(jira));
    assert!(store.issue_read_policy("factory", &control)?.is_none());
    for operations in [vec![], vec![Op::PageGet, Op::PageGet]] {
        let mut bad = update(Some(1), true);
        bad.operations = operations;
        assert!(
            store
                .set_confluence_read_policy("factory", bad, true, &control, |_| Ok(()))
                .is_err()
        );
    }
    Ok(())
}
#[test]
fn confluence_policy_migration_preserves_profiles_and_rolls_back_conflicts_and_triggers()
-> TestResult {
    let files = Files::new()?;
    let store = files.store()?;
    let control = control()?;
    let settings = store.settings().clone();
    let profile = store.get("factory", &control)?;
    drop(store);
    let raw = files.raw()?;
    raw.execute_batch("DROP TABLE IF EXISTS review_policies; DROP TABLE confluence_write_policies; DROP TABLE confluence_read_policies; PRAGMA user_version=9;")?;
    let migrated = files.store()?;
    assert_eq!(migrated.settings(), &settings);
    assert_eq!(migrated.get("factory", &control)?, profile);
    assert!(
        migrated
            .confluence_read_policy("factory", &control)?
            .is_none()
    );
    drop(migrated);
    assert_eq!(
        raw.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
        12
    );
    raw.execute_batch("DROP TABLE IF EXISTS review_policies; DROP TABLE confluence_write_policies; DROP TABLE confluence_read_policies; CREATE TABLE confluence_read_policies(foreign_data TEXT); PRAGMA user_version=9;")?;
    assert!(ProfileStore::open(&files.root, &control).is_err());
    assert_eq!(
        raw.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
        9
    );
    raw.execute_batch("DROP TABLE confluence_read_policies;")?;
    let migrated = files.store()?;
    drop(migrated);
    raw.execute_batch("CREATE TRIGGER confluence_policy_trigger AFTER INSERT ON confluence_read_policies BEGIN SELECT 1; END;")?;
    assert!(ProfileStore::open(&files.root, &control).is_err());
    Ok(())
}
#[test]
fn competing_confluence_policy_writers_preserve_one_revision_winner() -> TestResult {
    let files = Files::new()?;
    let store = files.store()?;
    store
        .set_confluence_read_policy("factory", update(None, true), true, &control()?, |_| Ok(()))?;
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let results = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let root = files.root.clone();
                let barrier = barrier.clone();
                scope.spawn(move || -> Result<_, CraftError> {
                    let control = control()?;
                    let store = ProfileStore::open(&root, &control)?;
                    barrier.wait();
                    store.set_confluence_read_policy(
                        "factory",
                        update(Some(1), false),
                        true,
                        &control,
                        |_| Ok(()),
                    )
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().map_err(|_| "writer panic"))
            .collect::<Result<Vec<_>, _>>()
    })?;
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert!(
        results
            .iter()
            .filter_map(|r| r.as_ref().err())
            .all(|e| e.code == ErrorCode::VersionConflict)
    );
    Ok(())
}
