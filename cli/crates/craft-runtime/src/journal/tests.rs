use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use craft_core::contracts::{
    AuthMode, CanonicalRequest, EffectClass, Envelope, SCHEMA_VERSION, Selection,
};
use craft_core::control::CancellationToken;
use craft_core::execution::{NativeRoute, OperationContext, OperationHandler, RouteSegment};
use craft_core::input::content_digest;
use craft_core::profiles::ProfileInput;
use craft_core::registry::{OperationDescriptor, ReconciliationClass, RetryClass, response_schema};
use craft_test_support::state::FaultPoint;
use serde_json::{Value, json};

use super::*;
use crate::control::SystemClock;
use crate::profiles::random_id;

mod process;

struct Fixture {
    root: PathBuf,
    parent: PathBuf,
}
impl Fixture {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        fs::create_dir_all(&parent)?;
        let parent = parent.canonicalize()?;
        Ok(Self {
            root: parent.join(random_id("journal-fixture-")?),
            parent,
        })
    }
    fn setup(
        &self,
        control: &InvocationControl,
    ) -> Result<(ProfileStore, GrantRef, NativeJournal), Box<dyn std::error::Error>> {
        let profiles = ProfileStore::open(&self.root, control)?;
        let profile = profiles.create(
            &ProfileInput {
                name: "team".into(),
                provider: "jira".into(),
                deployment: "cloud".into(),
                auth_mode: AuthMode::Delegated,
                grant: None,
                tenant: Some("primary".into()),
                defaults: BTreeMap::new(),
            },
            control,
            |_| Ok(()),
        )?;
        let journal = NativeJournal::open(&profiles, control)?;
        Ok((profiles, profile.grant, journal))
    }
    fn raw(&self) -> Result<Connection, rusqlite::Error> {
        let connection = Connection::open(self.root.join("state.sqlite3"))?;
        connection.execute_batch("PRAGMA journal_mode=PERSIST;")?;
        Ok(connection)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(path) = self.root.canonicalize()
            && path.starts_with(&self.parent)
            && path != self.parent
        {
            let _ = fs::remove_dir_all(path);
        }
    }
}
fn control(token: CancellationToken, timeout: u64) -> Result<InvocationControl, CraftError> {
    InvocationControl::new(Arc::new(SystemClock::default()), token, timeout)
}
fn target() -> ResourceRef {
    ResourceRef {
        provider: "jira".into(),
        deployment: "cloud".into(),
        tenant: "primary".into(),
        resource_type: "issue".into(),
        id: "ISSUE-1".into(),
    }
}
fn binding(grant: &GrantRef) -> OperationBinding {
    OperationBinding {
        operation_id: "create-record".into(),
        request_digest: content_digest(b"logical"),
        descriptor_digest: content_digest(b"descriptor"),
        target: target(),
        grant: grant.clone(),
        actor: "account-one".into(),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    title: String,
}
struct Handler;
impl OperationHandler for Handler {
    fn validate(&self, request: &CanonicalRequest) -> Result<(), CraftError> {
        let input: Input = serde_json::from_value(request.input.clone())
            .map_err(|_| CraftError::field("/title", "expected_title"))?;
        if input.title.is_empty() {
            return Err(CraftError::field("/title", "expected_title"));
        }
        Ok(())
    }
    fn execute(
        &self,
        _: &PreparedOperation,
        _: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        Ok(json!("unused"))
    }
}
fn registry() -> Result<Registry, CraftError> {
    let mut registry = Registry::default();
    registry.register_typed_handler::<Input>(OperationDescriptor {
        id: "jira.issue.create".into(), provider: "jira".into(), contract_version: SCHEMA_VERSION.into(), api_revision: "fixture-v1".into(), description: "Create a synthetic fixture issue.".into(), effect: EffectClass::Write,
        input_schema: json!({"type":"object","properties":{"title":{"type":"string"}},"required":["title"],"additionalProperties":false}),
        response_schema: response_schema::<String>()?, example: json!({"title":"example"}),
        response_example: serde_json::to_value(Envelope::success("jira.issue.create", "example".into(), json!("ISSUE-1"))).map_err(|_| CraftError::new(ErrorCode::Internal, "Cannot encode fixture"))?,
        projection: None, scope_alternatives: vec![vec!["write:issue".into()]], permissions: vec!["create".into()], tenant_conditions: Vec::new(),
        routes: vec![NativeRoute {
repeated_query_parameters: Vec::new(), id: "create".into(), method: HttpMethod::Post, path: vec![RouteSegment::Literal("issues".into())], query_parameters: vec![], continuation_parameters: vec![], response_max_bytes: 1024 }],
        retry_class: RetryClass::ReconcileBeforeReplay, reconciliation: ReconciliationClass::KnownResource,
    }, Arc::new(Handler))?;
    Ok(registry)
}
fn request() -> CanonicalRequest {
    CanonicalRequest {
        command: "jira.issue.create".into(),
        target: target(),
        input: json!({"title":"private-body-sentinel"}),
        selection: Selection::default(),
        preconditions: None,
    }
}
fn call() -> NativeCall {
    NativeCall {
        repeated_query: std::collections::BTreeMap::new(),
        route: "create".into(),
        path_parameters: BTreeMap::new(),
        query: BTreeMap::new(),
        body: Some(json!({"title":"private-body-sentinel"})),
    }
}
fn authority(grant: &GrantRef) -> MutationAuthority<'_> {
    MutationAuthority {
        apply: true,
        operation_id: "create-record",
        grant,
        actor: "account-one",
    }
}

#[test]
fn durable_bindings_reopen_and_refuse_changed_payload_target_actor_or_grant()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let control = control(CancellationToken::default(), 10_000)?;
    let (profiles, grant, journal) = fixture.setup(&control)?;
    let initial = journal.begin(&binding(&grant))?;
    assert_eq!(initial.revision, 0);
    let reopened = NativeJournal::open(&profiles, &control)?;
    assert_eq!(reopened.get("create-record")?, Some(initial.clone()));
    for field in ["payload", "target", "actor", "grant", "descriptor"] {
        let mut changed = binding(&grant);
        match field {
            "payload" => changed.request_digest = content_digest(b"other"),
            "target" => changed.target.id = "ISSUE-2".into(),
            "actor" => changed.actor = "account-two".into(),
            "grant" => changed.grant.id = "gr_00000000000000000000000000000000".into(),
            _ => changed.descriptor_digest = content_digest(b"new-descriptor"),
        }
        assert_eq!(reopened.begin(&changed).err(), Some(StoreError::Conflict));
    }
    assert_eq!(journal.get("create-record")?, Some(initial));
    Ok(())
}

#[test]
fn explicit_apply_commits_one_send_claim_and_receipts_survive_cancellation()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let token = CancellationToken::default();
    let invocation = control(token.clone(), 10_000)?;
    let (profiles, grant, journal) = fixture.setup(&invocation)?;
    let registry = registry()?;
    let operation = registry.prepare(request())?;
    let mut authority = authority(&grant);
    authority.apply = false;
    assert_eq!(
        journal
            .claim_send(&registry, &operation, &authority, "create", &call())
            .err()
            .ok_or("Expected apply refusal")?
            .code,
        ErrorCode::ApplyRequired
    );
    assert!(journal.get("create-record")?.is_none());
    authority.apply = true;
    let claim = journal.claim_send(&registry, &operation, &authority, "create", &call())?;
    assert!(
        journal.get("create-record")?.ok_or("Missing intent")?.steps["create"]
            .state
            .requires_reconciliation()
    );
    assert!(
        journal
            .claim_send(&registry, &operation, &authority, "create", &call())
            .is_err()
    );
    token.cancel();
    let outcome = StepOutcome {
        state: SubmissionState::Confirmed,
        confirmed_resources: vec![target()],
        job: None,
    };
    let confirmed = claim.record_outcome(&outcome)?;
    assert_eq!(
        confirmed.steps["create"].confirmed_resources,
        vec![target()]
    );
    let fresh = NativeJournal::open(&profiles, &control(CancellationToken::default(), 10_000)?)?;
    assert_eq!(fresh.get("create-record")?, Some(confirmed));
    assert!(
        fresh
            .claim_send(&registry, &operation, &authority, "create", &call())
            .is_err()
    );
    let data = fs::read(fixture.root.join("state.sqlite3"))?;
    assert!(
        !data
            .windows(b"private-body-sentinel".len())
            .any(|bytes| bytes == b"private-body-sentinel")
    );
    Ok(())
}

#[test]
fn every_local_commit_boundary_preserves_intent_and_never_mints_an_uncertain_claim()
-> Result<(), Box<dyn std::error::Error>> {
    for point in [
        FaultPoint::JournalBeginBeforeCommit,
        FaultPoint::JournalBeginAfterCommit,
        FaultPoint::JournalUpdateBeforeCommit,
        FaultPoint::JournalUpdateAfterCommit,
    ] {
        for boundary in 1..=if matches!(
            point,
            FaultPoint::JournalUpdateBeforeCommit | FaultPoint::JournalUpdateAfterCommit
        ) {
            2
        } else {
            1
        } {
            let fixture = Fixture::new()?;
            let invocation = control(CancellationToken::default(), 10_000)?;
            let (profiles, grant, journal) = fixture.setup(&invocation)?;
            let registry = registry()?;
            let operation = registry.prepare(request())?;
            journal.inner.faults.fail_on(point, boundary)?;
            assert!(
                journal
                    .claim_send(&registry, &operation, &authority(&grant), "create", &call())
                    .is_err()
            );
            assert!(journal.inner.faults.calls(point)? >= boundary);
            let fresh = NativeJournal::open(&profiles, &invocation)?;
            if fresh
                .get("create-record")?
                .and_then(|record| record.steps.get("create").cloned())
                .is_some_and(|step| step.state.requires_reconciliation())
            {
                assert!(
                    fresh
                        .claim_send(&registry, &operation, &authority(&grant), "create", &call())
                        .is_err()
                );
            } else {
                fresh.claim_send(&registry, &operation, &authority(&grant), "create", &call())?;
            }
        }
    }
    Ok(())
}

struct MigrationClock(AtomicUsize);
impl craft_core::storage::Clock for MigrationClock {
    fn unix_millis(&self) -> u64 {
        0
    }
    fn monotonic_millis(&self) -> u64 {
        if self.0.fetch_add(1, Ordering::SeqCst) >= 3 {
            100
        } else {
            0
        }
    }
}

#[test]
fn legacy_migration_is_atomic_and_preserves_settings_profiles_and_preferences()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let invocation = control(CancellationToken::default(), 10_000)?;
    let (profiles, _, journal) = fixture.setup(&invocation)?;
    let settings = profiles.settings().clone();
    profiles.use_profile("team", &invocation, |_| Ok(()))?;
    let profile = profiles.get("team", &invocation)?;
    drop(journal);
    drop(profiles);
    fixture.raw()?.execute_batch(
        "DROP TABLE IF EXISTS confluence_admin_policies; DROP TABLE IF EXISTS review_policies; DROP TABLE confluence_write_policies; DROP TABLE confluence_read_policies; DROP TABLE issue_recovery_policies; DROP TABLE issue_mutation_policies; DROP TABLE field_read_policies; DROP TABLE issue_read_policies; DROP TABLE project_read_policies; DROP TABLE provider_cursors; DROP TABLE create_policies; DROP TABLE operation_records; PRAGMA user_version=1;",
    )?;
    let deadline = InvocationControl::new(
        Arc::new(MigrationClock(AtomicUsize::new(0))),
        CancellationToken::default(),
        50,
    )?;
    assert_eq!(
        ProfileStore::open(&fixture.root, &deadline)
            .err()
            .ok_or("Expected migration cancellation")?
            .code,
        ErrorCode::DeadlineExceeded
    );
    let raw = fixture.raw()?;
    assert_eq!(
        raw.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?,
        1
    );
    assert_eq!(
        raw.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name='operation_records'",
            [],
            |row| row.get::<_, i64>(0)
        )?,
        0
    );
    drop(raw);
    let migrated = ProfileStore::open(&fixture.root, &invocation)?;
    assert_eq!(migrated.settings(), &settings);
    assert_eq!(migrated.get("team", &invocation)?, profile);
    assert_eq!(migrated.active(&invocation)?.as_deref(), Some("team"));
    assert_eq!(
        fixture
            .raw()?
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?,
        13
    );
    drop(migrated);
    fixture.raw()?.execute_batch("DROP TABLE IF EXISTS confluence_admin_policies; DROP TABLE IF EXISTS review_policies; DROP TABLE confluence_write_policies; DROP TABLE confluence_read_policies; DROP TABLE issue_recovery_policies; DROP TABLE issue_mutation_policies; DROP TABLE field_read_policies; DROP TABLE issue_read_policies; DROP TABLE project_read_policies; DROP TABLE provider_cursors; DROP TABLE create_policies; DROP TABLE operation_records; CREATE TABLE operation_records (unrecognized TEXT); PRAGMA user_version=1;")?;
    assert!(ProfileStore::open(&fixture.root, &invocation).is_err());
    assert_eq!(
        fixture
            .raw()?
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?,
        1
    );
    Ok(())
}

#[test]
fn projection_changes_keep_logical_identity_but_native_step_changes_conflict()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let invocation = control(CancellationToken::default(), 10_000)?;
    let (_, grant, journal) = fixture.setup(&invocation)?;
    let registry = registry()?;
    let operation = registry.prepare(request())?;
    let mut selected = request();
    selected.selection.max_bytes = 8_192;
    let changed = registry.prepare(selected)?;
    assert_ne!(operation.digest(), changed.digest());
    assert_eq!(
        operation.mutation_binding("create-record", &grant, "account-one")?,
        changed.mutation_binding("create-record", &grant, "account-one")?
    );
    journal
        .inner
        .faults
        .fail_on(FaultPoint::JournalUpdateBeforeCommit, 2)?;
    assert!(
        journal
            .claim_send(&registry, &operation, &authority(&grant), "create", &call())
            .is_err()
    );
    let mut different = call();
    different.body = Some(json!({"title":"changed-native-input"}));
    assert!(
        journal
            .claim_send(
                &registry,
                &changed,
                &authority(&grant),
                "create",
                &different
            )
            .is_err()
    );
    journal.claim_send(&registry, &changed, &authority(&grant), "create", &call())?;
    Ok(())
}

#[test]
fn receipt_failures_never_erase_ids_regress_terminal_state_or_wait_after_cancellation()
-> Result<(), Box<dyn std::error::Error>> {
    for point in [
        FaultPoint::JournalUpdateBeforeCommit,
        FaultPoint::JournalUpdateAfterCommit,
    ] {
        let fixture = Fixture::new()?;
        let token = CancellationToken::default();
        let invocation = control(token.clone(), 10_000)?;
        let (profiles, grant, journal) = fixture.setup(&invocation)?;
        let registry = registry()?;
        let operation = registry.prepare(request())?;
        let claim =
            journal.claim_send(&registry, &operation, &authority(&grant), "create", &call())?;
        journal.inner.faults.fail_on(point, 1)?;
        let outcome = StepOutcome {
            state: SubmissionState::Confirmed,
            confirmed_resources: vec![target()],
            job: Some(RemoteJob {
                resource: target(),
                state: craft_core::contracts::JobState::Accepted,
            }),
        };
        token.cancel();
        assert!(claim.record_outcome(&outcome).is_err());
        let fresh =
            NativeJournal::open(&profiles, &control(CancellationToken::default(), 10_000)?)?;
        let retained = fresh.get("create-record")?.ok_or("Missing operation")?;
        assert_eq!(
            retained.steps["create"].confirmed_resources.is_empty(),
            point == FaultPoint::JournalUpdateBeforeCommit
        );
        let confirmed = claim.record_outcome(&outcome)?;
        assert_eq!(
            confirmed.steps["create"].confirmed_resources,
            vec![target()]
        );
        assert!(
            claim
                .record_outcome(&StepOutcome {
                    state: SubmissionState::Unknown,
                    confirmed_resources: Vec::new(),
                    job: None
                })
                .is_err()
        );
        let mut erased = confirmed.clone();
        erased.revision += 1;
        erased
            .steps
            .get_mut("create")
            .ok_or("Missing step")?
            .confirmed_resources
            .clear();
        assert_eq!(
            fresh.compare_and_swap(confirmed.revision, &erased),
            Err(StoreError::Conflict)
        );
        let raw = fixture.raw()?;
        raw.execute_batch("BEGIN;")?;
        raw.query_row("SELECT count(*) FROM operation_records", [], |row| {
            row.get::<_, i64>(0)
        })?;
        let started = std::time::Instant::now();
        assert_eq!(
            claim.record_outcome(&outcome).err(),
            Some(StoreError::Unavailable)
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        raw.execute_batch("ROLLBACK;")?;
        assert_eq!(fresh.get("create-record")?, Some(confirmed));
    }
    Ok(())
}

#[test]
fn corrupt_rows_fail_closed_and_oversized_revisions_or_records_never_commit()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let invocation = control(CancellationToken::default(), 10_000)?;
    let (_, grant, journal) = fixture.setup(&invocation)?;
    let initial = journal.begin(&binding(&grant))?;
    let raw = fixture.raw()?;
    let original: String = raw.query_row("SELECT payload FROM operation_records", [], |row| {
        row.get(0)
    })?;
    let record: Value = serde_json::from_str(&original)?;
    let mut variants = Vec::new();
    for (field, value) in [
        ("format", json!(2)),
        (
            "installation",
            json!("ins_00000000000000000000000000000000"),
        ),
        ("unknown", json!(true)),
    ] {
        let mut changed = record.clone();
        changed[field] = value;
        variants.push(serde_json::to_string(&changed)?);
    }
    for (field, value) in [
        ("operation_id", json!("other")),
        ("actor", json!("")),
        ("request_digest", json!("wrong")),
    ] {
        let mut changed = record.clone();
        changed["record"]["binding"][field] = value;
        variants.push(serde_json::to_string(&changed)?);
    }
    let mut changed = record.clone();
    changed["record"]["revision"] = json!(1);
    variants.push(serde_json::to_string(&changed)?);
    variants.push("{\"format\":1,\"format\":1}".into());
    for payload in variants {
        raw.execute("UPDATE operation_records SET payload=?1", [payload.clone()])?;
        assert_eq!(
            journal.get("create-record").err(),
            Some(StoreError::Corrupt)
        );
        assert!(journal.begin(&binding(&grant)).is_err());
        assert_eq!(
            raw.query_row("SELECT payload FROM operation_records", [], |row| row
                .get::<_, String>(0))?,
            payload
        );
    }
    raw.execute("UPDATE operation_records SET payload=?1", [&original])?;
    let mut next = initial.clone();
    next.revision = u64::MAX;
    assert!(journal.compare_and_swap(0, &next).is_err());
    next.revision = 1;
    let many: Vec<_> = (0..1_000)
        .map(|index| {
            let mut resource = target();
            resource.id = format!("{index}-{}", "x".repeat(200));
            resource
        })
        .collect();
    for index in 0..20 {
        next.steps.insert(
            format!("step-{index}"),
            JournalStep {
                request_digest: Some(content_digest(b"request")),
                state: SubmissionState::Confirmed,
                confirmed_resources: many.clone(),
                job: None,
            },
        );
    }
    assert_eq!(journal.encode(&next).err(), Some(StoreError::TooLarge));
    assert_eq!(journal.get("create-record")?, Some(initial));
    Ok(())
}

#[test]
fn busy_commit_waits_without_replaying_its_body_and_rolls_back_on_deadline()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let invocation = control(CancellationToken::default(), 10_000)?;
    let (profiles, grant, journal) = fixture.setup(&invocation)?;
    let initial = journal.begin(&binding(&grant))?;
    let mut prepared = initial.clone();
    prepared.revision = 1;
    prepared
        .steps
        .insert("create".into(), JournalStep::default());
    let reader = fixture.raw()?;
    reader.execute_batch("BEGIN;")?;
    reader.query_row("SELECT count(*) FROM operation_records", [], |row| {
        row.get::<_, i64>(0)
    })?;
    let writer = journal.clone();
    let next = prepared.clone();
    let worker = std::thread::spawn(move || writer.compare_and_swap(0, &next));
    let started = std::time::Instant::now();
    while journal
        .inner
        .faults
        .calls(FaultPoint::JournalUpdateBeforeCommit)?
        == 0
    {
        if started.elapsed() > Duration::from_secs(2) {
            return Err("Writer never reached commit".into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    std::thread::sleep(Duration::from_millis(30));
    assert!(
        !worker.is_finished(),
        "A busy commit did not wait for the reader"
    );
    reader.execute_batch("ROLLBACK;")?;
    worker.join().map_err(|_| "Commit worker panicked")??;
    assert_eq!(
        journal
            .inner
            .faults
            .calls(FaultPoint::JournalUpdateBeforeCommit)?,
        1
    );
    assert_eq!(journal.get("create-record")?, Some(prepared.clone()));
    let mut bounded = NativeJournal::open(&profiles, &invocation)?;
    bounded.control = control(CancellationToken::default(), 40)?;
    let mut changed = prepared.clone();
    changed.revision += 1;
    changed
        .steps
        .get_mut("create")
        .ok_or("Missing step")?
        .request_digest = Some(content_digest(b"native"));
    reader.execute_batch("BEGIN;")?;
    reader.query_row("SELECT count(*) FROM operation_records", [], |row| {
        row.get::<_, i64>(0)
    })?;
    assert_eq!(
        bounded.compare_and_swap(1, &changed),
        Err(StoreError::DeadlineExceeded)
    );
    reader.execute_batch("ROLLBACK;")?;
    assert_eq!(journal.get("create-record")?, Some(prepared));
    Ok(())
}

#[test]
fn journal_claims_gate_loopback_sends_and_lost_responses_require_reconciliation()
-> Result<(), Box<dyn std::error::Error>> {
    use craft_test_support::http::{Exchange, ExpectedRequest, HttpRecorder, Reply};
    use std::io::{Read, Write};
    use std::net::TcpStream;
    let fixture = Fixture::new()?;
    let invocation = control(CancellationToken::default(), 10_000)?;
    let (profiles, grant, journal) = fixture.setup(&invocation)?;
    let registry = registry()?;
    let operation = registry.prepare(request())?;
    let untouched = HttpRecorder::start(Vec::new())?;
    journal
        .inner
        .faults
        .fail_on(FaultPoint::JournalUpdateAfterCommit, 2)?;
    let failed = journal.claim_send(&registry, &operation, &authority(&grant), "create", &call());
    if failed.is_ok() {
        let _ = TcpStream::connect(untouched.address())?;
    }
    assert!(failed.is_err());
    assert_eq!(untouched.request_count(), 0);
    untouched.verify()?;
    let fresh = NativeJournal::open(&profiles, &invocation)?;
    assert!(
        fresh
            .claim_send(&registry, &operation, &authority(&grant), "create", &call())
            .is_err()
    );
    let authority = MutationAuthority {
        operation_id: "another-operation",
        ..authority(&grant)
    };
    let body = serde_json::to_vec(call().body.as_ref().ok_or("Missing request body")?)?;
    let remote = HttpRecorder::start(vec![Exchange {
        request: ExpectedRequest {
            method: "POST".into(),
            target: "/issues".into(),
            headers: BTreeMap::new(),
            body: body.clone(),
        },
        reply: Reply::DisconnectAfterRead,
    }])?;
    let claim = fresh.claim_send(&registry, &operation, &authority, "create", &call())?;
    assert_eq!(
        fresh
            .get("another-operation")?
            .ok_or("Missing pre-send marker")?
            .steps["create"]
            .state,
        SubmissionState::SendStarted
    );
    let mut stream = TcpStream::connect(remote.address())?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    write!(
        stream,
        "POST /issues HTTP/1.1\r\nHost: fixture\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )?;
    stream.write_all(&body)?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    assert!(response.is_empty());
    claim.record_outcome(&StepOutcome {
        state: SubmissionState::Unknown,
        confirmed_resources: Vec::new(),
        job: None,
    })?;
    assert!(
        fresh
            .claim_send(&registry, &operation, &authority, "create", &call())
            .is_err()
    );
    assert_eq!(remote.request_count(), 1);
    assert_eq!(remote.verify()?.len(), 1);
    Ok(())
}
