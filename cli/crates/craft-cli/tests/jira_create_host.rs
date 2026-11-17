//! Owned-server acceptance of the actual internal host, not a public cloud command.
#[path = "support/conformance.rs"]
mod conformance;
use conformance::Observed;

#[path = "jira_create_host/comment_writes.rs"]
mod comment_writes;
#[path = "jira_create_host/link_lifecycle.rs"]
mod link_lifecycle;
#[path = "jira_create_host/link_reads.rs"]
mod link_reads;
#[path = "jira_create_host/links.rs"]
mod links;
#[path = "jira_create_host/recovery.rs"]
mod recovery;
#[path = "jira_create_host/reviewed_create.rs"]
mod reviewed_create;
#[path = "jira_create_host/saved_source.rs"]
mod saved_source;
#[path = "../../craft-runtime/src/transport/tls_fixture.rs"]
mod tls_fixture;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use craft_cli::issue_create::host::{CreatePolicy, PreparedCreate};
use craft_core::contracts::{AuthMode, CanonicalRequest, GrantRef, ResourceRef, Selection};
use craft_core::control::{CancellationToken, InvocationControl};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::profiles::ProfileInput;
use craft_core::storage::{Clock, OperationJournal, SecretBytes, SubmissionState};
use craft_provider_jira::created_issue::{COMMAND, ROUTE};
use craft_runtime::auth::ServiceCredentials;
use craft_runtime::control::SystemClock;
use craft_runtime::create_policy::CreatePolicyUpdate;
use craft_runtime::journal::NativeJournal;
use craft_runtime::profiles::ProfileStore;
use craft_runtime::secrets::IncompletePolicy;
use craft_runtime::synthetic_acceptance::{ACCESS_TOKEN, CLIENT_ID, CLIENT_SECRET, ServiceFixture};
use craft_runtime::transport::TlsPolicy;
use craft_test_support::state::ManualClock;
use serde_json::{Value, json};
use tls_fixture::{Reply, TlsServer};

struct PolicyClock {
    base: ManualClock,
    journal: NativeJournal,
    store: Mutex<ProfileStore>,
    control: InvocationControl,
    change: bool,
    disable: bool,
    changed: AtomicBool,
}
impl Clock for PolicyClock {
    fn unix_millis(&self) -> u64 {
        let claimed = self.change
            && self.journal.get(OPERATION).is_ok_and(|value| {
                value.is_some_and(|record| {
                    record
                        .steps
                        .get(ROUTE)
                        .is_some_and(|step| step.state == SubmissionState::SendStarted)
                })
            });
        if claimed
            && !self.changed.load(Ordering::SeqCst)
            && let Ok(store) = self.store.lock()
        {
            let change = store.set_create_policy(
                "factory",
                CreatePolicyUpdate {
                    target: target(),
                    expected_actor: "actor-one".into(),
                    enabled: !self.disable,
                    expected_revision: Some(1),
                },
                true,
                &self.control,
                |_| Ok(()),
            );
            self.changed.store(change.is_ok(), Ordering::SeqCst);
        }
        self.base.unix_millis()
    }
    fn monotonic_millis(&self) -> u64 {
        self.base.monotonic_millis()
    }
}

#[test]
fn typed_command_resolves_private_policy_and_refuses_its_change_before_auth_or_after_claim()
-> TestResult {
    use craft_cli::issue_create::command::{IssueCreateArgs, PreparedInvocation};
    use craft_cli::profiles::InvocationOptions;
    for mode in ["success", "before_auth", "revoked_claim", "changed_claim"] {
        let setup = Setup::new()?;
        let issuer = issuer(SCOPES)?;
        let mut replies = vec![identity()];
        if mode != "before_auth" {
            replies.push(identity());
            replies.extend(metadata()?);
        }
        if mode == "success" {
            replies.push(Reply::json(201, br#"{"id":"10003","key":"ED-24"}"#, &[]));
        }
        let product = TlsServer::start(Reply::Sequence(replies))?;
        setup.profiles.set_create_policy(
            "factory",
            CreatePolicyUpdate {
                target: target(),
                expected_actor: "actor-one".into(),
                enabled: true,
                expected_revision: None,
            },
            true,
            &control(CancellationToken::default(), 15_000)?,
            |_| Ok(()),
        )?;
        let clock = Arc::new(PolicyClock {
            base: ManualClock::new(1_000),
            journal: setup.journal.clone(),
            store: Mutex::new(ProfileStore::open(
                &setup.files.root,
                &control(CancellationToken::default(), 30_000)?,
            )?),
            control: control(CancellationToken::default(), 30_000)?,
            change: matches!(mode, "revoked_claim" | "changed_claim"),
            disable: mode == "revoked_claim",
            changed: AtomicBool::new(false),
        });
        let fixture = setup.fixture(&issuer, &product, clock.clone())?;
        let locked = fixture.lock(
            &setup.grant,
            &control(CancellationToken::default(), 30_000)?,
        )?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        let mut value = input();
        value["operation_id"] = json!(OPERATION);
        value["apply"] = json!(true);
        value["project_id"] = json!("10000");
        let args: IssueCreateArgs = serde_json::from_value(value)?;
        let options = InvocationOptions {
            state_root: Some(&setup.files.root),
            profile: Some("factory"),
            request_id: "typed-command-request",
            max_bytes: 16_384,
        };
        let prepared = PreparedInvocation::prepare(
            args,
            &["issue_key".into()],
            &options,
            &setup.profiles,
            &control(CancellationToken::default(), 15_000)?,
        )?;
        if mode == "before_auth" {
            setup.profiles.set_create_policy(
                "factory",
                CreatePolicyUpdate {
                    target: target(),
                    expected_actor: "actor-one".into(),
                    enabled: true,
                    expected_revision: Some(1),
                },
                true,
                &control(CancellationToken::default(), 15_000)?,
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
                control(CancellationToken::default(), 15_000)?,
            )
            .observed_selecting(&["issue_key"]);
        assert_eq!(result.envelope.request_id, "typed-command-request");
        assert_eq!(result.envelope.ok, mode == "success");
        let data = result
            .envelope
            .data
            .as_ref()
            .ok_or("Missing complete command receipt")?;
        assert_eq!(
            data["receipt"]["request_state"],
            match mode {
                "success" => "confirmed",
                "before_auth" => "not_sent",
                _ => "sent_unknown",
            }
        );
        if mode == "success" {
            assert_eq!(data["receipt"]["confirmed_resources"][0]["id"], "10003");
        } else {
            assert_eq!(
                result.envelope.error.as_ref().map(|error| error.code),
                Some(if mode == "revoked_claim" {
                    ErrorCode::PolicyDenied
                } else {
                    ErrorCode::VersionConflict
                })
            );
        }
        if mode == "before_auth" {
            assert!(setup.journal.get(OPERATION)?.is_none());
        } else {
            assert_eq!(
                setup
                    .journal
                    .get(OPERATION)?
                    .ok_or("Missing journal")?
                    .steps[ROUTE]
                    .state,
                if mode == "success" {
                    SubmissionState::Confirmed
                } else {
                    SubmissionState::Unknown
                }
            );
        }
        assert_eq!(
            clock.changed.load(Ordering::SeqCst),
            matches!(mode, "revoked_claim" | "changed_claim")
        );
        assert_eq!(
            product.finish()?.len(),
            match mode {
                "success" => 8,
                "before_auth" => 1,
                _ => 7,
            }
        );
        assert_eq!(issuer.finish()?.len(), 1);
        drop(locked);
        drop(fixture);
        drop(clock);
        setup.cleanup()?;
    }
    Ok(())
}

type TestResult = Result<(), Box<dyn std::error::Error>>;
const SCOPES: &str = "read:jira-user read:jira-work write:jira-work";
const OPERATION: &str = "create-report";

fn control(token: CancellationToken, ms: u64) -> Result<InvocationControl, CraftError> {
    InvocationControl::new(Arc::new(SystemClock::default()), token, ms)
}
fn target() -> ResourceRef {
    ResourceRef {
        provider: "jira".into(),
        deployment: "cloud".into(),
        tenant: "00000000-1111-2222-3333-444444444444".into(),
        resource_type: "project".into(),
        id: "10000".into(),
    }
}
fn input() -> Value {
    json!({"issue_type_id":"10001","summary":"Literal <tag> 世界","description":"Reviewed detail","parent_issue_id":"20000","assignee_account_id":"account-two"})
}
fn host(grant: &GrantRef, actor: &str) -> Result<PreparedCreate, CraftError> {
    PreparedCreate::prepare(
        CanonicalRequest {
            command: COMMAND.into(),
            target: target(),
            input: input(),
            selection: Selection {
                fields: vec!["issue_key".into()],
                ..Default::default()
            },
            preconditions: None,
        },
        OPERATION,
        true,
        "request",
        CreatePolicy {
            enabled: true,
            target: target(),
            grant: grant.clone(),
            expected_actor: actor.into(),
        },
    )
}
fn identity() -> Reply {
    Reply::json(200, br#"{"accountId":"actor-one","active":true}"#, &[])
}
/// Expires a manual-clock invocation deadline once the product has received `threshold`
/// requests, so a stalled resource read must end in `DEADLINE_EXCEEDED` before the fixture
/// gives up on the stalled connection.
fn deadline_at_request(
    product: &TlsServer,
    threshold: usize,
) -> Result<
    (
        InvocationControl,
        std::thread::JoinHandle<Result<(), String>>,
    ),
    CraftError,
> {
    let clock = Arc::new(ManualClock::new(1000));
    let invocation = InvocationControl::new(clock.clone(), CancellationToken::default(), 10_000)?;
    let counter = product.request_counter();
    let worker = std::thread::spawn(move || -> Result<(), String> {
        let start = std::time::Instant::now();
        while counter.load(Ordering::SeqCst) < threshold {
            if start.elapsed() > Duration::from_secs(5) {
                return Err("Expected request was not observed".into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        clock
            .advance(Duration::from_millis(10_000))
            .map_err(|error| error.to_string())
    });
    Ok((invocation, worker))
}
fn join_deadline(
    worker: Option<std::thread::JoinHandle<Result<(), String>>>,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(worker) = worker {
        worker.join().map_err(|_| "Deadline worker panicked")??;
    }
    Ok(())
}
fn issuer(scopes: &str) -> Result<TlsServer, Box<dyn std::error::Error>> {
    TlsServer::start(Reply::json(
        200,
        &serde_json::to_vec(
            &json!({"access_token":ACCESS_TOKEN,"expires_in":3600,"token_type":"Bearer","scope":scopes}),
        )?,
        &[],
    ))
}
fn metadata() -> Result<Vec<Reply>, Box<dyn std::error::Error>> {
    let field = |id: &str, kind: &str, required: bool| json!({"fieldId":id,"key":id,"name":id,"required":required,"operations":["set"],"schema":{"type":kind,"system":id}});
    Ok(vec![
        Reply::json(200,br#"{"id":"10000","key":"ED"}"#,&[]),
        Reply::json(200,br#"{"startAt":0,"maxResults":50,"total":1,"issueTypes":[{"id":"10001","name":"Subtask","subtask":true}]}"#,&[]),
        Reply::json(200,&serde_json::to_vec(&json!({"startAt":0,"maxResults":50,"total":6,"fields":[field("project","project",true),field("issuetype","issuetype",true),field("summary","string",true),field("description","string",false),field("parent","issuelink",true),field("assignee","user",false)]}))?,&[]),
        Reply::json(200,br#"{"id":"20000","fields":{"project":{"id":"10000"},"issuetype":{"id":"10002","subtask":false}}}"#,&[]),
        Reply::json(200,br#"[{"accountId":"account-two","active":true,"emailAddress":"private-account-sentinel"}]"#,&[]),
    ])
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
            root: craft_test_support::state::fixture_root(&parent, "composed-create", stamp),
            parent,
        })
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
struct Setup {
    profiles: ProfileStore,
    grant: GrantRef,
    journal: NativeJournal,
    files: Files,
}
impl Setup {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let files = Files::new()?;
        let control = control(CancellationToken::default(), 30_000)?;
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
            control(CancellationToken::default(), 30_000)?,
            clock,
        )
    }
    fn assert_no_plaintext(&self) -> TestResult {
        for entry in fs::read_dir(&self.files.root)? {
            let path = entry?.path();
            if path.is_file() {
                let bytes = fs::read(path)?;
                for text in [
                    CLIENT_ID,
                    CLIENT_SECRET,
                    ACCESS_TOKEN,
                    "private-account-sentinel",
                ] {
                    assert!(
                        !bytes
                            .windows(text.len())
                            .any(|part| part == text.as_bytes())
                    );
                }
            }
        }
        Ok(())
    }

    fn cleanup(self) -> TestResult {
        let root = self.files.root.clone();
        drop(self);
        assert!(
            !root.try_exists()?,
            "Owned composed-host fixture was not removed"
        );
        Ok(())
    }
}

#[test]
fn cached_host_sends_one_qualified_create_and_preserves_success_partial_or_lost_results()
-> TestResult {
    for mode in ["fresh", "renewed", "partial", "lost"] {
        let setup = Setup::new()?;
        let issuer = issuer(SCOPES)?;
        let mut replies = vec![identity(), identity()];
        replies.extend(metadata()?);
        replies.push(match mode {
            "lost" => Reply::Disconnect,
            "partial" => Reply::json(
                201,
                br#"{"id":"10003","key":"ED-24","transition":{"status":400}}"#,
                &[],
            ),
            _ => Reply::json(
                201,
                br#"{"id":"10003","key":"ED-24","self":"https://untrusted.invalid/ignored"}"#,
                &[],
            ),
        });
        let product = TlsServer::start(Reply::Sequence(replies))?;
        let clock = Arc::new(ManualClock::new(1_000));
        let fixture = setup.fixture(&issuer, &product, clock.clone())?;
        let locked = fixture.lock(
            &setup.grant,
            &control(CancellationToken::default(), 30_000)?,
        )?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        let original = locked.read()?.version;
        if mode == "renewed" {
            clock.advance(Duration::from_secs(3_600))?;
        }
        let result = host(&setup.grant, "actor-one")?
            .execute_cached(
                fixture.client(),
                &locked,
                &setup.journal,
                TlsPolicy::AdditionalCaBundle(product.root()),
                control(CancellationToken::default(), 15_000)?,
            )
            .observed_selecting(&["issue_key"]);
        let encoded = craft_cli::output::encode(result.envelope, 16_384, result.output_policy)?;
        let envelope: Value = serde_json::from_slice(&encoded.bytes)?;
        let data = &envelope["data"];
        assert_eq!(envelope["ok"], !matches!(mode, "partial" | "lost"));
        assert_eq!(
            data["receipt"]["request_state"],
            if mode == "lost" {
                "sent_unknown"
            } else {
                "confirmed"
            }
        );
        assert_eq!(data["persistence"], "recorded");
        assert_eq!(data["replay_authorized"], false);
        if mode == "renewed" {
            assert_eq!(data["credential_cache_update"]["write_state"], "confirmed");
            assert_ne!(locked.read()?.version, original);
        } else {
            assert!(data["credential_cache_update"].is_null());
        }
        if mode != "lost" {
            assert_eq!(data["receipt"]["confirmed_resources"][0]["id"], "10003");
        }
        let saved = setup
            .journal
            .get(OPERATION)?
            .ok_or("Missing create journal")?;
        assert_eq!(
            saved.steps[ROUTE].state,
            if mode == "lost" {
                SubmissionState::Unknown
            } else {
                SubmissionState::Confirmed
            }
        );
        let replay = host(&setup.grant, "actor-one")?
            .execute_cached(
                fixture.client(),
                &locked,
                &setup.journal,
                TlsPolicy::AdditionalCaBundle(product.root()),
                control(CancellationToken::default(), 15_000)?,
            )
            .observed_selecting(&["issue_key"]);
        assert_eq!(
            replay.envelope.error.as_ref().map(|error| error.code),
            Some(ErrorCode::OperationIdReused)
        );
        if mode != "lost" {
            assert_eq!(
                replay
                    .envelope
                    .data
                    .as_ref()
                    .ok_or("Missing replay receipt")?["receipt"]["confirmed_resources"][0]["id"],
                "10003"
            );
        }
        assert_eq!(product.request_counter().load(Ordering::SeqCst), 8);
        assert_eq!(
            issuer.request_count(),
            if mode == "renewed" { 2 } else { 1 }
        );
        let requests = product.finish()?;
        assert_eq!(requests.len(), 8);
        assert!(requests[..7].iter().all(|request| request.method == "GET"));
        assert_eq!(requests[7].method, "POST");
        assert!(requests.iter().all(|request| {
            request
                .target
                .starts_with(&format!("/ex/jira/{}/rest/api/3/", target().tenant))
                && request.headers.get("authorization") == Some(&format!("Bearer {ACCESS_TOKEN}"))
        }));
        let body: Value = serde_json::from_slice(&requests[7].body)?;
        assert_eq!(
            body,
            json!({"fields":{"project":{"id":"10000"},"issuetype":{"id":"10001"},"summary":"Literal <tag> 世界","description":{"type":"doc","version":1,"content":[{"type":"paragraph","content":[{"type":"text","text":"Reviewed detail"}]}]},"parent":{"id":"20000"},"assignee":{"accountId":"account-two"}}})
        );
        assert!(requests[5].target.contains("fields=project%2Cissuetype"));
        assert!(requests[6].target.contains("accountId=account-two"));
        issuer.finish()?;
        drop(locked);
        drop(fixture);
        setup.assert_no_plaintext()?;
        setup.cleanup()?;
    }
    Ok(())
}

#[test]
fn scope_or_actor_denial_precedes_every_resource_read_and_send_claim() -> TestResult {
    for (scopes, actor) in [
        ("read:jira-user read:jira-work", "actor-one"),
        ("read:jira-user write:jira-work", "actor-one"),
        (
            "read:application-role:jira read:group:jira read:user:jira read:avatar:jira read:jira-work write:jira-work",
            "actor-one",
        ),
        (SCOPES, "other-actor"),
    ] {
        let setup = Setup::new()?;
        let issuer = issuer(scopes)?;
        let product = TlsServer::start(Reply::Sequence(vec![identity(), identity()]))?;
        let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1_000)))?;
        let locked = fixture.lock(
            &setup.grant,
            &control(CancellationToken::default(), 30_000)?,
        )?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        let result = host(&setup.grant, actor)?
            .execute_cached(
                fixture.client(),
                &locked,
                &setup.journal,
                TlsPolicy::AdditionalCaBundle(product.root()),
                control(CancellationToken::default(), 15_000)?,
            )
            .observed_selecting(&["issue_key"]);
        assert_eq!(
            result.envelope.error.as_ref().map(|error| error.code),
            Some(if actor == "actor-one" {
                ErrorCode::PermissionDenied
            } else {
                ErrorCode::PolicyDenied
            })
        );
        assert_eq!(
            result
                .envelope
                .data
                .as_ref()
                .ok_or("Missing refusal receipt")?["receipt"]["request_state"],
            "not_sent"
        );
        assert!(setup.journal.get(OPERATION)?.is_none());
        assert_eq!(product.finish()?.len(), 2);
        assert_eq!(issuer.finish()?.len(), 1);
        drop(locked);
        drop(fixture);
        setup.cleanup()?;
    }
    Ok(())
}

#[test]
fn failed_project_parent_or_account_qualification_preserves_cache_update_without_a_claim()
-> TestResult {
    for mode in ["project", "parent", "account"] {
        let setup = Setup::new()?;
        let issuer = issuer(SCOPES)?;
        let mut steps = metadata()?;
        let count = match mode {
            "project" => {
                steps[0] = Reply::json(200, br#"{"id":"99999","key":"OTHER"}"#, &[]);
                1
            }
            "parent" => {
                steps[3]=Reply::json(200,br#"{"id":"20000","fields":{"project":{"id":"99999"},"issuetype":{"id":"10002","subtask":false}}}"#,&[]);
                4
            }
            _ => {
                steps[4] = Reply::json(
                    200,
                    br#"[{"accountId":"another-account","active":true}]"#,
                    &[],
                );
                5
            }
        };
        steps.truncate(count);
        let mut replies = vec![identity(), identity()];
        replies.extend(steps);
        let product = TlsServer::start(Reply::Sequence(replies))?;
        let clock = Arc::new(ManualClock::new(1_000));
        let fixture = setup.fixture(&issuer, &product, clock.clone())?;
        let locked = fixture.lock(
            &setup.grant,
            &control(CancellationToken::default(), 30_000)?,
        )?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        clock.advance(Duration::from_secs(3_600))?;
        let result = host(&setup.grant, "actor-one")?
            .execute_cached(
                fixture.client(),
                &locked,
                &setup.journal,
                TlsPolicy::AdditionalCaBundle(product.root()),
                control(CancellationToken::default(), 15_000)?,
            )
            .observed_selecting(&["issue_key"]);
        assert_eq!(
            result.envelope.error.as_ref().map(|error| error.code),
            Some(ErrorCode::PolicyDenied)
        );
        let data = result.envelope.data.ok_or("Missing separate effects")?;
        assert_eq!(data["receipt"]["request_state"], "not_sent");
        assert_eq!(data["credential_cache_update"]["write_state"], "confirmed");
        assert!(setup.journal.get(OPERATION)?.is_none());
        assert_eq!(product.finish()?.len(), count + 2);
        assert_eq!(issuer.finish()?.len(), 2);
        drop(locked);
        drop(fixture);
        setup.cleanup()?;
    }
    Ok(())
}

struct ClaimClock {
    base: ManualClock,
    journal: NativeJournal,
    shift: u64,
    cancel: Option<CancellationToken>,
}
impl Clock for ClaimClock {
    fn unix_millis(&self) -> u64 {
        let claimed = self.journal.get(OPERATION).is_ok_and(|value| {
            value.is_some_and(|record| {
                record
                    .steps
                    .get(ROUTE)
                    .is_some_and(|step| step.state == SubmissionState::SendStarted)
            })
        });
        if claimed {
            if let Some(token) = &self.cancel {
                token.cancel();
            }
            self.base.unix_millis().saturating_add(self.shift)
        } else {
            self.base.unix_millis()
        }
    }
    fn monotonic_millis(&self) -> u64 {
        self.base.monotonic_millis()
    }
}

#[test]
fn committed_claim_rechecks_metadata_token_and_cancellation_without_post_or_reacquisition()
-> TestResult {
    for (shift, cancel, expected) in [
        (60_000, false, ErrorCode::VersionConflict),
        (3_600_000, false, ErrorCode::AuthRequired),
        (0, true, ErrorCode::Cancelled),
    ] {
        let setup = Setup::new()?;
        let issuer = issuer(SCOPES)?;
        let mut replies = vec![identity(), identity()];
        replies.extend(metadata()?);
        let product = TlsServer::start(Reply::Sequence(replies))?;
        let signal = CancellationToken::default();
        let clock = Arc::new(ClaimClock {
            base: ManualClock::new(1_000),
            journal: setup.journal.clone(),
            shift,
            cancel: cancel.then(|| signal.clone()),
        });
        let fixture = setup.fixture(&issuer, &product, clock)?;
        let locked = fixture.lock(
            &setup.grant,
            &control(CancellationToken::default(), 30_000)?,
        )?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        let result = host(&setup.grant, "actor-one")?
            .execute_cached(
                fixture.client(),
                &locked,
                &setup.journal,
                TlsPolicy::AdditionalCaBundle(product.root()),
                control(signal, 15_000)?,
            )
            .observed_selecting(&["issue_key"]);
        assert_eq!(
            result.envelope.error.as_ref().map(|error| error.code),
            Some(expected)
        );
        assert_eq!(
            result
                .envelope
                .data
                .as_ref()
                .ok_or("Missing claimed refusal")?["receipt"]["request_state"],
            "sent_unknown"
        );
        assert_eq!(
            setup
                .journal
                .get(OPERATION)?
                .ok_or("Missing committed claim")?
                .steps[ROUTE]
                .state,
            SubmissionState::Unknown
        );
        let replay = host(&setup.grant, "actor-one")?
            .execute_cached(
                fixture.client(),
                &locked,
                &setup.journal,
                TlsPolicy::AdditionalCaBundle(product.root()),
                control(CancellationToken::default(), 15_000)?,
            )
            .observed_selecting(&["issue_key"]);
        assert_eq!(
            replay.envelope.error.as_ref().map(|error| error.code),
            Some(ErrorCode::OperationIdReused)
        );
        assert_eq!(product.finish()?.len(), 7);
        assert_eq!(issuer.finish()?.len(), 1);
        drop(locked);
        drop(fixture);
        setup.cleanup()?;
    }
    Ok(())
}

#[test]
fn stalled_metadata_observes_deadline_without_an_issue_send_or_retry() -> TestResult {
    let setup = Setup::new()?;
    let issuer = issuer(SCOPES)?;
    let product = TlsServer::start(Reply::Sequence(vec![identity(), identity(), Reply::Stall]))?;
    let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1_000)))?;
    let locked = fixture.lock(
        &setup.grant,
        &control(CancellationToken::default(), 30_000)?,
    )?;
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref()],
        Some("actor-one"),
    )?;
    let deadline = InvocationControl::new(
        Arc::new(RequestDeadlineClock {
            base: SystemClock::default(),
            requests: product.request_counter(),
        }),
        CancellationToken::default(),
        15_000,
    )?;
    let result = host(&setup.grant, "actor-one")?
        .execute_cached(
            fixture.client(),
            &locked,
            &setup.journal,
            TlsPolicy::AdditionalCaBundle(product.root()),
            deadline,
        )
        .observed_selecting(&["issue_key"]);
    assert!(matches!(
        result.envelope.error.as_ref().map(|error| error.code),
        Some(ErrorCode::DeadlineExceeded)
    ));
    assert_eq!(
        result
            .envelope
            .data
            .as_ref()
            .ok_or("Missing timeout result")?["receipt"]["request_state"],
        "not_sent"
    );
    assert!(setup.journal.get(OPERATION)?.is_none());
    assert_eq!(product.finish()?.len(), 3);
    assert_eq!(issuer.finish()?.len(), 1);
    drop(locked);
    drop(fixture);
    setup.cleanup()?;
    Ok(())
}

struct RequestDeadlineClock {
    base: SystemClock,
    requests: Arc<AtomicUsize>,
}
impl Clock for RequestDeadlineClock {
    fn unix_millis(&self) -> u64 {
        self.base.unix_millis()
    }
    fn monotonic_millis(&self) -> u64 {
        self.base.monotonic_millis()
            + if self.requests.load(Ordering::SeqCst) >= 3 {
                60_000
            } else {
                0
            }
    }
}

#[test]
fn synthetic_client_refuses_foreign_credential_cache_and_issuer_token_material() -> TestResult {
    for field in ["client_secret", "access_token"] {
        let setup = Setup::new()?;
        let issuer = issuer(SCOPES)?;
        let product = TlsServer::start(identity())?;
        let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1_000)))?;
        let locked = fixture.lock(
            &setup.grant,
            &control(CancellationToken::default(), 30_000)?,
        )?;
        let foreign = ServiceCredentials::from_provisioning(SecretBytes::new(
            br#"{"client_id":"foreign-client","client_secret":"foreign-secret-sentinel"}"#.to_vec(),
        )?)?;
        assert_eq!(
            fixture
                .client()
                .acquire(&foreign)
                .err()
                .ok_or("Expected credential refusal")?
                .error
                .code,
            ErrorCode::PolicyDenied
        );
        assert_eq!(issuer.connections(), 0);
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        let snapshot = locked.read()?;
        let mut bundle: Value =
            serde_json::from_slice(snapshot.secret.as_ref().ok_or("Missing bundle")?.expose())?;
        if field == "client_secret" {
            bundle["credentials"][field] = json!("foreign-secret-sentinel");
        } else {
            bundle["token"][field] = json!("foreign-token-sentinel");
        }
        locked.replace(
            &snapshot.version,
            &SecretBytes::new(serde_json::to_vec(&bundle)?)?,
            IncompletePolicy::KeepPrevious,
        )?;
        assert_eq!(
            fixture
                .client()
                .open_service_access(&locked, &target().tenant_ref())
                .err()
                .ok_or("Expected cache refusal")?
                .error
                .code,
            ErrorCode::PolicyDenied
        );
        assert_eq!(issuer.finish()?.len(), 1);
        assert_eq!(product.finish()?.len(), 1);
        drop(locked);
        drop(fixture);
        setup.cleanup()?;
    }
    let setup = Setup::new()?;
    let issuer=TlsServer::start(Reply::json(200,br#"{"access_token":"foreign-token-sentinel","expires_in":3600,"token_type":"Bearer","scope":"read:jira-user"}"#,&[]))?;
    let product = TlsServer::start(Reply::Disconnect)?;
    let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1_000)))?;
    assert_eq!(
        fixture
            .client()
            .acquire(&fixture.service_credentials()?)
            .err()
            .ok_or("Expected token refusal")?
            .error
            .code,
        ErrorCode::PolicyDenied
    );
    assert_eq!(product.connections(), 0);
    assert_eq!(issuer.finish()?.len(), 1);
    drop(fixture);
    setup.cleanup()?;
    Ok(())
}
