use craft_core::contracts::{AuthMode, ResourceRef};
use craft_core::control::{CancellationToken, InvocationControl};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::profiles::ProfileInput;
use craft_runtime::control::SystemClock;
use craft_runtime::issue_read_policy::{IssueReadOperation as Read, IssueReadPolicyUpdate};
use craft_runtime::profiles::ProfileStore;
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn download_requires_explicit_new_format_and_can_coexist_with_all_reads() -> TestResult {
    let files = Files::new()?;
    let store = files.setup()?;
    let c = control()?;
    let old = store.set_issue_read_policy("factory", update(None, true), true, &c, |_| Ok(()))?;
    assert!(
        store
            .verify_issue_read_policy(&old, Read::AttachmentDownload, &c)
            .is_err()
    );
    let raw = files.raw()?;
    for format in 1..=4 {
        let mut forged = serde_json::to_value(&old)?;
        forged["format"] = json!(format);
        forged["operations"] = json!([Read::AttachmentDownload]);
        raw.execute(
            "UPDATE issue_read_policies SET payload=?1 WHERE profile='factory'",
            [serde_json::to_string(&forged)?],
        )?;
        assert!(store.issue_read_policy("factory", &c).is_err());
    }
    raw.execute(
        "UPDATE issue_read_policies SET payload=?1 WHERE profile='factory'",
        [serde_json::to_string(&old)?],
    )?;
    let mut next = update(Some(old.revision), true);
    next.operations = vec![
        Read::Get,
        Read::Search,
        Read::EditMetadata,
        Read::Transitions,
        Read::Permissions,
        Read::LinkGet,
        Read::LinkList,
        Read::LinkTypes,
        Read::CommentGet,
        Read::CommentList,
        Read::AttachmentGet,
        Read::AttachmentList,
        Read::AttachmentSettings,
        Read::AttachmentDownload,
    ];
    let current = store.set_issue_read_policy("factory", next, true, &c, |_| Ok(()))?;
    assert_eq!(current.format, 5);
    assert_eq!(current.operations.len(), 14);
    for op in &current.operations {
        store.verify_issue_read_policy(&current, *op, &c)?;
    }
    Ok(())
}

#[test]
fn attachment_reads_require_new_format_and_all_thirteen_allowances_can_coexist() -> TestResult {
    let files = Files::new()?;
    let store = files.setup()?;
    let c = control()?;
    let old = store.set_issue_read_policy("factory", update(None, true), true, &c, |_| Ok(()))?;
    let raw = files.raw()?;
    let attachments = [
        Read::AttachmentGet,
        Read::AttachmentList,
        Read::AttachmentSettings,
    ];
    for format in 1..=3 {
        for operation in attachments {
            let mut forged = serde_json::to_value(&old)?;
            forged["format"] = json!(format);
            forged["operations"] = json!([operation]);
            raw.execute(
                "UPDATE issue_read_policies SET payload=?1 WHERE profile='factory'",
                [serde_json::to_string(&forged)?],
            )?;
            assert!(store.issue_read_policy("factory", &c).is_err());
        }
    }
    raw.execute(
        "UPDATE issue_read_policies SET payload=?1 WHERE profile='factory'",
        [serde_json::to_string(&old)?],
    )?;
    let operations = vec![
        Read::Get,
        Read::Search,
        Read::EditMetadata,
        Read::Transitions,
        Read::Permissions,
        Read::LinkGet,
        Read::LinkList,
        Read::LinkTypes,
        Read::CommentGet,
        Read::CommentList,
        Read::AttachmentGet,
        Read::AttachmentList,
        Read::AttachmentSettings,
    ];
    let record = store.set_issue_read_policy(
        "factory",
        IssueReadPolicyUpdate {
            operations: operations.clone(),
            ..update(Some(1), true)
        },
        true,
        &c,
        |_| Ok(()),
    )?;
    assert_eq!(record.format, 4);
    assert_eq!(record.operations, operations);
    for operation in operations {
        store.verify_issue_read_policy(&record, operation, &c)?;
    }
    assert!(
        store
            .verify_issue_read_policy(&old, Read::AttachmentGet, &c)
            .is_err()
    );
    Ok(())
}

#[test]
fn comments_require_new_explicit_format_and_all_ten_reads_can_coexist() -> TestResult {
    let files = Files::new()?;
    let store = files.setup()?;
    let c = control()?;
    let old = store.set_issue_read_policy("factory", update(None, true), true, &c, |_| Ok(()))?;
    let raw = files.raw()?;
    for format in [1, 2] {
        for operation in [Read::CommentGet, Read::CommentList] {
            let mut forged = serde_json::to_value(&old)?;
            forged["format"] = json!(format);
            forged["operations"] = json!([operation]);
            raw.execute(
                "UPDATE issue_read_policies SET payload=?1 WHERE profile='factory'",
                [serde_json::to_string(&forged)?],
            )?;
            assert!(store.issue_read_policy("factory", &c).is_err());
        }
    }
    raw.execute(
        "UPDATE issue_read_policies SET payload=?1 WHERE profile='factory'",
        [serde_json::to_string(&old)?],
    )?;
    let operations = vec![
        Read::Get,
        Read::Search,
        Read::EditMetadata,
        Read::Transitions,
        Read::Permissions,
        Read::LinkGet,
        Read::LinkList,
        Read::LinkTypes,
        Read::CommentGet,
        Read::CommentList,
    ];
    let record = store.set_issue_read_policy(
        "factory",
        IssueReadPolicyUpdate {
            operations: operations.clone(),
            ..update(Some(1), true)
        },
        true,
        &c,
        |_| Ok(()),
    )?;
    assert_eq!(record.format, 3);
    assert_eq!(record.operations, operations);
    for operation in operations {
        store.verify_issue_read_policy(&record, operation, &c)?;
    }
    assert!(
        store
            .verify_issue_read_policy(&old, Read::CommentGet, &c)
            .is_err()
    );
    Ok(())
}

#[test]
fn legacy_read_payloads_cannot_gain_link_operations_without_explicit_upgrade() -> TestResult {
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    let original =
        store.set_issue_read_policy("factory", update(None, true), true, &control, |_| Ok(()))?;
    assert_eq!(original.format, 1);
    let raw = files.raw()?;
    for op in [Read::LinkGet, Read::LinkList, Read::LinkTypes] {
        let mut forged = serde_json::to_value(&original)?;
        forged["operations"] = json!([op]);
        raw.execute(
            "UPDATE issue_read_policies SET payload=?1 WHERE profile='factory'",
            [serde_json::to_string(&forged)?],
        )?;
        assert!(store.issue_read_policy("factory", &control).is_err());
    }
    raw.execute(
        "UPDATE issue_read_policies SET payload=?1 WHERE profile='factory'",
        [serde_json::to_string(&original)?],
    )?;
    let linked = store.set_issue_read_policy(
        "factory",
        IssueReadPolicyUpdate {
            operations: vec![Read::LinkGet, Read::LinkList, Read::LinkTypes],
            ..update(Some(1), true)
        },
        true,
        &control,
        |_| Ok(()),
    )?;
    assert_eq!(linked.format, 2);
    for op in [Read::LinkGet, Read::LinkList, Read::LinkTypes] {
        store.verify_issue_read_policy(&linked, op, &control)?;
    }
    assert!(
        store
            .verify_issue_read_policy(&linked, Read::Get, &control)
            .is_err()
    );
    assert!(
        store
            .verify_issue_read_policy(&original, Read::Get, &control)
            .is_err()
    );
    Ok(())
}

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
fn update(revision: Option<u64>, enabled: bool) -> IssueReadPolicyUpdate {
    IssueReadPolicyUpdate {
        target: target(),
        expected_actor: "actor-one".into(),
        operations: vec![Read::Search, Read::Get],
        enabled,
        expected_revision: revision,
    }
}

#[test]
fn current_user_permission_checks_require_an_independent_appended_allowance() -> TestResult {
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    let mut old = update(None, true);
    old.operations = vec![
        Read::Get,
        Read::Search,
        Read::EditMetadata,
        Read::Transitions,
    ];
    let original = store.set_issue_read_policy("factory", old, true, &control, |_| Ok(()))?;
    assert_eq!(
        store
            .verify_issue_read_policy(&original, Read::Permissions, &control)
            .err()
            .ok_or("Expected new operation refusal")?
            .code,
        ErrorCode::PolicyDenied
    );
    let mut selected = update(Some(1), true);
    selected.operations = vec![Read::Permissions];
    let only = store.set_issue_read_policy("factory", selected, true, &control, |_| Ok(()))?;
    store.verify_issue_read_policy(&only, Read::Permissions, &control)?;
    for operation in [
        Read::Get,
        Read::Search,
        Read::EditMetadata,
        Read::Transitions,
    ] {
        assert_eq!(
            store
                .verify_issue_read_policy(&only, operation, &control)
                .err()
                .ok_or("Expected other operation refusal")?
                .code,
            ErrorCode::PolicyDenied
        );
    }
    let mut all = update(Some(2), true);
    all.operations = vec![
        Read::Permissions,
        Read::Transitions,
        Read::EditMetadata,
        Read::Search,
        Read::Get,
    ];
    let all = store.set_issue_read_policy("factory", all, true, &control, |_| Ok(()))?;
    assert_eq!(
        all.operations,
        vec![
            Read::Get,
            Read::Search,
            Read::EditMetadata,
            Read::Transitions,
            Read::Permissions
        ]
    );
    assert_eq!(
        ProfileStore::open(&files.root, &control)?.issue_read_policy("factory", &control)?,
        Some(all)
    );
    assert_eq!(
        files
            .raw()?
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?,
        12
    );
    assert!(store.field_read_policy("factory", &control)?.is_none());
    assert!(store.project_read_policy("factory", &control)?.is_none());
    Ok(())
}

#[test]
fn enabled_issue_read_authority_requires_disable_before_actor_transfer() -> TestResult {
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    let original =
        store.set_issue_read_policy("factory", update(None, true), true, &control, |_| Ok(()))?;
    let mut transfer = update(Some(1), true);
    transfer.expected_actor = "actor-two".into();
    assert_eq!(
        store
            .set_issue_read_policy("factory", transfer, true, &control, |_| Ok(()))
            .err()
            .ok_or("Expected actor transfer refusal")?
            .code,
        ErrorCode::PolicyDenied
    );
    assert_eq!(
        store.issue_read_policy("factory", &control)?,
        Some(original)
    );
    store.set_issue_read_policy(
        "factory",
        update(Some(1), false),
        true,
        &control,
        |_| Ok(()),
    )?;
    let mut transfer = update(Some(2), true);
    transfer.expected_actor = "actor-two".into();
    let transferred =
        store.set_issue_read_policy("factory", transfer, true, &control, |_| Ok(()))?;
    assert_eq!(transferred.expected_actor, "actor-two");
    assert_eq!(transferred.revision, 3);
    Ok(())
}

#[test]
fn metadata_allowances_are_explicit_and_preserve_legacy_record_bytes_and_schema() -> TestResult {
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    let old =
        store.set_issue_read_policy("factory", update(None, true), true, &control, |_| Ok(()))?;
    let raw = files.raw()?;
    let payload: String = raw.query_row("SELECT payload FROM issue_read_policies", [], |row| {
        row.get(0)
    })?;
    assert_eq!(
        serde_json::to_value(&old.operations)?,
        json!(["jira.issue.get", "jira.issue.search"])
    );
    let settings = store.settings().clone();
    let schema: String=raw.query_row("SELECT group_concat(sql,';') FROM (SELECT sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY name)",[],|row|row.get(0))?;
    let reopened = ProfileStore::open(&files.root, &control)?;
    assert_eq!(
        reopened.issue_read_policy("factory", &control)?,
        Some(old.clone())
    );
    assert_eq!(
        raw.query_row("SELECT payload FROM issue_read_policies", [], |row| row
            .get::<_, String>(
            0
        ))?,
        payload
    );
    for operation in [Read::EditMetadata, Read::Transitions] {
        assert_eq!(
            store
                .verify_issue_read_policy(&old, operation, &control)
                .err()
                .ok_or("Expected missing allowance")?
                .code,
            ErrorCode::PolicyDenied
        );
    }
    let mut both = update(Some(1), true);
    both.operations = vec![
        Read::Transitions,
        Read::Search,
        Read::EditMetadata,
        Read::Get,
    ];
    assert!(
        store
            .set_issue_read_policy("factory", both, true, &control, |_| Err(CraftError::new(
                ErrorCode::OutputBudgetTooSmall,
                "No receipt space."
            )))
            .is_err()
    );
    assert_eq!(store.issue_read_policy("factory", &control)?, Some(old));
    let mut both = update(Some(1), true);
    both.operations = vec![
        Read::Transitions,
        Read::Search,
        Read::EditMetadata,
        Read::Get,
    ];
    let selected = store.set_issue_read_policy("factory", both, true, &control, |_| Ok(()))?;
    assert_eq!(
        selected.operations,
        vec![
            Read::Get,
            Read::Search,
            Read::EditMetadata,
            Read::Transitions
        ]
    );
    for operation in &selected.operations {
        store.verify_issue_read_policy(&selected, *operation, &control)?;
    }
    assert_eq!(
        ProfileStore::open(&files.root, &control)?.settings(),
        &settings
    );
    assert_eq!(
        raw.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?,
        12
    );
    assert_eq!(raw.query_row("SELECT group_concat(sql,';') FROM (SELECT sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY name)",[],|row|row.get::<_,String>(0))?,schema);
    assert!(store.field_read_policy("factory", &control)?.is_none());
    assert!(store.project_read_policy("factory", &control)?.is_none());
    Ok(())
}

#[test]
fn malformed_metadata_allowances_and_disabled_profile_target_transfers_fail_closed() -> TestResult {
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    let mut update = update(None, true);
    update.operations = vec![Read::EditMetadata, Read::Transitions];
    let selected = store.set_issue_read_policy("factory", update, true, &control, |_| Ok(()))?;
    let raw = files.raw()?;
    for operations in [
        json!(["jira.metadata.edit", "jira.metadata.edit"]),
        json!(["jira.transition.list", "jira.metadata.edit"]),
        json!(["jira.transition.apply"]),
        json!([]),
    ] {
        let mut value = serde_json::to_value(&selected)?;
        value["operations"] = operations;
        raw.execute(
            "UPDATE issue_read_policies SET payload=?1",
            [serde_json::to_string(&value)?],
        )?;
        assert!(store.issue_read_policy("factory", &control).is_err());
    }
    raw.execute(
        "UPDATE issue_read_policies SET payload=?1",
        [serde_json::to_string(&selected)?],
    )?;
    store.set_issue_read_policy(
        "factory",
        self::update(Some(1), false),
        true,
        &control,
        |_| Ok(()),
    )?;
    let mut transfer = self::update(Some(2), true);
    transfer.target.tenant = "11111111-1111-2222-3333-444444444444".into();
    transfer.target.id = transfer.target.tenant.clone();
    assert!(
        store
            .set_issue_read_policy("factory", transfer, true, &control, |_| Ok(()))
            .is_err()
    );
    assert_eq!(
        store
            .issue_read_policy("factory", &control)?
            .ok_or("Missing policy")?
            .revision,
        2
    );
    Ok(())
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
            root: parent.join(format!("issue-read-policy-{}-{stamp}", std::process::id())),
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
            let _ = fs::remove_dir_all(root);
        }
    }
}

#[test]
fn issue_configuration_requires_apply_preflight_revision_and_explicit_operations() -> TestResult {
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    assert!(store.issue_read_policy("factory", &control)?.is_none());
    assert_eq!(
        store
            .set_issue_read_policy("factory", update(None, true), false, &control, |_| Ok(()))
            .err()
            .ok_or("Expected apply refusal")?
            .code,
        ErrorCode::ApplyRequired
    );
    assert!(
        store
            .set_issue_read_policy("factory", update(None, true), true, &control, |_| Err(
                CraftError::new(
                    ErrorCode::OutputBudgetTooSmall,
                    "Cannot emit configuration receipt."
                )
            ))
            .is_err()
    );
    assert!(store.issue_read_policy("factory", &control)?.is_none());
    let first =
        store.set_issue_read_policy("factory", update(None, true), true, &control, |_| Ok(()))?;
    assert_eq!(first.operations, vec![Read::Get, Read::Search]);
    assert_eq!(
        ProfileStore::open(&files.root, &control)?.issue_read_policy("factory", &control)?,
        Some(first.clone())
    );
    assert!(
        store
            .set_issue_read_policy("factory", update(None, true), true, &control, |_| Ok(()))
            .is_err()
    );
    let mut only_get = update(Some(1), true);
    only_get.operations = vec![Read::Get];
    let second = store.set_issue_read_policy("factory", only_get, true, &control, |_| Ok(()))?;
    assert_eq!(
        store
            .verify_issue_read_policy(&first, Read::Search, &control)
            .err()
            .ok_or("Expected operation removal refusal")?
            .code,
        ErrorCode::PolicyDenied
    );
    assert_eq!(
        store
            .verify_issue_read_policy(&first, Read::Get, &control)
            .err()
            .ok_or("Expected captured revision refusal")?
            .code,
        ErrorCode::VersionConflict
    );
    store.verify_issue_read_policy(&second, Read::Get, &control)?;
    store.set_issue_read_policy(
        "factory",
        update(Some(2), false),
        true,
        &control,
        |_| Ok(()),
    )?;
    assert_eq!(
        store
            .verify_issue_read_policy(&second, Read::Get, &control)
            .err()
            .ok_or("Expected revocation")?
            .code,
        ErrorCode::PolicyDenied
    );
    Ok(())
}

#[test]
fn issue_configuration_rejects_wrong_profiles_targets_actors_and_operation_sets() -> TestResult {
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    for mode in ["site", "type", "actor", "empty", "duplicate"] {
        let mut update = update(None, true);
        match mode {
            "site" => update.target.tenant = "11111111-1111-2222-3333-444444444444".into(),
            "type" => update.target.resource_type = "project".into(),
            "actor" => update.expected_actor = "unknown".into(),
            "empty" => update.operations.clear(),
            _ => update.operations = vec![Read::Get, Read::Get],
        }
        assert!(
            store
                .set_issue_read_policy("factory", update, true, &control, |_| Ok(()))
                .is_err(),
            "{mode}"
        );
    }
    for (name, provider, auth_mode) in [
        ("delegated", "jira", AuthMode::Delegated),
        ("other", "confluence", AuthMode::Service),
    ] {
        store.create(
            &ProfileInput {
                name: name.into(),
                provider: provider.into(),
                deployment: "cloud".into(),
                auth_mode,
                grant: None,
                tenant: Some(target().tenant),
                defaults: BTreeMap::new(),
            },
            &control,
            |_| Ok(()),
        )?;
        assert!(
            store
                .set_issue_read_policy(name, update(None, true), true, &control, |_| Ok(()))
                .is_err()
        );
    }
    assert!(store.issue_read_policy("factory", &control)?.is_none());
    Ok(())
}

#[test]
fn competing_issue_configuration_updates_keep_one_revision_winner() -> TestResult {
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    store.set_issue_read_policy("factory", update(None, true), true, &control, |_| Ok(()))?;
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
                    store.set_issue_read_policy(
                        "factory",
                        update(Some(1), index == 0),
                        true,
                        &control,
                        |_| Ok(()),
                    )
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().map_err(|_| "Policy writer panicked"))
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
    assert_eq!(
        store
            .issue_read_policy("factory", &control)?
            .ok_or("Missing policy")?
            .revision,
        2
    );
    Ok(())
}

#[test]
fn corrupt_issue_configuration_never_supplies_authority_or_echoes_payload() -> TestResult {
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    let first =
        store.set_issue_read_policy("factory", update(None, true), true, &control, |_| Ok(()))?;
    let raw = files.raw()?;
    for field in [
        "installation",
        "profile",
        "grant",
        "revision",
        "target",
        "actor",
        "unknown_operation",
        "duplicate_operation",
        "unsorted",
        "extra",
    ] {
        let mut value = serde_json::to_value(&first)?;
        match field {
            "installation" => value["installation"] = json!("ins_00000000000000000000000000000000"),
            "profile" => value["profile"] = json!("other"),
            "grant" => value["grant"]["id"] = json!("gr_00000000000000000000000000000000"),
            "revision" => value["revision"] = json!(2),
            "target" => value["target"]["id"] = json!("other"),
            "actor" => value["expected_actor"] = json!("unknown"),
            "unknown_operation" => value["operations"] = json!(["jira.issue.delete"]),
            "duplicate_operation" => {
                value["operations"] = json!(["jira.issue.get", "jira.issue.get"])
            }
            "unsorted" => value["operations"] = json!(["jira.issue.search", "jira.issue.get"]),
            _ => value["extra"] = json!("private-sentinel"),
        }
        raw.execute(
            "UPDATE issue_read_policies SET payload=?1",
            [serde_json::to_string(&value)?],
        )?;
        let error = store
            .issue_read_policy("factory", &control)
            .err()
            .ok_or("Expected corrupt record refusal")?;
        assert!(!serde_json::to_string(&error)?.contains("private-sentinel"));
    }
    let mut bytes = serde_json::to_string(&first)?;
    bytes.insert_str(1, "\"format\":1,");
    raw.execute("UPDATE issue_read_policies SET payload=?1", [bytes])?;
    assert!(store.issue_read_policy("factory", &control).is_err());
    Ok(())
}

#[test]
fn issue_policy_migration_conflicts_and_triggers_rollback_without_repair() -> TestResult {
    let files = Files::new()?;
    drop(files.setup()?);
    let raw = files.raw()?;
    raw.execute_batch("DROP TABLE IF EXISTS review_policies; DROP TABLE confluence_write_policies; DROP TABLE confluence_read_policies; DROP TABLE issue_recovery_policies; DROP TABLE issue_mutation_policies; DROP TABLE field_read_policies; DROP TABLE issue_read_policies; DROP TABLE project_read_policies; DROP TABLE provider_cursors; DROP TABLE create_policies; DROP TABLE operation_records; CREATE TABLE issue_read_policies(foreign_data TEXT); PRAGMA user_version=1;")?;
    assert!(ProfileStore::open(&files.root, &control()?).is_err());
    assert_eq!(
        raw.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?,
        1
    );
    assert_eq!(raw.query_row("SELECT count(*) FROM sqlite_schema WHERE name IN ('operation_records','create_policies','provider_cursors','project_read_policies')",[],|row|row.get::<_,i64>(0))?,0);
    assert_eq!(
        raw.query_row("SELECT count(*) FROM profiles", [], |row| row
            .get::<_, i64>(0))?,
        1
    );
    let other = Files::new()?;
    drop(other.setup()?);
    other.raw()?.execute_batch("CREATE TRIGGER unexpected_issue_policy AFTER INSERT ON issue_read_policies BEGIN SELECT 1; END;")?;
    assert!(ProfileStore::open(&other.root, &control()?).is_err());
    Ok(())
}

#[test]
fn issue_configuration_migration_preserves_settings_policies_journal_and_both_cursor_kinds()
-> TestResult {
    use craft_core::input::canonical_digest;
    use craft_core::pagination::{
        ListBinding, ListRequest, NativePage, NativePosition, PageSource,
    };
    use craft_core::storage::{OperationBinding, OperationJournal};
    use craft_runtime::create_policy::CreatePolicyUpdate;
    use craft_runtime::journal::NativeJournal;
    use craft_runtime::profile_pages::ProfileListRequest;
    struct Source;
    impl PageSource for Source {
        fn check_access(
            &mut self,
            _: &ListBinding,
            control: &InvocationControl,
        ) -> Result<(), CraftError> {
            control.check()
        }
        fn fetch(
            &mut self,
            _: &ListBinding,
            _: Option<&NativePosition>,
            _: &InvocationControl,
        ) -> Result<NativePage, CraftError> {
            Ok(NativePage {
                context: None,
                items: vec![json!({"id":"1"}), json!({"id":"2"})],
                next: None,
            })
        }
    }
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    let settings = store.settings().clone();
    let profile = store.get("factory", &control)?;
    store.use_profile("factory", &control, |_| Ok(()))?;
    store.create(
        &ProfileInput {
            name: "second".into(),
            provider: "jira".into(),
            deployment: "cloud".into(),
            auth_mode: AuthMode::Service,
            grant: Some(profile.grant.clone()),
            tenant: profile.tenant.clone(),
            defaults: BTreeMap::new(),
        },
        &control,
        |_| Ok(()),
    )?;
    let local = store.list_page(
        ProfileListRequest {
            cursor: None,
            binding: "profile-list",
            limit: 1,
        },
        &SystemClock::default(),
        &control,
        |_| Ok(true),
    )?;
    let digest = canonical_digest(&json!({}))?;
    let binding = ListBinding {
        operation: "jira.project.list".into(),
        descriptor_digest: digest.clone(),
        schema_digest: digest.clone(),
        projection_digest: digest.clone(),
        api_revision: "v3".into(),
        target: target(),
        grant: profile.grant.clone(),
        actor: "actor-one".into(),
        query: BTreeMap::new(),
    };
    let native = store.provider_page(
        ListRequest {
            binding: &binding,
            cursor: None,
            limit: 1,
        },
        &SystemClock::default(),
        &control,
        &mut Source,
        |_| Ok(true),
    )?;
    let project = ResourceRef {
        resource_type: "project".into(),
        id: "10000".into(),
        ..target()
    };
    let create = store.set_create_policy(
        "factory",
        CreatePolicyUpdate {
            target: project.clone(),
            expected_actor: "actor-one".into(),
            enabled: false,
            expected_revision: None,
        },
        true,
        &control,
        |_| Ok(()),
    )?;
    let journal = NativeJournal::open(&store, &control)?;
    let operation = journal.begin(&OperationBinding {
        operation_id: "create-report".into(),
        request_digest: digest.clone(),
        descriptor_digest: digest,
        target: project,
        grant: profile.grant.clone(),
        actor: "actor-one".into(),
    })?;
    drop(journal);
    let project_read = store.set_project_read_policy(
        "factory",
        craft_runtime::project_read_policy::ProjectReadPolicyUpdate {
            target: target(),
            expected_actor: "actor-one".into(),
            continuation_site: "https://craft-test.atlassian.net/".into(),
            enabled: true,
            expected_revision: None,
        },
        true,
        &control,
        |_| Ok(()),
    )?;
    drop(store);
    let raw = files.raw()?;
    raw.execute_batch(
        "DROP TABLE IF EXISTS review_policies; DROP TABLE confluence_write_policies; DROP TABLE confluence_read_policies; DROP TABLE issue_recovery_policies; DROP TABLE issue_mutation_policies; DROP TABLE field_read_policies; DROP TABLE issue_read_policies; PRAGMA user_version=5;",
    )?;
    let migrated = ProfileStore::open(&files.root, &control)?;
    assert_eq!(migrated.settings(), &settings);
    assert_eq!(migrated.get("factory", &control)?, profile);
    assert_eq!(migrated.active(&control)?.as_deref(), Some("factory"));
    assert_eq!(migrated.create_policy("factory", &control)?, Some(create));
    assert_eq!(
        NativeJournal::open(&migrated, &control)?.get("create-report")?,
        Some(operation)
    );
    assert_eq!(
        migrated.project_read_policy("factory", &control)?,
        Some(project_read)
    );
    assert!(migrated.issue_read_policy("factory", &control)?.is_none());
    assert_eq!(
        migrated
            .list_page(
                ProfileListRequest {
                    cursor: local.page.next_cursor.as_deref(),
                    binding: "profile-list",
                    limit: 1
                },
                &SystemClock::default(),
                &control,
                |_| Ok(true)
            )?
            .items[0]
            .name,
        "second"
    );
    let resumed = migrated.provider_page(
        ListRequest {
            binding: &binding,
            cursor: native.page.next_cursor.as_deref(),
            limit: 100,
        },
        &SystemClock::default(),
        &control,
        &mut Source,
        |_| Ok(true),
    )?;
    assert!(resumed.cached);
    assert_eq!(resumed.items, vec![json!({"id":"2"})]);
    assert_eq!(
        raw.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?,
        12
    );
    Ok(())
}
