use craft_core::contracts::{AuthMode, ResourceRef};
use craft_core::control::{CancellationToken, InvocationControl};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::profiles::ProfileInput;
use craft_runtime::control::SystemClock;
use craft_runtime::profiles::ProfileStore;
use craft_runtime::project_read_policy::{
    ProjectContextOperation as Op, ProjectContextPermissionUpdate, ProjectReadPolicyUpdate,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
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
        provider: "jira".into(),
        deployment: "cloud".into(),
        tenant: cloud.into(),
        resource_type: "site".into(),
        id: cloud.into(),
    }
}
fn update(revision: Option<u64>, enabled: bool) -> ProjectReadPolicyUpdate {
    ProjectReadPolicyUpdate {
        target: target(),
        expected_actor: "actor-one".into(),
        continuation_site: "https://craft-test.atlassian.net".into(),
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
            root: parent.join(format!(
                "project-context-permission-{}-{stamp}",
                std::process::id()
            )),
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
                tenant: Some(target().tenant),
                defaults: BTreeMap::new(),
            },
            &control,
            |_| Ok(()),
        )?;
        Ok(store)
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

fn project(id: &str) -> ResourceRef {
    ResourceRef {
        resource_type: "project".into(),
        id: id.into(),
        ..target()
    }
}
fn permission(
    id: &str,
    revision: u64,
    enabled: bool,
    operations: Vec<Op>,
) -> ProjectContextPermissionUpdate {
    ProjectContextPermissionUpdate {
        target: project(id),
        operations,
        enabled,
        expected_revision: revision,
    }
}

#[test]
fn legacy_records_gain_context_permission_only_after_applied_preflighted_revision_update()
-> TestResult {
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    assert!(
        store
            .set_project_context_permission(
                "factory",
                permission("10000", 1, true, vec![Op::Get]),
                true,
                &control,
                |_| Ok(())
            )
            .is_err()
    );
    let old = store.set_project_read_policy(
        "factory",
        update(None, false),
        true,
        &control,
        |_| Ok(()),
    )?;
    assert_eq!(old.format, 1);
    assert!(old.context_reads.is_none());
    assert!(
        store
            .verify_project_context_policy(&old, &project("10000"), Op::Get, &control)
            .is_err()
    );
    assert_eq!(
        store
            .set_project_context_permission(
                "factory",
                permission("10000", 1, true, vec![Op::Get]),
                false,
                &control,
                |_| Ok(())
            )
            .err()
            .ok_or("Expected apply refusal")?
            .code,
        ErrorCode::ApplyRequired
    );
    assert!(
        store
            .set_project_context_permission(
                "factory",
                permission("10000", 1, true, vec![Op::Get]),
                true,
                &control,
                |_| Err(CraftError::new(
                    ErrorCode::OutputBudgetTooSmall,
                    "Cannot emit permission receipt."
                ))
            )
            .is_err()
    );
    assert_eq!(store.project_read_policy("factory", &control)?, Some(old));
    let new = store.set_project_context_permission(
        "factory",
        permission("10000", 1, true, vec![Op::Get]),
        true,
        &control,
        |_| Ok(()),
    )?;
    assert_eq!(new.format, 2);
    assert!(!new.enabled);
    assert_eq!(new.revision, 2);
    store.verify_project_context_policy(&new, &project("10000"), Op::Get, &control)?;
    assert!(
        store
            .verify_project_context_policy(&new, &project("10000"), Op::Statuses, &control)
            .is_err()
    );
    assert!(store.verify_project_read_policy(&new, &control).is_err());
    assert_eq!(
        ProfileStore::open(&files.root, &control)?.project_read_policy("factory", &control)?,
        Some(new)
    );
    assert_eq!(
        files
            .raw()?
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?,
        13
    );
    Ok(())
}

#[test]
fn listing_edits_preserve_permissions_and_enabled_permissions_cannot_transfer_actor() -> TestResult
{
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    let old =
        store.set_project_read_policy("factory", update(None, true), true, &control, |_| Ok(()))?;
    let first = store.set_project_context_permission(
        "factory",
        permission("ED", 1, true, vec![Op::Statuses, Op::Get]),
        true,
        &control,
        |_| Ok(()),
    )?;
    assert_eq!(
        store
            .verify_project_read_policy(&old, &control)
            .err()
            .ok_or("Expected revision refusal")?
            .code,
        ErrorCode::VersionConflict
    );
    let mut other_actor = update(Some(2), true);
    other_actor.expected_actor = "actor-two".into();
    assert_eq!(
        store
            .set_project_read_policy("factory", other_actor, true, &control, |_| Ok(()))
            .err()
            .ok_or("Expected actor transfer refusal")?
            .code,
        ErrorCode::PolicyDenied
    );
    assert_eq!(
        store.project_read_policy("factory", &control)?,
        Some(first.clone())
    );
    let listing_disabled =
        store.set_project_read_policy("factory", update(Some(2), false), true, &control, |_| {
            Ok(())
        })?;
    assert_eq!(listing_disabled.context_reads, first.context_reads);
    store.verify_project_context_policy(
        &listing_disabled,
        &project("ED"),
        Op::Statuses,
        &control,
    )?;
    let disabled = store.set_project_context_permission(
        "factory",
        permission("ED", 3, false, vec![Op::Get, Op::Statuses]),
        true,
        &control,
        |_| Ok(()),
    )?;
    assert_eq!(
        store
            .verify_project_context_policy(&listing_disabled, &project("ED"), Op::Get, &control)
            .err()
            .ok_or("Expected revocation")?
            .code,
        ErrorCode::PolicyDenied
    );
    let mut other_actor = update(Some(disabled.revision), false);
    other_actor.expected_actor = "actor-two".into();
    let changed =
        store.set_project_read_policy("factory", other_actor, true, &control, |_| Ok(()))?;
    assert_eq!(changed.expected_actor, "actor-two");
    assert_eq!(changed.context_reads, disabled.context_reads);
    assert!(
        store
            .verify_project_context_policy(&changed, &project("ED"), Op::Get, &control)
            .is_err()
    );
    Ok(())
}

#[test]
fn exact_selectors_operations_and_other_permission_intent_survive_updates() -> TestResult {
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    store.set_project_read_policy("factory", update(None, true), true, &control, |_| Ok(()))?;
    let first = store.set_project_context_permission(
        "factory",
        permission("ED", 1, true, vec![Op::Get]),
        true,
        &control,
        |_| Ok(()),
    )?;
    let second = store.set_project_context_permission(
        "factory",
        permission("10000", 2, true, vec![Op::Statuses]),
        true,
        &control,
        |_| Ok(()),
    )?;
    assert_eq!(
        second
            .context_reads
            .as_ref()
            .ok_or("Missing permissions")?
            .iter()
            .map(|entry| entry.target.id.as_str())
            .collect::<Vec<_>>(),
        vec!["10000", "ED"]
    );
    store.verify_project_context_policy(&second, &project("ED"), Op::Get, &control)?;
    for (id, operation) in [
        ("ed", Op::Get),
        ("10000", Op::Get),
        ("OTHER", Op::Get),
        ("ED", Op::Statuses),
    ] {
        assert!(
            store
                .verify_project_context_policy(&second, &project(id), operation, &control)
                .is_err()
        );
    }
    assert_eq!(
        store
            .verify_project_context_policy(&first, &project("ED"), Op::Get, &control)
            .err()
            .ok_or("Expected captured revision refusal")?
            .code,
        ErrorCode::VersionConflict
    );
    for id in ["ED/other", "https://untrusted.invalid"] {
        assert!(
            store
                .set_project_context_permission(
                    "factory",
                    permission(id, 3, true, vec![Op::Get]),
                    true,
                    &control,
                    |_| Ok(())
                )
                .is_err()
        );
    }
    for operations in [vec![], vec![Op::Get, Op::Get]] {
        assert!(
            store
                .set_project_context_permission(
                    "factory",
                    permission("ED", 3, true, operations),
                    true,
                    &control,
                    |_| Ok(())
                )
                .is_err()
        );
    }
    let mut foreign = permission("ED", 3, true, vec![Op::Get]);
    foreign.target.tenant = "11111111-1111-2222-3333-444444444444".into();
    assert!(
        store
            .set_project_context_permission("factory", foreign, true, &control, |_| Ok(()))
            .is_err()
    );
    assert_eq!(
        store.project_read_policy("factory", &control)?,
        Some(second)
    );
    Ok(())
}

#[test]
fn competing_context_edits_use_one_whole_record_revision_winner() -> TestResult {
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    store.set_project_read_policy("factory", update(None, true), true, &control, |_| Ok(()))?;
    let stores = [
        ProfileStore::open(&files.root, &control)?,
        ProfileStore::open(&files.root, &control)?,
    ];
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let results = std::thread::scope(|scope| {
        let workers: Vec<_> = stores
            .into_iter()
            .enumerate()
            .map(|(index, store)| {
                let barrier = barrier.clone();
                let control = control.clone();
                scope.spawn(move || {
                    barrier.wait();
                    store.set_project_context_permission(
                        "factory",
                        permission(
                            if index == 0 { "10000" } else { "20000" },
                            1,
                            true,
                            vec![Op::Get],
                        ),
                        true,
                        &control,
                        |_| Ok(()),
                    )
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().map_err(|_| "Permission writer panicked"))
            .collect::<Result<Vec<_>, _>>()
    })?;
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter_map(|result| result.as_ref().err())
            .map(|error| error.code)
            .collect::<Vec<_>>(),
        vec![ErrorCode::VersionConflict]
    );
    let current = store
        .project_read_policy("factory", &control)?
        .ok_or("Missing record")?;
    assert_eq!(current.revision, 2);
    assert_eq!(current.context_reads.ok_or("Missing permissions")?.len(), 1);
    Ok(())
}

#[test]
fn permission_capacity_refuses_without_eviction_or_revision_changes() -> TestResult {
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    store.set_project_read_policy("factory", update(None, true), true, &control, |_| Ok(()))?;
    for index in 0..32 {
        store.set_project_context_permission(
            "factory",
            permission(&(10000 + index).to_string(), 1 + index, true, vec![Op::Get]),
            true,
            &control,
            |_| Ok(()),
        )?;
    }
    let before = store
        .project_read_policy("factory", &control)?
        .ok_or("Missing record")?;
    assert!(
        store
            .set_project_context_permission(
                "factory",
                permission("EXTRA", 33, true, vec![Op::Get]),
                true,
                &control,
                |_| Ok(())
            )
            .is_err()
    );
    assert_eq!(
        store.project_read_policy("factory", &control)?,
        Some(before)
    );
    Ok(())
}

#[test]
fn corrupt_or_legacy_embedded_permissions_fail_closed_without_echoing_payload() -> TestResult {
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    store.set_project_read_policy("factory", update(None, true), true, &control, |_| Ok(()))?;
    let first = store.set_project_context_permission(
        "factory",
        permission("ED", 1, true, vec![Op::Get]),
        true,
        &control,
        |_| Ok(()),
    )?;
    let raw = files.raw()?;
    for mode in [
        "legacy",
        "missing",
        "unknown_format",
        "unknown_operation",
        "duplicate_operation",
        "duplicate_target",
        "foreign",
        "unknown_field",
    ] {
        let mut value = serde_json::to_value(&first)?;
        match mode {
            "legacy" => value["format"] = json!(1),
            "missing" => value["context_reads"] = Value::Null,
            "unknown_format" => value["format"] = json!(3),
            "unknown_operation" => {
                value["context_reads"][0]["operations"] = json!(["jira.project.delete"])
            }
            "duplicate_operation" => {
                value["context_reads"][0]["operations"] =
                    json!(["jira.project.get", "jira.project.get"])
            }
            "duplicate_target" => {
                value["context_reads"] =
                    json!([value["context_reads"][0], value["context_reads"][0]])
            }
            "foreign" => {
                value["context_reads"][0]["target"]["tenant"] =
                    json!("11111111-1111-2222-3333-444444444444")
            }
            _ => value["context_reads"][0]["unexpected"] = json!("private-sentinel"),
        }
        raw.execute(
            "UPDATE project_read_policies SET payload=?1",
            [serde_json::to_string(&value)?],
        )?;
        let error = store
            .project_read_policy("factory", &control)
            .err()
            .ok_or("Expected corrupt permission refusal")?;
        assert!(!serde_json::to_string(&error)?.contains("private-sentinel"));
    }
    let mut bytes = serde_json::to_string(&first)?;
    bytes.insert_str(1, "\"format\":2,");
    raw.execute("UPDATE project_read_policies SET payload=?1", [bytes])?;
    assert!(store.project_read_policy("factory", &control).is_err());
    Ok(())
}
