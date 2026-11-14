//! Trusted, non-secret profile metadata. OAuth material belongs exclusively in native keyrings.

use std::ffi::OsString;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use craft_core::contracts::{GrantRef, RequestState};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::input::parse_object;
use craft_core::profiles::{
    GrantMetadata, Profile, ProfileInput, ProfileSummary, RepositoryHints, validate_profile_name,
};
use craft_platform::PrivateRoot;
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Row, Transaction, TransactionBehavior, params,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

const APPLICATION_ID: i64 = 0x4352_4654;
const SCHEMA_VERSION: i64 = 12;
const MAX_PAYLOAD_BYTES: usize = 65_536;
const MAX_PROFILES: i64 = 1_000;
const NATIVE_PAGE_SIZE: usize = 101;
const DATABASE: &str = "state.sqlite3";
const OPERATION_TABLE: &str = "CREATE TABLE operation_records (operation_id TEXT PRIMARY KEY COLLATE BINARY, grant_id TEXT NOT NULL REFERENCES grants(id), revision INTEGER NOT NULL CHECK(revision>=0), payload TEXT NOT NULL CHECK(length(CAST(payload AS BLOB))<=1048576))";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeBackend {
    WindowsCredentialManager,
    MacosKeychain,
    LinuxSecretService,
}

impl NativeBackend {
    fn current() -> Result<Self, CraftError> {
        if cfg!(target_os = "windows") {
            Ok(Self::WindowsCredentialManager)
        } else if cfg!(target_os = "macos") {
            Ok(Self::MacosKeychain)
        } else if cfg!(target_os = "linux") {
            Ok(Self::LinuxSecretService)
        } else {
            Err(CraftError::new(
                ErrorCode::UnsupportedCapability,
                "Native state is unsupported on this OS.",
            ))
        }
    }
}

/// Backend selection is recorded explicitly; this metadata does not prove vault availability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserSettings {
    pub schema_version: u32,
    pub installation_id: String,
    pub credential_backend: NativeBackend,
}

impl UserSettings {
    fn validate(&self) -> Result<(), CraftError> {
        if self.schema_version != 1
            || !is_opaque_id(&self.installation_id, "ins_")
            || self.credential_backend != NativeBackend::current()?
        {
            return Err(invalid_state());
        }
        Ok(())
    }
}

/// The environment comes from the trusted launcher, never a repository configuration file.
pub fn resolve_state_root(
    explicit: Option<&Path>,
    environment: impl Fn(&str) -> Option<OsString>,
) -> Result<PathBuf, CraftError> {
    let selected = if let Some(path) = explicit {
        path.to_path_buf()
    } else if let Some(path) = environment("CRAFT_STATE_ROOT") {
        PathBuf::from(path)
    } else if cfg!(target_os = "windows") {
        PathBuf::from(environment("LOCALAPPDATA").ok_or_else(missing_home)?).join("Craft")
    } else if cfg!(target_os = "macos") {
        PathBuf::from(environment("HOME").ok_or_else(missing_home)?)
            .join("Library/Application Support/Craft")
    } else if let Some(path) = environment("XDG_STATE_HOME") {
        PathBuf::from(path).join("craft")
    } else {
        PathBuf::from(environment("HOME").ok_or_else(missing_home)?).join(".local/state/craft")
    };
    if !selected.is_absolute() || selected.to_str().is_none() {
        return Err(CraftError::field(
            "/state_root",
            "expected_absolute_native_state_path",
        ));
    }
    Ok(selected)
}

fn missing_home() -> CraftError {
    CraftError::new(
        ErrorCode::InputInvalid,
        "No native user state location exists; supply --state-root.",
    )
}

pub(super) fn invalid_state() -> CraftError {
    let mut error = CraftError::new(
        ErrorCode::InputInvalid,
        "Local state has an unsupported schema or invalid configuration.",
    );
    error.next_action =
        "Inspect the selected state root and restore valid state; do not replace it automatically."
            .into();
    error
}

pub(super) fn state_error(error: rusqlite::Error) -> CraftError {
    let code = match error.sqlite_error_code() {
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
            ErrorCode::VersionConflict
        }
        _ => ErrorCode::Internal,
    };
    let mut error = CraftError::new(code, "Cannot read or update trusted local state.");
    error.next_action = "Check state permissions and concurrent invocations; inspect profiles before retrying a write.".into();
    error
}

fn path_error(_: std::io::Error) -> CraftError {
    let mut error = CraftError::new(
        ErrorCode::PermissionDenied,
        "State path is inaccessible, linked or not private.",
    );
    error.next_action = "Use an absolute private state root owned by the current OS user.".into();
    error
}

pub(super) fn is_opaque_id(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|suffix| {
        suffix.len() == 32
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

pub(super) fn random_id(prefix: &str) -> Result<String, CraftError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| {
        CraftError::new(ErrorCode::Internal, "Native random source is unavailable.")
    })?;
    let mut value = String::from(prefix);
    use std::fmt::Write;
    for byte in bytes {
        write!(&mut value, "{byte:02x}")
            .map_err(|_| CraftError::new(ErrorCode::Internal, "Cannot encode local identity."))?;
    }
    Ok(value)
}

fn encode<T: Serialize>(value: &T) -> Result<String, CraftError> {
    let payload = serde_json::to_string(value).map_err(|_| invalid_state())?;
    if payload.len() > MAX_PAYLOAD_BYTES {
        return Err(CraftError::field("/profile", "profile_state_too_large"));
    }
    Ok(payload)
}

fn decode<T: DeserializeOwned>(row: &Row<'_>, column: usize) -> rusqlite::Result<T> {
    // Inspect the borrowed SQLite value before allocating a bounded typed payload.
    let payload = row
        .get_ref(column)?
        .as_str()
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    if payload.len() > MAX_PAYLOAD_BYTES {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let value = parse_object(payload.as_bytes()).map_err(|_| rusqlite::Error::InvalidQuery)?;
    serde_json::from_value(value).map_err(|_| rusqlite::Error::InvalidQuery)
}

fn checked_profile(row: &Row<'_>) -> rusqlite::Result<Profile> {
    let profile: Profile = decode(row, 0)?;
    let grant: GrantMetadata = decode(row, 1)?;
    let key = row
        .get_ref(2)?
        .as_str()
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let grant_key = row
        .get_ref(3)?
        .as_str()
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    profile
        .validate()
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    grant
        .validate()
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let input = ProfileInput {
        name: profile.name.clone(),
        provider: profile.provider.clone(),
        deployment: profile.deployment.clone(),
        auth_mode: profile.auth_mode,
        grant: Some(profile.grant.clone()),
        tenant: profile.tenant.clone(),
        defaults: profile.defaults.clone(),
    };
    if profile.name != key
        || profile.grant != grant.grant
        || grant_key != grant.grant.id
        || profile.auth_mode != grant.auth_mode
        || !is_opaque_id(&grant.grant.id, "gr_")
        || input
            .authority()
            .map_err(|_| rusqlite::Error::InvalidQuery)?
            != grant.authority
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(profile)
}

/// Repeats one idempotent connection statement while another connection holds the lock.
/// Opening uses these before any transaction, and a concurrent writer must delay them
/// within the invocation budget rather than fail them as unreadable state.
fn wait_busy<T>(
    control: &InvocationControl,
    mut statement: impl FnMut() -> rusqlite::Result<T>,
) -> Result<T, CraftError> {
    loop {
        control.check()?;
        match statement() {
            Ok(value) => return Ok(value),
            Err(error)
                if matches!(
                    error.sqlite_error_code(),
                    Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
                ) =>
            {
                std::thread::sleep(control.remaining()?.min(Duration::from_millis(10)));
            }
            Err(error) => return Err(state_error(error)),
        }
    }
}

pub(super) fn begin<'a>(
    connection: &'a Connection,
    control: &InvocationControl,
) -> Result<Transaction<'a>, CraftError> {
    loop {
        control.check()?;
        match Transaction::new_unchecked(connection, TransactionBehavior::Immediate) {
            Ok(transaction) => return Ok(transaction),
            Err(error)
                if matches!(
                    error.sqlite_error_code(),
                    Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
                ) =>
            {
                std::thread::sleep(control.remaining()?.min(Duration::from_millis(10)));
            }
            Err(error) => return Err(state_error(error)),
        }
    }
}

pub(super) fn commit(
    transaction: Transaction<'_>,
    control: Option<&InvocationControl>,
) -> Result<(), CraftError> {
    loop {
        if let Some(control) = control {
            control.check()?;
        }
        match transaction.execute_batch("COMMIT") {
            Ok(()) => return Ok(()),
            Err(error)
                if matches!(
                    error.sqlite_error_code(),
                    Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
                ) && !transaction.is_autocommit() =>
            {
                // SQLite leaves a busy COMMIT active. Retry only that commit, never its body.
                let Some(control) = control else {
                    return Err(state_error(error));
                };
                std::thread::sleep(control.remaining()?.min(Duration::from_millis(10)));
            }
            Err(_) => return Err(commit_outcome_unknown()),
        }
    }
}

/// A failed commit acknowledgment cannot establish whether durable state changed.
pub fn commit_outcome_unknown() -> CraftError {
    let mut error = CraftError::new(
        ErrorCode::WriteOutcomeUnknown,
        "Local state commit outcome is unknown.",
    );
    error.details.write_state = RequestState::SentUnknown;
    error.next_action =
        "Read the named profile and active selection from the same state root before retrying."
            .into();
    error
}

pub struct ProfileStore {
    // Fields drop in declaration order: SQLite must close before its pinned files and root.
    pub(super) connection: Connection,
    _database: File,
    _journal: File,
    root: PrivateRoot,
    settings: UserSettings,
}

impl ProfileStore {
    /// Opens only the fixed private database and journal, refusing foreign schema/version state.
    pub fn open(path: &Path, control: &InvocationControl) -> Result<Self, CraftError> {
        control.check()?;
        let root = PrivateRoot::open(path).map_err(path_error)?;
        for suffix in ["-wal", "-shm"] {
            match std::fs::symlink_metadata(root.path().join(format!("{DATABASE}{suffix}"))) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                _ => return Err(invalid_state()),
            }
        }
        let database = root.open_file(DATABASE).map_err(path_error)?;
        if database.metadata().map_err(path_error)?.len() > 67_108_864 {
            return Err(invalid_state());
        }
        let mut header = Vec::new();
        (&database)
            .take(100)
            .read_to_end(&mut header)
            .map_err(path_error)?;
        if !header.is_empty()
            && (header.len() != 100
                || &header[..16] != b"SQLite format 3\0"
                || header[18] != 1
                || header[19] != 1)
        {
            return Err(invalid_state());
        }
        let journal = root
            .open_file("state.sqlite3-journal")
            .map_err(path_error)?;
        let connection = Connection::open_with_flags(
            root.path().join(DATABASE),
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(state_error)?;
        connection
            .busy_timeout(Duration::ZERO)
            .map_err(state_error)?;
        let page_size: i64 = wait_busy(control, || {
            connection.query_row("PRAGMA page_size", [], |row| row.get(0))
        })?;
        if page_size != 4096 {
            return Err(invalid_state());
        }
        wait_busy(control, || {
            connection.execute_batch(
                "PRAGMA trusted_schema=OFF; PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;",
            )
        })?;
        // Persistent rollback journals allow every SQLite path to remain pinned and private.
        let mode: String = wait_busy(control, || {
            connection.query_row("PRAGMA journal_mode=PERSIST", [], |row| row.get(0))
        })?;
        if mode != "persist" {
            return Err(invalid_state());
        }
        wait_busy(control, || {
            connection.execute_batch("PRAGMA journal_size_limit=0; PRAGMA max_page_count=16384;")
        })?;
        let transaction = begin(&connection, control)?;
        let application: i64 = transaction
            .query_row("PRAGMA application_id", [], |row| row.get(0))
            .map_err(state_error)?;
        let mut version: i64 = transaction
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(state_error)?;
        if application == 0 && version == 0 {
            let tables: i64 = transaction
                .query_row("SELECT count(*) FROM sqlite_schema", [], |row| row.get(0))
                .map_err(state_error)?;
            if tables != 0 {
                return Err(invalid_state());
            }
            transaction.execute_batch(
                "CREATE TABLE settings (singleton INTEGER PRIMARY KEY CHECK(singleton=1), payload TEXT NOT NULL CHECK(length(payload)<=65536));
                 CREATE TABLE grants (id TEXT PRIMARY KEY, payload TEXT NOT NULL CHECK(length(payload)<=65536));
                 CREATE TABLE profiles (name TEXT PRIMARY KEY COLLATE BINARY, grant_id TEXT NOT NULL REFERENCES grants(id), payload TEXT NOT NULL CHECK(length(payload)<=65536));
                 CREATE TABLE preferences (singleton INTEGER PRIMARY KEY CHECK(singleton=1), active TEXT NOT NULL REFERENCES profiles(name));
                 CREATE TABLE profile_cursors (handle TEXT PRIMARY KEY, expires INTEGER NOT NULL, payload TEXT NOT NULL CHECK(length(payload)<=131072));
                 PRAGMA application_id=1129465428; PRAGMA user_version=1;"
            ).map_err(state_error)?;
            let settings = UserSettings {
                schema_version: 1,
                installation_id: random_id("ins_")?,
                credential_backend: NativeBackend::current()?,
            };
            transaction
                .execute(
                    "INSERT INTO settings(singleton,payload) VALUES(1,?1)",
                    [encode(&settings)?],
                )
                .map_err(state_error)?;
            version = 1;
        } else if application != APPLICATION_ID || !(1..=SCHEMA_VERSION).contains(&version) {
            return Err(invalid_state());
        }
        if version == 1 {
            transaction
                .execute(OPERATION_TABLE, [])
                .map_err(|_| invalid_state())?;
            transaction
                .pragma_update(None, "user_version", 2)
                .map_err(state_error)?;
            version = 2;
        }
        if version == 2 {
            transaction
                .execute(crate::create_policy::TABLE, [])
                .map_err(|_| invalid_state())?;
            transaction
                .pragma_update(None, "user_version", 3)
                .map_err(state_error)?;
            version = 3;
        }
        if version == 3 {
            transaction
                .execute(crate::pagination::TABLE, [])
                .map_err(|_| invalid_state())?;
            transaction
                .pragma_update(None, "user_version", 4)
                .map_err(state_error)?;
            version = 4;
        }
        if version == 4 {
            transaction
                .execute(crate::project_read_policy::TABLE, [])
                .map_err(|_| invalid_state())?;
            transaction
                .pragma_update(None, "user_version", 5)
                .map_err(state_error)?;
            version = 5;
        }
        if version == 5 {
            transaction
                .execute(crate::issue_read_policy::TABLE, [])
                .map_err(|_| invalid_state())?;
            transaction
                .pragma_update(None, "user_version", 6)
                .map_err(state_error)?;
            version = 6;
        }
        if version == 6 {
            transaction
                .execute(crate::field_read_policy::TABLE, [])
                .map_err(|_| invalid_state())?;
            transaction
                .pragma_update(None, "user_version", 7)
                .map_err(state_error)?;
            version = 7;
        }
        if version == 7 {
            transaction
                .execute(crate::issue_mutation_policy::TABLE, [])
                .map_err(|_| invalid_state())?;
            transaction
                .pragma_update(None, "user_version", 8)
                .map_err(state_error)?;
            version = 8;
        }
        if version == 8 {
            transaction
                .execute(crate::issue_recovery_policy::TABLE, [])
                .map_err(|_| invalid_state())?;
            transaction
                .pragma_update(None, "user_version", 9)
                .map_err(state_error)?;
            version = 9;
        }
        if version == 9 {
            transaction
                .execute(crate::confluence_read_policy::TABLE, [])
                .map_err(|_| invalid_state())?;
            transaction
                .pragma_update(None, "user_version", 10)
                .map_err(state_error)?;
            version = 10;
        }
        if version == 10 {
            transaction
                .execute(crate::confluence_write_policy::TABLE, [])
                .map_err(|_| invalid_state())?;
            transaction
                .pragma_update(None, "user_version", 11)
                .map_err(state_error)?;
            version = 11;
        }
        if version == 11 {
            transaction
                .execute(crate::review_policy::TABLE, [])
                .map_err(|_| invalid_state())?;
            transaction
                .pragma_update(None, "user_version", SCHEMA_VERSION)
                .map_err(state_error)?;
        }
        let schema: String = transaction
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type='table' AND name='operation_records'",
                [],
                |row| row.get(0),
            )
            .map_err(|_| invalid_state())?;
        let triggers: i64 = transaction.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type='trigger' AND tbl_name='operation_records'", [], |row| row.get(0)
        ).map_err(|_| invalid_state())?;
        if schema != OPERATION_TABLE || triggers != 0 {
            return Err(invalid_state());
        }
        let policy_schema: String = transaction
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type='table' AND name='create_policies'",
                [],
                |row| row.get(0),
            )
            .map_err(|_| invalid_state())?;
        let policy_triggers:i64=transaction.query_row("SELECT count(*) FROM sqlite_schema WHERE type='trigger' AND tbl_name='create_policies'",[],|row|row.get(0)).map_err(|_|invalid_state())?;
        if policy_schema != crate::create_policy::TABLE || policy_triggers != 0 {
            return Err(invalid_state());
        }
        let cursor_schema: String = transaction
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type='table' AND name='provider_cursors'",
                [],
                |row| row.get(0),
            )
            .map_err(|_| invalid_state())?;
        let cursor_triggers: i64 = transaction.query_row("SELECT count(*) FROM sqlite_schema WHERE type='trigger' AND tbl_name='provider_cursors'", [], |row| row.get(0)).map_err(|_| invalid_state())?;
        if cursor_schema != crate::pagination::TABLE || cursor_triggers != 0 {
            return Err(invalid_state());
        }
        let read_schema: String = transaction
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type='table' AND name='project_read_policies'",
                [],
                |row| row.get(0),
            )
            .map_err(|_| invalid_state())?;
        let read_triggers:i64=transaction.query_row("SELECT count(*) FROM sqlite_schema WHERE type='trigger' AND tbl_name='project_read_policies'",[],|row|row.get(0)).map_err(|_|invalid_state())?;
        if read_schema != crate::project_read_policy::TABLE || read_triggers != 0 {
            return Err(invalid_state());
        }
        let issue_schema: String = transaction
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type='table' AND name='issue_read_policies'",
                [],
                |row| row.get(0),
            )
            .map_err(|_| invalid_state())?;
        let issue_triggers: i64 = transaction.query_row("SELECT count(*) FROM sqlite_schema WHERE type='trigger' AND tbl_name='issue_read_policies'", [], |row| row.get(0)).map_err(|_| invalid_state())?;
        if issue_schema != crate::issue_read_policy::TABLE || issue_triggers != 0 {
            return Err(invalid_state());
        }
        let field_schema: String = transaction
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type='table' AND name='field_read_policies'",
                [],
                |row| row.get(0),
            )
            .map_err(|_| invalid_state())?;
        let field_triggers: i64 = transaction.query_row("SELECT count(*) FROM sqlite_schema WHERE type='trigger' AND tbl_name='field_read_policies'", [], |row|row.get(0)).map_err(|_|invalid_state())?;
        if field_schema != crate::field_read_policy::TABLE || field_triggers != 0 {
            return Err(invalid_state());
        }
        let mutation_schema:String=transaction.query_row("SELECT sql FROM sqlite_schema WHERE type='table' AND name='issue_mutation_policies'",[],|row|row.get(0)).map_err(|_|invalid_state())?;
        let mutation_triggers:i64=transaction.query_row("SELECT count(*) FROM sqlite_schema WHERE type='trigger' AND tbl_name='issue_mutation_policies'",[],|row|row.get(0)).map_err(|_|invalid_state())?;
        if mutation_schema != crate::issue_mutation_policy::TABLE || mutation_triggers != 0 {
            return Err(invalid_state());
        }
        let recovery_schema: String = transaction
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type='table' AND name='issue_recovery_policies'",
                [],
                |row| row.get(0),
            )
            .map_err(|_| invalid_state())?;
        let recovery_triggers: i64 = transaction.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type='trigger' AND tbl_name='issue_recovery_policies'", [], |row| row.get(0)
        ).map_err(|_| invalid_state())?;
        if recovery_schema != crate::issue_recovery_policy::TABLE || recovery_triggers != 0 {
            return Err(invalid_state());
        }
        let confluence_schema: String = transaction.query_row(
            "SELECT sql FROM sqlite_schema WHERE type='table' AND name='confluence_read_policies'",[],|row|row.get(0)
        ).map_err(|_|invalid_state())?;
        let confluence_triggers:i64=transaction.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type='trigger' AND tbl_name='confluence_read_policies'",[],|row|row.get(0)
        ).map_err(|_|invalid_state())?;
        if confluence_schema != crate::confluence_read_policy::TABLE || confluence_triggers != 0 {
            return Err(invalid_state());
        }
        let write_schema: String = transaction.query_row("SELECT sql FROM sqlite_schema WHERE type='table' AND name='confluence_write_policies'", [], |row| row.get(0)).map_err(|_| invalid_state())?;
        let write_triggers: i64 = transaction.query_row("SELECT count(*) FROM sqlite_schema WHERE type='trigger' AND tbl_name='confluence_write_policies'", [], |row| row.get(0)).map_err(|_| invalid_state())?;
        if write_schema != crate::confluence_write_policy::TABLE || write_triggers != 0 {
            return Err(invalid_state());
        }
        let review_schema: String = transaction
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type='table' AND name='review_policies'",
                [],
                |row| row.get(0),
            )
            .map_err(|_| invalid_state())?;
        let review_triggers: i64 = transaction.query_row("SELECT count(*) FROM sqlite_schema WHERE type='trigger' AND tbl_name='review_policies'", [], |row| row.get(0)).map_err(|_| invalid_state())?;
        if review_schema != crate::review_policy::TABLE || review_triggers != 0 {
            return Err(invalid_state());
        }
        let settings: UserSettings = transaction
            .query_row(
                "SELECT payload FROM settings WHERE singleton=1",
                [],
                |row| decode(row, 0),
            )
            .map_err(|_| invalid_state())?;
        settings.validate()?;
        control.check()?;
        commit(transaction, Some(control))?;
        Ok(Self {
            connection,
            _database: database,
            _journal: journal,
            root,
            settings,
        })
    }

    pub fn path(&self) -> &Path {
        self.root.path()
    }
    pub fn settings(&self) -> &UserSettings {
        &self.settings
    }

    fn read_profile(connection: &Connection, name: &str) -> Result<Option<Profile>, CraftError> {
        validate_profile_name(name)?;
        connection.query_row(
            "SELECT p.payload,g.payload,p.name,p.grant_id FROM profiles p LEFT JOIN grants g ON g.id=p.grant_id WHERE p.name=?1",
            [name],
            checked_profile,
        ).optional().map_err(|_| invalid_state())
    }

    pub fn get(&self, name: &str, control: &InvocationControl) -> Result<Profile, CraftError> {
        control.check()?;
        Self::read_profile(&self.connection, name)?
            .ok_or_else(|| CraftError::field("/name", "profile_not_found"))
    }

    pub fn active(&self, control: &InvocationControl) -> Result<Option<String>, CraftError> {
        control.check()?;
        let active: Option<String> = self
            .connection
            .query_row(
                "SELECT active FROM preferences WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| invalid_state())?;
        if let Some(name) = &active {
            self.get(name, control)?;
        }
        Ok(active)
    }

    /// Returns bounded, validated aliases sharing one issued grant, without reading credentials.
    pub fn shared_profiles(
        &self,
        grant: &GrantRef,
        control: &InvocationControl,
    ) -> Result<Vec<String>, CraftError> {
        control.check()?;
        if !is_opaque_id(&grant.id, "gr_") {
            return Err(CraftError::field(
                "/grant",
                "expected_issued_grant_identity",
            ));
        }
        let mut statement = self.connection.prepare(
            "SELECT p.payload,g.payload,p.name,p.grant_id FROM profiles p LEFT JOIN grants g ON g.id=p.grant_id WHERE p.grant_id=?1 ORDER BY p.name COLLATE BINARY LIMIT 1001"
        ).map_err(|_| invalid_state())?;
        let rows = statement
            .query_map([&grant.id], checked_profile)
            .map_err(|_| invalid_state())?;
        let mut names = Vec::new();
        for row in rows {
            control.check()?;
            let profile = row.map_err(|_| invalid_state())?;
            if profile.grant != *grant || names.len() >= MAX_PROFILES as usize {
                return Err(invalid_state());
            }
            names.push(profile.name);
        }
        Ok(names)
    }

    /// The callback must validate the complete projected response before any mutation commits.
    pub fn create(
        &self,
        input: &ProfileInput,
        control: &InvocationControl,
        before_commit: impl FnOnce(&Profile) -> Result<(), CraftError>,
    ) -> Result<Profile, CraftError> {
        let authority = input.authority()?;
        let grant = match &input.grant {
            Some(grant) if is_opaque_id(&grant.id, "gr_") => grant.clone(),
            Some(_) => {
                return Err(CraftError::field(
                    "/grant",
                    "expected_existing_issued_grant",
                ));
            }
            None => GrantRef {
                id: random_id("gr_")?,
            },
        };
        let profile = input.with_grant(grant.clone())?;
        let payload = encode(&profile)?;
        let transaction = begin(&self.connection, control)?;
        let exists: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM profiles WHERE name=?1)",
                [&input.name],
                |row| row.get(0),
            )
            .map_err(state_error)?;
        if exists {
            return Err(CraftError::new(
                ErrorCode::VersionConflict,
                "Profile already exists; it was not replaced.",
            ));
        }
        let count: i64 = transaction
            .query_row("SELECT count(*) FROM profiles", [], |row| row.get(0))
            .map_err(state_error)?;
        if count >= MAX_PROFILES {
            return Err(CraftError::field("/name", "profile_store_capacity_reached"));
        }
        if input.grant.is_some() {
            let existing: Option<GrantMetadata> = transaction
                .query_row(
                    "SELECT payload FROM grants WHERE id=?1",
                    [&grant.id],
                    |row| decode(row, 0),
                )
                .optional()
                .map_err(|_| invalid_state())?;
            let existing =
                existing.ok_or_else(|| CraftError::field("/grant", "grant_not_found"))?;
            if existing.grant != grant
                || existing.authority != authority
                || existing.auth_mode != profile.auth_mode
            {
                return Err(CraftError::field(
                    "/grant",
                    "shared_grant_mode_or_authority_mismatch",
                ));
            }
        } else {
            let metadata = GrantMetadata {
                grant: grant.clone(),
                authority: authority.into(),
                auth_mode: profile.auth_mode,
            };
            transaction
                .execute(
                    "INSERT INTO grants(id,payload) VALUES(?1,?2)",
                    params![grant.id, encode(&metadata)?],
                )
                .map_err(state_error)?;
        }
        transaction
            .execute(
                "INSERT INTO profiles(name,grant_id,payload) VALUES(?1,?2,?3)",
                params![profile.name, grant.id, payload],
            )
            .map_err(state_error)?;
        before_commit(&profile)?;
        control.check()?;
        commit(transaction, Some(control))?;
        Ok(profile)
    }

    pub fn use_profile(
        &self,
        name: &str,
        control: &InvocationControl,
        before_commit: impl FnOnce(&Profile) -> Result<(), CraftError>,
    ) -> Result<Profile, CraftError> {
        let transaction = begin(&self.connection, control)?;
        let profile = Self::read_profile(&transaction, name)?
            .ok_or_else(|| CraftError::field("/name", "profile_not_found"))?;
        before_commit(&profile)?;
        transaction.execute("INSERT INTO preferences(singleton,active) VALUES(1,?1) ON CONFLICT(singleton) DO UPDATE SET active=excluded.active", [name]).map_err(state_error)?;
        control.check()?;
        commit(transaction, Some(control))?;
        Ok(profile)
    }

    /// Reads one bounded native page in binary name order, without claiming a snapshot traversal.
    pub fn summaries_after(
        &self,
        after: Option<&str>,
        control: &InvocationControl,
    ) -> Result<Vec<ProfileSummary>, CraftError> {
        control.check()?;
        if let Some(name) = after {
            validate_profile_name(name)?;
        }
        let mut statement = self.connection.prepare("SELECT p.payload,g.payload,p.name,p.grant_id FROM profiles p LEFT JOIN grants g ON g.id=p.grant_id WHERE (?1 IS NULL OR p.name>?1) ORDER BY p.name COLLATE BINARY LIMIT ?2").map_err(state_error)?;
        let rows = statement
            .query_map(params![after, NATIVE_PAGE_SIZE as u32], checked_profile)
            .map_err(state_error)?;
        let mut summaries = Vec::new();
        for row in rows {
            control.check()?;
            let profile = row.map_err(|_| invalid_state())?;
            profile.validate().map_err(|_| invalid_state())?;
            summaries.push(ProfileSummary::from(&profile));
        }
        Ok(summaries)
    }
}

/// Reads only this directory's hints. No upward search or security-setting merge is performed.
pub fn load_repository_hints(
    directory: &Path,
    control: &InvocationControl,
) -> Result<RepositoryHints, CraftError> {
    control.check()?;
    let path = directory.join(".craft.json");
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RepositoryHints::default());
        }
        Err(_) => {
            return Err(CraftError::field(
                "/repository_hints",
                "cannot_inspect_repository_hints",
            ));
        }
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(CraftError::field(
            "/repository_hints",
            "expected_regular_hint_file",
        ));
    }
    let file = File::open(path)
        .map_err(|_| CraftError::field("/repository_hints", "cannot_open_repository_hints"))?;
    let bytes = crate::control::read_bounded(file, MAX_PAYLOAD_BYTES, control)?;
    let hints: RepositoryHints = serde_json::from_value(parse_object(&bytes)?)
        .map_err(|_| CraftError::field("/repository_hints", "invalid_repository_hints"))?;
    hints.validate()?;
    Ok(hints)
}
