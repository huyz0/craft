use craft_core::contracts::{AuthMode, ResourceRef};
use craft_core::control::{CancellationToken, InvocationControl};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::profiles::ProfileInput;
use craft_runtime::control::SystemClock;
use craft_runtime::profiles::ProfileStore;
use craft_runtime::provider_policy::ProviderPolicyUpdate;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const MEMBER: &str = "5f1d2c3b4a5968778695a4b3";

fn control() -> Result<InvocationControl, CraftError> {
    InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        15_000,
    )
}

fn board() -> ResourceRef {
    ResourceRef {
        provider: "trello".into(),
        deployment: "cloud".into(),
        tenant: MEMBER.into(),
        resource_type: "account".into(),
        id: MEMBER.into(),
    }
}

fn update(
    class: &str,
    revision: Option<u64>,
    enabled: bool,
    operations: &[&str],
) -> ProviderPolicyUpdate {
    ProviderPolicyUpdate {
        class: class.into(),
        target: board(),
        operations: operations.iter().map(|name| (*name).into()).collect(),
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
            "provider-policy",
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
                    name: "cards".into(),
                    provider: "trello".into(),
                    deployment: "cloud".into(),
                    auth_mode: AuthMode::Service,
                    grant: None,
                    tenant: Some(MEMBER.into()),
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
fn a_missing_policy_grants_nothing_and_each_class_is_revisioned_separately() -> TestResult {
    let files = Files::new()?;
    let store = files.store()?;
    let control = control()?;
    assert!(
        store
            .provider_policy("cards", "trello.read", &control)?
            .is_none()
    );
    let read = store.set_provider_policy(
        "cards",
        update(
            "trello.read",
            None,
            true,
            &["trello.card.list", "trello.board.get"],
        ),
        true,
        &control,
        |_| Ok(()),
    )?;
    // Operations are stored in canonical order.
    assert_eq!(read.operations, ["trello.board.get", "trello.card.list"]);
    assert_eq!(read.revision, 1);
    assert!(
        store
            .provider_policy("cards", "trello.write", &control)?
            .is_none()
    );
    store.verify_provider_policy(&read, "trello.card.list", &control)?;
    assert_eq!(
        store
            .verify_provider_policy(&read, "trello.card.create", &control)
            .err()
            .ok_or("operation outside the policy")?
            .code,
        ErrorCode::PolicyDenied
    );
    let disabled = store.set_provider_policy(
        "cards",
        update("trello.read", Some(1), false, &["trello.card.list"]),
        true,
        &control,
        |_| Ok(()),
    )?;
    assert_eq!(disabled.revision, 2);
    assert_eq!(
        store
            .verify_provider_policy(&read, "trello.card.list", &control)
            .err()
            .ok_or("disabled")?
            .code,
        ErrorCode::PolicyDenied
    );
    Ok(())
}

#[test]
fn updates_need_apply_the_exact_revision_a_matching_provider_and_a_clean_preflight() -> TestResult {
    let files = Files::new()?;
    let store = files.store()?;
    let control = control()?;
    let attempt = |class: &str, revision, apply, ops: &[&str]| {
        store.set_provider_policy(
            "cards",
            update(class, revision, true, ops),
            apply,
            &control,
            |_| Ok(()),
        )
    };
    assert_eq!(
        attempt("trello.read", None, false, &["trello.card.get"])
            .err()
            .ok_or("apply")?
            .code,
        ErrorCode::ApplyRequired
    );
    // A Bitbucket class cannot be attached to a Trello profile, and operations must match the class's provider.
    assert!(attempt("bitbucket.read", None, true, &["bitbucket.pr.get"]).is_err());
    assert!(attempt("trello.read", None, true, &["bitbucket.pr.get"]).is_err());
    assert!(attempt("trello.read", None, true, &[]).is_err());
    assert!(attempt("trello.nonsense", None, true, &["trello.card.get"]).is_err());
    attempt("trello.read", None, true, &["trello.card.get"])?;
    assert_eq!(
        attempt("trello.read", None, true, &["trello.card.get"])
            .err()
            .ok_or("stale")?
            .code,
        ErrorCode::VersionConflict
    );
    let rejected = store.set_provider_policy(
        "cards",
        update("trello.write", None, true, &["trello.card.create"]),
        true,
        &control,
        |_| {
            Err(CraftError::new(
                ErrorCode::OutputBudgetTooSmall,
                "too small",
            ))
        },
    );
    assert!(rejected.is_err());
    assert!(
        store
            .provider_policy("cards", "trello.write", &control)?
            .is_none()
    );
    Ok(())
}

#[test]
fn a_target_belonging_to_another_member_is_refused() -> TestResult {
    let files = Files::new()?;
    let store = files.store()?;
    let mut other = update("trello.read", None, true, &["trello.card.get"]);
    other.target.tenant = "aaaaaaaaaaaaaaaaaaaaaaaa".into();
    assert!(
        store
            .set_provider_policy("cards", other, true, &control()?, |_| Ok(()))
            .is_err()
    );
    Ok(())
}

#[test]
fn the_store_migrates_from_schema_thirteen_and_refuses_a_foreign_table() -> TestResult {
    let files = Files::new()?;
    let store = files.store()?;
    let control = control()?;
    store.set_provider_policy(
        "cards",
        update("trello.read", None, true, &["trello.card.get"]),
        true,
        &control,
        |_| Ok(()),
    )?;
    drop(store);
    let raw = files.raw()?;
    raw.execute_batch("DROP TABLE provider_policies; PRAGMA user_version=13;")?;
    let migrated = files.store()?;
    assert!(
        migrated
            .provider_policy("cards", "trello.read", &control)?
            .is_none()
    );
    drop(migrated);
    assert_eq!(
        raw.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
        14
    );
    raw.execute_batch("DROP TABLE provider_policies; CREATE TABLE provider_policies(foreign_data TEXT); PRAGMA user_version=13;")?;
    assert!(ProfileStore::open(&files.root, &control).is_err());
    Ok(())
}
