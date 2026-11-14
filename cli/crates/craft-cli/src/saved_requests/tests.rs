use super::*;
use craft_core::contracts::{AuthMode, GrantRef, Selection};
use craft_core::control::CancellationToken;
use craft_core::profiles::ProfileInput;
use craft_core::storage::{ArtifactBlob, ArtifactReceipt, ArtifactWriter, JournalStep};
use craft_runtime::control::SystemClock;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
type TestResult = Result<(), Box<dyn std::error::Error>>;
struct Files {
    parent: PathBuf,
    root: PathBuf,
}
impl Files {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        std::fs::create_dir_all(&parent)?;
        let parent = parent.canonicalize()?;
        let root = craft_test_support::state::fixture_root(
            &parent,
            "saved-native-request",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos(),
        );
        Ok(Self { parent, root })
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
fn control() -> Result<InvocationControl, CraftError> {
    InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        30_000,
    )
}
fn original() -> Result<CanonicalRequest, serde_json::Error> {
    serde_json::from_value(json!({"command":"jira.issue.create",
        "target":{"provider":"jira","deployment":"cloud","tenant":"00000000-1111-2222-3333-444444444444","resource_type":"project","id":"100"},
        "input":{"issue_type_id":"101","summary":"Retained source"},"selection":Selection::default(),"preconditions":null}))
}
fn setup(
    files: &Files,
    control: &InvocationControl,
) -> Result<(ProfileStore, NativeJournal, GrantRef), Box<dyn std::error::Error>> {
    let store = ProfileStore::open(&files.root, control)?;
    let profile = store.create(
        &ProfileInput {
            name: "factory".into(),
            provider: "jira".into(),
            deployment: "cloud".into(),
            auth_mode: AuthMode::Service,
            grant: None,
            tenant: Some(original()?.target.tenant),
            defaults: Default::default(),
        },
        control,
        |_| Ok(()),
    )?;
    let journal = NativeJournal::open(&store, control)?;
    Ok((store, journal, profile.grant))
}
fn binding(original: &CanonicalRequest, grant: &GrantRef) -> Result<OperationBinding, CraftError> {
    let prepared = if original.command == "jira.issue.create" {
        crate::issue_create::host::prepare_original(original.clone())?
    } else {
        crate::issue_mutation::host::prepare_original(original.clone())?
    };
    prepared.mutation_binding("retained-intent", grant, "actor-one")
}
#[test]
fn later_credential_setup_failure_retains_source_without_native_claim() -> TestResult {
    let files = Files::new()?;
    let control = control()?;
    let (store, journal, grant) = setup(&files, &control)?;
    let original = original()?;
    let host = crate::issue_create::host::PreparedCreate::prepare(
        original.clone(),
        "retained-intent",
        true,
        "retained-setup-failure",
        crate::issue_create::host::CreatePolicy {
            enabled: true,
            target: original.target,
            grant,
            expected_actor: "actor-one".into(),
        },
    )?;
    let (host, source) = host
        .retain_source(&store, &journal, &control)
        .map_err(|_| "source retention failed")?;
    let result = host.reject_retained_source(
        CraftError::new(
            ErrorCode::PolicyDenied,
            "Synthetic credential setup failure.",
        ),
        source,
        &journal,
        &control,
    );
    assert!(!result.envelope.ok);
    let data = result.envelope.data.ok_or("retained receipt")?;
    assert_eq!(data["saved_request"]["write_state"], "confirmed");
    assert_eq!(data["receipt"]["request_state"], "not_sent");
    assert!(data["http_status"].is_null());
    assert!(journal.get("retained-intent")?.is_none());
    assert_eq!(
        load(&store, "retained-intent", &control)?
            .binding
            .operation_id,
        "retained-intent"
    );
    Ok(())
}
#[test]
fn all_supported_issue_action_sources_retain_native_intent_without_cli_controls() -> TestResult {
    let registry = crate::catalog::registry()?;
    for command in [
        "jira.issue.update",
        "jira.transition.apply",
        "jira.issue.assign",
        "jira.issue.unassign",
        "jira.issue.delete",
    ] {
        let files = Files::new()?;
        let c = control()?;
        let (store, journal, grant) = setup(&files, &c)?;
        let mut input = registry.descriptor(command)?.example.clone();
        for field in ["apply", "operation_id", "cloud_id"] {
            input.as_object_mut().ok_or("input")?.remove(field);
        }
        let mut request = original()?;
        request.command = command.into();
        request.target.resource_type = "issue".into();
        request.target.id = input["issue"].as_str().ok_or("issue")?.into();
        request.input = input;
        let bound = binding(&request, &grant)?;
        capture(
            &store,
            &journal,
            &request,
            &bound,
            crate::issue_mutation::route(command)?,
            &c,
        )
        .map_err(|f| f.error)?;
        let saved = load(&store, &bound.operation_id, &c)?;
        assert_eq!(saved.binding, bound);
        assert_eq!(
            serde_json::to_value(saved.original)?,
            serde_json::to_value(request)?
        );
    }
    Ok(())
}
#[test]
fn immutable_source_preserves_first_selection_and_refuses_changed_logical_input() -> TestResult {
    let files = Files::new()?;
    let c = control()?;
    let (store, journal, grant) = setup(&files, &c)?;
    let first = original()?;
    let bound = binding(&first, &grant)?;
    let receipt = capture(&store, &journal, &first, &bound, "create-issue", &c)
        .map_err(|f| f.error)?
        .ok_or("receipt")?;
    assert_eq!(receipt.write_state, RequestState::Confirmed);
    assert!(journal.get(&bound.operation_id)?.is_none());
    let mut projected = first.clone();
    projected.selection.fields = vec!["issue_key".into()];
    projected.selection.max_bytes = 1_048_576;
    assert_eq!(binding(&projected, &grant)?, bound);
    let reused = capture(&store, &journal, &projected, &bound, "create-issue", &c)
        .map_err(|f| f.error)?
        .ok_or("receipt")?;
    assert_eq!(reused.digest, receipt.digest);
    assert_eq!(
        serde_json::to_value(load(&store, &bound.operation_id, &c)?.original)?,
        serde_json::to_value(first)?
    );
    let mut changed = projected;
    changed.input["summary"] = json!("Other intent");
    let error = capture(
        &store,
        &journal,
        &changed,
        &binding(&changed, &grant)?,
        "create-issue",
        &c,
    )
    .err()
    .ok_or("conflict")?;
    assert_eq!(error.error.code, ErrorCode::OperationIdReused);
    assert_eq!(
        load(&store, &bound.operation_id, &c)?.original.input["summary"],
        "Retained source"
    );
    Ok(())
}
struct UnknownStore(NativeArtifacts);
struct UnknownWriter(Box<dyn ArtifactWriter>);
impl Write for UnknownWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}
impl ArtifactWriter for UnknownWriter {
    fn finish(self: Box<Self>) -> Result<ArtifactReceipt, StoreError> {
        self.0.finish()?;
        Err(StoreError::OutcomeUnknown)
    }
}
impl ArtifactStore for UnknownStore {
    fn begin(&self, spec: ArtifactSpec) -> Result<Box<dyn ArtifactWriter>, StoreError> {
        Ok(Box::new(UnknownWriter(self.0.begin(spec)?)))
    }
    fn receipt(&self, name: &str) -> Result<Option<ArtifactReceipt>, StoreError> {
        self.0.receipt(name)
    }
    fn read(&self, name: &str, max: u64) -> Result<Option<ArtifactBlob>, StoreError> {
        self.0.read(name, max)
    }
}
#[test]
fn lost_source_acknowledgment_retains_unknown_receipt_without_creating_a_send_record() -> TestResult
{
    let files = Files::new()?;
    let c = control()?;
    let (store, journal, grant) = setup(&files, &c)?;
    let original = original()?;
    let bound = binding(&original, &grant)?;
    let storage = artifacts(&store, &c)?;
    let failed = capture_using(&journal, &original, &bound, "create-issue", &c, || {
        Ok(Box::new(UnknownStore(storage)))
    })
    .err()
    .ok_or("unknown")?;
    assert_eq!(
        failed.receipt.ok_or("receipt")?.write_state,
        RequestState::SentUnknown
    );
    assert!(journal.get(&bound.operation_id)?.is_none());
    assert_eq!(load(&store, &bound.operation_id, &c)?.binding, bound);
    assert_eq!(
        capture(&store, &journal, &original, &bound, "create-issue", &c)
            .map_err(|f| f.error)?
            .ok_or("receipt")?
            .write_state,
        RequestState::Confirmed
    );
    Ok(())
}
struct HiddenFirstRead {
    inner: NativeArtifacts,
    reads: AtomicUsize,
}
impl ArtifactStore for HiddenFirstRead {
    fn begin(&self, spec: ArtifactSpec) -> Result<Box<dyn ArtifactWriter>, StoreError> {
        self.inner.begin(spec)
    }
    fn receipt(&self, name: &str) -> Result<Option<ArtifactReceipt>, StoreError> {
        self.inner.receipt(name)
    }
    fn read(&self, name: &str, max: u64) -> Result<Option<ArtifactBlob>, StoreError> {
        if self.reads.fetch_add(1, Ordering::SeqCst) == 0 {
            Ok(None)
        } else {
            self.inner.read(name, max)
        }
    }
}
#[test]
fn concurrent_publication_winner_is_read_without_replacing_its_source() -> TestResult {
    let files = Files::new()?;
    let c = control()?;
    let (store, journal, grant) = setup(&files, &c)?;
    let original = original()?;
    let bound = binding(&original, &grant)?;
    let first = capture(&store, &journal, &original, &bound, "create-issue", &c)
        .map_err(|f| f.error)?
        .ok_or("receipt")?;
    let inner = artifacts(&store, &c)?;
    let observed = capture_using(&journal, &original, &bound, "create-issue", &c, || {
        Ok(Box::new(HiddenFirstRead {
            inner,
            reads: AtomicUsize::new(0),
        }))
    })
    .map_err(|f| f.error)?
    .ok_or("receipt")?;
    assert_eq!(first.digest, observed.digest);
    Ok(())
}
#[test]
fn started_journal_is_not_given_a_new_source_or_storage_access() -> TestResult {
    let files = Files::new()?;
    let c = control()?;
    let (store, journal, grant) = setup(&files, &c)?;
    let original = original()?;
    let bound = binding(&original, &grant)?;
    let mut record = journal.begin(&bound)?;
    record.steps.insert(
        "create-issue".into(),
        JournalStep {
            request_digest: Some(content_digest(b"native")),
            state: SubmissionState::Prepared,
            confirmed_resources: vec![],
            job: None,
        },
    );
    record.revision += 1;
    journal.compare_and_swap(record.revision - 1, &record)?;
    record.steps.get_mut("create-issue").ok_or("step")?.state = SubmissionState::SendStarted;
    record.revision += 1;
    journal.compare_and_swap(record.revision - 1, &record)?;
    assert!(
        capture_using(&journal, &original, &bound, "create-issue", &c, || panic!(
            "terminal operation opened source storage"
        ))
        .map_err(|f| f.error)?
        .is_none()
    );
    assert_eq!(journal.get(&bound.operation_id)?.ok_or("record")?, record);
    assert!(!store.path().join("requests").exists());
    Ok(())
}
#[test]
fn source_with_a_recomputed_artifact_digest_cannot_change_native_binding() -> TestResult {
    let files = Files::new()?;
    let c = control()?;
    let (store, _, grant) = setup(&files, &c)?;
    let mut original = original()?;
    let bound = binding(&original, &grant)?;
    original.input["summary"] = json!("Changed after binding");
    let source = SavedRequest {
        format: 1,
        binding: bound.clone(),
        original,
    };
    let storage = artifacts(&store, &c)?;
    let mut writer = storage.begin(ArtifactSpec {
        name: name(&bound.operation_id),
        media_type: "application/json".into(),
        max_bytes: LIMIT,
        source: Some(bound.target),
    })?;
    writer.write_all(&serde_json::to_vec(&source)?)?;
    writer.finish()?;
    assert!(load(&store, &bound.operation_id, &c).is_err());
    Ok(())
}
#[test]
fn source_projection_and_reserved_receipt_capacity_agree() -> TestResult {
    let receipt = SavedReceipt {
        name: name(&"x".repeat(512)),
        digest: content_digest(b"source"),
        write_state: RequestState::SentUnknown,
    };
    assert!(serde_json::to_vec(&json!({"saved_request":receipt}))?.len() < RECEIPT_RESERVE);
    let fields = native_fields::<crate::issue_create::IssueCreateData>(
        crate::issue_create::projection(),
        &["saved_request.write_state".into()],
    )?;
    assert_eq!(fields, crate::issue_create::projection().always_returned);
    assert!(
        native_fields::<crate::issue_create::IssueCreateData>(
            crate::issue_create::projection(),
            &["saved_request.unknown".into()]
        )
        .is_err()
    );
    Ok(())
}

struct ChangedJournal {
    record: Option<JournalRecord>,
    reads: AtomicUsize,
}
impl OperationJournal for ChangedJournal {
    fn get(&self, _: &str) -> Result<Option<JournalRecord>, StoreError> {
        if self.reads.fetch_add(1, Ordering::SeqCst) == 0 {
            Ok(None)
        } else {
            self.record.clone().map(Some).ok_or(StoreError::Unavailable)
        }
    }
    fn begin(&self, _: &OperationBinding) -> Result<JournalRecord, StoreError> {
        panic!("source retention began a journal")
    }
    fn compare_and_swap(&self, _: u64, _: &JournalRecord) -> Result<(), StoreError> {
        panic!("source retention changed a journal")
    }
}
#[test]
fn source_failure_retains_concurrent_unknown_records_and_marks_unreadable_evidence() -> TestResult {
    let files = Files::new()?;
    let c = control()?;
    let (store, _, grant) = setup(&files, &c)?;
    let original = original()?;
    let bound = binding(&original, &grant)?;
    let record = JournalRecord {
        binding: bound.clone(),
        revision: 3,
        steps: std::collections::BTreeMap::from([(
            "create-issue".into(),
            JournalStep {
                request_digest: Some(content_digest(b"native")),
                state: SubmissionState::Unknown,
                confirmed_resources: vec![],
                job: None,
            },
        )]),
    };
    for observed in [Some(record.clone()), None] {
        let journal = ChangedJournal {
            record: observed.clone(),
            reads: AtomicUsize::new(0),
        };
        let failure = capture_using(&journal, &original, &bound, "create-issue", &c, || {
            Err(invalid("injected source failure"))
        })
        .err()
        .ok_or("failure")?;
        assert_eq!(failure.evidence_unavailable, observed.is_none());
        assert_eq!(failure.prior, observed);
    }
    assert!(!store.path().join("requests").exists());
    Ok(())
}
