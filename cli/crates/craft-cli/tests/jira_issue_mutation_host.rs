//! Owned TLS qualification of edit/transition preparation, single send and completion.
#[path = "support/conformance.rs"]
mod conformance;
use conformance::Observed;
#[path = "jira_issue_mutation_host/recovery.rs"]
mod recovery;
#[path = "jira_issue_mutation_host/saved_source.rs"]
mod saved_source;
#[path = "../../craft-runtime/src/transport/tls_fixture.rs"]
mod tls_fixture;
use craft_cli::issue_mutation::{
    self,
    host::{IssueMutationPolicy, MutationPermission, PreparedIssueMutation},
};
use craft_core::contracts::{AuthMode, CanonicalRequest, GrantRef, ResourceRef, Selection};
use craft_core::control::{CancellationToken, InvocationControl};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::profiles::ProfileInput;
use craft_core::storage::{Clock, OperationJournal, SubmissionState};
use craft_provider_jira::issue_context::mutation::actions::{
    ASSIGN_ISSUE, DELETE_ISSUE, UNASSIGN_ISSUE,
};
use craft_provider_jira::issue_context::mutation::{EDIT_ISSUE, TRANSITION_ISSUE};
use craft_runtime::control::SystemClock;
use craft_runtime::journal::NativeJournal;
use craft_runtime::profiles::ProfileStore;
use craft_runtime::synthetic_acceptance::{ACCESS_TOKEN, CLIENT_ID, CLIENT_SECRET, ServiceFixture};
use craft_runtime::transport::TlsPolicy;
use craft_test_support::state::ManualClock;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tls_fixture::{Reply, TlsServer};
type TestResult = Result<(), Box<dyn std::error::Error>>;
const OPERATION: &str = "issue-change";
const SCOPES: &str = "read:jira-user read:jira-work write:jira-work";
fn control(token: CancellationToken) -> Result<InvocationControl, CraftError> {
    InvocationControl::new(Arc::new(SystemClock::default()), token, 30_000)
}
fn target() -> ResourceRef {
    ResourceRef {
        provider: "jira".into(),
        deployment: "cloud".into(),
        tenant: "00000000-1111-2222-3333-444444444444".into(),
        resource_type: "issue".into(),
        id: "10000".into(),
    }
}
fn request(command: &str) -> CanonicalRequest {
    let mut input = json!({"issue":"10000","expected":{"id":"10000","key":"ED-7","project_id":"100","issue_type_id":"101","status_id":"102","updated":"2026-11-10T01:00:00.000+0000"},"changes":{"description":"Reviewed"}});
    if command == TRANSITION_ISSUE {
        input["transition_id"] = json!("31");
        input["expected_status_id"] = json!("103");
    }
    CanonicalRequest {
        command: command.into(),
        target: target(),
        input,
        selection: Selection::default(),
        preconditions: None,
    }
}
fn prepared(
    grant: &GrantRef,
    command: &str,
    actor: &str,
) -> Result<PreparedIssueMutation, CraftError> {
    PreparedIssueMutation::prepare(
        request(command),
        OPERATION,
        true,
        "original-request",
        IssueMutationPolicy {
            enabled: true,
            project: ResourceRef {
                resource_type: "project".into(),
                id: "100".into(),
                ..target()
            },
            grant: grant.clone(),
            expected_actor: actor.into(),
            permissions: vec![MutationPermission::Edit, MutationPermission::Transition],
        },
    )
}
fn identity() -> Reply {
    Reply::json(200, br#"{"accountId":"actor-one","active":true}"#, &[])
}
fn issuer(scopes: &str) -> Result<TlsServer, Box<dyn std::error::Error>> {
    TlsServer::start(Reply::json(
        200,
        &serde_json::to_vec(
            &json!({"access_token":ACCESS_TOKEN,"expires_in":120,"token_type":"Bearer","scope":scopes}),
        )?,
        &[],
    ))
}
fn state() -> Value {
    json!({"id":"10000","key":"ED-7","fields":{"project":{"id":"100"},"issuetype":{"id":"101"},"status":{"id":"102"},"updated":"2026-11-10T01:00:00.000+0000"}})
}
fn reply(value: Value) -> Result<Reply, serde_json::Error> {
    Ok(Reply::json(200, &serde_json::to_vec(&value)?, &[]))
}
fn metadata(command: &str, wrong_project: bool) -> Result<Vec<Reply>, Box<dyn std::error::Error>> {
    let field = |id: &str, required| json!({"key":"descriptive-key","name":id,"required":required,"operations":["set"],"schema":{"type":"string","system":id}});
    let fields = json!({"summary":field("summary",true),"description":field("description",false)});
    let mut state = state();
    if wrong_project {
        state["fields"]["project"]["id"] = json!("999");
    }
    let mut replies = vec![
        reply(state.clone())?,
        reply(if command == EDIT_ISSUE {
            json!({"fields":fields})
        } else {
            json!({"transitions":[{"id":"31","to":{"id":"103"},"hasScreen":true,"fields":fields}]})
        })?,
        reply(state.clone())?,
    ];
    if command == TRANSITION_ISSUE && !wrong_project {
        state["fields"]["summary"] = json!("Human summary");
        replies.push(reply(state)?);
    }
    Ok(replies)
}
struct Files {
    parent: PathBuf,
    root: PathBuf,
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
struct Setup {
    profiles: ProfileStore,
    grant: GrantRef,
    journal: NativeJournal,
    files: Files,
}
impl Setup {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        fs::create_dir_all(&parent)?;
        let parent = parent.canonicalize()?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let files = Files {
            root: craft_test_support::state::fixture_root(
                &parent,
                "composed-issue-mutation",
                stamp,
            ),
            parent,
        };
        let control = control(CancellationToken::default())?;
        let profiles = ProfileStore::open(&files.root, &control)?;
        let profile = profiles.create(
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
        let journal = NativeJournal::open(&profiles, &control)?;
        Ok(Self {
            profiles,
            grant: profile.grant,
            journal,
            files,
        })
    }
    fn fixture(
        &self,
        issuer: &TlsServer,
        product: &TlsServer,
        clock: Arc<dyn Clock>,
    ) -> Result<ServiceFixture, CraftError> {
        ServiceFixture::new(
            &self.profiles,
            issuer.address().port(),
            product.address().port(),
            [issuer.root(), product.root()].concat(),
            control(CancellationToken::default())?,
            clock,
        )
    }
    fn no_plaintext(&self) -> TestResult {
        for item in fs::read_dir(&self.files.root)? {
            let path = item?.path();
            if path.is_file() {
                let bytes = fs::read(path)?;
                for secret in [CLIENT_ID, CLIENT_SECRET, ACCESS_TOKEN] {
                    assert!(
                        !bytes
                            .windows(secret.len())
                            .any(|window| window == secret.as_bytes())
                    );
                }
            }
        }
        Ok(())
    }
}
fn assert_schema(envelope: &craft_core::contracts::Envelope) -> TestResult {
    assert!(
        jsonschema::validator_for(&issue_mutation::schema()?)?
            .is_valid(&serde_json::to_value(envelope)?)
    );
    assert_eq!(envelope.request_id, "original-request");
    Ok(())
}

#[path = "jira_issue_mutation_host/actions.rs"]
mod actions;

#[test]
fn stalled_claimed_mutation_obeys_cancellation_without_retry() -> TestResult {
    let setup = Setup::new()?;
    let issuer = issuer(SCOPES)?;
    let mut replies = vec![identity(), identity()];
    replies.extend(metadata(EDIT_ISSUE, false)?);
    replies.push(Reply::Stall);
    let product = TlsServer::start(Reply::Sequence(replies))?;
    let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1_000)))?;
    let token = CancellationToken::default();
    let control = control(token.clone())?;
    let locked = fixture.lock(&setup.grant, &control)?;
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref()],
        Some("actor-one"),
    )?;
    let counter = product.request_counter();
    let cancel = std::thread::spawn(move || {
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while counter.load(std::sync::atomic::Ordering::SeqCst) < 6
            && std::time::Instant::now() < until
        {
            std::thread::sleep(Duration::from_millis(1));
        }
        token.cancel();
    });
    let result = prepared(&setup.grant, EDIT_ISSUE, "actor-one")?
        .execute_cached(
            fixture.client(),
            &locked,
            &setup.journal,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control,
        )
        .observed();
    cancel.join().map_err(|_| "Cancellation worker panicked")?;
    assert_schema(&result.envelope)?;
    assert!(!result.envelope.ok);
    assert_eq!(
        result.envelope.data.as_ref().ok_or("data")?["receipt"]["request_state"],
        "sent_unknown"
    );
    assert_eq!(product.request_count(), 6);
    assert_eq!(product.connections(), 6);
    assert_eq!(product.finish()?.len(), 6);
    assert_eq!(issuer.finish()?.len(), 1);
    Ok(())
}

#[test]
fn cached_edit_and_transition_send_one_frozen_call_and_preserve_renewal_and_required_values()
-> TestResult {
    for command in [EDIT_ISSUE, TRANSITION_ISSUE] {
        for renew in [false, true] {
            let setup = Setup::new()?;
            let issuer = issuer(SCOPES)?;
            let mut replies = vec![identity(), identity()];
            replies.extend(metadata(command, false)?);
            replies.push(Reply::json(204, b"", &[("X-Request-Id", "native-one")]));
            let product = TlsServer::start(Reply::Sequence(replies))?;
            let clock = Arc::new(ManualClock::new(1_000));
            let fixture = setup.fixture(&issuer, &product, clock.clone())?;
            let control = control(CancellationToken::default())?;
            let locked = fixture.lock(&setup.grant, &control)?;
            fixture.client().provision_grant(
                &locked,
                fixture.service_credentials()?,
                &[target().tenant_ref()],
                Some("actor-one"),
            )?;
            if renew {
                clock.advance(Duration::from_secs(3_600))?;
            }
            let result = prepared(&setup.grant, command, "actor-one")?
                .execute_cached(
                    fixture.client(),
                    &locked,
                    &setup.journal,
                    TlsPolicy::AdditionalCaBundle(product.root()),
                    control.clone(),
                )
                .observed();
            assert_schema(&result.envelope)?;
            assert!(result.envelope.ok, "{:?}", result.envelope.error);
            let data = result.envelope.data.as_ref().ok_or("data")?;
            assert_eq!(data["receipt"]["request_state"], "confirmed");
            assert_eq!(data["receipt"]["confirmed_resources"], json!([target()]));
            assert_eq!(data["persistence"], "recorded");
            assert_eq!(data["credential_cache_update"].is_null(), !renew);
            let saved = setup.journal.get(OPERATION)?.ok_or("journal")?;
            assert_eq!(
                saved.steps[issue_mutation::route(command)?].state,
                SubmissionState::Confirmed
            );
            let repeated = prepared(&setup.grant, command, "actor-one")?
                .execute_cached(
                    fixture.client(),
                    &locked,
                    &setup.journal,
                    TlsPolicy::AdditionalCaBundle(product.root()),
                    control,
                )
                .observed();
            assert_eq!(
                repeated.envelope.error.as_ref().ok_or("error")?.code,
                ErrorCode::OperationIdReused
            );
            assert_eq!(
                repeated.envelope.data.as_ref().ok_or("data")?["prior_record"],
                json!(saved)
            );
            let observed = product.finish()?;
            assert_eq!(observed.len(), if command == EDIT_ISSUE { 6 } else { 7 });
            let last = observed.last().ok_or("request")?;
            assert_eq!(
                last.method,
                if command == EDIT_ISSUE { "PUT" } else { "POST" }
            );
            assert!(last.target.contains("/rest/api/3/issue/10000"));
            assert_eq!(
                last.headers.get("authorization").map(String::as_str),
                Some(format!("Bearer {ACCESS_TOKEN}").as_str())
            );
            let mut body = json!({"fields":{"description":{"type":"doc","version":1,"content":[{"type":"paragraph","content":[{"type":"text","text":"Reviewed"}]}]}}});
            if command == TRANSITION_ISSUE {
                body["transition"] = json!({"id":"31"});
                assert!(
                    observed
                        .iter()
                        .any(|request| request.target.contains("transitions.fields"))
                );
            } else {
                assert!(last.target.contains("notifyUsers=true"));
                assert!(last.target.contains("returnIssue=false"));
            }
            assert_eq!(serde_json::from_slice::<Value>(&last.body)?, body);
            assert_eq!(issuer.finish()?.len(), if renew { 2 } else { 1 });
            drop(locked);
            setup.no_plaintext()?;
        }
    }
    Ok(())
}

#[test]
fn lost_unqualified_and_refused_mutations_keep_journal_evidence_and_never_replay() -> TestResult {
    for command in [EDIT_ISSUE, TRANSITION_ISSUE] {
        for mode in ["lost", "returned_issue", "conflict", "rate"] {
            let setup = Setup::new()?;
            let issuer = issuer(SCOPES)?;
            let mut replies = vec![identity(), identity()];
            replies.extend(metadata(command, false)?);
            replies.push(match mode {
                "lost" => Reply::Disconnect,
                "returned_issue" => Reply::json(200, br#"{"id":"10000","status":"matched"}"#, &[]),
                "conflict" => Reply::json(409, b"private-provider-text", &[]),
                _ => Reply::json(429, b"private-provider-text", &[("Retry-After", "7")]),
            });
            let product = TlsServer::start(Reply::Sequence(replies))?;
            let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1_000)))?;
            let control = control(CancellationToken::default())?;
            let locked = fixture.lock(&setup.grant, &control)?;
            fixture.client().provision_grant(
                &locked,
                fixture.service_credentials()?,
                &[target().tenant_ref()],
                Some("actor-one"),
            )?;
            let result = prepared(&setup.grant, command, "actor-one")?
                .execute_cached(
                    fixture.client(),
                    &locked,
                    &setup.journal,
                    TlsPolicy::AdditionalCaBundle(product.root()),
                    control.clone(),
                )
                .observed();
            assert_schema(&result.envelope)?;
            assert!(!result.envelope.ok);
            let data = result.envelope.data.as_ref().ok_or("data")?;
            assert_eq!(
                data["receipt"]["request_state"],
                if mode == "conflict" {
                    "failed"
                } else {
                    "sent_unknown"
                }
            );
            assert_eq!(data["receipt"]["confirmed_resources"], json!([]));
            assert_eq!(data["replay_authorized"], false);
            assert!(!serde_json::to_string(&result.envelope)?.contains("private-provider-text"));
            if mode == "rate" {
                assert!(!data["retry_after"].is_null());
            }
            let repeat = prepared(&setup.grant, command, "actor-one")?
                .execute_cached(
                    fixture.client(),
                    &locked,
                    &setup.journal,
                    TlsPolicy::AdditionalCaBundle(product.root()),
                    control,
                )
                .observed();
            assert_eq!(
                repeat.envelope.error.as_ref().ok_or("error")?.code,
                ErrorCode::OperationIdReused
            );
            assert_eq!(
                product.finish()?.len(),
                if command == EDIT_ISSUE { 6 } else { 7 }
            );
            assert_eq!(issuer.finish()?.len(), 1);
        }
    }
    Ok(())
}

#[test]
fn scope_actor_and_changed_project_refuse_before_any_mutation_exchange() -> TestResult {
    for mode in ["scope", "actor", "project"] {
        let setup = Setup::new()?;
        let issuer = issuer(if mode == "scope" {
            "read:jira-user read:jira-work"
        } else {
            SCOPES
        })?;
        let mut replies = vec![identity(), identity()];
        if mode == "project" {
            replies.extend(metadata(EDIT_ISSUE, true)?);
        }
        let product = TlsServer::start(Reply::Sequence(replies))?;
        let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1_000)))?;
        let control = control(CancellationToken::default())?;
        let locked = fixture.lock(&setup.grant, &control)?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        let result = prepared(
            &setup.grant,
            EDIT_ISSUE,
            if mode == "actor" {
                "other-actor"
            } else {
                "actor-one"
            },
        )?
        .execute_cached(
            fixture.client(),
            &locked,
            &setup.journal,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control,
        )
        .observed();
        assert_schema(&result.envelope)?;
        assert!(!result.envelope.ok);
        assert_eq!(
            result.envelope.data.as_ref().ok_or("data")?["receipt"]["request_state"],
            "not_sent"
        );
        assert!(setup.journal.get(OPERATION)?.is_none());
        let requests = product.finish()?;
        assert_eq!(requests.len(), if mode == "project" { 5 } else { 2 });
        assert!(requests.iter().all(|request| request.method == "GET"));
        assert_eq!(issuer.finish()?.len(), 1);
    }
    Ok(())
}

struct ClaimClock {
    base: ManualClock,
    journal: NativeJournal,
    expire: bool,
}

struct CandidatePolicyClock {
    base: ManualClock,
    journal: NativeJournal,
    root: PathBuf,
    mode: &'static str,
    operations: Vec<craft_runtime::issue_mutation_policy::IssueMutationOperation>,
    read_boundary: Option<(Arc<std::sync::atomic::AtomicUsize>, usize)>,
    changed: std::sync::atomic::AtomicBool,
    claim_checks: std::sync::atomic::AtomicUsize,
}
impl Clock for CandidatePolicyClock {
    fn unix_millis(&self) -> u64 {
        let claimed = matches!(
            self.mode,
            "revoked_claim" | "changed_claim" | "revoked_frozen"
        ) && self.journal.get(OPERATION).is_ok_and(|record| {
            record.is_some_and(|record| {
                record
                    .steps
                    .values()
                    .any(|step| step.state == SubmissionState::SendStarted)
            })
        });
        let final_clock = (claimed
            && (self.mode != "revoked_frozen"
                || self
                    .claim_checks
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    >= 2))
            || self
                .read_boundary
                .as_ref()
                .is_some_and(|(counter, threshold)| {
                    counter.load(std::sync::atomic::Ordering::SeqCst) >= *threshold
                });
        if final_clock && !self.changed.load(std::sync::atomic::Ordering::SeqCst) {
            let result = (|| -> Result<(), Box<dyn std::error::Error>> {
                let control = control(CancellationToken::default())?;
                let store = ProfileStore::open(&self.root, &control)?;
                store.set_issue_mutation_policy(
                    "factory",
                    craft_runtime::issue_mutation_policy::IssueMutationPolicyUpdate {
                        target: ResourceRef {
                            resource_type: "project".into(),
                            id: "100".into(),
                            ..target()
                        },
                        expected_actor: "actor-one".into(),
                        enabled: self.mode == "changed_claim",
                        operations: self.operations.clone(),
                        expected_revision: Some(1),
                    },
                    true,
                    &control,
                    |_| Ok(()),
                )?;
                Ok(())
            })();
            self.changed
                .store(result.is_ok(), std::sync::atomic::Ordering::SeqCst);
        }
        self.base.unix_millis()
    }
    fn monotonic_millis(&self) -> u64 {
        self.base.monotonic_millis()
    }
}

#[test]
fn typed_mutation_candidates_use_project_records_and_refuse_captured_changes_at_claim_time()
-> TestResult {
    use craft_cli::issue_mutation::command::{IssueMutationArgs, PreparedInvocation};
    use craft_runtime::issue_mutation_policy::{
        IssueMutationOperation as Op, IssueMutationPolicyUpdate,
    };
    for command in [EDIT_ISSUE, TRANSITION_ISSUE] {
        for mode in [
            "success",
            "excluded",
            "before_auth",
            "revoked_claim",
            "changed_claim",
            "revoked_frozen",
        ] {
            let setup = Setup::new()?;
            let issuer = issuer(SCOPES)?;
            let mut replies = vec![identity()];
            if !matches!(mode, "excluded" | "before_auth") {
                replies.push(identity());
                replies.extend(metadata(command, false)?);
            }
            if mode == "success" {
                replies.push(Reply::json(204, b"", &[]));
            }
            let product = TlsServer::start(Reply::Sequence(replies))?;
            setup.profiles.set_issue_mutation_policy(
                "factory",
                IssueMutationPolicyUpdate {
                    target: ResourceRef {
                        resource_type: "project".into(),
                        id: "100".into(),
                        ..target()
                    },
                    expected_actor: "actor-one".into(),
                    enabled: true,
                    operations: if mode == "excluded" {
                        vec![if command == EDIT_ISSUE {
                            Op::Transition
                        } else {
                            Op::Edit
                        }]
                    } else {
                        vec![Op::Edit, Op::Transition]
                    },
                    expected_revision: None,
                },
                true,
                &control(CancellationToken::default())?,
                |_| Ok(()),
            )?;
            let mut value = request(command).input;
            value["operation_id"] = json!(OPERATION);
            value["apply"] = json!(true);
            let args = if command == EDIT_ISSUE {
                IssueMutationArgs::Edit(serde_json::from_value(value)?)
            } else {
                IssueMutationArgs::Transition(serde_json::from_value(value)?)
            };
            let options = craft_cli::profiles::InvocationOptions {
                state_root: Some(&setup.files.root),
                profile: Some("factory"),
                request_id: "original-request",
                max_bytes: 16_384,
            };
            let prepared = PreparedInvocation::prepare(
                args,
                &[],
                &options,
                &setup.profiles,
                &control(CancellationToken::default())?,
            );
            if mode == "excluded" {
                assert!(prepared.is_err());
                assert_eq!(product.finish()?.len(), 0);
                assert_eq!(issuer.finish()?.len(), 0);
                continue;
            }
            let prepared = prepared?;
            let clock = Arc::new(CandidatePolicyClock {
                base: ManualClock::new(1_000),
                journal: setup.journal.clone(),
                root: setup.files.root.clone(),
                mode,
                operations: vec![Op::Edit, Op::Transition],
                read_boundary: None,
                changed: std::sync::atomic::AtomicBool::new(false),
                claim_checks: std::sync::atomic::AtomicUsize::new(0),
            });
            let fixture = setup.fixture(&issuer, &product, clock.clone())?;
            let control = control(CancellationToken::default())?;
            let locked = fixture.lock(prepared.grant(), &control)?;
            fixture.client().provision_grant(
                &locked,
                fixture.service_credentials()?,
                &[target().tenant_ref()],
                Some("actor-one"),
            )?;
            clock.base.advance(Duration::from_secs(3_600))?;
            if mode == "before_auth" {
                setup.profiles.set_issue_mutation_policy(
                    "factory",
                    IssueMutationPolicyUpdate {
                        target: ResourceRef {
                            resource_type: "project".into(),
                            id: "100".into(),
                            ..target()
                        },
                        expected_actor: "actor-one".into(),
                        enabled: false,
                        operations: vec![Op::Edit, Op::Transition],
                        expected_revision: Some(1),
                    },
                    true,
                    &control,
                    |_| Ok(()),
                )?;
            }
            let result = prepared
                .execute_cached(
                    &setup.profiles,
                    fixture.client(),
                    &locked,
                    &setup.journal,
                    TlsPolicy::AdditionalCaBundle(product.root()),
                    control,
                )
                .observed();
            assert_schema(&result.envelope)?;
            assert_eq!(
                result.envelope.ok,
                mode == "success",
                "{mode}: {:?}",
                result.envelope.error
            );
            let data = result.envelope.data.as_ref().ok_or("data")?;
            assert_eq!(
                data["credential_cache_update"].is_null(),
                mode == "before_auth"
            );
            if mode == "success" {
                assert_eq!(data["receipt"]["request_state"], "confirmed");
            } else if mode == "before_auth" {
                assert_eq!(data["receipt"]["request_state"], "not_sent");
                assert!(setup.journal.get(OPERATION)?.is_none());
            } else {
                assert!(clock.changed.load(std::sync::atomic::Ordering::SeqCst));
                assert_eq!(data["receipt"]["request_state"], "sent_unknown");
                assert_eq!(
                    result.envelope.error.as_ref().ok_or("error")?.code,
                    if mode == "changed_claim" {
                        ErrorCode::VersionConflict
                    } else {
                        ErrorCode::PolicyDenied
                    }
                );
            }
            let metadata_requests = if command == EDIT_ISSUE { 5 } else { 6 };
            assert_eq!(
                product.finish()?.len(),
                if mode == "before_auth" {
                    1
                } else if mode == "success" {
                    metadata_requests + 1
                } else {
                    metadata_requests
                }
            );
            assert_eq!(
                issuer.finish()?.len(),
                if mode == "before_auth" { 1 } else { 2 }
            );
        }
    }
    Ok(())
}

#[test]
fn dynamic_policy_stops_each_read_boundary_and_retains_a_completed_cache_renewal() -> TestResult {
    for threshold in [1, 3, 5] {
        let setup = Setup::new()?;
        let issuer = issuer(SCOPES)?;
        let mut replies = vec![identity(), identity()];
        replies.extend(metadata(EDIT_ISSUE, false)?);
        let product = TlsServer::start(Reply::Sequence(replies))?;
        let clock = Arc::new(ManualClock::new(1_000));
        let fixture = setup.fixture(&issuer, &product, clock.clone())?;
        let control = control(CancellationToken::default())?;
        let locked = fixture.lock(&setup.grant, &control)?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        clock.advance(Duration::from_secs(3_600))?;
        let result = prepared(&setup.grant, EDIT_ISSUE, "actor-one")?
            .execute_cached_with_policy_check(
                fixture.client(),
                &locked,
                &setup.journal,
                TlsPolicy::AdditionalCaBundle(product.root()),
                control,
                || {
                    if product.request_count() >= threshold {
                        return Err(CraftError::new(
                            ErrorCode::PolicyDenied,
                            "Captured policy changed.",
                        ));
                    }
                    Ok(())
                },
            )
            .observed();
        assert_schema(&result.envelope)?;
        assert!(!result.envelope.ok);
        let data = result.envelope.data.as_ref().ok_or("data")?;
        assert_eq!(data["receipt"]["request_state"], "not_sent");
        assert_eq!(data["credential_cache_update"].is_null(), threshold == 1);
        assert!(setup.journal.get(OPERATION)?.is_none());
        assert_eq!(product.finish()?.len(), threshold);
        assert_eq!(issuer.finish()?.len(), if threshold == 1 { 1 } else { 2 });
    }
    Ok(())
}
impl Clock for ClaimClock {
    fn unix_millis(&self) -> u64 {
        self.base.unix_millis()
    }
    fn monotonic_millis(&self) -> u64 {
        let claimed = self.expire
            && self.journal.get(OPERATION).is_ok_and(|record| {
                record.is_some_and(|record| {
                    record
                        .steps
                        .values()
                        .any(|step| step.state == SubmissionState::SendStarted)
                })
            });
        self.base.monotonic_millis() + if claimed { 60_000 } else { 0 }
    }
}
#[test]
fn claim_time_expiry_revocation_and_cancellation_keep_uncertainty_without_http_send() -> TestResult
{
    for mode in ["expire", "revoke", "cancel"] {
        let setup = Setup::new()?;
        let issuer = issuer(SCOPES)?;
        let mut replies = vec![identity(), identity()];
        replies.extend(metadata(EDIT_ISSUE, false)?);
        let product = TlsServer::start(Reply::Sequence(replies))?;
        let clock = Arc::new(ClaimClock {
            base: ManualClock::new(1_000),
            journal: setup.journal.clone(),
            expire: mode == "expire",
        });
        let fixture = setup.fixture(&issuer, &product, clock)?;
        let token = CancellationToken::default();
        let control = control(token.clone())?;
        let locked = fixture.lock(&setup.grant, &control)?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        let result = prepared(&setup.grant, EDIT_ISSUE, "actor-one")?
            .execute_cached_with_policy_check(
                fixture.client(),
                &locked,
                &setup.journal,
                TlsPolicy::AdditionalCaBundle(product.root()),
                control,
                || {
                    let claimed = setup.journal.get(OPERATION).is_ok_and(|record| {
                        record.is_some_and(|record| {
                            record
                                .steps
                                .values()
                                .any(|step| step.state == SubmissionState::SendStarted)
                        })
                    });
                    if claimed && mode == "cancel" {
                        token.cancel();
                    }
                    if claimed && mode == "revoke" {
                        return Err(CraftError::new(ErrorCode::PolicyDenied, "Policy revoked."));
                    }
                    Ok(())
                },
            )
            .observed();
        assert_schema(&result.envelope)?;
        assert!(!result.envelope.ok);
        assert_eq!(
            result.envelope.data.as_ref().ok_or("data")?["receipt"]["request_state"],
            "sent_unknown"
        );
        assert_eq!(
            setup.journal.get(OPERATION)?.ok_or("journal")?.steps
                [issue_mutation::route(EDIT_ISSUE)?]
            .state,
            SubmissionState::Unknown
        );
        let requests = product.finish()?;
        assert_eq!(requests.len(), 5);
        assert!(requests.iter().all(|request| request.method == "GET"));
        assert_eq!(issuer.finish()?.len(), 1);
    }
    Ok(())
}
