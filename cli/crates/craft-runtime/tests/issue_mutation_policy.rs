use craft_core::contracts::{AuthMode, ResourceRef};
use craft_core::control::{CancellationToken, InvocationControl};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::profiles::ProfileInput;
use craft_runtime::control::SystemClock;
use craft_runtime::issue_mutation_policy::{
    IssueMutationOperation as Op, IssueMutationPolicyUpdate,
};
use craft_runtime::profiles::ProfileStore;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn comment_allowances_require_explicit_current_format_and_remain_independent() -> TestResult {
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    let created = store.set_issue_mutation_policy(
        "factory",
        update("100", None, true),
        true,
        &control,
        |_| Ok(()),
    )?;
    let raw = files.raw()?;
    let comments = [
        Op::CommentCreate,
        Op::CommentUpdateOwn,
        Op::CommentUpdateAny,
        Op::CommentDeleteOwn,
        Op::CommentDeleteAny,
    ];
    for operation in comments {
        for format in 1..=4 {
            let mut invalid = serde_json::to_value(&created)?;
            invalid["format"] = serde_json::json!(format);
            invalid["operations"] = serde_json::json!([operation]);
            raw.execute(
                "UPDATE issue_mutation_policies SET payload=?1 WHERE project_id='100'",
                [serde_json::to_string(&invalid)?],
            )?;
            assert!(
                store
                    .issue_mutation_policy("factory", "100", &control)
                    .is_err()
            );
        }
    }
    raw.execute(
        "UPDATE issue_mutation_policies SET payload=?1 WHERE project_id='100'",
        [serde_json::to_string(&created)?],
    )?;
    let mut previous = created;
    for operation in comments {
        let configured = store.set_issue_mutation_policy(
            "factory",
            IssueMutationPolicyUpdate {
                operations: vec![operation],
                ..update("100", Some(previous.revision), true)
            },
            true,
            &control,
            |_| Ok(()),
        )?;
        assert_eq!(configured.format, 5);
        store.verify_issue_mutation_policy(&configured, operation, &control)?;
        for other in comments {
            if other != operation {
                assert!(
                    store
                        .verify_issue_mutation_policy(&configured, other, &control)
                        .is_err()
                );
            }
        }
        previous = configured;
    }
    let all = vec![
        Op::Edit,
        Op::Transition,
        Op::Assign,
        Op::Unassign,
        Op::Delete,
        Op::DeleteAllSubtasks,
        Op::Link,
        Op::LinkDelete,
        Op::CommentCreate,
        Op::CommentUpdateOwn,
        Op::CommentUpdateAny,
        Op::CommentDeleteOwn,
        Op::CommentDeleteAny,
    ];
    let configured = store.set_issue_mutation_policy(
        "factory",
        IssueMutationPolicyUpdate {
            operations: all,
            ..update("100", Some(previous.revision), true)
        },
        true,
        &control,
        |_| Ok(()),
    )?;
    assert_eq!(configured.operations.len(), 13);
    Ok(())
}

#[test]
fn legacy_mutation_rows_never_grant_link_authority_and_explicit_updates_use_new_format()
-> TestResult {
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    let created = store.set_issue_mutation_policy(
        "factory",
        update("100", None, true),
        true,
        &control,
        |_| Ok(()),
    )?;
    let operation: Op = serde_json::from_value(serde_json::json!("jira.link.create"))?;
    let raw = files.raw()?;
    for format in [1, 2] {
        let mut invalid = serde_json::to_value(&created)?;
        invalid["format"] = serde_json::json!(format);
        invalid["operations"] = serde_json::json!(["jira.link.create"]);
        raw.execute(
            "UPDATE issue_mutation_policies SET payload=?1 WHERE project_id='100'",
            [serde_json::to_string(&invalid)?],
        )?;
        assert!(
            store
                .issue_mutation_policy("factory", "100", &control)
                .is_err()
        );
    }
    raw.execute(
        "UPDATE issue_mutation_policies SET payload=?1 WHERE project_id='100'",
        [serde_json::to_string(&created)?],
    )?;
    let change = || IssueMutationPolicyUpdate {
        operations: vec![operation],
        ..update("100", Some(1), true)
    };
    assert!(
        store
            .set_issue_mutation_policy("factory", change(), true, &control, |_| Err(
                CraftError::new(ErrorCode::OutputBudgetTooSmall, "No receipt capacity.")
            ))
            .is_err()
    );
    assert_eq!(
        store.issue_mutation_policy("factory", "100", &control)?,
        Some(created.clone())
    );
    let linked =
        store.set_issue_mutation_policy("factory", change(), true, &control, |_| Ok(()))?;
    assert_eq!(linked.format, 3);
    store.verify_issue_mutation_policy(&linked, operation, &control)?;
    assert!(
        store
            .verify_issue_mutation_policy(&linked, Op::Edit, &control)
            .is_err()
    );
    assert!(
        store
            .verify_issue_mutation_policy(&created, Op::Edit, &control)
            .is_err()
    );
    Ok(())
}

#[test]
fn legacy_rows_never_gain_action_authority_and_only_explicit_updates_upgrade_format() -> TestResult
{
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    let created = store.set_issue_mutation_policy(
        "factory",
        update("100", None, true),
        true,
        &control,
        |_| Ok(()),
    )?;
    assert_eq!(created.format, 2);
    let mut legacy = serde_json::to_value(&created)?;
    legacy["format"] = serde_json::json!(1);
    let payload = serde_json::to_string(&legacy)?;
    let raw = files.raw()?;
    raw.execute(
        "UPDATE issue_mutation_policies SET payload=?1 WHERE project_id='100'",
        [&payload],
    )?;
    let old = store
        .issue_mutation_policy("factory", "100", &control)?
        .ok_or("legacy")?;
    assert_eq!(old.format, 1);
    store.verify_issue_mutation_policy(&old, Op::Edit, &control)?;
    assert_eq!(
        raw.query_row(
            "SELECT payload FROM issue_mutation_policies WHERE project_id='100'",
            [],
            |r| r.get::<_, String>(0)
        )?,
        payload
    );
    for name in [
        "jira.issue.assign",
        "jira.issue.unassign",
        "jira.issue.delete",
        "jira.issue.delete-all-subtasks",
    ] {
        let operation: Op = serde_json::from_value(serde_json::json!(name))?;
        assert!(
            store
                .verify_issue_mutation_policy(&old, operation, &control)
                .is_err()
        );
        let mut bad = legacy.clone();
        bad["operations"] = serde_json::json!([name]);
        let bad = serde_json::to_string(&bad)?;
        raw.execute(
            "UPDATE issue_mutation_policies SET payload=?1 WHERE project_id='100'",
            [&bad],
        )?;
        assert!(
            store
                .issue_mutation_policy("factory", "100", &control)
                .is_err()
        );
        assert_eq!(
            raw.query_row(
                "SELECT payload FROM issue_mutation_policies WHERE project_id='100'",
                [],
                |r| r.get::<_, String>(0)
            )?,
            bad
        );
        raw.execute(
            "UPDATE issue_mutation_policies SET payload=?1 WHERE project_id='100'",
            [&payload],
        )?;
    }
    let operation: Op = serde_json::from_value(serde_json::json!("jira.issue.assign"))?;
    let make_update = || IssueMutationPolicyUpdate {
        operations: vec![operation],
        ..update("100", Some(1), true)
    };
    assert!(
        store
            .set_issue_mutation_policy("factory", make_update(), true, &control, |_| Err(
                CraftError::new(ErrorCode::OutputBudgetTooSmall, "No receipt capacity.")
            ))
            .is_err()
    );
    assert_eq!(
        store.issue_mutation_policy("factory", "100", &control)?,
        Some(old.clone())
    );
    let upgraded =
        store.set_issue_mutation_policy("factory", make_update(), true, &control, |_| Ok(()))?;
    assert_eq!(upgraded.format, 2);
    assert_eq!(upgraded.revision, 2);
    store.verify_issue_mutation_policy(&upgraded, operation, &control)?;
    assert!(
        store
            .verify_issue_mutation_policy(&old, Op::Edit, &control)
            .is_err()
    );
    assert!(
        store
            .set_issue_mutation_policy("factory", make_update(), true, &control, |_| Ok(()))
            .is_err()
    );
    assert_eq!(
        raw.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
        13
    );
    Ok(())
}

#[test]
fn expanded_policy_allows_only_the_exact_explicit_operation_and_canonical_set() -> TestResult {
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    let names = [
        "jira.issue.update",
        "jira.transition.apply",
        "jira.issue.assign",
        "jira.issue.unassign",
        "jira.issue.delete",
        "jira.issue.delete-all-subtasks",
    ];
    let operations: Vec<Op> = names
        .iter()
        .map(|name| serde_json::from_value(serde_json::json!(name)))
        .collect::<Result<_, _>>()?;
    for (index, operation) in operations.iter().enumerate() {
        let project = (100 + index).to_string();
        let record = store.set_issue_mutation_policy(
            "factory",
            IssueMutationPolicyUpdate {
                operations: vec![*operation],
                ..update(&project, None, true)
            },
            true,
            &control,
            |_| Ok(()),
        )?;
        for requested in &operations {
            assert_eq!(
                store
                    .verify_issue_mutation_policy(&record, *requested, &control)
                    .is_ok(),
                requested == operation
            );
        }
    }
    let all = store.set_issue_mutation_policy(
        "factory",
        IssueMutationPolicyUpdate {
            operations: operations.iter().rev().copied().collect(),
            ..update("200", None, true)
        },
        true,
        &control,
        |_| Ok(()),
    )?;
    assert_eq!(all.operations, operations);
    for operation in operations {
        assert!(
            store
                .set_issue_mutation_policy(
                    "factory",
                    IssueMutationPolicyUpdate {
                        operations: vec![operation, operation],
                        ..update("300", None, true)
                    },
                    true,
                    &control,
                    |_| Ok(())
                )
                .is_err()
        );
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
fn update(id: &str, revision: Option<u64>, enabled: bool) -> IssueMutationPolicyUpdate {
    IssueMutationPolicyUpdate {
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
            root: craft_test_support::state::fixture_root(&parent, "issue-mutation-policy", stamp),
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
fn mutation_policy_requires_explicit_apply_preflight_revision_and_project_operations() -> TestResult
{
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    assert!(
        store
            .issue_mutation_policy("factory", "100", &control)?
            .is_none()
    );
    assert!(
        store
            .set_issue_mutation_policy(
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
            .set_issue_mutation_policy("factory", update("100", None, true), true, &control, |_| {
                Err(CraftError::new(
                    ErrorCode::OutputBudgetTooSmall,
                    "No receipt capacity.",
                ))
            })
            .is_err()
    );
    assert!(
        store
            .issue_mutation_policy("factory", "100", &control)?
            .is_none()
    );
    let first = store.set_issue_mutation_policy(
        "factory",
        update("100", None, true),
        true,
        &control,
        |_| Ok(()),
    )?;
    assert_eq!(first.operations, vec![Op::Edit, Op::Transition]);
    store.verify_issue_mutation_policy(&first, Op::Edit, &control)?;
    let second = store.set_issue_mutation_policy(
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
    store.set_issue_mutation_policy("factory", edit, true, &control, |_| Ok(()))?;
    assert_eq!(
        store
            .verify_issue_mutation_policy(&first, Op::Transition, &control)
            .err()
            .ok_or("refusal")?
            .code,
        ErrorCode::PolicyDenied
    );
    assert_eq!(
        store
            .verify_issue_mutation_policy(&first, Op::Edit, &control)
            .err()
            .ok_or("refusal")?
            .code,
        ErrorCode::VersionConflict
    );
    store.verify_issue_mutation_policy(&second, Op::Transition, &control)?;
    assert!(
        store
            .set_issue_mutation_policy(
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
        reopened.issue_mutation_policy("factory", "200", &control)?,
        Some(second)
    );
    Ok(())
}
#[test]
fn enabled_actor_transfer_duplicate_operations_and_foreign_rows_are_refused() -> TestResult {
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    let first = store.set_issue_mutation_policy(
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
            .set_issue_mutation_policy("factory", transfer, true, &control, |_| Ok(()))
            .is_err()
    );
    store.set_issue_mutation_policy(
        "factory",
        update("100", Some(1), false),
        true,
        &control,
        |_| Ok(()),
    )?;
    let mut transfer = update("100", Some(2), true);
    transfer.expected_actor = "other-actor".into();
    let current =
        store.set_issue_mutation_policy("factory", transfer, true, &control, |_| Ok(()))?;
    assert_ne!(first.expected_actor, current.expected_actor);
    for ops in [vec![], vec![Op::Edit, Op::Edit]] {
        let mut bad = update("200", None, true);
        bad.operations = ops;
        assert!(
            store
                .set_issue_mutation_policy("factory", bad, true, &control, |_| Ok(()))
                .is_err()
        );
    }
    let raw = files.raw()?;
    let mut foreign = serde_json::to_value(&current)?;
    foreign["grant"]["id"] = serde_json::json!("gr_11111111111111111111111111111111");
    raw.execute("UPDATE issue_mutation_policies SET payload=?1 WHERE profile='factory' AND project_id='100'",[serde_json::to_string(&foreign)?])?;
    assert!(
        store
            .issue_mutation_policy("factory", "100", &control)
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
    drop(store);
    let raw = files.raw()?;
    raw.execute_batch("DROP TABLE IF EXISTS confluence_admin_policies; DROP TABLE IF EXISTS review_policies; DROP TABLE confluence_write_policies; DROP TABLE confluence_read_policies; DROP TABLE issue_recovery_policies; DROP TABLE issue_mutation_policies; PRAGMA user_version=7;")?;
    drop(raw);
    let migrated = ProfileStore::open(&files.root, &control()?)?;
    assert_eq!(migrated.get("factory", &control()?)?, profile);
    assert_eq!(migrated.settings(), &settings);
    assert!(
        migrated
            .issue_mutation_policy("factory", "100", &control()?)?
            .is_none()
    );
    drop(migrated);
    let raw = files.raw()?;
    assert_eq!(
        raw.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?,
        13
    );
    raw.execute_batch("DROP TABLE IF EXISTS confluence_admin_policies; DROP TABLE IF EXISTS review_policies; DROP TABLE confluence_write_policies; DROP TABLE confluence_read_policies; DROP TABLE issue_recovery_policies; DROP TABLE issue_mutation_policies; CREATE TABLE issue_mutation_policies (foreign_data TEXT); PRAGMA user_version=7;")?;
    assert!(ProfileStore::open(&files.root, &control()?).is_err());
    assert_eq!(
        raw.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?,
        7
    );
    Ok(())
}

#[test]
fn malformed_or_foreign_policy_payloads_and_schema_triggers_fail_without_repair() -> TestResult {
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    let original = store.set_issue_mutation_policy(
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
        raw.execute("UPDATE issue_mutation_policies SET payload=?1", [&payload])?;
        assert!(
            store
                .issue_mutation_policy("factory", "100", &control)
                .is_err()
        );
        assert_eq!(
            raw.query_row("SELECT payload FROM issue_mutation_policies", [], |row| row
                .get::<_, String>(0))?,
            payload
        );
    }
    raw.execute(
        "UPDATE issue_mutation_policies SET payload=?1",
        [serde_json::to_string(&original)?],
    )?;
    raw.execute_batch("CREATE TRIGGER mutation_policy_trigger AFTER INSERT ON issue_mutation_policies BEGIN SELECT 1; END;")?;
    assert!(ProfileStore::open(&files.root, &control).is_err());
    Ok(())
}

#[test]
fn competing_revision_writers_cannot_overwrite_a_winning_project_policy() -> TestResult {
    let files = Files::new()?;
    let store = files.setup()?;
    store.set_issue_mutation_policy(
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
                    let store = ProfileStore::open(&root, &control)?;
                    barrier.wait();
                    store.set_issue_mutation_policy(
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
            .issue_mutation_policy("factory", "100", &control()?)?
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
    let first = store.set_issue_mutation_policy(
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
        transaction.execute("INSERT INTO issue_mutation_policies(profile,project_id,revision,payload) VALUES('factory',?1,1,?2)",rusqlite::params![record.target.id,serde_json::to_string(&record)?])?;
    }
    transaction.commit()?;
    assert!(
        store
            .set_issue_mutation_policy(
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
            .set_issue_mutation_policy(
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

#[test]
fn attachment_upload_requires_new_explicit_format_and_all_mutation_allowances_coexist() -> TestResult
{
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    let old = store.set_issue_mutation_policy(
        "factory",
        update("100", None, true),
        true,
        &control,
        |_| Ok(()),
    )?;
    assert!(
        store
            .verify_issue_mutation_policy(&old, Op::AttachmentUpload, &control)
            .is_err()
    );
    let raw = files.raw()?;
    for format in 1..=5 {
        let mut forged = serde_json::to_value(&old)?;
        forged["format"] = serde_json::json!(format);
        forged["operations"] = serde_json::json!([Op::AttachmentUpload]);
        raw.execute("UPDATE issue_mutation_policies SET payload=?1 WHERE profile='factory' AND project_id='100'",[serde_json::to_string(&forged)?])?;
        assert!(
            store
                .issue_mutation_policy("factory", "100", &control)
                .is_err()
        );
    }
    raw.execute("UPDATE issue_mutation_policies SET payload=?1 WHERE profile='factory' AND project_id='100'",[serde_json::to_string(&old)?])?;
    let mut next = update("100", Some(old.revision), true);
    next.operations = vec![
        Op::Edit,
        Op::Transition,
        Op::Assign,
        Op::Unassign,
        Op::Delete,
        Op::DeleteAllSubtasks,
        Op::Link,
        Op::LinkDelete,
        Op::CommentCreate,
        Op::CommentUpdateOwn,
        Op::CommentUpdateAny,
        Op::CommentDeleteOwn,
        Op::CommentDeleteAny,
        Op::AttachmentUpload,
    ];
    let current = store.set_issue_mutation_policy("factory", next, true, &control, |_| Ok(()))?;
    assert_eq!(current.format, 6);
    assert_eq!(current.operations.len(), 14);
    for op in current.operations.iter().copied() {
        store.verify_issue_mutation_policy(&current, op, &control)?;
    }
    Ok(())
}
