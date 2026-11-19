use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};

use craft_core::contracts::{AuthMode, GrantRef};
use craft_core::control::{CancellationToken, InvocationControl};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::profiles::ProfileInput;
use craft_platform::PrivateRoot;
use craft_runtime::control::SystemClock;
use craft_runtime::profile_pages::ProfileListRequest;
use craft_runtime::profiles::{ProfileStore, load_repository_hints, resolve_state_root};
use craft_test_support::state::ManualClock;
use rusqlite::{Connection, params};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    parent: PathBuf,
    path: PathBuf,
}

impl Fixture {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        fs::create_dir_all(&parent)?;
        let parent = parent.canonicalize()?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let path = parent.join(format!(
            "profile-state-{}-{stamp}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        Ok(Self { parent, path })
    }
    fn raw(&self) -> rusqlite::Result<Connection> {
        let connection = Connection::open(self.path.join("state.sqlite3"))?;
        connection.execute_batch("PRAGMA journal_mode=PERSIST;")?;
        Ok(connection)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(path) = self.path.canonicalize()
            && path.starts_with(&self.parent)
            && path != self.parent
        {
            let _ = fs::remove_dir_all(path);
        }
    }
}

fn control() -> Result<InvocationControl, CraftError> {
    InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        5_000,
    )
}

fn input(name: &str) -> ProfileInput {
    ProfileInput {
        name: name.into(),
        provider: "jira".into(),
        deployment: "cloud".into(),
        auth_mode: AuthMode::Delegated,
        grant: None,
        tenant: Some("primary".into()),
        defaults: BTreeMap::new(),
    }
}

#[test]
fn private_profiles_and_active_selection_survive_restart_without_credentials()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let control = control()?;
    let store = ProfileStore::open(&fixture.path, &control)?;
    assert!(store.active(&control)?.is_none());
    let profile = store.create(&input("finance"), &control, |_| Ok(()))?;
    assert!(store.active(&control)?.is_none());
    assert_eq!(profile.grant.id.len(), 35);
    assert!(profile.grant.id.starts_with("gr_"));
    let settings = store.settings().clone();
    assert!(settings.installation_id.starts_with("ins_"));
    store.use_profile("finance", &control, |_| Ok(()))?;
    drop(store);
    let reopened = ProfileStore::open(&fixture.path, &control)?;
    assert_eq!(reopened.settings(), &settings);
    assert_eq!(reopened.active(&control)?.as_deref(), Some("finance"));
    assert_eq!(reopened.get("finance", &control)?, profile);
    assert_eq!(reopened.path(), fixture.path);
    assert!(!fixture.path.join("state.sqlite3-wal").exists());
    assert!(!fixture.path.join("state.sqlite3-shm").exists());
    let payload: String = fixture
        .raw()?
        .query_row("SELECT payload FROM grants", [], |row| row.get(0))?;
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&payload)?
            .as_object()
            .ok_or("Expected object")?
            .len(),
        3
    );
    Ok(())
}

#[test]
fn shared_grants_require_existing_issued_identity_and_matching_mode()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let control = control()?;
    let store = ProfileStore::open(&fixture.path, &control)?;
    let jira = store.create(&input("jira-team"), &control, |_| Ok(()))?;
    let mut confluence = input("wiki-team");
    confluence.provider = "confluence".into();
    confluence.grant = Some(jira.grant.clone());
    assert_eq!(
        store.create(&confluence, &control, |_| Ok(()))?.grant,
        jira.grant
    );
    confluence.name = "wrong-mode".into();
    confluence.auth_mode = AuthMode::Service;
    assert!(store.create(&confluence, &control, |_| Ok(())).is_err());
    confluence.auth_mode = AuthMode::Delegated;
    for id in [
        "unissued-private-sentinel",
        "gr_00000000000000000000000000000000",
    ] {
        confluence.grant = Some(GrantRef { id: id.into() });
        let error = store
            .create(&confluence, &control, |_| Ok(()))
            .err()
            .ok_or("Expected grant refusal")?;
        assert!(!serde_json::to_string(&error)?.contains(id));
    }
    let connection = fixture.raw()?;
    assert_eq!(
        connection.query_row("SELECT count(*) FROM grants", [], |row| row
            .get::<_, i64>(0))?,
        1
    );
    assert_eq!(
        connection.query_row("SELECT count(*) FROM profiles", [], |row| row
            .get::<_, i64>(0))?,
        2
    );
    Ok(())
}

#[test]
fn response_preflight_and_cancellation_roll_back_profile_grant_and_selection()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let control = control()?;
    let store = ProfileStore::open(&fixture.path, &control)?;
    let reject = |_: &craft_core::profiles::Profile| {
        Err(CraftError::new(
            ErrorCode::OutputBudgetTooSmall,
            "Response cannot fit.",
        ))
    };
    assert!(store.create(&input("refused"), &control, reject).is_err());
    assert_eq!(
        fixture
            .raw()?
            .query_row("SELECT count(*) FROM grants", [], |row| row
                .get::<_, i64>(0))?,
        0
    );
    store.create(&input("first"), &control, |_| Ok(()))?;
    store.create(&input("second"), &control, |_| Ok(()))?;
    store.use_profile("first", &control, |_| Ok(()))?;
    assert!(
        store
            .use_profile("second", &control, |_| Err(CraftError::new(
                ErrorCode::OutputBudgetTooSmall,
                "Response cannot fit."
            )))
            .is_err()
    );
    assert_eq!(store.active(&control)?.as_deref(), Some("first"));
    let token = CancellationToken::default();
    let cancelled = InvocationControl::new(Arc::new(SystemClock::default()), token.clone(), 5_000)?;
    let result = store.create(&input("cancelled"), &cancelled, |_| {
        token.cancel();
        Ok(())
    });
    assert_eq!(
        result.err().ok_or("Expected cancellation")?.code,
        ErrorCode::Cancelled
    );
    assert!(store.get("cancelled", &control).is_err());
    assert_eq!(
        fixture
            .raw()?
            .query_row("SELECT count(*) FROM grants", [], |row| row
                .get::<_, i64>(0))?,
        2
    );
    Ok(())
}

#[test]
fn competing_creates_do_not_overwrite_or_leave_orphaned_grants()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let control = control()?;
    let first = ProfileStore::open(&fixture.path, &control)?;
    let second = ProfileStore::open(&fixture.path, &control)?;
    let barrier = Arc::new(Barrier::new(2));
    let child_barrier = barrier.clone();
    let child_control = control.clone();
    let child = std::thread::spawn(move || {
        child_barrier.wait();
        first.create(&input("same"), &child_control, |_| Ok(()))
    });
    barrier.wait();
    let result = second.create(&input("same"), &control, |_| Ok(()));
    let child_result = child.join().map_err(|_| "Profile thread panicked")?;
    assert_ne!(result.is_ok(), child_result.is_ok());
    let failure = result
        .err()
        .or_else(|| child_result.err())
        .ok_or("Expected conflict")?;
    assert_eq!(failure.code, ErrorCode::VersionConflict);
    assert_eq!(
        fixture
            .raw()?
            .query_row("SELECT count(*) FROM grants", [], |row| row
                .get::<_, i64>(0))?,
        1
    );
    Ok(())
}

#[test]
fn lock_wait_respects_deadline_and_leaves_no_profile() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let store = ProfileStore::open(&fixture.path, &control()?)?;
    let connection = fixture.raw()?;
    connection.execute_batch("BEGIN IMMEDIATE;")?;
    let bounded = InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        50,
    )?;
    let error = store
        .create(&input("blocked"), &bounded, |_| Ok(()))
        .err()
        .ok_or("Expected deadline")?;
    assert_eq!(error.code, ErrorCode::DeadlineExceeded);
    connection.execute_batch("ROLLBACK;")?;
    assert!(store.get("blocked", &control()?).is_err());
    Ok(())
}

#[test]
fn refuses_foreign_database_future_schema_and_corrupt_security_settings()
-> Result<(), Box<dyn std::error::Error>> {
    let foreign = Fixture::new()?;
    let root = PrivateRoot::open(&foreign.path)?;
    let _pin = root.open_file("state.sqlite3")?;
    let connection = foreign.raw()?;
    connection.execute_batch("CREATE TABLE foreign_data (value TEXT);")?;
    drop(connection);
    assert!(ProfileStore::open(&foreign.path, &control()?).is_err());
    assert_eq!(
        foreign.raw()?.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name='foreign_data'",
            [],
            |row| row.get::<_, i64>(0)
        )?,
        1
    );
    let fixture = Fixture::new()?;
    drop(ProfileStore::open(&fixture.path, &control()?)?);
    fixture.raw()?.execute_batch("PRAGMA user_version=99;")?;
    assert!(ProfileStore::open(&fixture.path, &control()?).is_err());
    assert_eq!(
        fixture
            .raw()?
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?,
        99
    );
    fixture.raw()?.execute_batch("PRAGMA user_version=15;")?;
    for payload in [
        r#"{"schema_version":1,"installation_id":"ins_00000000000000000000000000000000","credential_backend":"file","secret":"private-sentinel"}"#,
        r#"{"schema_version":1,"schema_version":1,"installation_id":"ins_00000000000000000000000000000000","credential_backend":"windows_credential_manager"}"#,
    ] {
        fixture
            .raw()?
            .execute("UPDATE settings SET payload=?1", [payload])?;
        let error = ProfileStore::open(&fixture.path, &control()?)
            .err()
            .ok_or("Expected corrupt settings refusal")?;
        assert!(!serde_json::to_string(&error)?.contains("private-sentinel"));
    }
    Ok(())
}

#[test]
fn list_reads_a_bounded_ordered_page_and_can_continue_after_its_last_name()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let control = control()?;
    let store = ProfileStore::open(&fixture.path, &control)?;
    for index in 0..103 {
        store.create(&input(&format!("team-{index:03}")), &control, |_| Ok(()))?;
    }
    let first = store.summaries_after(None, &control)?;
    assert_eq!(first.len(), 101);
    assert_eq!(first[0].name, "team-000");
    let last = first.last().ok_or("Expected summaries")?;
    let next = store.summaries_after(Some(&last.name), &control)?;
    assert_eq!(
        next.iter()
            .map(|profile| profile.name.as_str())
            .collect::<Vec<_>>(),
        ["team-101", "team-102"]
    );
    let mut profile = store.get("team-000", &control)?;
    profile.tenant = Some("foreign".into());
    profile.name = "different-key".into();
    fixture.raw()?.execute(
        "UPDATE profiles SET payload=?1 WHERE name=?2",
        params![serde_json::to_string(&profile)?, "team-000"],
    )?;
    assert!(store.get("team-000", &control).is_err());
    assert!(store.summaries_after(None, &control).is_err());
    Ok(())
}

#[test]
fn immutable_cursors_preserve_remainders_across_restart_and_adjusted_budgets()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let control = control()?;
    let store = ProfileStore::open(&fixture.path, &control)?;
    let clock = ManualClock::new(1_000);
    for name in ["alpha", "bravo", "charlie", "delta", "echo"] {
        store.create(&input(name), &control, |_| Ok(()))?;
    }
    let first = store.list_page(
        ProfileListRequest {
            cursor: None,
            binding: "schema-and-fields",
            limit: 5,
        },
        &clock,
        &control,
        |page| Ok(page.items.len() <= 2),
    )?;
    assert_eq!(
        first
            .items
            .iter()
            .map(|profile| profile.name.as_str())
            .collect::<Vec<_>>(),
        ["alpha", "bravo"]
    );
    assert_eq!(first.page.stop_reason, "byte_limit");
    assert!(!first.cached);
    let cursor = first.page.next_cursor.ok_or("Expected cursor")?;
    assert!(cursor.starts_with("cur_"));
    drop(store);
    let store = ProfileStore::open(&fixture.path, &control)?;
    let next = store.list_page(
        ProfileListRequest {
            cursor: Some(&cursor),
            binding: "schema-and-fields",
            limit: 1,
        },
        &clock,
        &control,
        |_| Ok(true),
    )?;
    assert!(next.cached);
    assert_eq!(next.observed_at, 1_000);
    assert_eq!(next.items[0].name, "charlie");
    let replay = store.list_page(
        ProfileListRequest {
            cursor: Some(&cursor),
            binding: "schema-and-fields",
            limit: 100,
        },
        &clock,
        &control,
        |_| Ok(true),
    )?;
    assert_eq!(
        replay
            .items
            .iter()
            .map(|profile| profile.name.as_str())
            .collect::<Vec<_>>(),
        ["charlie", "delta", "echo"]
    );
    assert!(!replay.page.has_more);
    assert!(replay.page.next_cursor.is_none());
    let error = store
        .list_page(
            ProfileListRequest {
                cursor: Some(&cursor),
                binding: "different-fields",
                limit: 1,
            },
            &clock,
            &control,
            |_| Ok(true),
        )
        .err()
        .ok_or("Expected binding refusal")?;
    assert_eq!(error.details.issues[0].rule, "cursor_binding_mismatch");
    let other = Fixture::new()?;
    let foreign = ProfileStore::open(&other.path, &control)?;
    assert!(
        foreign
            .list_page(
                ProfileListRequest {
                    cursor: Some(&cursor),
                    binding: "schema-and-fields",
                    limit: 1
                },
                &clock,
                &control,
                |_| Ok(true)
            )
            .is_err()
    );
    clock.advance(std::time::Duration::from_secs(901))?;
    let error = store
        .list_page(
            ProfileListRequest {
                cursor: Some(&cursor),
                binding: "schema-and-fields",
                limit: 1,
            },
            &clock,
            &control,
            |_| Ok(true),
        )
        .err()
        .ok_or("Expected expiry")?;
    assert_eq!(error.details.issues[0].rule, "cursor_expired");
    Ok(())
}

#[test]
fn cursor_traversal_crosses_native_pages_without_duplicates_or_gaps()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let control = control()?;
    let store = ProfileStore::open(&fixture.path, &control)?;
    for index in 0..103 {
        store.create(&input(&format!("team-{index:03}")), &control, |_| Ok(()))?;
    }
    let clock = ManualClock::new(1_000);
    let mut cursor = None;
    let mut names = Vec::new();
    for _ in 0..5 {
        let page = store.list_page(
            ProfileListRequest {
                cursor: cursor.as_deref(),
                binding: "schema-and-fields",
                limit: 37,
            },
            &clock,
            &control,
            |_| Ok(true),
        )?;
        names.extend(page.items.into_iter().map(|profile| profile.name));
        cursor = page.page.next_cursor;
        if !page.page.has_more {
            break;
        }
    }
    assert!(cursor.is_none());
    assert_eq!(
        names,
        (0..103)
            .map(|index| format!("team-{index:03}"))
            .collect::<Vec<_>>()
    );
    let error = store
        .list_page(
            ProfileListRequest {
                cursor: None,
                binding: "schema-and-fields",
                limit: 1,
            },
            &clock,
            &control,
            |_| Ok(false),
        )
        .err()
        .ok_or("Expected too-small budget")?;
    assert_eq!(error.code, ErrorCode::OutputBudgetTooSmall);
    Ok(())
}

#[test]
fn repository_hints_are_bounded_closed_and_never_inherited_from_parent()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let _root = PrivateRoot::open(&fixture.path)?;
    fs::write(
        fixture.path.join(".craft.json"),
        br#"{"profile":"finance"}"#,
    )?;
    assert_eq!(
        load_repository_hints(&fixture.path, &control()?)?
            .profile
            .as_deref(),
        Some("finance")
    );
    let child = fixture.path.join("nested");
    fs::create_dir(&child)?;
    assert!(
        load_repository_hints(&child, &control()?)?
            .profile
            .is_none()
    );
    for payload in [
        br#"{"credential_backend":"file","secret":"private-sentinel"}"#.as_slice(),
        br#"{"profile":"one","profile":"two"}"#.as_slice(),
    ] {
        fs::write(fixture.path.join(".craft.json"), payload)?;
        let error = load_repository_hints(&fixture.path, &control()?)
            .err()
            .ok_or("Expected hint refusal")?;
        assert!(!serde_json::to_string(&error)?.contains("private-sentinel"));
    }
    fs::write(fixture.path.join(".craft.json"), vec![b' '; 65_537])?;
    assert!(load_repository_hints(&fixture.path, &control()?).is_err());
    Ok(())
}

#[test]
fn root_precedence_is_explicit_then_launcher_then_native_home_without_cwd_fallback()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let launcher_root = fixture.path.join("launcher");
    let explicit = fixture.path.join("explicit");
    let environment = |key: &str| match key {
        "CRAFT_STATE_ROOT" => Some(launcher_root.clone().into_os_string()),
        "LOCALAPPDATA" | "HOME" | "XDG_STATE_HOME" => Some(fixture.path.clone().into_os_string()),
        _ => None,
    };
    assert_eq!(resolve_state_root(Some(&explicit), environment)?, explicit);
    assert_eq!(resolve_state_root(None, environment)?, launcher_root);
    assert!(resolve_state_root(Some(Path::new("relative")), environment).is_err());
    assert!(resolve_state_root(None, |_| None).is_err());
    assert!(resolve_state_root(None, |_| Some(OsString::new())).is_err());
    let native = resolve_state_root(None, |key| {
        if key == "CRAFT_STATE_ROOT" {
            None
        } else {
            Some(fixture.path.clone().into_os_string())
        }
    })?;
    #[cfg(windows)]
    assert_eq!(native, fixture.path.join("Craft"));
    #[cfg(target_os = "macos")]
    assert_eq!(
        native,
        fixture.path.join("Library/Application Support/Craft")
    );
    #[cfg(target_os = "linux")]
    assert_eq!(native, fixture.path.join("craft"));
    assert!(!fixture.path.exists());
    Ok(())
}
