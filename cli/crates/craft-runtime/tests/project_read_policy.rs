use craft_core::contracts::{AuthMode, ResourceRef};
use craft_core::control::{CancellationToken, InvocationControl};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::profiles::ProfileInput;
use craft_runtime::control::SystemClock;
use craft_runtime::profiles::ProfileStore;
use craft_runtime::project_read_policy::ProjectReadPolicyUpdate;
use serde_json::json;
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
                "project-read-policy-{}-{stamp}",
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

#[test]
fn read_configuration_apply_preflight_revision_and_revocation_survive_reopen() -> TestResult {
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    assert!(store.project_read_policy("factory", &control)?.is_none());
    assert_eq!(
        store
            .set_project_read_policy("factory", update(None, true), false, &control, |_| Ok(()))
            .err()
            .ok_or("Expected apply refusal")?
            .code,
        ErrorCode::ApplyRequired
    );
    assert!(
        store
            .set_project_read_policy("factory", update(None, true), true, &control, |_| Err(
                CraftError::new(ErrorCode::OutputBudgetTooSmall, "Fixture preflight failed.")
            ))
            .is_err()
    );
    assert!(store.project_read_policy("factory", &control)?.is_none());
    let first =
        store.set_project_read_policy("factory", update(None, true), true, &control, |_| Ok(()))?;
    assert_eq!(first.continuation_site, "https://craft-test.atlassian.net/");
    assert_eq!(
        ProfileStore::open(&files.root, &control)?.project_read_policy("factory", &control)?,
        Some(first.clone())
    );
    assert_eq!(
        store
            .set_project_read_policy("factory", update(None, true), true, &control, |_| Ok(()))
            .err()
            .ok_or("Expected stale write")?
            .code,
        ErrorCode::VersionConflict
    );
    let disabled =
        store.set_project_read_policy("factory", update(Some(1), false), true, &control, |_| {
            Ok(())
        })?;
    assert_eq!(disabled.revision, 2);
    assert_eq!(
        store
            .verify_project_read_policy(&first, &control)
            .err()
            .ok_or("Expected revocation")?
            .code,
        ErrorCode::PolicyDenied
    );
    store.set_project_read_policy("factory", update(Some(2), true), true, &control, |_| Ok(()))?;
    assert_eq!(
        store
            .verify_project_read_policy(&first, &control)
            .err()
            .ok_or("Expected changed revision")?
            .code,
        ErrorCode::VersionConflict
    );
    Ok(())
}

#[test]
fn competing_read_configuration_updates_keep_one_revision_winner() -> TestResult {
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
        let mut workers = Vec::new();
        for (index, store) in stores.into_iter().enumerate() {
            let barrier = barrier.clone();
            let control = control.clone();
            workers.push(scope.spawn(move || {
                barrier.wait();
                store.set_project_read_policy(
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
            .map(|worker| worker.join().map_err(|_| "Configuration worker panicked"))
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
    Ok(())
}

#[test]
fn invalid_targets_sites_actors_and_corrupt_record_bindings_fail_closed() -> TestResult {
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    for field in ["tenant", "kind", "id", "actor", "site"] {
        let mut bad = update(None, true);
        match field {
            "tenant" => bad.target.tenant = "11111111-1111-2222-3333-444444444444".into(),
            "kind" => bad.target.resource_type = "project".into(),
            "id" => bad.target.id = "other".into(),
            "actor" => bad.expected_actor = "anonymous".into(),
            _ => bad.continuation_site = "https://evil.invalid".into(),
        };
        assert!(
            store
                .set_project_read_policy("factory", bad, true, &control, |_| Ok(()))
                .is_err()
        );
    }
    for (name, provider, mode) in [
        ("delegated", "jira", AuthMode::Delegated),
        ("confluence", "confluence", AuthMode::Service),
    ] {
        store.create(
            &ProfileInput {
                name: name.into(),
                provider: provider.into(),
                deployment: "cloud".into(),
                auth_mode: mode,
                grant: None,
                tenant: Some(target().tenant),
                defaults: BTreeMap::new(),
            },
            &control,
            |_| Ok(()),
        )?;
        assert!(
            store
                .set_project_read_policy(name, update(None, true), true, &control, |_| Ok(()))
                .is_err()
        );
    }
    let first =
        store.set_project_read_policy("factory", update(None, true), true, &control, |_| Ok(()))?;
    let raw = files.raw()?;
    for field in [
        "installation",
        "profile",
        "grant",
        "revision",
        "target",
        "extra",
        "site",
    ] {
        let mut value = serde_json::to_value(&first)?;
        match field {
            "installation" => value["installation"] = json!("ins_00000000000000000000000000000000"),
            "profile" => value["profile"] = json!("other"),
            "grant" => value["grant"]["id"] = json!("gr_00000000000000000000000000000000"),
            "revision" => value["revision"] = json!(2),
            "target" => value["target"]["id"] = json!("other"),
            "site" => value["continuation_site"] = json!("https://evil.invalid"),
            _ => value["extra"] = json!("private-sentinel"),
        };
        raw.execute(
            "UPDATE project_read_policies SET payload=?1",
            [serde_json::to_string(&value)?],
        )?;
        let error = store
            .project_read_policy("factory", &control)
            .err()
            .ok_or("Expected corrupt record")?;
        assert!(!serde_json::to_string(&error)?.contains("private-sentinel"));
    }
    let mut bytes = serde_json::to_string(&first)?;
    bytes.insert_str(1, "\"format\":1,");
    raw.execute("UPDATE project_read_policies SET payload=?1", [bytes])?;
    assert!(store.project_read_policy("factory", &control).is_err());
    Ok(())
}

#[test]
fn read_configuration_migration_preserves_existing_cursors_policy_and_journal_atomically()
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
    drop(store);
    let raw = files.raw()?;
    raw.execute_batch(
        "DROP TABLE IF EXISTS provider_policies; DROP TABLE IF EXISTS confluence_admin_policies; DROP TABLE IF EXISTS review_policies; DROP TABLE confluence_write_policies; DROP TABLE confluence_read_policies; DROP TABLE issue_recovery_policies; DROP TABLE issue_mutation_policies; DROP TABLE field_read_policies; DROP TABLE issue_read_policies; DROP TABLE project_read_policies; PRAGMA user_version=4;",
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
    assert!(migrated.project_read_policy("factory", &control)?.is_none());
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
        14
    );
    drop(migrated);
    raw.execute_batch("DROP TABLE IF EXISTS provider_policies; DROP TABLE IF EXISTS confluence_admin_policies; DROP TABLE IF EXISTS review_policies; DROP TABLE confluence_write_policies; DROP TABLE confluence_read_policies; DROP TABLE issue_recovery_policies; DROP TABLE issue_mutation_policies; DROP TABLE field_read_policies; DROP TABLE issue_read_policies; DROP TABLE project_read_policies; DROP TABLE provider_cursors; DROP TABLE create_policies; DROP TABLE operation_records; CREATE TABLE project_read_policies(foreign_data TEXT); PRAGMA user_version=1;")?;
    assert!(ProfileStore::open(&files.root, &control).is_err());
    assert_eq!(
        raw.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?,
        1
    );
    assert_eq!(raw.query_row("SELECT count(*) FROM sqlite_schema WHERE name IN ('operation_records','create_policies','provider_cursors')",[],|row|row.get::<_,i64>(0))?,0);
    assert_eq!(
        raw.query_row("SELECT count(*) FROM profiles", [], |row| row
            .get::<_, i64>(0))?,
        2
    );
    let other = Files::new()?;
    drop(other.setup()?);
    other.raw()?.execute_batch("CREATE TRIGGER unexpected_read_policy AFTER INSERT ON project_read_policies BEGIN SELECT 1; END;")?;
    assert!(ProfileStore::open(&other.root, &control).is_err());
    Ok(())
}
