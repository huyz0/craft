//! Owned TLS qualification for current-user permissions and explicit context semantics.
#[path = "../../craft-runtime/src/transport/tls_fixture.rs"]
mod tls_fixture;
use craft_cli::permission_reads::{PermissionReadPolicy, PreparedPermissionRead};
use craft_core::contracts::{
    AuthMode, CanonicalRequest, Envelope, GrantRef, ResourceRef, Selection,
};
use craft_core::control::{CancellationToken, InvocationControl};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::profiles::ProfileInput;
use craft_core::storage::Clock;
use craft_provider_jira::permissions::{COMMAND, JiraPermissions};
use craft_runtime::control::SystemClock;
use craft_runtime::profiles::ProfileStore;
use craft_runtime::synthetic_acceptance::{ACCESS_TOKEN, CLIENT_ID, CLIENT_SECRET, ServiceFixture};
use craft_runtime::transport::TlsPolicy;
use craft_test_support::state::ManualClock;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tls_fixture::{Reply, TlsServer};
type TestResult = Result<(), Box<dyn std::error::Error>>;
const CLOUD: &str = "00000000-1111-2222-3333-444444444444";
static ROOT_SEQUENCE: AtomicU64 = AtomicU64::new(0);
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
    TlsServer::start(reply(
        json!({"access_token":ACCESS_TOKEN,"expires_in":120,"token_type":"Bearer","scope":scopes}),
    )?)
}
fn reply(body: Value) -> Result<Reply, serde_json::Error> {
    Ok(Reply::json(200, &serde_json::to_vec(&body)?, &[]))
}
fn row(key: &str, decision: bool) -> Value {
    json!({"key":key,"id":"12","havePermission":decision,"type":"PROJECT","name":"Permission 文字","description":"unselected-sentinel","instruction":"unselected-sentinel"})
}
fn body() -> Value {
    json!({"permissions":{"EDIT_ISSUES":row("EDIT_ISSUES",true),"BROWSE_PROJECTS":row("BROWSE_PROJECTS",false)}})
}
fn prepared(
    grant: &GrantRef,
    input: Value,
    fields: &[&str],
    cursor: Option<&str>,
    limit: u16,
    max_bytes: u32,
) -> Result<PreparedPermissionRead, CraftError> {
    PreparedPermissionRead::prepare(
        CanonicalRequest {
            command: COMMAND.into(),
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
        PermissionReadPolicy {
            target: target(),
            grant: grant.clone(),
            expected_actor: "actor-one".into(),
        },
    )
}
fn data(envelope: &Envelope) -> Result<&Value, Box<dyn std::error::Error>> {
    envelope.data.as_ref().ok_or_else(|| "Missing data".into())
}
fn cursor(envelope: &Envelope) -> Result<&str, Box<dyn std::error::Error>> {
    envelope
        .page
        .as_ref()
        .and_then(|page| page.next_cursor.as_deref())
        .ok_or_else(|| "Missing cursor".into())
}
fn code(envelope: &Envelope) -> Option<ErrorCode> {
    envelope.error.as_ref().map(|error| error.code)
}
fn validate(envelope: &Envelope, fields: &[&str], max_bytes: u32) -> TestResult {
    let mut registry = craft_core::registry::Registry::default();
    registry.register_provider(&JiraPermissions)?;
    let descriptor = registry.descriptor(&envelope.command)?;
    let selected: Vec<_> = fields.iter().map(|value| (*value).into()).collect();
    let projection = registry
        .projection(&envelope.command, &selected)?
        .ok_or("Missing projection")?;
    let schema = projection.response_schema(&descriptor.response_schema)?;
    let validator = jsonschema::validator_for(&schema)?;
    let value = serde_json::to_value(envelope)?;
    let errors: Vec<_> = validator
        .iter_errors(&value)
        .map(|error| error.to_string())
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    assert!(serde_json::to_vec(envelope)?.len() < max_bytes as usize);
    assert_eq!(envelope.request_id, "original-request");
    assert_eq!(data(envelope)?["atomic_snapshot"], false);
    assert_eq!(data(envelope)?["mutation_authorized"], false);
    Ok(())
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
        let sequence = ROOT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = parent.join(format!(
            "composed-permissions-{}-{stamp}-{sequence}",
            std::process::id()
        ));
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
                for sentinel in [
                    CLIENT_ID,
                    CLIENT_SECRET,
                    ACCESS_TOKEN,
                    "unselected-sentinel",
                ] {
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

#[test]
fn permission_context_queries_use_one_get_and_keep_global_project_issue_comment_semantics()
-> TestResult {
    for (field, value, parameter, kind) in [
        ("global", "", "", "global"),
        ("project", "ED", "projectKey=ED", "project"),
        ("project", "100", "projectId=100", "project"),
        ("issue", "OLD-1", "issueKey=OLD-1", "issue"),
        ("issue", "10000", "issueId=10000", "issue"),
        ("comment_id", "12", "commentId=12", "comment"),
    ] {
        let setup = Setup::new()?;
        let issuer = issuer("read:jira-user read:jira-work")?;
        let product = TlsServer::start(Reply::Sequence(vec![
            identity(),
            identity(),
            reply(json!({"permissions":{"BROWSE_PROJECTS":row("BROWSE_PROJECTS",true)}}))?,
        ]))?;
        let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
        let locked = fixture.lock(&setup.grant, &control()?)?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        let mut input = json!({"permissions":["BROWSE_PROJECTS"]});
        if field != "global" {
            input[field] = json!(value);
        }
        let result = prepared(&setup.grant, input, &[], None, 20, 16_384)?.execute_cached(
            fixture.client(),
            &locked,
            &setup.store,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control()?,
        );
        assert!(result.envelope.ok, "{:?}", result.envelope.error);
        validate(&result.envelope, &[], 16_384)?;
        assert_eq!(data(&result.envelope)?["context"]["kind"], kind);
        assert_eq!(data(&result.envelope)?["items"][0]["have_permission"], true);
        let records = product.finish()?;
        assert_eq!(records.len(), 3);
        let request = &records[2];
        assert_eq!(request.method, "GET");
        assert!(request.body.is_empty());
        assert!(
            request
                .target
                .starts_with(&format!("/ex/jira/{CLOUD}/rest/api/3/mypermissions?"))
        );
        assert!(request.target.contains("permissions=BROWSE_PROJECTS"));
        if !parameter.is_empty() {
            assert!(request.target.contains(parameter));
        }
        assert_eq!(
            request.headers.get("authorization").map(String::as_str),
            Some("Bearer craft-synthetic-access-token")
        );
        assert_eq!(issuer.finish()?.len(), 1);
    }
    Ok(())
}

#[test]
fn permission_remainders_reopen_and_replay_with_false_and_unknown_after_renewal() -> TestResult {
    let setup = Setup::new()?;
    let issuer = issuer("read:jira-user read:jira-work")?;
    let product = TlsServer::start(Reply::Sequence(vec![
        identity(),
        identity(),
        reply(body())?,
        identity(),
        identity(),
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
    clock.advance(Duration::from_secs(120))?;
    let input =
        json!({"permissions":["EDIT_ISSUES","BROWSE_PROJECTS","plugin:unknown"],"issue":"OLD-1"});
    let first = prepared(&setup.grant, input.clone(), &[], None, 1, 16_384)?.execute_cached(
        fixture.client(),
        &locked,
        &setup.store,
        TlsPolicy::AdditionalCaBundle(product.root()),
        control()?,
    );
    assert!(first.envelope.ok, "{:?}", first.envelope.error);
    validate(&first.envelope, &[], 16_384)?;
    assert_eq!(
        data(&first.envelope)?["credential_cache_update"]["write_state"],
        "confirmed"
    );
    let reopened = ProfileStore::open(&setup.root, &control()?)?;
    clock.advance(Duration::from_secs(120))?;
    for limit in [2, 1] {
        let result = prepared(
            &setup.grant,
            input.clone(),
            &[],
            Some(cursor(&first.envelope)?),
            limit,
            16_384,
        )?
        .execute_cached(
            fixture.client(),
            &locked,
            &reopened,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control()?,
        );
        assert!(result.envelope.ok, "{:?}", result.envelope.error);
        validate(&result.envelope, &[], 16_384)?;
        assert_eq!(data(&result.envelope)?["cached"], true);
        assert_eq!(
            data(&result.envelope)?["items"][0]["have_permission"],
            false
        );
        if limit == 2 {
            assert_eq!(data(&result.envelope)?["items"][1]["returned"], false);
            assert!(data(&result.envelope)?["items"][1]["have_permission"].is_null());
        }
        for field in ["context", "requested_permissions", "observation"] {
            assert_eq!(
                data(&result.envelope)?[field],
                data(&first.envelope)?[field]
            );
        }
    }
    let output = craft_cli::output::encode(first.envelope, 16_384, first.output_policy)?;
    assert!(!String::from_utf8(output.bytes)?.contains("unselected-sentinel"));
    drop(locked);
    setup.no_plaintext_credentials()?;
    let records = product.finish()?;
    assert_eq!(records.len(), 5);
    assert!(
        records[2]
            .target
            .contains("permissions=EDIT_ISSUES%2CBROWSE_PROJECTS%2Cplugin%3Aunknown")
    );
    assert_eq!(issuer.finish()?.len(), 3);
    Ok(())
}

#[test]
fn permission_cursor_context_key_order_and_projection_mismatch_refuse_before_auth() -> TestResult {
    let setup = Setup::new()?;
    let issuer = issuer("read:jira-user read:jira-work")?;
    let product = TlsServer::start(Reply::Sequence(vec![
        identity(),
        identity(),
        reply(body())?,
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
    let input = json!({"permissions":["EDIT_ISSUES","BROWSE_PROJECTS"],"issue":"OLD-1"});
    let first = prepared(&setup.grant, input.clone(), &[], None, 1, 16_384)?.execute_cached(
        fixture.client(),
        &locked,
        &setup.store,
        TlsPolicy::AdditionalCaBundle(product.root()),
        control()?,
    );
    assert!(first.envelope.ok, "{:?}", first.envelope.error);
    clock.advance(Duration::from_secs(120))?;
    for mode in ["context", "keys", "order", "fields"] {
        let mut changed = input.clone();
        let mut fields = vec![];
        match mode {
            "context" => changed["issue"] = json!("OLD-2"),
            "keys" => changed["permissions"] = json!(["EDIT_ISSUES"]),
            "order" => changed["permissions"] = json!(["BROWSE_PROJECTS", "EDIT_ISSUES"]),
            _ => fields = vec!["description"],
        }
        let result = prepared(
            &setup.grant,
            changed,
            &fields,
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
        assert_eq!(
            code(&result.envelope),
            Some(ErrorCode::CursorMismatch),
            "{mode}"
        );
        validate(&result.envelope, &fields, 16_384)?;
        assert_eq!(data(&result.envelope)?["items"], json!([]));
    }
    assert_eq!(product.finish()?.len(), 3);
    assert_eq!(issuer.finish()?.len(), 1);
    Ok(())
}

struct DeadlineClock(Arc<AtomicUsize>);
impl Clock for DeadlineClock {
    fn unix_millis(&self) -> u64 {
        1000
    }
    fn monotonic_millis(&self) -> u64 {
        if self.0.load(Ordering::SeqCst) >= 3 {
            1001
        } else {
            0
        }
    }
}

#[test]
fn permission_denials_bad_json_warnings_and_interrupted_reads_keep_cache_receipts_without_retry()
-> TestResult {
    for mode in ["403", "warning", "duplicate", "disconnect", "deadline"] {
        let setup = Setup::new()?;
        let issuer = issuer("read:jira-user read:jira-work")?;
        let resource=match mode {"403"=>Reply::json(403,br#"{"errorMessages":["private-sentinel"]}"#,&[]),"warning"=>reply(json!({"permissions":{},"warningMessages":["private-sentinel"]}))?,"duplicate"=>Reply::json(200,br#"{"permissions":{"EDIT_ISSUES":{"havePermission":true,"havePermission":false}}}"#,&[]),"disconnect"=>Reply::Disconnect,_=>Reply::Stall};
        let product = TlsServer::start(Reply::Sequence(vec![identity(), identity(), resource]))?;
        let clock = Arc::new(ManualClock::new(1000));
        let fixture = setup.fixture(&issuer, &product, clock.clone())?;
        let locked = fixture.lock(&setup.grant, &control()?)?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        clock.advance(Duration::from_secs(120))?;
        let invocation = if matches!(mode, "deadline" | "disconnect") {
            InvocationControl::new(
                Arc::new(DeadlineClock(product.request_counter())),
                CancellationToken::default(),
                1000,
            )?
        } else {
            control()?
        };
        let result = prepared(
            &setup.grant,
            json!({"permissions":["EDIT_ISSUES"]}),
            &[],
            None,
            20,
            16_384,
        )?
        .execute_cached(
            fixture.client(),
            &locked,
            &setup.store,
            TlsPolicy::AdditionalCaBundle(product.root()),
            invocation,
        );
        assert!(!result.envelope.ok);
        validate(&result.envelope, &[], 16_384)?;
        if mode == "deadline" {
            assert_eq!(code(&result.envelope), Some(ErrorCode::DeadlineExceeded));
        }
        assert_eq!(data(&result.envelope)?["items"], json!([]));
        assert!(result.envelope.page.is_none());
        assert_eq!(
            data(&result.envelope)?["credential_cache_update"]["write_state"],
            "confirmed"
        );
        assert!(!serde_json::to_string(&result.envelope)?.contains("private-sentinel"));
        assert_eq!(product.request_count(), 3);
        assert_eq!(product.connections(), 3);
        assert_eq!(product.finish()?.len(), 3);
        assert_eq!(issuer.finish()?.len(), 2);
    }
    Ok(())
}

#[test]
fn oversized_permission_metadata_recovers_exact_inert_description_from_buffer() -> TestResult {
    let setup = Setup::new()?;
    let issuer = issuer("read:jira-user read:jira-work")?;
    let mut raw = json!({"permissions":{"EDIT_ISSUES":row("EDIT_ISSUES",true)}});
    raw["permissions"]["EDIT_ISSUES"]["description"] = json!("\\".repeat(8192));
    let product = TlsServer::start(Reply::Sequence(vec![
        identity(),
        identity(),
        reply(raw)?,
        identity(),
    ]))?;
    let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
    let locked = fixture.lock(&setup.grant, &control()?)?;
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref()],
        Some("actor-one"),
    )?;
    let input = json!({"permissions":["EDIT_ISSUES"],"project":"ED"});
    let fields = ["description"];
    let first = prepared(&setup.grant, input.clone(), &fields, None, 1, 8192)?.execute_cached(
        fixture.client(),
        &locked,
        &setup.store,
        TlsPolicy::AdditionalCaBundle(product.root()),
        control()?,
    );
    assert_eq!(code(&first.envelope), Some(ErrorCode::ItemTooLarge));
    validate(&first.envelope, &fields, 8192)?;
    let recovered = prepared(
        &setup.grant,
        input,
        &fields,
        Some(cursor(&first.envelope)?),
        1,
        65_536,
    )?
    .execute_cached(
        fixture.client(),
        &locked,
        &setup.store,
        TlsPolicy::AdditionalCaBundle(product.root()),
        control()?,
    );
    assert!(recovered.envelope.ok, "{:?}", recovered.envelope.error);
    validate(&recovered.envelope, &fields, 65_536)?;
    assert_eq!(data(&recovered.envelope)?["cached"], true);
    assert_eq!(
        data(&recovered.envelope)?["items"][0]["description"],
        "\\".repeat(8192)
    );
    assert_eq!(
        data(&recovered.envelope)?["context"],
        data(&first.envelope)?["context"]
    );
    assert_eq!(product.finish()?.len(), 4);
    assert_eq!(issuer.finish()?.len(), 1);
    Ok(())
}

struct ConfigClock {
    base: ManualClock,
    store: Mutex<ProfileStore>,
    requests: Arc<AtomicUsize>,
    threshold: usize,
    mode: &'static str,
    changed: AtomicBool,
}
impl Clock for ConfigClock {
    fn unix_millis(&self) -> u64 {
        use craft_runtime::issue_read_policy::{IssueReadOperation, IssueReadPolicyUpdate};
        if self.requests.load(Ordering::SeqCst) >= self.threshold
            && !self.changed.load(Ordering::SeqCst)
            && let Ok(store) = self.store.lock()
            && let Ok(control) = control()
        {
            let result = store.set_issue_read_policy(
                "factory",
                IssueReadPolicyUpdate {
                    target: target(),
                    expected_actor: "actor-one".into(),
                    operations: vec![if self.mode == "remove" {
                        IssueReadOperation::Get
                    } else {
                        IssueReadOperation::Permissions
                    }],
                    enabled: matches!(self.mode, "changed" | "remove"),
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
fn parsed_permission_candidates_obey_policy_changes_and_missing_scope_on_native_and_cached_paths()
-> TestResult {
    use clap::Parser;
    use craft_cli::permission_reads::command::{PermissionCheckArgs, PreparedInvocation};
    use craft_cli::profiles::InvocationOptions;
    use craft_runtime::issue_read_policy::{IssueReadOperation, IssueReadPolicyUpdate};
    #[derive(Parser)]
    struct Candidate {
        #[command(flatten)]
        args: PermissionCheckArgs,
    }
    for mode in [
        "success",
        "scope",
        "before_auth",
        "during_auth",
        "after_native",
        "changed",
        "remove",
        "buffered",
    ] {
        let setup = Setup::new()?;
        let issuer = issuer(if mode == "scope" {
            "read:jira-user"
        } else {
            "read:jira-user read:jira-work"
        })?;
        let product = TlsServer::start(Reply::Sequence(vec![
            identity(),
            identity(),
            reply(body())?,
            identity(),
        ]))?;
        let record = || IssueReadPolicyUpdate {
            target: target(),
            expected_actor: "actor-one".into(),
            operations: vec![IssueReadOperation::Permissions],
            enabled: true,
            expected_revision: None,
        };
        setup
            .store
            .set_issue_read_policy("factory", record(), true, &control()?, |_| Ok(()))?;
        let clock = Arc::new(ConfigClock {
            base: ManualClock::new(1000),
            store: Mutex::new(ProfileStore::open(&setup.root, &control()?)?),
            requests: product.request_counter(),
            threshold: match mode {
                "during_auth" => 2,
                "buffered" => 4,
                _ => 3,
            },
            mode,
            changed: AtomicBool::new(matches!(mode, "success" | "scope" | "before_auth")),
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
            request_id: "original-request",
            max_bytes: 16_384,
        };
        let args = Candidate::try_parse_from([
            "candidate",
            "--permission",
            "EDIT_ISSUES",
            "--permission",
            "BROWSE_PROJECTS",
            "--issue",
            "OLD-1",
            "--limit",
            "1",
        ])?
        .args;
        let prepared = PreparedInvocation::prepare(args, &[], &options, &setup.store, &control()?)?;
        if mode == "before_auth" {
            let mut record = record();
            record.enabled = false;
            record.expected_revision = Some(1);
            setup
                .store
                .set_issue_read_policy("factory", record, true, &control()?, |_| Ok(()))?;
        }
        if !matches!(mode, "success" | "before_auth") {
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
            let args = Candidate::try_parse_from([
                "candidate",
                "--permission",
                "EDIT_ISSUES",
                "--permission",
                "BROWSE_PROJECTS",
                "--issue",
                "OLD-1",
                "--cursor",
                cursor(&result.envelope)?,
            ])?
            .args;
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
        validate(&result.envelope, &[], 16_384)?;
        if mode == "success" {
            assert!(result.envelope.ok, "{:?}", result.envelope.error);
        } else {
            assert_eq!(
                code(&result.envelope),
                Some(if mode == "scope" {
                    ErrorCode::PermissionDenied
                } else if mode == "changed" {
                    ErrorCode::VersionConflict
                } else {
                    ErrorCode::PolicyDenied
                }),
                "{mode}"
            );
            assert_eq!(data(&result.envelope)?["items"], json!([]));
            assert!(result.envelope.page.is_none());
            if mode != "before_auth" {
                assert_eq!(
                    data(&result.envelope)?["credential_cache_update"]["write_state"],
                    "confirmed"
                );
            }
        }
        assert_eq!(
            product.finish()?.len(),
            match mode {
                "before_auth" => 1,
                "scope" | "during_auth" => 2,
                "buffered" => 4,
                _ => 3,
            },
            "{mode}"
        );
        assert_eq!(
            issuer.finish()?.len(),
            match mode {
                "success" | "before_auth" => 1,
                "buffered" => 3,
                _ => 2,
            }
        );
    }
    Ok(())
}
