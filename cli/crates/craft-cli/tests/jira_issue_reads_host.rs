//! Owned TLS acceptance for actual issue/auth/cache/continuation composition.

#[path = "jira_issue_reads_host/attachments.rs"]
mod attachments;
#[path = "jira_issue_reads_host/comments.rs"]
mod comments;
#[path = "jira_issue_reads_host/deletes.rs"]
mod deletes;
#[path = "jira_issue_reads_host/downloads.rs"]
mod downloads;
#[path = "../../craft-runtime/src/transport/tls_fixture.rs"]
mod tls_fixture;
#[path = "jira_issue_reads_host/uploads.rs"]
mod uploads;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::{Mutex, atomic::AtomicBool};
use std::time::Duration;

use craft_cli::issue_reads::{IssueReadPolicy, PreparedIssueRead};
use craft_core::contracts::{
    AuthMode, CanonicalRequest, Envelope, GrantRef, ResourceRef, Selection,
};
use craft_core::control::{CancellationToken, InvocationControl};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::profiles::ProfileInput;
use craft_core::storage::Clock;
use craft_provider_jira::issue_reads::{GET, SEARCH};
use craft_runtime::control::SystemClock;
use craft_runtime::issue_read_policy::{IssueReadOperation, IssueReadPolicyUpdate};
use craft_runtime::profiles::ProfileStore;
use craft_runtime::synthetic_acceptance::{ACCESS_TOKEN, CLIENT_ID, CLIENT_SECRET, ServiceFixture};
use craft_runtime::transport::TlsPolicy;
use craft_test_support::state::ManualClock;
use serde_json::{Value, json};
use tls_fixture::{Reply, TlsServer};

type TestResult = Result<(), Box<dyn std::error::Error>>;
const CLOUD: &str = "00000000-1111-2222-3333-444444444444";
fn target() -> ResourceRef {
    ResourceRef {
        provider: "jira".into(),
        deployment: "cloud".into(),
        tenant: CLOUD.into(),
        resource_type: "site".into(),
        id: CLOUD.into(),
    }
}
fn control() -> Result<InvocationControl, CraftError> {
    InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        30_000,
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
fn issue(id: u64) -> Value {
    json!({"id":id.to_string(),"key":format!("ED-{id}"),"self":"https://untrusted.invalid","fields":{"summary":format!("Issue {id} 文字"),"project":{"id":"10","key":"ED"},"status":{"id":"1","name":"Open"},"assignee":{"emailAddress":"private-sentinel"},"description":"private-sentinel"}})
}
fn reply(body: Value) -> Result<Reply, serde_json::Error> {
    Ok(Reply::json(200, &serde_json::to_vec(&body)?, &[]))
}
fn search_input() -> Value {
    json!({"jql":"project = ED\nORDER BY key","page_size":50,"reconcile_issue_ids":["1","2"]})
}
fn prepared(
    grant: &GrantRef,
    command: &str,
    input: Value,
    fields: &[&str],
    cursor: Option<&str>,
    limit: u16,
    max_bytes: u32,
) -> Result<PreparedIssueRead, CraftError> {
    PreparedIssueRead::prepare(
        CanonicalRequest {
            command: command.into(),
            target: target(),
            input,
            selection: Selection {
                fields: fields.iter().map(|field| (*field).into()).collect(),
                cursor: cursor.map(String::from),
                limit,
                max_bytes,
            },
            preconditions: None,
        },
        "original-request",
        IssueReadPolicy {
            target: target(),
            grant: grant.clone(),
            expected_actor: "actor-one".into(),
        },
    )
}
fn cursor(envelope: &Envelope) -> Result<&str, Box<dyn std::error::Error>> {
    envelope
        .page
        .as_ref()
        .and_then(|page| page.next_cursor.as_deref())
        .ok_or_else(|| "Missing cursor".into())
}
fn data(envelope: &Envelope) -> Result<&Value, Box<dyn std::error::Error>> {
    envelope.data.as_ref().ok_or_else(|| "Missing data".into())
}
fn code(envelope: &Envelope) -> Option<ErrorCode> {
    envelope.error.as_ref().map(|error| error.code)
}
struct Setup {
    store: ProfileStore,
    grant: GrantRef,
    root: PathBuf,
    parent: PathBuf,
}
impl Setup {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        fs::create_dir_all(&parent)?;
        let parent = parent.canonicalize()?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let root = craft_test_support::state::fixture_root(&parent, "composed-issues", stamp);
        let store = ProfileStore::open(&root, &control()?)?;
        let profile = store.create(
            &ProfileInput {
                name: "factory".into(),
                provider: "jira".into(),
                deployment: "cloud".into(),
                auth_mode: AuthMode::Service,
                grant: None,
                tenant: Some(CLOUD.into()),
                defaults: BTreeMap::new(),
            },
            &control()?,
            |_| Ok(()),
        )?;
        Ok(Self {
            store,
            grant: profile.grant,
            root,
            parent,
        })
    }
    fn fixture(
        &self,
        issuer: &TlsServer,
        product: &TlsServer,
        clock: Arc<dyn Clock>,
    ) -> Result<ServiceFixture, CraftError> {
        ServiceFixture::new(
            &self.store,
            issuer.address().port(),
            product.address().port(),
            [issuer.root(), product.root()].concat(),
            control()?,
            clock,
        )
    }
    fn no_plaintext_credentials(&self) -> TestResult {
        for entry in fs::read_dir(&self.root)? {
            let path = entry?.path();
            if path.is_file() {
                let bytes = fs::read(path)?;
                for sentinel in [CLIENT_ID, CLIENT_SECRET, ACCESS_TOKEN, "private-sentinel"] {
                    assert!(
                        !bytes
                            .windows(sentinel.len())
                            .any(|part| part == sentinel.as_bytes())
                    );
                }
            }
        }
        Ok(())
    }
}
impl Drop for Setup {
    fn drop(&mut self) {
        if let Ok(root) = self.root.canonicalize()
            && root.starts_with(&self.parent)
            && root != self.parent
        {
            let _ = fs::remove_dir_all(root);
        }
    }
}

struct IssuePolicyClock {
    base: ManualClock,
    store: Mutex<ProfileStore>,
    requests: Arc<std::sync::atomic::AtomicUsize>,
    threshold: usize,
    operation: IssueReadOperation,
    mode: &'static str,
    changed: AtomicBool,
}
impl Clock for IssuePolicyClock {
    fn unix_millis(&self) -> u64 {
        if self.requests.load(Ordering::SeqCst) >= self.threshold
            && !self.changed.load(Ordering::SeqCst)
            && let Ok(store) = self.store.lock()
            && let Ok(control) = control()
        {
            let operations = if self.mode == "remove_operation" {
                vec![if self.operation == IssueReadOperation::Get {
                    IssueReadOperation::Search
                } else {
                    IssueReadOperation::Get
                }]
            } else {
                vec![IssueReadOperation::Get, IssueReadOperation::Search]
            };
            let result = store.set_issue_read_policy(
                "factory",
                IssueReadPolicyUpdate {
                    target: target(),
                    expected_actor: "actor-one".into(),
                    operations,
                    enabled: matches!(self.mode, "changed_native" | "remove_operation"),
                    expected_revision: Some(1),
                },
                true,
                &control,
                |_| Ok(()),
            );
            self.changed.store(result.is_ok(), Ordering::SeqCst);
        }
        self.base.unix_millis()
    }
    fn monotonic_millis(&self) -> u64 {
        self.base.monotonic_millis()
    }
}

#[test]
fn typed_issue_candidates_refuse_configuration_changes_before_auth_and_native_or_buffered_output()
-> TestResult {
    use clap::Parser;
    use craft_cli::issue_reads::command::{
        IssueGetArgs, IssueReadArgs, IssueSearchArgs, PreparedInvocation,
    };
    use craft_cli::profiles::InvocationOptions;
    #[derive(Parser)]
    struct Get {
        #[command(flatten)]
        args: IssueGetArgs,
    }
    #[derive(Parser)]
    struct Search {
        #[command(flatten)]
        args: IssueSearchArgs,
    }
    for operation in [IssueReadOperation::Get, IssueReadOperation::Search] {
        for mode in [
            "success",
            "before_auth",
            "during_auth",
            "after_native",
            "changed_native",
            "remove_operation",
            "buffered",
        ] {
            if operation == IssueReadOperation::Get && mode == "buffered" {
                continue;
            }
            let setup = Setup::new()?;
            setup.store.set_issue_read_policy(
                "factory",
                IssueReadPolicyUpdate {
                    target: target(),
                    expected_actor: "actor-one".into(),
                    operations: vec![IssueReadOperation::Get, IssueReadOperation::Search],
                    enabled: true,
                    expected_revision: None,
                },
                true,
                &control()?,
                |_| Ok(()),
            )?;
            let issuer = issuer("read:jira-user read:jira-work")?;
            let mut replies = vec![identity()];
            if mode != "before_auth" {
                replies.push(identity());
            }
            if !matches!(mode, "before_auth" | "during_auth") {
                replies.push(reply(if operation == IssueReadOperation::Get {
                    issue(1)
                } else {
                    json!({"issues":[issue(1),issue(2)],"isLast":true})
                })?);
            }
            if mode == "buffered" {
                replies.push(identity());
            }
            let product = TlsServer::start(Reply::Sequence(replies))?;
            let clock = Arc::new(IssuePolicyClock {
                base: ManualClock::new(1000),
                store: Mutex::new(ProfileStore::open(&setup.root, &control()?)?),
                requests: product.request_counter(),
                threshold: match mode {
                    "during_auth" => 2,
                    "after_native" | "changed_native" | "remove_operation" => 3,
                    "buffered" => 4,
                    _ => usize::MAX,
                },
                operation,
                mode,
                changed: AtomicBool::new(false),
            });
            let fixture = setup.fixture(&issuer, &product, clock.clone())?;
            let locked = fixture.lock(&setup.grant, &control()?)?;
            fixture.client().provision_grant(
                &locked,
                fixture.service_credentials()?,
                &[target().tenant_ref()],
                Some("actor-one"),
            )?;
            let options = InvocationOptions {
                state_root: Some(&setup.root),
                profile: Some("factory"),
                request_id: "candidate-request",
                max_bytes: 16_384,
            };
            let args = if operation == IssueReadOperation::Get {
                IssueReadArgs::Get(Get::try_parse_from(["candidate", "--issue", "ED-1"])?.args)
            } else {
                IssueReadArgs::Search(
                    Search::try_parse_from([
                        "candidate",
                        "--jql",
                        "project = ED",
                        "--limit",
                        "1",
                        "--reconcile-issue-ids",
                        "1,2",
                    ])?
                    .args,
                )
            };
            let prepared =
                PreparedInvocation::prepare(args, &[], &options, &setup.store, &control()?)?;
            assert_eq!(prepared.grant(), &setup.grant);
            if mode == "before_auth" {
                setup.store.set_issue_read_policy(
                    "factory",
                    IssueReadPolicyUpdate {
                        target: target(),
                        expected_actor: "actor-one".into(),
                        operations: vec![operation],
                        enabled: false,
                        expected_revision: Some(1),
                    },
                    true,
                    &control()?,
                    |_| Ok(()),
                )?;
            }
            if matches!(
                mode,
                "during_auth" | "after_native" | "changed_native" | "remove_operation"
            ) {
                clock.base.advance(Duration::from_secs(120))?;
            }
            let mut result = prepared.execute_cached(
                &setup.store,
                fixture.client(),
                &locked,
                TlsPolicy::AdditionalCaBundle(product.root()),
                control()?,
            );
            if mode == "buffered" {
                assert!(result.envelope.ok, "{:?}", result.envelope.error);
                let args = IssueReadArgs::Search(
                    Search::try_parse_from([
                        "candidate",
                        "--jql",
                        "project = ED",
                        "--cursor",
                        cursor(&result.envelope)?,
                        "--reconcile-issue-ids",
                        "1,2",
                    ])?
                    .args,
                );
                let prepared =
                    PreparedInvocation::prepare(args, &[], &options, &setup.store, &control()?)?;
                clock.base.advance(Duration::from_secs(120))?;
                result = prepared.execute_cached(
                    &setup.store,
                    fixture.client(),
                    &locked,
                    TlsPolicy::AdditionalCaBundle(product.root()),
                    control()?,
                );
            }
            assert_eq!(result.envelope.request_id, "candidate-request");
            if mode == "success" {
                assert!(result.envelope.ok, "{:?}", result.envelope.error);
            } else {
                assert_eq!(
                    code(&result.envelope),
                    Some(if mode == "changed_native" {
                        ErrorCode::VersionConflict
                    } else {
                        ErrorCode::PolicyDenied
                    }),
                    "{} {mode}",
                    operation.command()
                );
                if operation == IssueReadOperation::Get {
                    assert!(data(&result.envelope)?["issue"].is_null());
                } else {
                    assert_eq!(data(&result.envelope)?["items"], json!([]));
                    assert!(result.envelope.page.is_none());
                }
                if mode != "before_auth" {
                    assert_eq!(
                        data(&result.envelope)?["credential_cache_update"]["write_state"],
                        "confirmed"
                    );
                }
                if mode != "before_auth" {
                    assert!(clock.changed.load(Ordering::SeqCst));
                }
            }
            let bytes =
                craft_cli::output::encode(result.envelope, 16_384, result.output_policy)?.bytes;
            assert!(bytes.len() <= 16_384);
            assert_eq!(
                product.finish()?.len(),
                match mode {
                    "before_auth" => 1,
                    "during_auth" => 2,
                    "buffered" => 4,
                    _ => 3,
                }
            );
            assert_eq!(
                issuer.finish()?.len(),
                if matches!(mode, "success" | "before_auth") {
                    1
                } else {
                    2
                }
            );
        }
    }
    Ok(())
}

#[test]
fn get_performs_one_masked_live_read_and_preserves_renewal_and_alias_evidence() -> TestResult {
    for renew in [false, true] {
        let setup = Setup::new()?;
        let issuer = issuer("read:jira-user read:jira-work")?;
        let product = TlsServer::start(Reply::Sequence(vec![
            identity(),
            identity(),
            reply(issue(1))?,
        ]))?;
        let clock = Arc::new(ManualClock::new(1000));
        let fixture = setup.fixture(&issuer, &product, clock.clone())?;
        let locked = fixture.lock(&setup.grant, &control()?)?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        if renew {
            clock.advance(Duration::from_secs(3600))?;
        }
        let result = prepared(
            &setup.grant,
            GET,
            json!({"issue":"OLD-7"}),
            &[],
            None,
            50,
            16_384,
        )?
        .execute_cached(
            fixture.client(),
            &locked,
            &setup.store,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control()?,
        );
        assert!(result.envelope.ok, "{:?}", result.envelope.error);
        let data = data(&result.envelope)?;
        assert_eq!(data["requested_issue"], "OLD-7");
        assert_eq!(data["identity_resolution"], "resolved_key");
        assert_eq!(data["issue"]["id"], "1");
        assert!(data["issue"].get("description").is_none());
        assert_eq!(data["credential_cache_update"].is_null(), !renew);
        assert_eq!(result.envelope.request_id, "original-request");
        let bytes = craft_cli::output::encode(result.envelope, 16_384, result.output_policy)?.bytes;
        assert!(bytes.len() <= 16_384);
        assert!(!String::from_utf8(bytes)?.contains("private-sentinel"));
        assert_eq!(product.connections(), 3);
        assert_eq!(product.request_count(), 3);
        let requests = product.finish()?;
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[2].method, "GET");
        assert!(
            requests[2]
                .target
                .starts_with(&format!("/ex/jira/{CLOUD}/rest/api/3/issue/OLD-7?"))
        );
        assert!(
            requests[2]
                .target
                .contains("fields=id%2Ckey%2Cproject%2Cstatus%2Csummary")
        );
        assert!(requests[2].target.contains("updateHistory=false"));
        assert!(requests[2].target.contains("failFast=true"));
        assert_eq!(
            requests[2].headers.get("authorization").map(String::as_str),
            Some("Bearer craft-synthetic-access-token")
        );
        assert_eq!(issuer.finish()?.len(), if renew { 2 } else { 1 });
        drop(locked);
        drop(fixture);
        setup.no_plaintext_credentials()?;
    }
    Ok(())
}

#[test]
fn search_buffers_remainder_and_resumes_opaque_post_token_after_renewal() -> TestResult {
    let setup = Setup::new()?;
    let issuer = issuer("read:jira-user read:jira-work")?;
    let token = "https://untrusted.invalid/?token=文字";
    let product = TlsServer::start(Reply::Sequence(vec![
        identity(),
        identity(),
        reply(json!({"issues":[issue(1),issue(2)],"isLast":false,"nextPageToken":token}))?,
        identity(),
        identity(),
        reply(json!({"issues":[issue(3)],"isLast":true}))?,
    ]))?;
    let clock = Arc::new(ManualClock::new(1000));
    let fixture = setup.fixture(&issuer, &product, clock.clone())?;
    let locked = fixture.lock(&setup.grant, &control()?)?;
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref()],
        Some("actor-one"),
    )?;
    let first = prepared(
        &setup.grant,
        SEARCH,
        search_input(),
        &["summary"],
        None,
        1,
        16_384,
    )?
    .execute_cached(
        fixture.client(),
        &locked,
        &setup.store,
        TlsPolicy::AdditionalCaBundle(product.root()),
        control()?,
    );
    assert!(first.envelope.ok, "{:?}", first.envelope.error);
    assert_eq!(data(&first.envelope)?["items"][0]["id"], "1");
    clock.advance(Duration::from_secs(120))?;
    let second = prepared(
        &setup.grant,
        SEARCH,
        search_input(),
        &["summary"],
        Some(cursor(&first.envelope)?),
        1,
        16_384,
    )?
    .execute_cached(
        fixture.client(),
        &locked,
        &setup.store,
        TlsPolicy::AdditionalCaBundle(product.root()),
        control()?,
    );
    assert!(second.envelope.ok, "{:?}", second.envelope.error);
    assert_eq!(data(&second.envelope)?["items"][0]["id"], "2");
    assert_eq!(data(&second.envelope)?["cached"], true);
    assert_eq!(
        data(&second.envelope)?["observation"],
        data(&first.envelope)?["observation"]
    );
    assert_eq!(
        data(&second.envelope)?["credential_cache_update"]["write_state"],
        "confirmed"
    );
    let last = prepared(
        &setup.grant,
        SEARCH,
        search_input(),
        &["summary"],
        Some(cursor(&second.envelope)?),
        1,
        16_384,
    )?
    .execute_cached(
        fixture.client(),
        &locked,
        &setup.store,
        TlsPolicy::AdditionalCaBundle(product.root()),
        control()?,
    );
    assert!(last.envelope.ok, "{:?}", last.envelope.error);
    assert_eq!(data(&last.envelope)?["items"][0]["id"], "3");
    assert!(!last.envelope.page.as_ref().ok_or("Missing page")?.has_more);
    assert_eq!(
        last.envelope
            .page
            .as_ref()
            .ok_or("Missing page")?
            .consistency,
        "live"
    );
    let requests = product.finish()?;
    assert_eq!(requests.len(), 6);
    for index in [2, 5] {
        assert_eq!(requests[index].method, "POST");
        assert_eq!(
            requests[index].target,
            format!("/ex/jira/{CLOUD}/rest/api/3/search/jql")
        );
        let body: Value = serde_json::from_slice(&requests[index].body)?;
        assert_eq!(body["jql"], search_input()["jql"]);
        assert_eq!(body["reconcileIssues"], json!([1, 2]));
        assert_eq!(body["fields"], json!(["id", "summary"]));
        assert_eq!(body["maxResults"], 50);
        if index == 5 {
            assert_eq!(body["nextPageToken"], token);
        } else {
            assert!(body.get("nextPageToken").is_none());
        }
    }
    assert_eq!(issuer.finish()?.len(), 2);
    drop(locked);
    drop(fixture);
    setup.no_plaintext_credentials()?;
    Ok(())
}

#[test]
fn changed_search_semantics_refuse_cursor_before_identity_or_exchange() -> TestResult {
    let setup = Setup::new()?;
    let issuer = issuer("read:jira-user read:jira-work")?;
    let product = TlsServer::start(Reply::Sequence(vec![
        identity(),
        identity(),
        reply(json!({"issues":[issue(1),issue(2)],"isLast":true}))?,
    ]))?;
    let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
    let locked = fixture.lock(&setup.grant, &control()?)?;
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref()],
        Some("actor-one"),
    )?;
    let first = prepared(
        &setup.grant,
        SEARCH,
        search_input(),
        &["summary"],
        None,
        1,
        16_384,
    )?
    .execute_cached(
        fixture.client(),
        &locked,
        &setup.store,
        TlsPolicy::AdditionalCaBundle(product.root()),
        control()?,
    );
    assert!(first.envelope.ok, "{:?}", first.envelope.error);
    for change in ["jql", "reconcile", "fields", "size"] {
        let mut input = search_input();
        match change {
            "jql" => input["jql"] = json!("project = OTHER"),
            "reconcile" => input["reconcile_issue_ids"] = json!(["3"]),
            "size" => input["page_size"] = json!(10),
            _ => {}
        }
        let result = prepared(
            &setup.grant,
            SEARCH,
            input,
            if change == "fields" {
                &["description"]
            } else {
                &["summary"]
            },
            Some(cursor(&first.envelope)?),
            1,
            16_384,
        )?
        .execute_cached(
            fixture.client(),
            &locked,
            &setup.store,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control()?,
        );
        assert_eq!(
            code(&result.envelope),
            Some(ErrorCode::CursorMismatch),
            "{change}"
        );
    }
    assert_eq!(product.finish()?.len(), 3);
    assert_eq!(issuer.finish()?.len(), 1);
    Ok(())
}

#[test]
fn search_warning_denial_disconnect_and_bad_data_keep_renewal_receipts_without_successor()
-> TestResult {
    for mode in [
        "warning",
        "forbidden",
        "disconnect",
        "malformed",
        "redirect",
    ] {
        let setup = Setup::new()?;
        let issuer = issuer("read:jira-user read:jira-work")?;
        let response = match mode {
            "warning" => reply(json!({"issues":[],"warnings":[{"message":"private-sentinel"}]}))?,
            "forbidden" => Reply::json(403, br#"{"message":"private-sentinel"}"#, &[]),
            "disconnect" => Reply::Disconnect,
            "redirect" => Reply::json(302, b"{}", &[("Location", "https://untrusted.invalid")]),
            _ => reply(json!({"issues":[{"id":17}]}))?,
        };
        let product = TlsServer::start(Reply::Sequence(vec![identity(), identity(), response]))?;
        let clock = Arc::new(ManualClock::new(1000));
        let fixture = setup.fixture(&issuer, &product, clock.clone())?;
        let locked = fixture.lock(&setup.grant, &control()?)?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        clock.advance(Duration::from_secs(3600))?;
        let result = prepared(&setup.grant, SEARCH, search_input(), &[], None, 20, 16_384)?
            .execute_cached(
                fixture.client(),
                &locked,
                &setup.store,
                TlsPolicy::AdditionalCaBundle(product.root()),
                control()?,
            );
        assert!(!result.envelope.ok, "{mode}");
        assert!(result.envelope.page.is_none());
        assert_eq!(data(&result.envelope)?["items"], json!([]));
        assert_eq!(
            data(&result.envelope)?["credential_cache_update"]["write_state"],
            "confirmed"
        );
        assert!(!serde_json::to_string(&result.envelope)?.contains("private-sentinel"));
        assert_eq!(product.finish()?.len(), 3);
        assert_eq!(issuer.finish()?.len(), 2);
    }
    Ok(())
}

#[test]
fn issue_scope_actor_and_dynamic_policy_denial_prevent_unqualified_output() -> TestResult {
    for mode in ["scope", "actor", "before_auth", "after_get", "after_search"] {
        let setup = Setup::new()?;
        let issuer = issuer(if mode == "scope" {
            "read:jira-user"
        } else {
            "read:jira-user read:jira-work"
        })?;
        let mut replies = vec![identity()];
        if mode != "before_auth" {
            replies.push(identity());
        }
        if mode == "after_get" {
            replies.push(reply(issue(1))?);
        }
        if mode == "after_search" {
            replies.push(reply(json!({"issues":[issue(1)],"isLast":true}))?);
        }
        let product = TlsServer::start(Reply::Sequence(replies))?;
        let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
        let locked = fixture.lock(&setup.grant, &control()?)?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        let command = if mode == "after_search" { SEARCH } else { GET };
        let input = if command == GET {
            json!({"issue":"1"})
        } else {
            search_input()
        };
        let host = if mode == "actor" {
            PreparedIssueRead::prepare(
                CanonicalRequest {
                    command: command.into(),
                    target: target(),
                    input,
                    selection: Selection::default(),
                    preconditions: None,
                },
                "request",
                IssueReadPolicy {
                    target: target(),
                    grant: setup.grant.clone(),
                    expected_actor: "other-actor".into(),
                },
            )?
        } else {
            prepared(&setup.grant, command, input, &[], None, 20, 16_384)?
        };
        let counter = product.request_counter();
        let result = host.execute_cached_with_policy_check(
            fixture.client(),
            &locked,
            &setup.store,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control()?,
            || {
                if (mode == "before_auth" && counter.load(Ordering::SeqCst) >= 1)
                    || (mode.starts_with("after_") && counter.load(Ordering::SeqCst) >= 3)
                {
                    Err(CraftError::new(
                        ErrorCode::PolicyDenied,
                        "Issue policy was revoked.",
                    ))
                } else {
                    Ok(())
                }
            },
        );
        assert!(!result.envelope.ok, "{mode}");
        assert_eq!(
            code(&result.envelope),
            Some(if mode == "scope" {
                ErrorCode::PermissionDenied
            } else {
                ErrorCode::PolicyDenied
            })
        );
        if command == GET {
            assert!(data(&result.envelope)?["issue"].is_null());
        } else {
            assert_eq!(data(&result.envelope)?["items"], json!([]));
        }
        assert_eq!(
            product.finish()?.len(),
            if mode == "before_auth" {
                1
            } else if mode.starts_with("after_") {
                3
            } else {
                2
            }
        );
        assert_eq!(issuer.finish()?.len(), 1);
    }
    Ok(())
}

#[test]
fn oversized_get_drops_body_and_search_retains_same_position_recovery() -> TestResult {
    for command in [GET, SEARCH] {
        let setup = Setup::new()?;
        let issuer = issuer("read:jira-user read:jira-work")?;
        let mut large = issue(1);
        large["fields"]["description"] = json!("文字".repeat(1500));
        let mut replies = vec![
            identity(),
            identity(),
            reply(if command == GET {
                large.clone()
            } else {
                json!({"issues":[large.clone()],"isLast":true})
            })?,
            identity(),
        ];
        if command == GET {
            replies.push(reply(large)?);
        }
        let product = TlsServer::start(Reply::Sequence(replies))?;
        let clock = Arc::new(ManualClock::new(1000));
        let fixture = setup.fixture(&issuer, &product, clock.clone())?;
        let locked = fixture.lock(&setup.grant, &control()?)?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        clock.advance(Duration::from_secs(3600))?;
        let input = if command == GET {
            json!({"issue":"1"})
        } else {
            search_input()
        };
        let first = prepared(
            &setup.grant,
            command,
            input.clone(),
            &["description"],
            None,
            20,
            4096,
        )?
        .execute_cached(
            fixture.client(),
            &locked,
            &setup.store,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control()?,
        );
        assert_eq!(code(&first.envelope), Some(ErrorCode::ItemTooLarge));
        assert_eq!(
            data(&first.envelope)?["credential_cache_update"]["write_state"],
            "confirmed"
        );
        let next = if command == SEARCH {
            Some(cursor(&first.envelope)?)
        } else {
            assert!(first.envelope.page.is_none());
            assert!(data(&first.envelope)?["issue"].is_null());
            None
        };
        let last = prepared(
            &setup.grant,
            command,
            input,
            &["description"],
            next,
            20,
            16_384,
        )?
        .execute_cached(
            fixture.client(),
            &locked,
            &setup.store,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control()?,
        );
        assert!(last.envelope.ok, "{:?}", last.envelope.error);
        if command == SEARCH {
            assert_eq!(data(&last.envelope)?["cached"], true);
            assert_eq!(data(&last.envelope)?["items"][0]["id"], "1");
        }
        let bytes = craft_cli::output::encode(first.envelope, 4096, first.output_policy)?.bytes;
        assert!(bytes.len() <= 4096);
        assert_eq!(product.finish()?.len(), if command == GET { 5 } else { 4 });
        assert_eq!(issuer.finish()?.len(), 2);
    }
    Ok(())
}

#[test]
fn repeated_search_tokens_fail_without_publishing_a_successor() -> TestResult {
    let setup = Setup::new()?;
    let issuer = issuer("read:jira-user read:jira-work")?;
    let product = TlsServer::start(Reply::Sequence(vec![
        identity(),
        identity(),
        reply(json!({"issues":[],"isLast":false,"nextPageToken":"same"}))?,
        identity(),
        reply(json!({"issues":[],"isLast":false,"nextPageToken":"same"}))?,
    ]))?;
    let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
    let locked = fixture.lock(&setup.grant, &control()?)?;
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref()],
        Some("actor-one"),
    )?;
    let first = prepared(&setup.grant, SEARCH, search_input(), &[], None, 20, 16_384)?
        .execute_cached(
            fixture.client(),
            &locked,
            &setup.store,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control()?,
        );
    assert!(first.envelope.ok, "{:?}", first.envelope.error);
    let last = prepared(
        &setup.grant,
        SEARCH,
        search_input(),
        &[],
        Some(cursor(&first.envelope)?),
        20,
        16_384,
    )?
    .execute_cached(
        fixture.client(),
        &locked,
        &setup.store,
        TlsPolicy::AdditionalCaBundle(product.root()),
        control()?,
    );
    assert_eq!(code(&last.envelope), Some(ErrorCode::PaginationStalled));
    assert!(last.envelope.page.is_none());
    assert_eq!(product.finish()?.len(), 5);
    assert_eq!(issuer.finish()?.len(), 1);
    Ok(())
}

#[test]
fn invocation_control_bounds_identity_and_search_io_without_retry() -> TestResult {
    for stage in ["identity", "search"] {
        let setup = Setup::new()?;
        let issuer = issuer("read:jira-user read:jira-work")?;
        let mut replies = vec![identity()];
        if stage == "search" {
            replies.push(identity());
        }
        replies.push(Reply::Stall);
        let product = TlsServer::start(Reply::Sequence(replies))?;
        let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
        let locked = fixture.lock(&setup.grant, &control()?)?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        let clock = Arc::new(ManualClock::new(1000));
        let cancel = CancellationToken::default();
        let invocation = InvocationControl::new(clock.clone(), cancel.clone(), 10_000)?;
        let counter = product.request_counter();
        let threshold = if stage == "identity" { 2 } else { 3 };
        let worker = std::thread::spawn(move || -> Result<(), String> {
            let start = std::time::Instant::now();
            while counter.load(Ordering::SeqCst) < threshold {
                if start.elapsed() > Duration::from_secs(5) {
                    return Err("Expected request was not observed".into());
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            if stage == "identity" {
                clock
                    .advance(Duration::from_millis(10_000))
                    .map_err(|error| error.to_string())?;
            } else {
                cancel.cancel();
            }
            Ok(())
        });
        let result = prepared(&setup.grant, SEARCH, search_input(), &[], None, 20, 16_384)?
            .execute_cached(
                fixture.client(),
                &locked,
                &setup.store,
                TlsPolicy::AdditionalCaBundle(product.root()),
                invocation,
            );
        worker
            .join()
            .map_err(|_| "Control worker panicked")?
            .map_err(|error| error.to_string())?;
        assert_eq!(
            code(&result.envelope),
            Some(if stage == "identity" {
                ErrorCode::DeadlineExceeded
            } else {
                ErrorCode::Cancelled
            })
        );
        assert!(result.envelope.page.is_none());
        assert_eq!(product.finish()?.len(), threshold);
        assert_eq!(issuer.finish()?.len(), 1);
    }
    Ok(())
}
