use craft_core::contracts::{AuthMode, ResourceRef};
use craft_core::control::{CancellationToken, InvocationControl};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::profiles::ProfileInput;
use craft_runtime::confluence_write_policy::{
    ConfluenceWriteOperation as Op, ConfluenceWritePolicyUpdate,
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
        resource_type: "space".into(),
        id: "100".into(),
    }
}
fn update(revision: Option<u64>, enabled: bool) -> ConfluenceWritePolicyUpdate {
    ConfluenceWritePolicyUpdate {
        target: target(),
        expected_actor: "actor-one".into(),
        operations: vec![Op::Update, Op::Create],
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
            "confluence-write-policy",
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
fn write_authority_requires_apply_preflight_cas_and_exact_operation() -> TestResult {
    let files = Files::new()?;
    let store = files.store()?;
    let c = control()?;
    assert!(
        store
            .confluence_write_policy("factory", "100", &c)?
            .is_none()
    );
    assert!(
        store
            .set_confluence_write_policy("factory", update(None, true), false, &c, |_| Ok(()))
            .is_err()
    );
    assert!(
        store
            .set_confluence_write_policy("factory", update(None, true), true, &c, |_| Err(
                CraftError::new(ErrorCode::OutputBudgetTooSmall, "No receipt capacity.")
            ))
            .is_err()
    );
    assert!(
        store
            .confluence_write_policy("factory", "100", &c)?
            .is_none()
    );
    let first =
        store.set_confluence_write_policy("factory", update(None, true), true, &c, |_| Ok(()))?;
    assert_eq!(first.operations, vec![Op::Create, Op::Update]);
    store.verify_confluence_write_policy(&first, Op::Create, &c)?;
    assert!(store.confluence_read_policy("factory", &c)?.is_none());
    assert!(
        store
            .confluence_write_policy("factory", "200", &c)?
            .is_none()
    );
    let mut next = update(Some(1), true);
    next.operations = vec![Op::Create];
    let second = store.set_confluence_write_policy("factory", next, true, &c, |_| Ok(()))?;
    assert_eq!(
        store
            .verify_confluence_write_policy(&first, Op::Create, &c)
            .err()
            .ok_or("changed")?
            .code,
        ErrorCode::VersionConflict
    );
    assert_eq!(
        store
            .verify_confluence_write_policy(&second, Op::Update, &c)
            .err()
            .ok_or("excluded")?
            .code,
        ErrorCode::PolicyDenied
    );
    assert!(
        store
            .set_confluence_write_policy("factory", update(Some(1), false), true, &c, |_| Ok(()))
            .is_err()
    );
    assert_eq!(
        files
            .store()?
            .confluence_write_policy("factory", "100", &c)?,
        Some(second)
    );
    Ok(())
}
#[test]
fn write_records_bind_space_actor_grant_and_installation_without_repair() -> TestResult {
    let files = Files::new()?;
    let store = files.store()?;
    let c = control()?;
    store.set_confluence_write_policy("factory", update(None, true), true, &c, |_| Ok(()))?;
    let mut transfer = update(Some(1), false);
    transfer.expected_actor = "actor-two".into();
    assert!(
        store
            .set_confluence_write_policy("factory", transfer, true, &c, |_| Ok(()))
            .is_err()
    );
    store.set_confluence_write_policy("factory", update(Some(1), false), true, &c, |_| Ok(()))?;
    let mut transfer = update(Some(2), true);
    transfer.expected_actor = "actor-two".into();
    let current = store.set_confluence_write_policy("factory", transfer, true, &c, |_| Ok(()))?;
    let raw = files.raw()?;
    for patch in [
        json!({"format":2}),
        json!({"installation":"foreign"}),
        json!({"profile":"other"}),
        json!({"revision":1}),
        json!({"expected_actor":"anonymous"}),
        json!({"grant":{"id":"foreign-grant"}}),
        json!({"operations":["confluence.page.get"]}),
        json!({"operations":["confluence.page.update","confluence.page.create"]}),
    ] {
        let mut value = serde_json::to_value(&current)?;
        for (k, v) in patch.as_object().ok_or("patch")? {
            value[k] = v.clone();
        }
        let payload = serde_json::to_string(&value)?;
        raw.execute(
            "UPDATE confluence_write_policies SET payload=?1",
            [&payload],
        )?;
        assert!(store.confluence_write_policy("factory", "100", &c).is_err());
        assert_eq!(
            raw.query_row("SELECT payload FROM confluence_write_policies", [], |r| {
                r.get::<_, String>(0)
            })?,
            payload
        );
    }
    Ok(())
}
#[test]
fn write_table_migrates_empty_and_preserves_reads_while_conflicts_and_triggers_refuse() -> TestResult
{
    use craft_runtime::confluence_read_policy::{
        ConfluenceReadOperation, ConfluenceReadPolicyUpdate,
    };
    let files = Files::new()?;
    let store = files.store()?;
    let c = control()?;
    let read = store.set_confluence_read_policy(
        "factory",
        ConfluenceReadPolicyUpdate {
            target: ResourceRef {
                resource_type: "site".into(),
                id: target().tenant,
                ..target()
            },
            expected_actor: "actor-one".into(),
            operations: vec![ConfluenceReadOperation::PageGet],
            enabled: true,
            expected_revision: None,
        },
        true,
        &c,
        |_| Ok(()),
    )?;
    let profile = store.get("factory", &c)?;
    let settings = store.settings().clone();
    drop(store);
    let raw = files.raw()?;
    raw.execute_batch(
        "DROP TABLE IF EXISTS provider_policies; DROP TABLE IF EXISTS confluence_admin_policies; DROP TABLE IF EXISTS review_policies; DROP TABLE confluence_write_policies; PRAGMA user_version=10;",
    )?;
    let migrated = files.store()?;
    assert_eq!(migrated.get("factory", &c)?, profile);
    assert_eq!(migrated.settings(), &settings);
    assert_eq!(migrated.confluence_read_policy("factory", &c)?, Some(read));
    assert!(
        migrated
            .confluence_write_policy("factory", "100", &c)?
            .is_none()
    );
    drop(migrated);
    assert_eq!(
        raw.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
        14
    );
    raw.execute_batch("DROP TABLE IF EXISTS provider_policies; DROP TABLE IF EXISTS confluence_admin_policies; DROP TABLE IF EXISTS review_policies; DROP TABLE confluence_write_policies; CREATE TABLE confluence_write_policies(foreign_data TEXT); PRAGMA user_version=10;")?;
    assert!(ProfileStore::open(&files.root, &c).is_err());
    assert_eq!(
        raw.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
        10
    );
    raw.execute_batch(
        "DROP TABLE IF EXISTS provider_policies; DROP TABLE IF EXISTS confluence_admin_policies; DROP TABLE IF EXISTS review_policies; DROP TABLE confluence_write_policies;",
    )?;
    drop(files.store()?);
    raw.execute_batch("CREATE TRIGGER write_policy_trigger AFTER INSERT ON confluence_write_policies BEGIN SELECT 1; END;")?;
    assert!(ProfileStore::open(&files.root, &c).is_err());
    Ok(())
}
#[test]
fn duplicate_operations_and_foreign_sites_do_not_create_write_authority() -> TestResult {
    let files = Files::new()?;
    let store = files.store()?;
    let c = control()?;
    for mode in ["empty", "duplicate", "space", "provider", "site"] {
        let mut u = update(None, true);
        match mode {
            "empty" => u.operations = vec![],
            "duplicate" => u.operations = vec![Op::Create, Op::Create],
            "space" => u.target.id = "0100".into(),
            "provider" => u.target.provider = "jira".into(),
            "site" => u.target.tenant = "11111111-1111-2222-3333-444444444444".into(),
            _ => {}
        }
        assert!(
            store
                .set_confluence_write_policy("factory", u, true, &c, |_| Ok(()))
                .is_err(),
            "{mode}"
        );
    }
    Ok(())
}
