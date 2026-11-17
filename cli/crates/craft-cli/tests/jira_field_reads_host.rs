//! Owned TLS acceptance of field discovery, service access and immutable buffered pages.
#[path = "support/conformance.rs"]
mod conformance;
use conformance::Observed;
#[path = "../../craft-runtime/src/transport/tls_fixture.rs"]
mod tls_fixture;
use craft_cli::field_reads::{FieldReadPolicy, PreparedFieldRead};
use craft_core::contracts::{
    AuthMode, CanonicalRequest, Envelope, GrantRef, ResourceRef, Selection,
};
use craft_core::control::{CancellationToken, InvocationControl};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::profiles::ProfileInput;
use craft_core::storage::Clock;
use craft_provider_jira::fields::{JiraFields, LIST, SEARCH};
use craft_runtime::control::SystemClock;
use craft_runtime::profiles::ProfileStore;
use craft_runtime::synthetic_acceptance::{ACCESS_TOKEN, CLIENT_ID, CLIENT_SECRET, ServiceFixture};
use craft_runtime::transport::TlsPolicy;
use craft_test_support::state::ManualClock;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tls_fixture::{Reply, TlsServer};
type TestResult = Result<(), Box<dyn std::error::Error>>;
const CLOUD: &str = "00000000-1111-2222-3333-444444444444";

struct ConfigClock {
    base: ManualClock,
    store: Mutex<ProfileStore>,
    requests: Arc<AtomicUsize>,
    threshold: usize,
    operation: craft_runtime::field_read_policy::FieldReadOperation,
    mode: &'static str,
    changed: AtomicBool,
}
impl Clock for ConfigClock {
    fn unix_millis(&self) -> u64 {
        use craft_runtime::field_read_policy::{FieldReadOperation, FieldReadPolicyUpdate};
        if self.requests.load(Ordering::SeqCst) >= self.threshold
            && !self.changed.load(Ordering::SeqCst)
            && let Ok(store) = self.store.lock()
            && let Ok(control) = control()
        {
            let result = store.set_field_read_policy(
                "factory",
                FieldReadPolicyUpdate {
                    target: target(),
                    expected_actor: "actor-one".into(),
                    continuation_site: "https://craft-test.atlassian.net".into(),
                    operations: vec![if self.mode == "remove_operation" {
                        if self.operation == FieldReadOperation::List {
                            FieldReadOperation::Search
                        } else {
                            FieldReadOperation::List
                        }
                    } else {
                        self.operation
                    }],
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
fn parsed_field_candidates_use_separate_records_and_stop_on_captured_configuration_changes()
-> TestResult {
    use clap::Parser;
    use craft_cli::field_reads::command::{
        FieldListArgs, FieldReadArgs, FieldSearchArgs, PreparedInvocation,
    };
    use craft_cli::profiles::InvocationOptions;
    use craft_runtime::field_read_policy::{FieldReadOperation, FieldReadPolicyUpdate};
    #[derive(Parser)]
    struct List {
        #[command(flatten)]
        args: FieldListArgs,
    }
    #[derive(Parser)]
    struct Search {
        #[command(flatten)]
        args: FieldSearchArgs,
    }
    for operation in [FieldReadOperation::List, FieldReadOperation::Search] {
        for mode in [
            "success",
            "before_auth",
            "during_auth",
            "after_native",
            "changed_native",
            "remove_operation",
            "buffered",
        ] {
            let setup = Setup::new()?;
            let issuer = issuer("read:jira-user read:jira-work")?;
            let values = json!([field("customfield_1"), field("customfield_2")]);
            let body = if operation == FieldReadOperation::List {
                values
            } else {
                json!({"startAt":0,"maxResults":50,"isLast":true,"values":values})
            };
            let product = TlsServer::start(Reply::Sequence(vec![
                identity(),
                identity(),
                reply(body)?,
                identity(),
            ]))?;
            setup.store.set_field_read_policy(
                "factory",
                FieldReadPolicyUpdate {
                    target: target(),
                    expected_actor: "actor-one".into(),
                    continuation_site: "https://craft-test.atlassian.net".into(),
                    operations: vec![operation],
                    enabled: true,
                    expected_revision: None,
                },
                true,
                &control()?,
                |_| Ok(()),
            )?;
            let clock = Arc::new(ConfigClock {
                base: ManualClock::new(1000),
                store: Mutex::new(ProfileStore::open(&setup.root, &control()?)?),
                requests: product.request_counter(),
                threshold: match mode {
                    "during_auth" => 2,
                    "buffered" => 4,
                    _ => 3,
                },
                operation,
                mode,
                changed: AtomicBool::new(matches!(mode, "success" | "before_auth")),
            });
            let fixture = setup.fixture(&issuer, &product, clock.clone())?;
            let locked = fixture.lock(&setup.grant, &control()?)?;
            fixture.client().provision_grant(
                &locked,
                fixture.service_credentials()?,
                &[target().tenant_ref()],
                Some("actor-one"),
            )?;
            let args = if operation == FieldReadOperation::List {
                FieldReadArgs::List(List::try_parse_from(["candidate", "--limit", "1"])?.args)
            } else {
                FieldReadArgs::Search(Search::try_parse_from(["candidate", "--limit", "1"])?.args)
            };
            let options = InvocationOptions {
                state_root: Some(&setup.root),
                profile: Some("factory"),
                request_id: "candidate-request",
                max_bytes: 16_384,
            };
            let prepared =
                PreparedInvocation::prepare(args, &[], &options, &setup.store, &control()?)?;
            if mode == "before_auth" {
                setup.store.set_field_read_policy(
                    "factory",
                    FieldReadPolicyUpdate {
                        target: target(),
                        expected_actor: "actor-one".into(),
                        continuation_site: "https://craft-test.atlassian.net".into(),
                        operations: vec![operation],
                        enabled: false,
                        expected_revision: Some(1),
                    },
                    true,
                    &control()?,
                    |_| Ok(()),
                )?;
            }
            if !matches!(mode, "success" | "before_auth") {
                clock.base.advance(Duration::from_secs(120))?;
            }
            let mut result = prepared
                .execute_cached(
                    &setup.store,
                    fixture.client(),
                    &locked,
                    TlsPolicy::AdditionalCaBundle(product.root()),
                    control()?,
                )
                .observed();
            if mode == "buffered" {
                assert!(result.envelope.ok, "{:?}", result.envelope.error);
                let next = cursor(&result.envelope)?;
                let args = if operation == FieldReadOperation::List {
                    FieldReadArgs::List(List::try_parse_from(["candidate", "--cursor", next])?.args)
                } else {
                    FieldReadArgs::Search(
                        Search::try_parse_from(["candidate", "--cursor", next])?.args,
                    )
                };
                let prepared =
                    PreparedInvocation::prepare(args, &[], &options, &setup.store, &control()?)?;
                clock.base.advance(Duration::from_secs(120))?;
                result = prepared
                    .execute_cached(
                        &setup.store,
                        fixture.client(),
                        &locked,
                        TlsPolicy::AdditionalCaBundle(product.root()),
                        control()?,
                    )
                    .observed();
            }
            assert_eq!(result.envelope.request_id, "candidate-request");
            schema(&result.envelope, &[])?;
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
                assert_eq!(data(&result.envelope)?["items"], json!([]));
                assert!(result.envelope.page.is_none());
                if mode != "before_auth" {
                    assert!(clock.changed.load(Ordering::SeqCst));
                    assert_eq!(
                        data(&result.envelope)?["credential_cache_update"]["write_state"],
                        "confirmed"
                    );
                }
            }
            assert!(
                setup
                    .store
                    .issue_read_policy("factory", &control()?)?
                    .is_none()
            );
            assert!(
                setup
                    .store
                    .project_read_policy("factory", &control()?)?
                    .is_none()
            );
            assert!(
                craft_cli::output::encode(result.envelope, 16_384, result.output_policy)?
                    .bytes
                    .len()
                    <= 16_384
            );
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
                match mode {
                    "success" | "before_auth" => 1,
                    "buffered" => 3,
                    _ => 2,
                }
            );
        }
    }
    Ok(())
}
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
fn reply(body: Value) -> Result<Reply, serde_json::Error> {
    Ok(Reply::json(200, &serde_json::to_vec(&body)?, &[]))
}
fn field(id: &str) -> Value {
    json!({"id":id,"name":format!("Field {id} 文字"),"custom":true,"schema":{"type":"string","custom":"plugin:inert","configuration":{"$ref":"https://untrusted.invalid","instructions":"private-sentinel"}},"description":"private-sentinel","self":"https://untrusted.invalid"})
}
fn selection(fields: &[&str], cursor: Option<&str>, limit: u16, max_bytes: u32) -> Selection {
    Selection {
        fields: fields.iter().map(|field| (*field).into()).collect(),
        cursor: cursor.map(String::from),
        limit,
        max_bytes,
    }
}
fn prepared(
    grant: &GrantRef,
    command: &str,
    input: Value,
    selection: Selection,
) -> Result<PreparedFieldRead, CraftError> {
    PreparedFieldRead::prepare(
        CanonicalRequest {
            command: command.into(),
            target: target(),
            input,
            selection,
            preconditions: None,
        },
        "original-request",
        FieldReadPolicy {
            target: target(),
            grant: grant.clone(),
            expected_actor: "actor-one".into(),
            continuation_site: "https://craft-test.atlassian.net".into(),
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
fn schema(envelope: &Envelope, selected: &[&str]) -> TestResult {
    let mut registry = craft_core::registry::Registry::default();
    registry.register_provider(&JiraFields)?;
    let descriptor = registry.descriptor(&envelope.command)?;
    let projection = registry
        .projection(
            &envelope.command,
            &selected
                .iter()
                .map(|field| (*field).into())
                .collect::<Vec<_>>(),
        )?
        .ok_or("Missing projection")?;
    assert!(
        jsonschema::validator_for(&projection.response_schema(&descriptor.response_schema)?)?
            .is_valid(&serde_json::to_value(envelope)?)
    );
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
        let root = craft_test_support::state::fixture_root(&parent, "composed-field-reads", stamp);
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

#[test]
fn field_list_buffers_terminal_records_across_reopen_without_refetching() -> TestResult {
    let setup = Setup::new()?;
    let issuer = issuer("read:jira-user read:jira-work")?;
    let product = TlsServer::start(Reply::Sequence(vec![
        identity(),
        identity(),
        reply(json!([field("customfield_1"), field("customfield_2")]))?,
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
    let first = prepared(
        &setup.grant,
        LIST,
        json!({}),
        selection(&[], None, 1, 16_384),
    )?
    .execute_cached(
        fixture.client(),
        &locked,
        &setup.store,
        TlsPolicy::AdditionalCaBundle(product.root()),
        control()?,
    )
    .observed();
    assert!(first.envelope.ok, "{:?}", first.envelope.error);
    schema(&first.envelope, &[])?;
    assert_eq!(data(&first.envelope)?["coverage"], "visibility_dependent");
    assert_eq!(
        data(&first.envelope)?["credential_cache_update"]["write_state"],
        "confirmed"
    );
    let reopened = ProfileStore::open(&setup.root, &control()?)?;
    let last = prepared(
        &setup.grant,
        LIST,
        json!({}),
        selection(&[], Some(cursor(&first.envelope)?), 20, 16_384),
    )?
    .execute_cached(
        fixture.client(),
        &locked,
        &reopened,
        TlsPolicy::AdditionalCaBundle(product.root()),
        control()?,
    )
    .observed();
    assert!(last.envelope.ok, "{:?}", last.envelope.error);
    schema(&last.envelope, &[])?;
    assert_eq!(data(&last.envelope)?["cached"], true);
    assert_eq!(data(&last.envelope)?["items"][0]["id"], "customfield_2");
    assert!(!last.envelope.page.as_ref().ok_or("Missing page")?.has_more);
    assert_eq!(
        data(&first.envelope)?["observation"],
        data(&last.envelope)?["observation"]
    );
    let bytes = craft_cli::output::encode(first.envelope, 16_384, first.output_policy)?.bytes;
    assert!(!String::from_utf8(bytes)?.contains("private-sentinel"));
    drop(locked);
    setup.no_plaintext_credentials()?;
    let records = product.finish()?;
    assert_eq!(records.len(), 4);
    assert_eq!(
        records[2].target,
        format!("/ex/jira/{CLOUD}/rest/api/3/field")
    );
    assert_eq!(issuer.finish()?.len(), 2);
    Ok(())
}

#[test]
fn field_search_site_links_preserve_wire_arrays_and_fetch_one_page_after_remainders() -> TestResult
{
    let setup = Setup::new()?;
    let issuer = issuer("read:jira-user read:jira-work")?;
    let next = "https://craft-test.atlassian.net/rest/api/3/field/search?startAt=2&maxResults=2&id=customfield_1&id=customfield_2&id=customfield_3";
    let product = TlsServer::start(Reply::Sequence(vec![
        identity(),
        identity(),
        reply(
            json!({"startAt":0,"maxResults":2,"isLast":false,"nextPage":next,"total":0,"values":[field("customfield_1"),field("customfield_2")]}),
        )?,
        identity(),
        identity(),
        reply(json!({"startAt":2,"maxResults":2,"isLast":true,"values":[field("customfield_3")]}))?,
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
    let input = json!({"ids":["customfield_1","customfield_2","customfield_3"],"types":["custom","system"],"project_ids":[10000,10001],"query":"literal, + & 文字","order_by":"+name"});
    let fields = ["key", "last_used.type"];
    let first = prepared(
        &setup.grant,
        SEARCH,
        input.clone(),
        selection(&fields, None, 1, 16_384),
    )?
    .execute_cached(
        fixture.client(),
        &locked,
        &setup.store,
        TlsPolicy::AdditionalCaBundle(product.root()),
        control()?,
    )
    .observed();
    assert!(first.envelope.ok, "{:?}", first.envelope.error);
    let second = prepared(
        &setup.grant,
        SEARCH,
        input.clone(),
        selection(&fields, Some(cursor(&first.envelope)?), 1, 16_384),
    )?
    .execute_cached(
        fixture.client(),
        &locked,
        &setup.store,
        TlsPolicy::AdditionalCaBundle(product.root()),
        control()?,
    )
    .observed();
    assert!(second.envelope.ok, "{:?}", second.envelope.error);
    assert_eq!(data(&second.envelope)?["cached"], true);
    let third = prepared(
        &setup.grant,
        SEARCH,
        input,
        selection(&fields, Some(cursor(&second.envelope)?), 1, 16_384),
    )?
    .execute_cached(
        fixture.client(),
        &locked,
        &setup.store,
        TlsPolicy::AdditionalCaBundle(product.root()),
        control()?,
    )
    .observed();
    assert!(third.envelope.ok, "{:?}", third.envelope.error);
    schema(&third.envelope, &fields)?;
    assert_eq!(data(&third.envelope)?["items"][0]["id"], "customfield_3");
    assert_eq!(data(&third.envelope)?["coverage"], "classic_project_fields");
    let records = product.finish()?;
    assert_eq!(records.len(), 6);
    for (index, start) in [(2, "0"), (5, "2")] {
        let record = &records[index];
        assert_eq!(record.method, "GET");
        assert!(record.body.is_empty());
        assert!(
            record
                .target
                .starts_with(&format!("/ex/jira/{CLOUD}/rest/api/3/field/search?"))
        );
        for expected in [
            "id=customfield_1&id=customfield_2&id=customfield_3",
            "type=custom&type=system",
            "projectIds=10000&projectIds=10001",
            "maxResults=50",
            "orderBy=%2Bname",
            "expand=key%2ClastUsed",
            "query=literal%2C+%2B+%26+%E6%96%87%E5%AD%97",
            &format!("startAt={start}"),
        ] {
            assert!(record.target.contains(expected), "{}", record.target);
        }
        assert_eq!(
            record.headers.get("authorization").map(String::as_str),
            Some("Bearer craft-synthetic-access-token")
        );
    }
    assert_eq!(issuer.finish()?.len(), 1);
    drop(locked);
    setup.no_plaintext_credentials()?;
    Ok(())
}

#[test]
fn field_scope_actor_and_early_policy_denial_prevent_resource_fetches() -> TestResult {
    for mode in ["scope", "actor", "before_auth"] {
        let setup = Setup::new()?;
        let issuer = issuer(if mode == "scope" {
            "read:jira-user"
        } else {
            "read:jira-user read:jira-work"
        })?;
        let product = TlsServer::start(Reply::Sequence(vec![identity(), identity()]))?;
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
        let prepared = PreparedFieldRead::prepare(
            CanonicalRequest {
                command: SEARCH.into(),
                target: target(),
                input: json!({}),
                selection: selection(&[], None, 20, 16_384),
                preconditions: None,
            },
            "request",
            FieldReadPolicy {
                target: target(),
                grant: setup.grant.clone(),
                expected_actor: if mode == "actor" {
                    "actor-two"
                } else {
                    "actor-one"
                }
                .into(),
                continuation_site: "https://craft-test.atlassian.net".into(),
            },
        )?;
        let result = prepared
            .execute_cached_with_policy_check(
                fixture.client(),
                &locked,
                &setup.store,
                TlsPolicy::AdditionalCaBundle(product.root()),
                control()?,
                || {
                    if mode == "before_auth" {
                        Err(CraftError::new(ErrorCode::PolicyDenied, "Policy disabled."))
                    } else {
                        Ok(())
                    }
                },
            )
            .observed();
        assert_eq!(
            code(&result.envelope),
            Some(if mode == "scope" {
                ErrorCode::PermissionDenied
            } else {
                ErrorCode::PolicyDenied
            })
        );
        schema(&result.envelope, &[])?;
        assert_eq!(data(&result.envelope)?["items"], json!([]));
        assert!(result.envelope.page.is_none());
        if mode != "before_auth" {
            assert_eq!(
                data(&result.envelope)?["credential_cache_update"]["write_state"],
                "confirmed"
            );
        }
        assert_eq!(
            product.finish()?.len(),
            if mode == "before_auth" { 1 } else { 2 }
        );
        assert_eq!(
            issuer.finish()?.len(),
            if mode == "before_auth" { 1 } else { 2 }
        );
    }
    Ok(())
}

#[test]
fn field_failures_keep_renewal_evidence_without_retry_or_published_cursor() -> TestResult {
    for mode in ["403", "disconnect", "bad_next", "malformed"] {
        let setup = Setup::new()?;
        let issuer = issuer("read:jira-user read:jira-work")?;
        let resource = match mode {
            "403" => Reply::json(403, br#"{"message":"private-sentinel"}"#, &[]),
            "disconnect" => Reply::Disconnect,
            "bad_next" => reply(
                json!({"startAt":0,"maxResults":50,"isLast":false,"nextPage":"https://foreign.invalid/?startAt=2","values":[]}),
            )?,
            _ => reply(
                json!({"startAt":0,"maxResults":50,"isLast":true,"values":[{"id":"customfield_1"}]}),
            )?,
        };
        let product = TlsServer::start(Reply::Sequence({
            // A dropped connection is a retryable safe-read failure: three bounded attempts.
            let attempts = if mode == "disconnect" { 3 } else { 1 };
            let mut replies = vec![identity(), identity()];
            replies.extend(std::iter::repeat_n(resource, attempts));
            replies
        }))?;
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
        let result = prepared(
            &setup.grant,
            SEARCH,
            json!({}),
            selection(&[], None, 20, 16_384),
        )?
        .execute_cached(
            fixture.client(),
            &locked,
            &setup.store,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control()?,
        )
        .observed();
        assert!(!result.envelope.ok);
        schema(&result.envelope, &[])?;
        assert_eq!(data(&result.envelope)?["items"], json!([]));
        assert_eq!(
            data(&result.envelope)?["credential_cache_update"]["write_state"],
            "confirmed"
        );
        assert!(result.envelope.page.is_none());
        assert!(!serde_json::to_string(&result.envelope)?.contains("private-sentinel"));
        assert_eq!(
            product.finish()?.len(),
            if mode == "disconnect" { 5 } else { 3 }
        );
        assert_eq!(issuer.finish()?.len(), 2);
    }
    Ok(())
}

#[test]
fn oversized_selected_field_recovers_from_buffer_with_a_larger_output_budget() -> TestResult {
    let setup = Setup::new()?;
    let issuer = issuer("read:jira-user read:jira-work")?;
    let mut raw = field("customfield_1");
    raw["description"] = json!("d".repeat(8000));
    let product = TlsServer::start(Reply::Sequence(vec![
        identity(),
        identity(),
        reply(json!({"startAt":0,"maxResults":50,"isLast":true,"values":[raw]}))?,
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
    let fields = ["description"];
    let first = prepared(
        &setup.grant,
        SEARCH,
        json!({}),
        selection(&fields, None, 20, 4096),
    )?
    .execute_cached(
        fixture.client(),
        &locked,
        &setup.store,
        TlsPolicy::AdditionalCaBundle(product.root()),
        control()?,
    )
    .observed();
    assert_eq!(code(&first.envelope), Some(ErrorCode::ItemTooLarge));
    schema(&first.envelope, &fields)?;
    assert_eq!(data(&first.envelope)?["items"], json!([]));
    assert_eq!(
        data(&first.envelope)?["credential_cache_update"]["write_state"],
        "confirmed"
    );
    let last = prepared(
        &setup.grant,
        SEARCH,
        json!({}),
        selection(&fields, Some(cursor(&first.envelope)?), 20, 16_384),
    )?
    .execute_cached(
        fixture.client(),
        &locked,
        &setup.store,
        TlsPolicy::AdditionalCaBundle(product.root()),
        control()?,
    )
    .observed();
    assert!(last.envelope.ok, "{:?}", last.envelope.error);
    schema(&last.envelope, &fields)?;
    assert_eq!(data(&last.envelope)?["cached"], true);
    assert_eq!(
        data(&last.envelope)?["items"][0]["description"]
            .as_str()
            .ok_or("Description")?
            .len(),
        8000
    );
    assert!(
        craft_cli::output::encode(first.envelope, 4096, first.output_policy)?
            .bytes
            .len()
            <= 4096
    );
    assert_eq!(product.finish()?.len(), 4);
    assert_eq!(issuer.finish()?.len(), 2);
    Ok(())
}

#[test]
fn field_cursor_filter_or_projection_mismatch_is_refused_before_auth() -> TestResult {
    let setup = Setup::new()?;
    let issuer = issuer("read:jira-user read:jira-work")?;
    let product = TlsServer::start(Reply::Sequence(vec![
        identity(),
        identity(),
        reply(
            json!({"startAt":0,"maxResults":50,"isLast":true,"values":[field("customfield_1"),field("customfield_2")]}),
        )?,
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
        json!({}),
        selection(&[], None, 1, 16_384),
    )?
    .execute_cached(
        fixture.client(),
        &locked,
        &setup.store,
        TlsPolicy::AdditionalCaBundle(product.root()),
        control()?,
    )
    .observed();
    assert!(first.envelope.ok, "{:?}", first.envelope.error);
    clock.advance(Duration::from_secs(120))?;
    for (input, fields) in [
        (json!({"query":"changed"}), vec![]),
        (json!({}), vec!["description"]),
        (json!({"types":["custom"]}), vec![]),
        (json!({"project_ids":[10000]}), vec![]),
    ] {
        let result = prepared(
            &setup.grant,
            SEARCH,
            input,
            selection(&fields, Some(cursor(&first.envelope)?), 20, 16_384),
        )?
        .execute_cached(
            fixture.client(),
            &locked,
            &setup.store,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control()?,
        )
        .observed();
        assert_eq!(code(&result.envelope), Some(ErrorCode::CursorMismatch));
        assert!(data(&result.envelope)?["credential_cache_update"].is_null());
    }
    assert_eq!(product.finish()?.len(), 3);
    assert_eq!(issuer.finish()?.len(), 1);
    Ok(())
}

struct RevokingClock {
    base: ManualClock,
    armed: Arc<AtomicBool>,
    revoked: Arc<AtomicBool>,
}

#[test]
fn stalled_field_resource_obeys_invocation_deadline_without_retry() -> TestResult {
    let setup = Setup::new()?;
    let issuer = issuer("read:jira-user read:jira-work")?;
    let product = TlsServer::start(Reply::Sequence(vec![identity(), identity(), Reply::Stall]))?;
    let count = product.request_counter();
    let clock = Arc::new(ManualClock::new(1000));
    let fixture = setup.fixture(&issuer, &product, clock)?;
    let locked = fixture.lock(&setup.grant, &control()?)?;
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref()],
        Some("actor-one"),
    )?;
    let invocation = InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        1000,
    )?;
    let result = prepared(
        &setup.grant,
        LIST,
        json!({}),
        selection(&[], None, 20, 16_384),
    )?
    .execute_cached(
        fixture.client(),
        &locked,
        &setup.store,
        TlsPolicy::AdditionalCaBundle(product.root()),
        invocation,
    )
    .observed();
    assert_eq!(code(&result.envelope), Some(ErrorCode::DeadlineExceeded));
    schema(&result.envelope, &[])?;
    assert!(result.envelope.page.is_none());
    assert_eq!(data(&result.envelope)?["items"], json!([]));
    assert_eq!(count.load(Ordering::SeqCst), 3);
    assert_eq!(product.connections(), 3);
    assert_eq!(product.finish()?.len(), 3);
    assert_eq!(issuer.finish()?.len(), 1);
    Ok(())
}
impl Clock for RevokingClock {
    fn unix_millis(&self) -> u64 {
        if self.armed.load(Ordering::SeqCst) {
            self.revoked.store(true, Ordering::SeqCst);
        }
        self.base.unix_millis()
    }
    fn monotonic_millis(&self) -> u64 {
        self.base.monotonic_millis()
    }
}
#[test]
fn field_policy_changes_during_final_clock_and_cursor_work_suppress_native_or_buffered_output()
-> TestResult {
    for buffered in [false, true] {
        let setup = Setup::new()?;
        let issuer = issuer("read:jira-user read:jira-work")?;
        let product = TlsServer::start(Reply::Sequence(vec![
            identity(),
            identity(),
            reply(json!([field("customfield_1"), field("customfield_2")]))?,
            identity(),
        ]))?;
        let armed = Arc::new(AtomicBool::new(false));
        let revoked = Arc::new(AtomicBool::new(false));
        let clock = Arc::new(RevokingClock {
            base: ManualClock::new(1000),
            armed: armed.clone(),
            revoked: revoked.clone(),
        });
        let fixture = setup.fixture(&issuer, &product, clock.clone())?;
        let locked = fixture.lock(&setup.grant, &control()?)?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        let first = if buffered {
            Some(
                prepared(
                    &setup.grant,
                    LIST,
                    json!({}),
                    selection(&[], None, 1, 16_384),
                )?
                .execute_cached(
                    fixture.client(),
                    &locked,
                    &setup.store,
                    TlsPolicy::AdditionalCaBundle(product.root()),
                    control()?,
                )
                .observed(),
            )
        } else {
            None
        };
        let next = first
            .as_ref()
            .map(|result| cursor(&result.envelope))
            .transpose()?;
        clock.base.advance(Duration::from_secs(120))?;
        let checks = AtomicUsize::new(0);
        let result = prepared(
            &setup.grant,
            LIST,
            json!({}),
            selection(&[], next, 1, 16_384),
        )?
        .execute_cached_with_policy_check(
            fixture.client(),
            &locked,
            &setup.store,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control()?,
            || {
                if revoked.load(Ordering::SeqCst) {
                    return Err(CraftError::new(ErrorCode::PolicyDenied, "Policy changed."));
                }
                if product.request_count() >= if buffered { 4 } else { 3 }
                    && checks.fetch_add(1, Ordering::SeqCst) + 1 == 2
                {
                    armed.store(true, Ordering::SeqCst);
                }
                Ok(())
            },
        )
        .observed();
        assert!(revoked.load(Ordering::SeqCst));
        assert_eq!(code(&result.envelope), Some(ErrorCode::PolicyDenied));
        schema(&result.envelope, &[])?;
        assert_eq!(data(&result.envelope)?["items"], json!([]));
        assert!(result.envelope.page.is_none());
        assert_eq!(
            data(&result.envelope)?["credential_cache_update"]["write_state"],
            "confirmed"
        );
        assert_eq!(product.finish()?.len(), if buffered { 4 } else { 3 });
        assert_eq!(issuer.finish()?.len(), 2);
    }
    Ok(())
}
