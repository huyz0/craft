use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use craft_core::contracts::{AuthMode, ResourceRef};
use craft_core::control::{CancellationToken, InvocationControl};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::profiles::{Profile, ProfileInput};
use craft_core::storage::{OperationBinding, OperationJournal};
use craft_runtime::control::SystemClock;
use craft_runtime::create_policy::{CreatePolicyRecord, CreatePolicyUpdate};
use craft_runtime::journal::NativeJournal;
use craft_runtime::profiles::ProfileStore;
use serde_json::json;

#[test]
fn competing_policy_writers_keep_one_revision_winner() -> TestResult {
    let files = Files::new()?;
    let (store, _) = files.setup()?;
    let control = control()?;
    store.set_create_policy("factory", update(None, true), true, &control, |_| Ok(()))?;
    let stores = [
        ProfileStore::open(&files.root, &control)?,
        ProfileStore::open(&files.root, &control)?,
    ];
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let results = std::thread::scope(|scope| {
        let mut workers = Vec::new();
        for (index, store) in stores.into_iter().enumerate() {
            let barrier = barrier.clone();
            let control = control.clone();
            workers.push(scope.spawn(move || {
                barrier.wait();
                store.set_create_policy(
                    "factory",
                    update(Some(1), index == 0),
                    true,
                    &control,
                    |_| Ok(()),
                )
            }));
        }
        workers
            .into_iter()
            .map(|worker| worker.join().map_err(|_| "Policy writer panicked"))
            .collect::<Result<Vec<_>, _>>()
    })
    .map_err(|error| error.to_string())?;
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter_map(|result| result.as_ref().err())
            .map(|error| error.code)
            .collect::<Vec<_>>(),
        vec![ErrorCode::VersionConflict]
    );
    let winner = results
        .into_iter()
        .find_map(Result::ok)
        .ok_or("Missing policy winner")?;
    assert_eq!(winner.revision, 2);
    assert_eq!(store.create_policy("factory", &control)?, Some(winner));
    Ok(())
}

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
        provider: "jira".into(),
        deployment: "cloud".into(),
        tenant: "00000000-1111-2222-3333-444444444444".into(),
        resource_type: "project".into(),
        id: "10000".into(),
    }
}
fn update(revision: Option<u64>, enabled: bool) -> CreatePolicyUpdate {
    CreatePolicyUpdate {
        target: target(),
        expected_actor: "actor-one".into(),
        enabled,
        expected_revision: revision,
    }
}
struct Files {
    parent: PathBuf,
    root: PathBuf,
}
impl Files {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        fs::create_dir_all(&parent)?;
        let parent = parent.canonicalize()?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        Ok(Self {
            root: parent.join(format!("create-policy-{}-{stamp}", std::process::id())),
            parent,
        })
    }
    fn setup(&self) -> Result<(ProfileStore, Profile), Box<dyn std::error::Error>> {
        let control = control()?;
        let store = ProfileStore::open(&self.root, &control)?;
        let profile = store.create(
            &ProfileInput {
                name: "factory".into(),
                provider: "jira".into(),
                deployment: "cloud".into(),
                auth_mode: AuthMode::Service,
                grant: None,
                tenant: Some(target().tenant),
                defaults: BTreeMap::from([("project".into(), target())]),
            },
            &control,
            |_| Ok(()),
        )?;
        Ok((store, profile))
    }
    fn raw(&self) -> Result<rusqlite::Connection, rusqlite::Error> {
        let connection = rusqlite::Connection::open(self.root.join("state.sqlite3"))?;
        connection.execute_batch("PRAGMA journal_mode=PERSIST;")?;
        Ok(connection)
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        if let Ok(root) = self.root.canonicalize()
            && root.starts_with(&self.parent)
            && root != self.parent
        {
            let _ = fs::remove_dir_all(root);
        }
    }
}

#[test]
fn policy_apply_preflight_revision_and_revocation_survive_reopen() -> TestResult {
    let files = Files::new()?;
    let (store, _) = files.setup()?;
    let control = control()?;
    assert!(store.create_policy("factory", &control)?.is_none());
    assert_eq!(
        store
            .set_create_policy("factory", update(None, true), false, &control, |_| Ok(()))
            .err()
            .ok_or("Expected apply refusal")?
            .code,
        ErrorCode::ApplyRequired
    );
    assert_eq!(
        store
            .set_create_policy("factory", update(None, true), true, &control, |_| Err(
                CraftError::new(ErrorCode::OutputBudgetTooSmall, "Fixture preflight.")
            ))
            .err()
            .ok_or("Expected preflight refusal")?
            .code,
        ErrorCode::OutputBudgetTooSmall
    );
    assert!(store.create_policy("factory", &control)?.is_none());
    let first =
        store.set_create_policy("factory", update(None, true), true, &control, |record| {
            assert_eq!(record.revision, 1);
            Ok(())
        })?;
    assert_eq!(
        ProfileStore::open(&files.root, &control)?.create_policy("factory", &control)?,
        Some(first.clone())
    );
    assert_eq!(
        store
            .set_create_policy("factory", update(None, true), true, &control, |_| Ok(()))
            .err()
            .ok_or("Expected stale revision")?
            .code,
        ErrorCode::VersionConflict
    );
    let disabled = store.set_create_policy(
        "factory",
        update(Some(1), false),
        true,
        &control,
        |_| Ok(()),
    )?;
    assert_eq!(disabled.revision, 2);
    assert_eq!(
        store
            .verify_create_policy(&first, &control)
            .err()
            .ok_or("Expected revoked policy")?
            .code,
        ErrorCode::PolicyDenied
    );
    let changed =
        store.set_create_policy("factory", update(Some(2), true), true, &control, |_| Ok(()))?;
    assert_eq!(
        store
            .verify_create_policy(&first, &control)
            .err()
            .ok_or("Expected changed policy")?
            .code,
        ErrorCode::VersionConflict
    );
    store.verify_create_policy(&changed, &control)?;
    Ok(())
}

#[test]
fn wrong_policy_target_actor_or_delegated_profile_cannot_be_enabled() -> TestResult {
    let files = Files::new()?;
    let (store, _) = files.setup()?;
    let control = control()?;
    for mode in ["tenant", "kind", "id", "actor"] {
        let mut input = update(None, true);
        match mode {
            "tenant" => input.target.tenant = "11111111-1111-2222-3333-444444444444".into(),
            "kind" => input.target.resource_type = "issue".into(),
            "id" => input.target.id = "PROJECT".into(),
            _ => input.expected_actor = "anonymous".into(),
        }
        assert!(
            store
                .set_create_policy("factory", input, true, &control, |_| Ok(()))
                .is_err()
        );
    }
    store.create(
        &ProfileInput {
            name: "personal".into(),
            provider: "jira".into(),
            deployment: "cloud".into(),
            auth_mode: AuthMode::Delegated,
            grant: None,
            tenant: Some(target().tenant),
            defaults: BTreeMap::new(),
        },
        &control,
        |_| Ok(()),
    )?;
    assert!(
        store
            .set_create_policy("personal", update(None, true), true, &control, |_| Ok(()))
            .is_err()
    );
    assert!(store.create_policy("factory", &control)?.is_none());
    Ok(())
}

#[test]
fn corrupt_policy_identity_revision_duplicate_and_unknown_data_are_refused() -> TestResult {
    let files = Files::new()?;
    let (store, _) = files.setup()?;
    let control = control()?;
    let record =
        store.set_create_policy("factory", update(None, true), true, &control, |_| Ok(()))?;
    let original = serde_json::to_value(&record)?;
    for mode in [
        "installation",
        "grant",
        "actor",
        "revision",
        "unknown",
        "duplicate",
    ] {
        let mut bad = original.clone();
        match mode {
            "installation" => bad["installation"] = json!("ins_11111111111111111111111111111111"),
            "grant" => bad["grant"]["id"] = json!("gr_11111111111111111111111111111111"),
            "actor" => bad["expected_actor"] = json!("unknown"),
            "revision" => bad["revision"] = json!(2),
            "unknown" => bad["credential"] = json!("private-policy-sentinel"),
            _ => {}
        }
        let mut text = serde_json::to_string(&bad)?;
        if mode == "duplicate" {
            text = text.replace("\"enabled\":true", "\"enabled\":true,\"enabled\":false");
        }
        files.raw()?.execute(
            "UPDATE create_policies SET payload=?1 WHERE profile='factory'",
            [text],
        )?;
        let error = store
            .create_policy("factory", &control)
            .err()
            .ok_or("Expected corrupt policy refusal")?;
        assert_eq!(error.code, ErrorCode::InputInvalid);
        assert!(!serde_json::to_string(&error)?.contains("private-policy-sentinel"));
        files.raw()?.execute(
            "UPDATE create_policies SET payload=?1 WHERE profile='factory'",
            [serde_json::to_string(&record)?],
        )?;
    }
    let parsed: CreatePolicyRecord = serde_json::from_value::<CreatePolicyRecord>(original)?;
    assert_eq!(parsed, record);
    Ok(())
}

#[test]
fn policy_schema_migration_preserves_journal_and_rolls_back_foreign_table_conflicts() -> TestResult
{
    let files = Files::new()?;
    let (store, profile) = files.setup()?;
    let control = control()?;
    let settings = store.settings().clone();
    store.use_profile("factory", &control, |_| Ok(()))?;
    let journal = NativeJournal::open(&store, &control)?;
    let binding = OperationBinding {
        operation_id: "create-report".into(),
        request_digest: format!("sha256:{}", "0".repeat(64)),
        descriptor_digest: format!("sha256:{}", "1".repeat(64)),
        target: target(),
        grant: profile.grant.clone(),
        actor: "actor-one".into(),
    };
    let saved = journal.begin(&binding)?;
    drop(journal);
    drop(store);
    files.raw()?.execute_batch(
        "DROP TABLE IF EXISTS review_policies; DROP TABLE confluence_write_policies; DROP TABLE confluence_read_policies; DROP TABLE issue_recovery_policies; DROP TABLE issue_mutation_policies; DROP TABLE field_read_policies; DROP TABLE issue_read_policies; DROP TABLE project_read_policies; DROP TABLE provider_cursors; DROP TABLE create_policies; PRAGMA user_version=2;",
    )?;
    let migrated = ProfileStore::open(&files.root, &control)?;
    assert_eq!(migrated.settings(), &settings);
    assert_eq!(migrated.get("factory", &control)?, profile);
    assert_eq!(migrated.active(&control)?.as_deref(), Some("factory"));
    assert!(migrated.create_policy("factory", &control)?.is_none());
    assert_eq!(
        NativeJournal::open(&migrated, &control)?.get("create-report")?,
        Some(saved)
    );
    drop(migrated);
    assert_eq!(
        files
            .raw()?
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?,
        12
    );
    files.raw()?.execute_batch("DROP TABLE IF EXISTS review_policies; DROP TABLE confluence_write_policies; DROP TABLE confluence_read_policies; DROP TABLE issue_recovery_policies; DROP TABLE issue_mutation_policies; DROP TABLE field_read_policies; DROP TABLE issue_read_policies; DROP TABLE project_read_policies; DROP TABLE provider_cursors; DROP TABLE create_policies; DROP TABLE operation_records; CREATE TABLE create_policies(foreign_data TEXT); PRAGMA user_version=1;")?;
    assert!(ProfileStore::open(&files.root, &control).is_err());
    let raw = files.raw()?;
    assert_eq!(
        raw.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?,
        1
    );
    assert_eq!(
        raw.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name='operation_records'",
            [],
            |row| row.get::<_, i64>(0)
        )?,
        0
    );
    Ok(())
}
