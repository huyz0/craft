use super::*;
use craft_cli::planning_review::application::{
    ApplyArgs, Consistency, ExecutionData, apply_native,
};
use craft_core::control::{CancellationToken, InvocationControl};
use craft_core::storage::OperationJournal;
use craft_runtime::confluence_write_policy::{
    ConfluenceWriteOperation, ConfluenceWritePolicyUpdate,
};
use craft_runtime::control::SystemClock;
use craft_runtime::issue_mutation_policy::{IssueMutationOperation, IssueMutationPolicyUpdate};
use craft_runtime::journal::NativeJournal;

const WRITE_SCOPES: &str = "read:confluence-user read:space:confluence read:page:confluence read:comment:confluence write:page:confluence read:jira-user read:jira-work write:jira-work";
fn invocation() -> Result<InvocationControl, CraftError> {
    InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        120_000,
    )
}
fn configure_writes(prepared: &Prepared) -> TestResult {
    prepared.setup.store.set_confluence_write_policy(
        "factory",
        ConfluenceWritePolicyUpdate {
            target: ResourceRef {
                resource_type: "space".into(),
                id: "100".into(),
                ..target()
            },
            expected_actor: "actor-one".into(),
            enabled: true,
            operations: vec![ConfluenceWriteOperation::Create],
            expected_revision: None,
        },
        true,
        &invocation()?,
        |_| Ok(()),
    )?;
    prepared.setup.store.set_issue_mutation_policy(
        "jira-review",
        IssueMutationPolicyUpdate {
            target: ResourceRef {
                provider: "jira".into(),
                resource_type: "project".into(),
                id: "10000".into(),
                ..target()
            },
            expected_actor: "actor-one".into(),
            enabled: true,
            operations: vec![IssueMutationOperation::Link],
            expected_revision: None,
        },
        true,
        &invocation()?,
        |_| Ok(()),
    )?;
    Ok(())
}
fn preview(prepared: &Prepared) -> Result<String, Box<dyn std::error::Error>> {
    let replies = verification_reads(prepared, "approved")?;
    let count = replies.len();
    let product = TlsServer::start(Reply::Sequence(replies))?;
    let issuer = super::super::issuer(WRITE_SCOPES)?;
    let fixture = prepared
        .setup
        .fixture(&issuer, &product, prepared.clock.clone())?;
    let artifacts = NativeArtifacts::open(
        &prepared.setup.root.join("reviews"),
        32 * 1024 * 1024,
        &invocation()?,
    )?;
    let (data, error) = craft_cli::planning_review::tickets::preview_native(
        VerifyArgs {
            review_id: "ledger-review".into(),
            apply: true,
        },
        "factory",
        "application-preview",
        BindingEnvironment {
            store: &prepared.setup.store,
            credentials: &prepared.credentials,
            client: fixture.client(),
            tls: TlsPolicy::AdditionalCaBundle(product.root()),
            control: invocation()?,
        },
        &artifacts,
    );
    assert!(error.is_none(), "{error:?}");
    assert_eq!(product.finish()?.len(), count);
    issuer.finish()?;
    Ok(data.verification_artifact.ok_or("preview artifact")?.name)
}
fn issue_reads(id: &str, lost: bool) -> Result<Vec<Reply>, Box<dyn std::error::Error>> {
    let mut replies = vec![prep::jira_identity()];
    replies.extend(prep::metadata("prepared", false)?.into_iter().take(3));
    replies.push(if lost {
        Reply::Disconnect
    } else {
        Reply::json(
            201,
            &serde_json::to_vec(&json!({"id":id,"key":format!("PAY-{id}")}))?,
            &[],
        )
    });
    Ok(replies)
}
fn link_reads(lost: bool) -> Result<Vec<Reply>, serde_json::Error> {
    let types = json!({"issueLinkTypes":[{"id":"10000","name":"Blocks","inward":"is blocked by","outward":"blocks"}]});
    let outward = json!({"id":"11002","fields":{"project":{"id":"10000"}}});
    let inward = json!({"id":"11001","fields":{"project":{"id":"10000"}}});
    let permissions = json!({"permissions":{"BROWSE_PROJECTS":{"key":"BROWSE_PROJECTS","havePermission":true},"LINK_ISSUES":{"key":"LINK_ISSUES","havePermission":true}}});
    let mut replies = vec![prep::jira_identity()];
    for row in [
        types.clone(),
        outward.clone(),
        inward.clone(),
        permissions.clone(),
        permissions,
        outward,
        inward,
        types,
    ] {
        replies.push(reply(row)?);
    }
    replies.push(if lost {
        Reply::Disconnect
    } else {
        Reply::json(201, b"", &[])
    });
    Ok(replies)
}
fn page_reads(prepared: &Prepared, refused: bool) -> Result<Vec<Reply>, serde_json::Error> {
    let mut replies = vec![identity()];
    if refused {
        replies.push(Reply::json(403, b"{}", &[]));
    } else {
        replies.push(reply(space())?);
        replies.push(reply(prepared.plan.clone())?);
        replies.push(Reply::json(200,br#"{"id":"501","spaceId":"100","title":"Approved ticket results","status":"current","version":{"number":1}}"#,&[]));
    }
    Ok(replies)
}
fn args(preview: &str) -> ApplyArgs {
    ApplyArgs {
        run_id: "approved-delivery".into(),
        preview_artifact: preview.into(),
        result_title: "Approved ticket results".into(),
        consistency: Some(Consistency::RevalidateEachBatch),
        apply: true,
    }
}
type RunObservation = (
    ExecutionData,
    Option<CraftError>,
    Vec<tls_fixture::Observed>,
);
fn execute(
    prepared: &Prepared,
    preview: &str,
    resume: bool,
    request: &str,
    replies: Vec<Reply>,
    artifacts: &dyn ArtifactStore,
) -> Result<RunObservation, Box<dyn std::error::Error>> {
    let count = replies.len();
    let product = TlsServer::start(Reply::Sequence(replies))?;
    let issuer = super::super::issuer(WRITE_SCOPES)?;
    let fixture = prepared
        .setup
        .fixture(&issuer, &product, prepared.clock.clone())?;
    let control = invocation()?;
    let journal = NativeJournal::open(&prepared.setup.store, &control)?;
    let client = fixture.client().for_invocation(control.clone())?;
    let (data, error) = apply_native(
        args(preview),
        resume,
        "factory",
        request,
        BindingEnvironment {
            store: &prepared.setup.store,
            credentials: &prepared.credentials,
            client: &client,
            tls: TlsPolicy::AdditionalCaBundle(product.root()),
            control,
        },
        artifacts,
        &journal,
    );
    let requests = product.finish()?;
    assert_eq!(requests.len(), count, "{error:?}");
    issuer.finish()?;
    Ok((data, error, requests))
}
fn full_reads(prepared: &Prepared, mode: &str) -> Result<Vec<Reply>, Box<dyn std::error::Error>> {
    let mode = match mode {
        "lost_inspection" => "lost",
        "link_inspection" => "link_lost",
        other => other,
    };
    let mut replies = verification_reads(prepared, "approved")?;
    replies.extend(issue_reads("11001", mode == "lost")?);
    if mode == "lost" || mode == "outcome" {
        return Ok(replies);
    }
    replies.extend(verification_reads(
        prepared,
        if mode == "source" {
            "stale_binding"
        } else {
            "approved"
        },
    )?);
    if mode == "source" {
        return Ok(replies);
    }
    replies.extend(issue_reads("11002", false)?);
    replies.extend(link_reads(mode == "link_lost")?);
    if mode == "link_lost" {
        return Ok(replies);
    }
    replies.extend(page_reads(prepared, mode == "page")?);
    Ok(replies)
}
#[test]
fn reviewed_ticket_run_creates_approved_issues_link_and_result_page_once() -> TestResult {
    let prepared = prepared("inward")?;
    configure_writes(&prepared)?;
    let preview = preview(&prepared)?;
    prepared.clock.advance(Duration::from_secs(3600))?;
    let artifacts = NativeArtifacts::open(
        &prepared.setup.root.join("reviews"),
        32 * 1024 * 1024,
        &invocation()?,
    )?;
    let (data, error, requests) = execute(
        &prepared,
        &preview,
        false,
        "apply-tickets",
        full_reads(&prepared, "success")?,
        &artifacts,
    )?;
    assert!(error.is_none(), "{error:?}");
    assert!(data.complete);
    assert!(data.resources_complete);
    assert_eq!(data.known_issues.len(), 2);
    assert_eq!(data.links_confirmed, 1);
    assert_eq!(
        data.result_page.as_ref().map(|p| p.id.as_str()),
        Some("501")
    );
    assert!(data.result_artifact.is_some());
    let posts: Vec<_> = requests.iter().filter(|r| r.method == "POST").collect();
    assert_eq!(posts.len(), 4);
    assert_eq!(
        serde_json::from_slice::<Value>(&posts[0].body)?["fields"]["summary"],
        "Define contract"
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&posts[1].body)?["fields"]["summary"],
        "Implement contract"
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&posts[2].body)?,
        json!({"type":{"id":"10000"},"inwardIssue":{"id":"11001"},"outwardIssue":{"id":"11002"}})
    );
    let result_page: Value = serde_json::from_slice(&posts[3].body)?;
    assert_eq!(result_page["parentId"], "101");
    assert!(
        result_page["body"]["value"]
            .as_str()
            .ok_or("result body")?
            .contains("11001")
    );
    let artifact = NativeArtifacts::open(
        &prepared.setup.root.join("reviews"),
        32 * 1024 * 1024,
        &invocation()?,
    )?;
    let (again, error, requests) = execute(
        &prepared,
        &preview,
        true,
        "resume-complete",
        vec![],
        &artifact,
    )?;
    assert!(error.is_none(), "{error:?}");
    assert!(again.complete);
    assert_eq!(again.result_artifact, data.result_artifact);
    assert!(requests.is_empty());
    prepared.setup.no_secrets()?;
    Ok(())
}

struct FailingPublication<'a> {
    store: &'a dyn ArtifactStore,
    target: &'a str,
    armed: std::sync::atomic::AtomicBool,
}
#[test]
fn retained_confirmed_completion_restores_resume_without_recreating_issue() -> TestResult {
    use craft_cli::planning_review::application::recovery::{RecordArgs, record_native};
    let prepared = prepared("inward")?;
    configure_writes(&prepared)?;
    let preview = preview(&prepared)?;
    prepared.clock.advance(Duration::from_secs(3600))?;
    let artifacts = NativeArtifacts::open(
        &prepared.setup.root.join("reviews"),
        32 * 1024 * 1024,
        &invocation()?,
    )?;
    let faulty = FailingPublication {
        store: &artifacts,
        target: "outcome",
        armed: std::sync::atomic::AtomicBool::new(true),
    };
    let (data, error, requests) = execute(
        &prepared,
        &preview,
        false,
        "retain-completion",
        full_reads(&prepared, "outcome")?,
        &faulty,
    )?;
    assert!(error.is_some());
    assert_eq!(requests.iter().filter(|r| r.method == "POST").count(), 1);
    let envelope = data.current_attempt.ok_or("retained envelope")?;
    let import = RecordArgs {
        run: args(&preview),
        task_id: data
            .known_issues
            .first()
            .ok_or("known issue")?
            .task_id
            .clone(),
        completion_file: "retained-completion.json".into(),
    };
    let recovered = NativeArtifacts::open(
        &prepared.setup.root.join("reviews"),
        32 * 1024 * 1024,
        &invocation()?,
    )?;
    let journal = NativeJournal::open(&prepared.setup.store, &invocation()?)?;
    let before = journal.get(data.current_operation_id.as_deref().ok_or("operation")?)?;
    let (restored, error) = record_native(&import, "factory", &envelope, &recovered, &journal);
    assert!(error.is_none(), "{error:?}");
    assert_eq!(restored.artifact_write_state, RequestState::Confirmed);
    assert!(restored.outcome_artifact.is_some());
    assert!(!restored.journal_updated && !restored.replay_authorized);
    assert_eq!(
        journal.get(restored.operation_id.as_deref().ok_or("operation")?)?,
        before
    );
    let (again, error) = record_native(&import, "factory", &envelope, &recovered, &journal);
    assert!(error.is_none(), "{error:?}");
    assert_eq!(again.outcome_artifact, restored.outcome_artifact);
    let mut altered = envelope.clone();
    altered.request_id = "different-completion".into();
    let (_, error) = record_native(&import, "factory", &altered, &recovered, &journal);
    assert!(error.is_some());
    let mut replies = verification_reads(&prepared, "approved")?;
    replies.extend(issue_reads("11002", false)?);
    replies.extend(link_reads(false)?);
    replies.extend(page_reads(&prepared, false)?);
    let resumed = NativeArtifacts::open(
        &prepared.setup.root.join("reviews"),
        32 * 1024 * 1024,
        &invocation()?,
    )?;
    let (finished, error, requests) = execute(
        &prepared,
        &preview,
        true,
        "resume-restored-completion",
        replies,
        &resumed,
    )?;
    assert!(error.is_none(), "{error:?}");
    assert!(finished.complete);
    assert_eq!(finished.known_issues.len(), 2);
    assert_eq!(requests.iter().filter(|r| r.method == "POST").count(), 3);
    prepared.setup.no_secrets()?;
    Ok(())
}

#[test]
fn newly_reviewed_reuse_completes_without_creating_the_selected_issue_or_resolving_its_origin()
-> TestResult {
    use craft_cli::issue_create::host::{CreatePolicy, PreparedCreate};
    use craft_core::contracts::{CanonicalRequest, Selection};
    use craft_core::storage::{JournalStep, SubmissionState};
    let prepared = super::prepared_with_reuse("inward", true)?;
    configure_writes(&prepared)?;
    let preview = preview(&prepared)?;
    let control = invocation()?;
    let policy = prepared
        .setup
        .store
        .create_policy("jira-review", &control)?
        .ok_or("policy")?;
    let old = PreparedCreate::prepare(
        CanonicalRequest {
            command: "jira.issue.create".into(),
            target: policy.target.clone(),
            input: json!({"issue_type_id":"10001","summary":"Prior attempted task"}),
            selection: Selection::default(),
            preconditions: None,
        },
        "prior-unknown-create",
        true,
        "prior-request",
        CreatePolicy {
            enabled: true,
            target: policy.target,
            grant: policy.grant,
            expected_actor: "actor-one".into(),
        },
    )?;
    let journal = NativeJournal::open(&prepared.setup.store, &control)?;
    let mut origin = journal.begin(old.binding())?;
    for state in [
        SubmissionState::Prepared,
        SubmissionState::SendStarted,
        SubmissionState::Unknown,
    ] {
        let revision = origin.revision;
        origin.revision += 1;
        origin.steps.insert(
            "create-issue".into(),
            JournalStep {
                request_digest: if state == SubmissionState::Prepared {
                    None
                } else {
                    Some(origin.binding.request_digest.clone())
                },
                state,
                confirmed_resources: vec![],
                job: None,
            },
        );
        journal.compare_and_swap(revision, &origin)?;
    }
    prepared.clock.advance(Duration::from_secs(3600))?;
    let mut replies = verification_reads(&prepared, "approved")?;
    replies.extend(verification_reads(&prepared, "approved")?);
    replies.extend(issue_reads("11002", false)?);
    replies.extend(link_reads(false)?);
    replies.extend(page_reads(&prepared, false)?);
    let artifacts = NativeArtifacts::open(
        &prepared.setup.root.join("reviews"),
        32 * 1024 * 1024,
        &invocation()?,
    )?;
    let (data, error, requests) = execute(
        &prepared,
        &preview,
        false,
        "apply-reviewed-reuse",
        replies,
        &artifacts,
    )?;
    assert!(error.is_none(), "{error:?}");
    assert!(data.complete && data.resources_complete);
    assert_eq!(data.known_issues.len(), 2);
    assert_eq!(data.reused_issues.len(), 1);
    assert_eq!(data.reused_issues[0].issue.id, "11001");
    assert!(data.reused_issues[0].origin_attribution_unresolved);
    assert_eq!(data.reused_issues[0].origin, origin);
    let journal = NativeJournal::open(&prepared.setup.store, &invocation()?)?;
    assert_eq!(journal.get("prior-unknown-create")?, Some(origin));
    assert_eq!(requests.iter().filter(|r| r.method == "POST").count(), 3);
    let page = requests
        .iter()
        .find(|r| r.method == "POST" && r.target.contains("/wiki/api/v2/pages"))
        .ok_or("result page")?;
    assert!(String::from_utf8_lossy(&page.body).contains("Explicitly reused issues"));
    let resumed = NativeArtifacts::open(
        &prepared.setup.root.join("reviews"),
        32 * 1024 * 1024,
        &invocation()?,
    )?;
    let (again, error, requests) = execute(
        &prepared,
        &preview,
        true,
        "resume-reviewed-reuse",
        vec![],
        &resumed,
    )?;
    assert!(error.is_none(), "{error:?}");
    assert!(again.complete && requests.is_empty());
    assert_eq!(again.reused_issues.len(), 1);
    assert!(again.reused_issues[0].origin_attribution_unresolved);
    let journal = NativeJournal::open(&prepared.setup.store, &invocation()?)?;
    let mut changed = journal.get("prior-unknown-create")?.ok_or("origin")?;
    let revision = changed.revision;
    changed.revision += 1;
    let step = changed.steps.get_mut("create-issue").ok_or("step")?;
    step.state = SubmissionState::Confirmed;
    step.confirmed_resources = vec![ResourceRef {
        id: "11999".into(),
        ..data.reused_issues[0].issue.clone()
    }];
    journal.compare_and_swap(revision, &changed)?;
    let resumed = NativeArtifacts::open(
        &prepared.setup.root.join("reviews"),
        32 * 1024 * 1024,
        &invocation()?,
    )?;
    let (stopped, error, requests) = execute(
        &prepared,
        &preview,
        true,
        "reuse-origin-conflict",
        vec![],
        &resumed,
    )?;
    assert_eq!(
        error.as_ref().map(|e| e.code),
        Some(ErrorCode::VersionConflict)
    );
    assert!(!stopped.complete && requests.is_empty());
    assert_eq!(stopped.known_issues[0].issue.id, "11001");
    assert_eq!(stopped.unresolved_record, Some(changed));
    assert!(stopped.reused_issues[0].origin_attribution_unresolved);
    prepared.setup.no_secrets()?;
    Ok(())
}
struct FailedWriter {
    inner: Box<dyn ArtifactWriter>,
    fail: bool,
}
impl Write for FailedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.inner.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}
impl ArtifactWriter for FailedWriter {
    fn finish(self: Box<Self>) -> Result<ArtifactReceipt, StoreError> {
        if self.fail {
            Err(StoreError::Unavailable)
        } else {
            self.inner.finish()
        }
    }
}
impl ArtifactStore for FailingPublication<'_> {
    fn begin(&self, spec: ArtifactSpec) -> Result<Box<dyn ArtifactWriter>, StoreError> {
        let selected = if self.target == "result" {
            spec.name.starts_with("ticket-results-")
        } else {
            spec.name.ends_with("-outcome.json")
        };
        let fail = selected && self.armed.swap(false, Ordering::SeqCst);
        Ok(Box::new(FailedWriter {
            inner: self.store.begin(spec)?,
            fail,
        }))
    }
    fn receipt(&self, name: &str) -> Result<Option<ArtifactReceipt>, StoreError> {
        self.store.receipt(name)
    }
    fn read(&self, name: &str, max: u64) -> Result<Option<ArtifactBlob>, StoreError> {
        self.store.read(name, max)
    }
}
fn failure_case(mode: &str) -> TestResult {
    let prepared = prepared("inward")?;
    configure_writes(&prepared)?;
    let preview = preview(&prepared)?;
    prepared.clock.advance(Duration::from_secs(3600))?;
    let artifacts = NativeArtifacts::open(
        &prepared.setup.root.join("reviews"),
        32 * 1024 * 1024,
        &invocation()?,
    )?;
    let faults = FailingPublication {
        store: &artifacts,
        target: mode,
        armed: std::sync::atomic::AtomicBool::new(true),
    };
    let inspection = FailingInspection {
        store: &artifacts,
        journal: NativeJournal::open(&prepared.setup.store, &invocation()?)?,
        run: Arc::new(std::sync::Mutex::new(None)),
        link: mode == "link_inspection",
        fired: std::sync::atomic::AtomicBool::new(false),
    };
    let store: &dyn ArtifactStore = if matches!(mode, "result" | "outcome") {
        &faults
    } else if matches!(mode, "lost_inspection" | "link_inspection") {
        &inspection
    } else {
        &artifacts
    };
    let (data, error, requests) = execute(
        &prepared,
        &preview,
        false,
        "apply-failure",
        full_reads(&prepared, mode)?,
        store,
    )?;
    assert!(error.is_some(), "{mode}");
    assert!(!data.complete);
    if matches!(mode, "lost_inspection" | "link_inspection") {
        assert!(inspection.fired.load(Ordering::SeqCst));
        assert_eq!(
            data.stop_error.as_ref().map(|e| e.code),
            Some(ErrorCode::Internal)
        );
        assert_eq!(
            data.current_attempt
                .as_ref()
                .and_then(|e| e.data.as_ref())
                .ok_or("unknown native attempt")?["receipt"]["request_state"],
            "sent_unknown"
        );
    }
    assert!(data.progress_artifact.is_some());
    if mode == "lost" {
        inspect_saved_issue(&prepared, &data)?;
    }
    let expected_issues = match mode {
        "lost" | "lost_inspection" => 0,
        "source" | "outcome" => 1,
        _ => 2,
    };
    assert_eq!(
        data.known_issues.len(),
        expected_issues,
        "{mode}: {error:?}"
    );
    assert_eq!(
        requests.iter().filter(|r| r.method == "POST").count(),
        if mode == "result" {
            4
        } else if matches!(mode, "page" | "link_lost" | "link_inspection") {
            3
        } else {
            1
        }
    );
    if mode == "result" {
        assert!(data.resources_complete);
        assert!(data.result_page.is_some());
        assert!(data.result_artifact.is_none());
        assert!(data.artifact_error.is_some());
    }
    if mode == "page" {
        assert!(data.resources_complete);
        assert!(data.result_page.is_none());
        assert_eq!(data.links_confirmed, 1);
    }
    if mode == "outcome" {
        assert_eq!(
            data.current_attempt
                .as_ref()
                .and_then(|e| e.data.as_ref())
                .ok_or("native completion")?["receipt"]["request_state"],
            "confirmed"
        );
    }
    if matches!(
        mode,
        "lost" | "link_lost" | "lost_inspection" | "link_inspection"
    ) {
        assert_eq!(
            error.as_ref().ok_or("unknown error")?.details.write_state,
            RequestState::SentUnknown
        );
    }
    if matches!(mode, "result" | "page") {
        prepared.setup.store.set_review_policy(
            "factory",
            ReviewPolicyUpdate {
                target: ResourceRef {
                    resource_type: "space".into(),
                    id: "100".into(),
                    ..target()
                },
                expected_actor: "actor-one".into(),
                reviewers: vec!["human-one".into()],
                excluded_actors: vec![],
                quorum: 1,
                enabled: false,
                expected_revision: Some(1),
            },
            true,
            &invocation()?,
            |_| Ok(()),
        )?;
    }
    prepared.clock.advance(Duration::from_secs(3600))?;
    let resumed_artifacts = NativeArtifacts::open(
        &prepared.setup.root.join("reviews"),
        32 * 1024 * 1024,
        &invocation()?,
    )?;
    let replies = if mode == "page" {
        page_reads(&prepared, false)?
    } else if mode == "source" {
        verification_reads(&prepared, "stale_binding")?
    } else {
        vec![]
    };
    let (resumed, error, requests) = execute(
        &prepared,
        &preview,
        true,
        "resume-failure",
        replies,
        &resumed_artifacts,
    )?;
    if matches!(mode, "result" | "page") {
        assert!(error.is_none(), "{mode}: {error:?}");
        assert!(resumed.complete);
        assert_eq!(resumed.known_issues.len(), 2);
        assert_eq!(resumed.links_confirmed, 1);
        assert!(resumed.result_artifact.is_some());
    } else {
        assert!(error.is_some());
        assert!(!resumed.complete);
        assert_eq!(resumed.known_issues.len(), expected_issues);
    }
    assert!(
        requests
            .iter()
            .all(|r| r.method != "POST" || r.target.contains("/wiki/api/v2/pages"))
    );
    assert_eq!(
        requests.iter().filter(|r| r.method == "POST").count(),
        usize::from(mode == "page")
    );
    prepared.setup.no_secrets()?;
    Ok(())
}

fn inspect_saved_issue(prepared: &Prepared, data: &ExecutionData) -> TestResult {
    use craft_cli::issue_recovery::command::{OperationReconcileArgs, PreparedInvocation};
    use craft_cli::profiles::InvocationOptions;
    use craft_runtime::issue_recovery_policy::{IssueRecoveryOperation, IssueRecoveryPolicyUpdate};

    let control = invocation()?;
    let operation = data.current_operation_id.as_ref().ok_or("operation")?;
    let attempt = data
        .current_attempt
        .as_ref()
        .and_then(|v| v.data.as_ref())
        .ok_or("attempt")?;
    assert_eq!(attempt["saved_request"]["write_state"], "confirmed");
    let sources = NativeArtifacts::open(
        &prepared.setup.store.path().join("requests"),
        2_097_152,
        &control,
    )?;
    let source = sources
        .read(
            attempt["saved_request"]["name"]
                .as_str()
                .ok_or("source name")?,
            2_097_152,
        )?
        .ok_or("source")?;
    let saved: Value = serde_json::from_slice(&source.bytes)?;
    let journal = NativeJournal::open(&prepared.setup.store, &control)?;
    let before = journal.get(operation)?.ok_or("journal")?;
    assert_eq!(saved["binding"], json!(before.binding));
    assert_eq!(saved["original"]["command"], "jira.issue.create");
    assert!(saved["original"]["input"].get("apply").is_none());
    prepared.setup.store.set_issue_recovery_policy(
        "jira-review",
        IssueRecoveryPolicyUpdate {
            target: before.binding.target.clone(),
            expected_actor: "actor-one".into(),
            enabled: true,
            operations: vec![IssueRecoveryOperation::Create],
            expected_revision: None,
        },
        true,
        &control,
        |_| Ok(()),
    )?;
    let recovery = PreparedInvocation::prepare(
        OperationReconcileArgs {
            operation_id: operation.clone(),
            original: None,
            saved: true,
            candidate_issue_ids: vec![],
        },
        &[],
        &InvocationOptions {
            state_root: Some(prepared.setup.store.path()),
            profile: Some("jira-review"),
            request_id: "saved-controller-investigation",
            max_bytes: 262_144,
        },
        &prepared.setup.store,
        &control,
    )?;
    let product = TlsServer::start(Reply::Sequence(vec![]))?;
    let issuer = super::super::issuer(WRITE_SCOPES)?;
    let fixture = prepared
        .setup
        .fixture(&issuer, &product, prepared.clock.clone())?;
    let locked = prepared.credentials.lock(recovery.grant(), &control)?;
    let inspected = recovery.execute_cached(
        &prepared.setup.store,
        fixture.client(),
        &locked,
        &journal,
        TlsPolicy::AdditionalCaBundle(product.root()),
        control,
    );
    assert!(inspected.envelope.ok, "{:?}", inspected.envelope.error);
    let inspected = inspected.envelope.data.ok_or("investigation")?;
    assert_eq!(inspected["assessment"], "unresolved");
    assert_eq!(inspected["replay_authorized"], false);
    assert_eq!(journal.get(operation)?, Some(before));
    assert!(product.finish()?.is_empty());
    assert!(issuer.finish()?.is_empty());
    Ok(())
}

#[test]
fn unavailable_controller_source_stops_before_issue_native_access() -> TestResult {
    let prepared = prepared("inward")?;
    configure_writes(&prepared)?;
    let preview = preview(&prepared)?;
    prepared.clock.advance(Duration::from_secs(3600))?;
    std::fs::write(
        prepared.setup.store.path().join("requests"),
        b"blocked source directory",
    )?;
    let artifacts = NativeArtifacts::open(
        &prepared.setup.root.join("reviews"),
        32 * 1024 * 1024,
        &invocation()?,
    )?;
    let (data, error, requests) = execute(
        &prepared,
        &preview,
        false,
        "blocked-controller-source",
        verification_reads(&prepared, "approved")?,
        &artifacts,
    )?;
    assert!(error.is_some());
    assert!(!data.complete);
    assert!(data.known_issues.is_empty());
    assert!(data.progress_artifact.is_some());
    assert!(requests.iter().all(|r| r.method == "GET"));
    let attempt = data
        .current_attempt
        .and_then(|v| v.data)
        .ok_or("source failure")?;
    assert_eq!(attempt["receipt"]["request_state"], "not_sent");
    assert!(attempt["saved_request"].is_null());
    let journal = NativeJournal::open(&prepared.setup.store, &invocation()?)?;
    assert!(
        journal
            .get(data.current_operation_id.as_deref().ok_or("operation")?)?
            .is_none()
    );
    prepared.setup.no_secrets()?;
    Ok(())
}
#[test]
fn result_artifact_failure_recovers_without_any_native_request() -> TestResult {
    failure_case("result")
}
#[test]
fn result_page_preclaim_failure_recovers_without_revalidating_or_recreating_tickets() -> TestResult
{
    failure_case("page")
}
#[test]
fn lost_ticket_creation_never_replays_during_resume() -> TestResult {
    failure_case("lost")
}
#[test]
fn confirmed_ticket_with_lost_completion_artifact_keeps_mapping_and_stops_resume() -> TestResult {
    failure_case("outcome")
}
#[test]
fn changed_reviewed_source_stops_later_batches_and_preserves_confirmed_ticket() -> TestResult {
    failure_case("source")
}
#[test]
fn lost_dependency_link_stops_resume_without_recreating_confirmed_issues() -> TestResult {
    failure_case("link_lost")
}

struct FailingInspection<'a> {
    store: &'a dyn ArtifactStore,
    journal: NativeJournal,
    run: Arc<std::sync::Mutex<Option<String>>>,
    link: bool,
    fired: std::sync::atomic::AtomicBool,
}
struct CapturedRun {
    inner: Box<dyn ArtifactWriter>,
    bytes: Vec<u8>,
    capture: bool,
    run: Arc<std::sync::Mutex<Option<String>>>,
}
impl Write for CapturedRun {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(bytes)?;
        if self.capture {
            self.bytes.extend_from_slice(&bytes[..n]);
        }
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}
impl ArtifactWriter for CapturedRun {
    fn finish(self: Box<Self>) -> Result<ArtifactReceipt, StoreError> {
        let receipt = self.inner.finish()?;
        if self.capture {
            let value: Value =
                serde_json::from_slice(&self.bytes).map_err(|_| StoreError::Invalid)?;
            let digest =
                craft_core::input::canonical_digest(&value).map_err(|_| StoreError::Invalid)?;
            *self.run.lock().map_err(|_| StoreError::Unavailable)? = Some(digest);
        }
        Ok(receipt)
    }
}
impl ArtifactStore for FailingInspection<'_> {
    fn begin(&self, spec: ArtifactSpec) -> Result<Box<dyn ArtifactWriter>, StoreError> {
        let capture = spec.media_type == "application/vnd.craft.ticket-run+json";
        Ok(Box::new(CapturedRun {
            inner: self.store.begin(spec)?,
            bytes: vec![],
            capture,
            run: self.run.clone(),
        }))
    }
    fn receipt(&self, name: &str) -> Result<Option<ArtifactReceipt>, StoreError> {
        self.store.receipt(name)
    }
    fn read(&self, name: &str, max: u64) -> Result<Option<ArtifactBlob>, StoreError> {
        if name.ends_with("-intent.json") && !self.fired.load(Ordering::SeqCst) {
            let run = self
                .run
                .lock()
                .map_err(|_| StoreError::Unavailable)?
                .clone();
            if let Some(run) = run {
                let key = if self.link {
                    "link:implement-contract:define-contract"
                } else {
                    "issue:define-contract"
                };
                let operation = format!(
                    "tickets-{}",
                    &craft_core::input::canonical_digest(&json!({"run":run,"step":key}))
                        .map_err(|_| StoreError::Invalid)?[7..]
                );
                if self
                    .journal
                    .get(&operation)?
                    .is_some_and(|r| r.steps.values().any(|s| s.state.requires_reconciliation()))
                    && !self.fired.swap(true, Ordering::SeqCst)
                {
                    return Err(StoreError::Unavailable);
                }
            }
        }
        self.store.read(name, max)
    }
}
#[test]
fn lost_issue_plus_post_attempt_inspection_failure_keeps_unknown_state() -> TestResult {
    failure_case("lost_inspection")
}
#[test]
fn lost_link_plus_post_attempt_inspection_failure_keeps_unknown_state() -> TestResult {
    failure_case("link_inspection")
}
