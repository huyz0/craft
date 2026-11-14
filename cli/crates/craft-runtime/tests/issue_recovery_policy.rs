use craft_core::contracts::{AuthMode, ResourceRef};
use craft_core::control::{CancellationToken, InvocationControl};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::profiles::ProfileInput;
use craft_runtime::control::SystemClock;
use craft_runtime::issue_recovery_policy::{
    IssueRecoveryOperation as Op, IssueRecoveryPolicyUpdate,
};
use craft_runtime::profiles::ProfileStore;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn legacy_recovery_payloads_cannot_gain_link_investigation_without_explicit_upgrade() -> TestResult
{
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    let original = store.set_issue_recovery_policy(
        "factory",
        update("100", None, true),
        true,
        &control,
        |_| Ok(()),
    )?;
    assert_eq!(original.format, 1);
    let raw = files.raw()?;
    for op in [Op::LinkCreate, Op::LinkDelete] {
        let mut forged = serde_json::to_value(&original)?;
        forged["operations"] = serde_json::json!([op]);
        raw.execute("UPDATE issue_recovery_policies SET payload=?1 WHERE profile='factory' AND project_id='100'", [serde_json::to_string(&forged)?])?;
        assert!(
            store
                .issue_recovery_policy("factory", "100", &control)
                .is_err()
        );
    }
    raw.execute("UPDATE issue_recovery_policies SET payload=?1 WHERE profile='factory' AND project_id='100'", [serde_json::to_string(&original)?])?;
    let linked = store.set_issue_recovery_policy(
        "factory",
        IssueRecoveryPolicyUpdate {
            operations: vec![Op::LinkCreate, Op::LinkDelete],
            ..update("100", Some(1), true)
        },
        true,
        &control,
        |_| Ok(()),
    )?;
    assert_eq!(linked.format, 2);
    for op in [Op::LinkCreate, Op::LinkDelete] {
        store.verify_issue_recovery_policy(&linked, op, &control)?;
    }
    assert!(
        store
            .verify_issue_recovery_policy(&linked, Op::Create, &control)
            .is_err()
    );
    assert!(
        store
            .verify_issue_recovery_policy(&original, Op::Edit, &control)
            .is_err()
    );
    Ok(())
}

#[test]
fn recovery_and_write_authority_are_independent_and_each_original_command_is_explicit() -> TestResult
{
    use craft_runtime::create_policy::CreatePolicyUpdate;
    use craft_runtime::issue_mutation_policy::{IssueMutationOperation, IssueMutationPolicyUpdate};
    use craft_runtime::issue_read_policy::{IssueReadOperation, IssueReadPolicyUpdate};
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    let create = store.set_create_policy(
        "factory",
        CreatePolicyUpdate {
            target: target("100"),
            expected_actor: "actor-one".into(),
            enabled: true,
            expected_revision: None,
        },
        true,
        &control,
        |_| Ok(()),
    )?;
    let mutation = store.set_issue_mutation_policy(
        "factory",
        IssueMutationPolicyUpdate {
            target: target("100"),
            expected_actor: "actor-one".into(),
            enabled: true,
            expected_revision: None,
            operations: vec![IssueMutationOperation::Edit],
        },
        true,
        &control,
        |_| Ok(()),
    )?;
    let site = ResourceRef {
        resource_type: "site".into(),
        id: target("100").tenant,
        ..target("100")
    };
    let read = store.set_issue_read_policy(
        "factory",
        IssueReadPolicyUpdate {
            target: site,
            expected_actor: "actor-one".into(),
            enabled: true,
            expected_revision: None,
            operations: vec![IssueReadOperation::Get],
        },
        true,
        &control,
        |_| Ok(()),
    )?;
    assert!(
        store
            .issue_recovery_policy("factory", "100", &control)?
            .is_none()
    );
    let operations = [
        Op::Create,
        Op::Edit,
        Op::Transition,
        Op::Assign,
        Op::Unassign,
        Op::Delete,
    ];
    for (index, op) in operations.iter().enumerate() {
        let project = (100 + index).to_string();
        let record = store.set_issue_recovery_policy(
            "factory",
            IssueRecoveryPolicyUpdate {
                operations: vec![*op],
                ..update(&project, None, true)
            },
            true,
            &control,
            |_| Ok(()),
        )?;
        assert_eq!(record.format, 1);
        assert_eq!(serde_json::to_value(op)?, serde_json::json!(op.command()));
        for requested in operations {
            assert_eq!(
                store
                    .verify_issue_recovery_policy(&record, requested, &control)
                    .is_ok(),
                requested == *op
            );
        }
    }
    assert_eq!(store.create_policy("factory", &control)?, Some(create));
    assert_eq!(
        store.issue_mutation_policy("factory", "100", &control)?,
        Some(mutation)
    );
    assert_eq!(store.issue_read_policy("factory", &control)?, Some(read));
    assert!(
        store
            .issue_mutation_policy("factory", "101", &control)?
            .is_none()
    );
    let all = store.set_issue_recovery_policy(
        "factory",
        IssueRecoveryPolicyUpdate {
            operations: operations.iter().rev().copied().collect(),
            ..update("200", None, true)
        },
        true,
        &control,
        |_| Ok(()),
    )?;
    assert_eq!(all.operations, operations);
    for name in [
        "jira.issue.delete-all-subtasks",
        "jira.operation.inspect",
        "jira.issue.get",
        "operation.reconcile",
    ] {
        assert!(serde_json::from_value::<Op>(serde_json::json!(name)).is_err());
    }
    Ok(())
}

fn control() -> Result<InvocationControl, CraftError> {
    InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        15_000,
    )
}
fn target(id: &str) -> ResourceRef {
    ResourceRef {
        provider: "jira".into(),
        deployment: "cloud".into(),
        tenant: "00000000-1111-2222-3333-444444444444".into(),
        resource_type: "project".into(),
        id: id.into(),
    }
}
fn update(id: &str, revision: Option<u64>, enabled: bool) -> IssueRecoveryPolicyUpdate {
    IssueRecoveryPolicyUpdate {
        target: target(id),
        expected_actor: "actor-one".into(),
        operations: vec![Op::Transition, Op::Edit],
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
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        Ok(Self {
            root: craft_test_support::state::fixture_root(&parent, "issue-recovery-policy", stamp),
            parent,
        })
    }
    fn setup(&self) -> Result<ProfileStore, Box<dyn std::error::Error>> {
        let control = control()?;
        let store = ProfileStore::open(&self.root, &control)?;
        store.create(
            &ProfileInput {
                name: "factory".into(),
                provider: "jira".into(),
                deployment: "cloud".into(),
                auth_mode: AuthMode::Service,
                grant: None,
                tenant: Some(target("100").tenant),
                defaults: BTreeMap::new(),
            },
            &control,
            |_| Ok(()),
        )?;
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
fn recovery_policy_requires_explicit_apply_preflight_revision_and_project_operations() -> TestResult
{
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    assert!(
        store
            .issue_recovery_policy("factory", "100", &control)?
            .is_none()
    );
    assert!(
        store
            .set_issue_recovery_policy(
                "factory",
                update("100", None, true),
                false,
                &control,
                |_| Ok(())
            )
            .is_err()
    );
    assert!(
        store
            .set_issue_recovery_policy("factory", update("100", None, true), true, &control, |_| {
                Err(CraftError::new(
                    ErrorCode::OutputBudgetTooSmall,
                    "No receipt capacity.",
                ))
            })
            .is_err()
    );
    assert!(
        store
            .issue_recovery_policy("factory", "100", &control)?
            .is_none()
    );
    let first = store.set_issue_recovery_policy(
        "factory",
        update("100", None, true),
        true,
        &control,
        |_| Ok(()),
    )?;
    assert_eq!(first.operations, vec![Op::Edit, Op::Transition]);
    store.verify_issue_recovery_policy(&first, Op::Edit, &control)?;
    let second = store.set_issue_recovery_policy(
        "factory",
        update("200", None, true),
        true,
        &control,
        |_| Ok(()),
    )?;
    assert_eq!(second.revision, 1);
    assert_eq!(first.revision, 1);
    let mut edit = update("100", Some(1), true);
    edit.operations = vec![Op::Edit];
    store.set_issue_recovery_policy("factory", edit, true, &control, |_| Ok(()))?;
    assert_eq!(
        store
            .verify_issue_recovery_policy(&first, Op::Transition, &control)
            .err()
            .ok_or("refusal")?
            .code,
        ErrorCode::PolicyDenied
    );
    assert_eq!(
        store
            .verify_issue_recovery_policy(&first, Op::Edit, &control)
            .err()
            .ok_or("refusal")?
            .code,
        ErrorCode::VersionConflict
    );
    store.verify_issue_recovery_policy(&second, Op::Transition, &control)?;
    assert!(
        store
            .set_issue_recovery_policy(
                "factory",
                update("100", Some(1), false),
                true,
                &control,
                |_| Ok(())
            )
            .is_err()
    );
    drop(store);
    let reopened = ProfileStore::open(&files.root, &control)?;
    assert_eq!(
        reopened.issue_recovery_policy("factory", "200", &control)?,
        Some(second)
    );
    Ok(())
}
#[test]
fn enabled_actor_transfer_duplicate_operations_and_foreign_rows_are_refused() -> TestResult {
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    let first = store.set_issue_recovery_policy(
        "factory",
        update("100", None, true),
        true,
        &control,
        |_| Ok(()),
    )?;
    let mut transfer = update("100", Some(1), false);
    transfer.expected_actor = "other-actor".into();
    assert!(
        store
            .set_issue_recovery_policy("factory", transfer, true, &control, |_| Ok(()))
            .is_err()
    );
    store.set_issue_recovery_policy(
        "factory",
        update("100", Some(1), false),
        true,
        &control,
        |_| Ok(()),
    )?;
    let mut transfer = update("100", Some(2), true);
    transfer.expected_actor = "other-actor".into();
    let current =
        store.set_issue_recovery_policy("factory", transfer, true, &control, |_| Ok(()))?;
    assert_ne!(first.expected_actor, current.expected_actor);
    for ops in [vec![], vec![Op::Edit, Op::Edit]] {
        let mut bad = update("200", None, true);
        bad.operations = ops;
        assert!(
            store
                .set_issue_recovery_policy("factory", bad, true, &control, |_| Ok(()))
                .is_err()
        );
    }
    let raw = files.raw()?;
    let mut foreign = serde_json::to_value(&current)?;
    foreign["grant"]["id"] = serde_json::json!("gr_11111111111111111111111111111111");
    raw.execute("UPDATE issue_recovery_policies SET payload=?1 WHERE profile='factory' AND project_id='100'",[serde_json::to_string(&foreign)?])?;
    assert!(
        store
            .issue_recovery_policy("factory", "100", &control)
            .is_err()
    );
    Ok(())
}
#[test]
fn empty_schema_migration_preserves_existing_state_and_conflicts_roll_back() -> TestResult {
    let files = Files::new()?;
    let store = files.setup()?;
    let profile = store.get("factory", &control()?)?;
    let settings = store.settings().clone();
    let mutation = store.set_issue_mutation_policy(
        "factory",
        craft_runtime::issue_mutation_policy::IssueMutationPolicyUpdate {
            target: target("100"),
            expected_actor: "actor-one".into(),
            enabled: true,
            expected_revision: None,
            operations: vec![craft_runtime::issue_mutation_policy::IssueMutationOperation::Edit],
        },
        true,
        &control()?,
        |_| Ok(()),
    )?;
    drop(store);
    let raw = files.raw()?;
    raw.execute_batch("DROP TABLE IF EXISTS review_policies; DROP TABLE confluence_write_policies; DROP TABLE confluence_read_policies; DROP TABLE issue_recovery_policies; PRAGMA user_version=8;")?;
    drop(raw);
    let migrated = ProfileStore::open(&files.root, &control()?)?;
    assert_eq!(migrated.get("factory", &control()?)?, profile);
    assert_eq!(migrated.settings(), &settings);
    assert_eq!(
        migrated.issue_mutation_policy("factory", "100", &control()?)?,
        Some(mutation)
    );
    assert!(
        migrated
            .issue_recovery_policy("factory", "100", &control()?)?
            .is_none()
    );
    drop(migrated);
    let raw = files.raw()?;
    assert_eq!(
        raw.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?,
        12
    );
    raw.execute_batch("DROP TABLE IF EXISTS review_policies; DROP TABLE confluence_write_policies; DROP TABLE confluence_read_policies; DROP TABLE issue_recovery_policies; CREATE TABLE issue_recovery_policies (foreign_data TEXT); PRAGMA user_version=8;")?;
    assert!(ProfileStore::open(&files.root, &control()?).is_err());
    assert_eq!(
        raw.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?,
        8
    );
    Ok(())
}

#[test]
fn malformed_or_foreign_policy_payloads_and_schema_triggers_fail_without_repair() -> TestResult {
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    let original = store.set_issue_recovery_policy(
        "factory",
        update("100", None, true),
        true,
        &control,
        |_| Ok(()),
    )?;
    let raw = files.raw()?;
    for patch in [
        serde_json::json!({"format":255}),
        serde_json::json!({"installation":"foreign"}),
        serde_json::json!({"revision":2}),
        serde_json::json!({"profile":"other"}),
        serde_json::json!({"expected_actor":"anonymous"}),
        serde_json::json!({"operations":["jira.issue.unrecognized"]}),
        serde_json::json!({"operations":["jira.issue.update","jira.issue.update"]}),
        serde_json::json!({"operations":["jira.transition.apply","jira.issue.update"]}),
    ] {
        let mut value = serde_json::to_value(&original)?;
        for (key, value_patch) in patch.as_object().ok_or("patch")? {
            value[key] = value_patch.clone();
        }
        let payload = serde_json::to_string(&value)?;
        raw.execute("UPDATE issue_recovery_policies SET payload=?1", [&payload])?;
        assert!(
            store
                .issue_recovery_policy("factory", "100", &control)
                .is_err()
        );
        assert_eq!(
            raw.query_row("SELECT payload FROM issue_recovery_policies", [], |row| row
                .get::<_, String>(0))?,
            payload
        );
    }
    raw.execute(
        "UPDATE issue_recovery_policies SET payload=?1",
        [serde_json::to_string(&original)?],
    )?;
    raw.execute_batch("CREATE TRIGGER recovery_policy_trigger AFTER INSERT ON issue_recovery_policies BEGIN SELECT 1; END;")?;
    assert!(ProfileStore::open(&files.root, &control).is_err());
    Ok(())
}

#[test]
fn competing_revision_writers_cannot_overwrite_a_winning_project_policy() -> TestResult {
    let files = Files::new()?;
    let store = files.setup()?;
    store.set_issue_recovery_policy(
        "factory",
        update("100", None, true),
        true,
        &control()?,
        |_| Ok(()),
    )?;
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let results = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let root = files.root.clone();
                let barrier = barrier.clone();
                scope.spawn(move || -> Result<_, CraftError> {
                    let control = control()?;
                    // Reach the barrier even when opening fails, so the other writer cannot wait forever.
                    let opened = ProfileStore::open(&root, &control);
                    barrier.wait();
                    let store = opened?;
                    store.set_issue_recovery_policy(
                        "factory",
                        update("100", Some(1), false),
                        true,
                        &control,
                        |_| Ok(()),
                    )
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().map_err(|_| "Policy writer panicked"))
            .collect::<Result<Vec<_>, _>>()
    })?;
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert!(
        results
            .iter()
            .filter_map(|result| result.as_ref().err())
            .all(|error| error.code == ErrorCode::VersionConflict)
    );
    assert_eq!(
        store
            .issue_recovery_policy("factory", "100", &control()?)?
            .ok_or("policy")?
            .revision,
        2
    );
    Ok(())
}

#[test]
fn per_profile_project_limit_allows_existing_rows_to_be_disabled() -> TestResult {
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    let first = store.set_issue_recovery_policy(
        "factory",
        update("100", None, true),
        true,
        &control,
        |_| Ok(()),
    )?;
    let mut raw = files.raw()?;
    let transaction = raw.transaction()?;
    for id in 1000..1255 {
        let mut record = first.clone();
        record.target.id = id.to_string();
        transaction.execute("INSERT INTO issue_recovery_policies(profile,project_id,revision,payload) VALUES('factory',?1,1,?2)",rusqlite::params![record.target.id,serde_json::to_string(&record)?])?;
    }
    transaction.commit()?;
    assert!(
        store
            .set_issue_recovery_policy(
                "factory",
                update("9999", None, true),
                true,
                &control,
                |_| Ok(())
            )
            .is_err()
    );
    assert!(
        !store
            .set_issue_recovery_policy(
                "factory",
                update("100", Some(1), false),
                true,
                &control,
                |_| Ok(())
            )?
            .enabled
    );
    Ok(())
}
