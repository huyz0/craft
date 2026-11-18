use craft_core::contracts::{AuthMode, ResourceRef};
use craft_core::control::{CancellationToken, InvocationControl};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::profiles::ProfileInput;
use craft_runtime::confluence_admin_policy::{
    ConfluenceAdminOperation as Op, ConfluenceAdminPolicyUpdate,
};
use craft_runtime::control::SystemClock;
use craft_runtime::profiles::ProfileStore;
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
fn update(revision: Option<u64>, enabled: bool) -> ConfluenceAdminPolicyUpdate {
    ConfluenceAdminPolicyUpdate {
        target: target(),
        expected_actor: "actor-one".into(),
        operations: vec![Op::SpaceCreate],
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
            "confluence-admin-policy",
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
fn site_admin_policy_is_explicit_revisioned_and_migrates_from_schema_twelve() -> TestResult {
    let files = Files::new()?;
    let store = files.store()?;
    let control = control()?;
    assert!(
        store
            .confluence_admin_policy("factory", &control)?
            .is_none()
    );
    assert_eq!(
        store
            .set_confluence_admin_policy("factory", update(None, true), false, &control, |_| Ok(()))
            .err()
            .ok_or("apply")?
            .code,
        ErrorCode::ApplyRequired
    );
    let mut empty = update(None, true);
    empty.operations.clear();
    assert!(
        store
            .set_confluence_admin_policy("factory", empty, true, &control, |_| Ok(()))
            .is_err()
    );
    let mut space = update(None, true);
    space.target.resource_type = "space".into();
    space.target.id = "100".into();
    assert!(
        store
            .set_confluence_admin_policy("factory", space, true, &control, |_| Ok(()))
            .is_err()
    );
    let record =
        store.set_confluence_admin_policy("factory", update(None, true), true, &control, |_| {
            Ok(())
        })?;
    assert_eq!(record.revision, 1);
    store.verify_confluence_admin_policy(&record, Op::SpaceCreate, &control)?;
    assert_eq!(
        store
            .set_confluence_admin_policy("factory", update(None, true), true, &control, |_| Ok(()))
            .err()
            .ok_or("revision")?
            .code,
        ErrorCode::VersionConflict
    );
    let disabled = store.set_confluence_admin_policy(
        "factory",
        update(Some(1), false),
        true,
        &control,
        |_| Ok(()),
    )?;
    assert_eq!(
        store
            .verify_confluence_admin_policy(&record, Op::SpaceCreate, &control)
            .err()
            .ok_or("disabled")?
            .code,
        ErrorCode::PolicyDenied
    );
    assert!(!disabled.enabled);
    assert!(store.confluence_read_policy("factory", &control)?.is_none());
    drop(store);
    let raw = files.raw()?;
    raw.execute_batch("DROP TABLE IF EXISTS provider_policies; DROP TABLE confluence_admin_policies; PRAGMA user_version=12;")?;
    let migrated = files.store()?;
    assert!(
        migrated
            .confluence_admin_policy("factory", &control)?
            .is_none()
    );
    drop(migrated);
    assert_eq!(
        raw.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
        14
    );
    raw.execute_batch("DROP TABLE IF EXISTS provider_policies; DROP TABLE confluence_admin_policies; CREATE TABLE confluence_admin_policies(foreign_data TEXT); PRAGMA user_version=12;")?;
    assert!(ProfileStore::open(&files.root, &control).is_err());
    Ok(())
}
