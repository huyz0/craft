//! Actual Confluence host, sealed access and buffered pagination against owned TLS servers.
#[path = "confluence/comments.rs"]
mod comments;
#[path = "confluence/documents.rs"]
mod documents;
#[path = "confluence/inventory.rs"]
mod inventory;
#[path = "confluence/publication.rs"]
mod publication;
#[path = "confluence/review_preparation.rs"]
mod review_preparation;
#[path = "confluence/review_verification.rs"]
mod review_verification;
#[path = "../../craft-runtime/src/transport/tls_fixture.rs"]
mod tls_fixture;
#[path = "confluence/uploads.rs"]
mod uploads;
use craft_cli::confluence_reads::{ConfluenceReadPolicy, PreparedConfluenceRead};
use craft_core::contracts::{AuthMode, CanonicalRequest, GrantRef, ResourceRef, Selection};
use craft_core::control::{CancellationToken, InvocationControl};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::profiles::ProfileInput;
use craft_core::storage::Clock;
use craft_provider_confluence::{PAGE_GET, PAGE_LIST, PAGE_SEARCH, SPACE_GET, SPACE_LIST};
use craft_runtime::control::SystemClock;
use craft_runtime::profiles::ProfileStore;
use craft_runtime::synthetic_acceptance::{ACCESS_TOKEN, CLIENT_ID, CLIENT_SECRET, ServiceFixture};
use craft_runtime::transport::TlsPolicy;
use craft_test_support::state::ManualClock;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::Ordering};
use std::time::Duration;
use tls_fixture::{Reply, TlsServer};
type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn malformed_continuation_disconnect_and_stall_stop_without_unbound_requests() -> TestResult {
    for mode in ["foreign", "changed_filter", "disconnect", "stall"] {
        let setup = Setup::new()?;
        let issuer = issuer(SCOPES)?;
        let response = match mode {
            "disconnect" => Reply::Disconnect,
            "stall" => Reply::Stall,
            _ => reply(
                json!({"results":[],"_links":{"next":if mode=="foreign" {"https://untrusted.invalid/wiki/api/v2/pages?cursor=x"}else{"/wiki/api/v2/pages?cursor=x&status=trashed"}}}),
            )?,
        };
        // A dropped connection is a retryable safe-read failure: three bounded attempts.
        let attempts = if mode == "disconnect" { 3 } else { 1 };
        let mut replies = vec![identity(), identity()];
        replies.extend(std::iter::repeat_n(response, attempts));
        let product = TlsServer::start(Reply::Sequence(replies))?;
        let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
        let locked = fixture.lock(&setup.grant, &control()?)?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        let control = InvocationControl::new(
            Arc::new(SystemClock::default()),
            CancellationToken::default(),
            if mode == "stall" { 1500 } else { 10_000 },
        )?;
        let result = prepared(&setup.grant, PAGE_LIST, json!({}), &[], None, 20, 16384)?
            .execute_cached(
                fixture.client(),
                &locked,
                &setup.store,
                TlsPolicy::AdditionalCaBundle(product.root()),
                control,
            );
        assert!(!result.envelope.ok, "{mode}");
        assert!(result.envelope.page.is_none());
        assert_eq!(result.envelope.data.ok_or("data")?["items"], json!([]));
        if mode == "stall" {
            assert_eq!(
                result.envelope.error.ok_or("error")?.code,
                ErrorCode::DeadlineExceeded
            );
        }
        let expected = if mode == "disconnect" { 5 } else { 3 };
        assert_eq!(product.connections(), expected);
        assert_eq!(product.finish()?.len(), expected);
        assert_eq!(issuer.finish()?.len(), 1);
    }
    Ok(())
}
const CLOUD: &str = "00000000-1111-2222-3333-444444444444";
const SCOPES: &str =
    "read:confluence-user read:page:confluence read:space:confluence search:confluence";

struct CapturedPolicyClock {
    base: ManualClock,
    root: PathBuf,
    counter: Arc<std::sync::atomic::AtomicUsize>,
    threshold: usize,
    delay: usize,
    hits: std::sync::atomic::AtomicUsize,
    armed: std::sync::atomic::AtomicBool,
    changed: std::sync::atomic::AtomicBool,
}
impl Clock for CapturedPolicyClock {
    fn unix_millis(&self) -> u64 {
        if self.armed.load(Ordering::SeqCst)
            && self.counter.load(Ordering::SeqCst) >= self.threshold
            && self.hits.fetch_add(1, Ordering::SeqCst) >= self.delay
            && !self.changed.load(Ordering::SeqCst)
        {
            let result = (|| -> Result<(), CraftError> {
                let control = control()?;
                let store = ProfileStore::open(&self.root, &control)?;
                store.set_confluence_read_policy(
                    "factory",
                    craft_runtime::confluence_read_policy::ConfluenceReadPolicyUpdate {
                        target: target(),
                        expected_actor: "actor-one".into(),
                        enabled: false,
                        expected_revision: Some(1),
                        operations: all_reads(),
                    },
                    true,
                    &control,
                    |_| Ok(()),
                )?;
                Ok(())
            })();
            self.changed.store(result.is_ok(), Ordering::SeqCst);
        }
        self.base.unix_millis()
    }
    fn monotonic_millis(&self) -> u64 {
        self.base.monotonic_millis()
    }
}

#[test]
fn typed_confluence_commands_recheck_captured_records_before_auth_after_reads_and_final_clock()
-> TestResult {
    use craft_runtime::confluence_read_policy::{
        ConfluenceReadOperation as Op, ConfluenceReadPolicyUpdate,
    };
    for mode in [
        "excluded",
        "before_auth",
        "changed_allowed",
        "after_identity",
        "after_read",
        "final_clock",
        "buffered",
    ] {
        let setup = Setup::new()?;
        let issuer = issuer(SCOPES)?;
        enable_reads(&setup)?;
        let mut replies = vec![identity()];
        if matches!(
            mode,
            "after_identity" | "after_read" | "final_clock" | "buffered"
        ) {
            replies.push(identity());
        }
        if matches!(mode, "after_read" | "final_clock") {
            replies.push(reply(page(101))?);
        }
        if mode == "buffered" {
            replies.push(reply(json!({"results":[page(101),page(102)],"_links":{}}))?);
        }
        let product = TlsServer::start(Reply::Sequence(replies))?;
        let clock = Arc::new(CapturedPolicyClock {
            base: ManualClock::new(1000),
            root: setup.root.clone(),
            counter: product.request_counter(),
            threshold: if mode == "after_identity" { 2 } else { 3 },
            delay: if mode == "final_clock" { 2 } else { 0 },
            hits: std::sync::atomic::AtomicUsize::new(0),
            armed: std::sync::atomic::AtomicBool::new(false),
            changed: std::sync::atomic::AtomicBool::new(false),
        });
        let fixture = setup.fixture(&issuer, &product, clock.clone())?;
        let locked = fixture.lock(&setup.grant, &control()?)?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        clock.base.advance(Duration::from_secs(3600))?;
        if mode == "excluded" {
            setup.store.set_confluence_read_policy(
                "factory",
                ConfluenceReadPolicyUpdate {
                    target: target(),
                    expected_actor: "actor-one".into(),
                    enabled: true,
                    expected_revision: Some(1),
                    operations: vec![Op::SpaceGet],
                },
                true,
                &control()?,
                |_| Ok(()),
            )?;
            assert!(
                candidate(&setup, PAGE_GET, json!({"id":"101"}), &[], None, 20, 16384).is_err()
            );
        } else {
            let (command, input, continuation) = if mode == "buffered" {
                let first = candidate(&setup, PAGE_LIST, json!({}), &[], None, 1, 16384)?
                    .execute_cached(
                        &setup.store,
                        fixture.client(),
                        &locked,
                        TlsPolicy::AdditionalCaBundle(product.root()),
                        control()?,
                    );
                assert!(first.envelope.ok);
                (
                    PAGE_LIST,
                    json!({}),
                    Some(
                        first
                            .envelope
                            .page
                            .ok_or("page")?
                            .next_cursor
                            .ok_or("cursor")?,
                    ),
                )
            } else {
                (PAGE_GET, json!({"id":"101"}), None)
            };
            let prepared = candidate(
                &setup,
                command,
                input,
                &[],
                continuation.as_deref(),
                if mode == "buffered" { 1 } else { 20 },
                16384,
            )?;
            if matches!(mode, "before_auth" | "changed_allowed" | "buffered") {
                setup.store.set_confluence_read_policy(
                    "factory",
                    ConfluenceReadPolicyUpdate {
                        target: target(),
                        expected_actor: "actor-one".into(),
                        enabled: mode == "changed_allowed",
                        expected_revision: Some(1),
                        operations: all_reads(),
                    },
                    true,
                    &control()?,
                    |_| Ok(()),
                )?;
            }
            clock.armed.store(
                matches!(mode, "after_identity" | "after_read" | "final_clock"),
                Ordering::SeqCst,
            );
            let result = prepared.execute_cached(
                &setup.store,
                fixture.client(),
                &locked,
                TlsPolicy::AdditionalCaBundle(product.root()),
                control()?,
            );
            assert!(!result.envelope.ok, "{mode}");
            assert!(result.envelope.page.is_none());
            let data = result.envelope.data.ok_or("data")?;
            if command == PAGE_GET {
                assert!(data["item"].is_null());
            } else {
                assert_eq!(data["items"], json!([]));
            }
            assert_eq!(
                data["credential_cache_update"].is_null(),
                matches!(mode, "before_auth" | "changed_allowed" | "buffered")
            );
            if matches!(mode, "after_identity" | "after_read" | "final_clock") {
                assert!(clock.changed.load(Ordering::SeqCst), "{mode}");
            }
            if mode == "changed_allowed" {
                assert_eq!(
                    result.envelope.error.ok_or("error")?.code,
                    ErrorCode::VersionConflict
                );
            }
        }
        assert_eq!(
            product.finish()?.len(),
            match mode {
                "after_identity" => 2,
                "after_read" | "final_clock" | "buffered" => 3,
                _ => 1,
            }
        );
        assert_eq!(
            issuer.finish()?.len(),
            if matches!(
                mode,
                "after_identity" | "after_read" | "final_clock" | "buffered"
            ) {
                2
            } else {
                1
            }
        );
        drop(locked);
        setup.no_secrets()?;
    }
    Ok(())
}
fn control() -> Result<InvocationControl, CraftError> {
    InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        30_000,
    )
}
fn target() -> ResourceRef {
    ResourceRef {
        provider: "confluence".into(),
        deployment: "cloud".into(),
        tenant: CLOUD.into(),
        resource_type: "site".into(),
        id: CLOUD.into(),
    }
}
fn identity() -> Reply {
    Reply::json(200, br#"{"accountId":"actor-one","type":"known"}"#, &[])
}
fn reply(value: Value) -> Result<Reply, serde_json::Error> {
    Ok(Reply::json(200, &serde_json::to_vec(&value)?, &[]))
}
fn issuer(scopes: &str) -> Result<TlsServer, Box<dyn std::error::Error>> {
    TlsServer::start(reply(
        json!({"access_token":ACCESS_TOKEN,"expires_in":120,"scope":scopes,"token_type":"Bearer"}),
    )?)
}
fn page(id: u64) -> Value {
    json!({"id":id.to_string(),"spaceId":"100","status":"current","title":format!("Plan {id} 文字"),"version":{"number":3},"body":{"storage":{"representation":"storage","value":"<p>Plan</p>"}},"ownerId":"private-sentinel"})
}
fn space() -> Value {
    json!({"id":"100","key":"ENG","name":"Engineering","type":"global","status":"current"})
}
fn search_item(id: u64) -> Value {
    json!({"content":{"id":id.to_string(),"type":"page","status":"current","title":"Plan","space":{"id":100},"version":{"number":3}}})
}
fn prepared(
    grant: &GrantRef,
    command: &str,
    input: Value,
    fields: &[&str],
    cursor: Option<&str>,
    limit: u16,
    maximum: u32,
) -> Result<PreparedConfluenceRead, CraftError> {
    PreparedConfluenceRead::prepare(
        CanonicalRequest {
            command: command.into(),
            target: target(),
            input,
            selection: Selection {
                fields: fields.iter().map(|s| (*s).into()).collect(),
                cursor: cursor.map(String::from),
                limit,
                max_bytes: maximum,
            },
            preconditions: None,
        },
        "confluence-request",
        ConfluenceReadPolicy {
            target: target(),
            grant: grant.clone(),
            expected_actor: "actor-one".into(),
            enabled: true,
            allowed_commands: vec![command.into()],
        },
    )
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
        std::fs::create_dir_all(&parent)?;
        let parent = parent.canonicalize()?;
        let root = craft_test_support::state::fixture_root(
            &parent,
            "confluence-context",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos(),
        );
        let store = ProfileStore::open(&root, &control()?)?;
        let profile = store.create(
            &ProfileInput {
                name: "factory".into(),
                provider: "confluence".into(),
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
    fn no_secrets(&self) -> TestResult {
        for entry in std::fs::read_dir(&self.root)? {
            let path = entry?.path();
            if path.is_file() {
                let bytes = std::fs::read(path)?;
                for value in [ACCESS_TOKEN, CLIENT_ID, CLIENT_SECRET, "private-sentinel"] {
                    assert!(!bytes.windows(value.len()).any(|b| b == value.as_bytes()));
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
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

fn all_reads() -> Vec<craft_runtime::confluence_read_policy::ConfluenceReadOperation> {
    use craft_runtime::confluence_read_policy::ConfluenceReadOperation as Op;
    vec![
        Op::SpaceGet,
        Op::SpaceList,
        Op::PageGet,
        Op::PageList,
        Op::PageSearch,
    ]
}
fn enable_reads(setup: &Setup) -> Result<(), CraftError> {
    setup.store.set_confluence_read_policy(
        "factory",
        craft_runtime::confluence_read_policy::ConfluenceReadPolicyUpdate {
            target: target(),
            expected_actor: "actor-one".into(),
            enabled: true,
            expected_revision: None,
            operations: all_reads(),
        },
        true,
        &control()?,
        |_| Ok(()),
    )?;
    Ok(())
}
fn candidate(
    setup: &Setup,
    command: &str,
    mut input: Value,
    fields: &[&str],
    cursor: Option<&str>,
    limit: u16,
    maximum: u32,
) -> Result<craft_cli::confluence_reads::command::PreparedInvocation, CraftError> {
    use craft_cli::confluence_reads::command::{ConfluenceReadArgs, PreparedInvocation};
    if !matches!(command, PAGE_GET | SPACE_GET) {
        input["limit"] = json!(limit);
        input["cursor"] = json!(cursor);
    }
    let options = craft_cli::profiles::InvocationOptions {
        state_root: Some(&setup.root),
        profile: Some("factory"),
        request_id: "confluence-request",
        max_bytes: maximum as usize,
    };
    PreparedInvocation::prepare(
        ConfluenceReadArgs::decode(command, input)?,
        &fields.iter().map(|s| (*s).into()).collect::<Vec<_>>(),
        &options,
        &setup.store,
        &control()?,
    )
}
#[test]
fn native_space_page_and_search_routes_use_verified_confluence_access_and_selected_bodies()
-> TestResult {
    for (command, input, raw, path, fields) in [
        (
            SPACE_GET,
            json!({"id":"100"}),
            space(),
            "/wiki/api/v2/spaces/100",
            vec![],
        ),
        (
            SPACE_LIST,
            json!({"key":"ENG"}),
            json!({"results":[space()],"_links":{}}),
            "/wiki/api/v2/spaces",
            vec![],
        ),
        (
            PAGE_GET,
            json!({"id":"101"}),
            page(101),
            "/wiki/api/v2/pages/101",
            vec!["title", "body_storage", "version"],
        ),
        (
            PAGE_LIST,
            json!({}),
            json!({"results":[page(101)],"_links":{}}),
            "/wiki/api/v2/pages",
            vec![],
        ),
        (
            PAGE_LIST,
            json!({"space_id":"100"}),
            json!({"results":[page(101)],"_links":{}}),
            "/wiki/api/v2/spaces/100/pages",
            vec![],
        ),
        (
            PAGE_SEARCH,
            json!({"cql":"type = page AND space = ENG"}),
            json!({"results":[search_item(101)],"start":0,"size":1,"limit":25,"_links":{}}),
            "/wiki/rest/api/search",
            vec![],
        ),
    ] {
        let setup = Setup::new()?;
        enable_reads(&setup)?;
        let issuer = issuer(SCOPES)?;
        let product = TlsServer::start(Reply::Sequence(vec![identity(), identity(), reply(raw)?]))?;
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
        let result = candidate(&setup, command, input, &fields, None, 20, 16384)?.execute_cached(
            &setup.store,
            fixture.client(),
            &locked,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control()?,
        );
        assert!(result.envelope.ok, "{command}: {:?}", result.envelope.error);
        assert!(
            jsonschema::validator_for(&craft_provider_confluence::response_schema_for(command)?)?
                .is_valid(&serde_json::to_value(&result.envelope)?)
        );
        assert_eq!(
            result.envelope.data.as_ref().ok_or("data")?["credential_cache_update"]["write_state"],
            "confirmed"
        );
        let output = craft_cli::output::encode(result.envelope, 16384, result.output_policy)?;
        assert!(output.bytes.len() <= 16384);
        assert!(!String::from_utf8(output.bytes)?.contains("private-sentinel"));
        assert_eq!(product.connections(), 3);
        let requests = product.finish()?;
        assert_eq!(requests.len(), 3);
        assert!(requests.iter().all(|r| r.method == "GET"));
        assert!(requests.iter().all(|r| r.body.is_empty()));
        assert_eq!(
            requests[2].target.split('?').next(),
            Some(format!("/ex/confluence/{CLOUD}{path}").as_str())
        );
        assert!(
            requests[2]
                .headers
                .iter()
                .any(|(name, value)| name.eq_ignore_ascii_case("authorization")
                    && value == &format!("Bearer {ACCESS_TOKEN}"))
        );
        assert_eq!(issuer.finish()?.len(), 2);
        drop(locked);
        setup.no_secrets()?;
    }
    Ok(())
}
#[test]
fn v2_and_cql_pages_reopen_buffer_without_refetch_and_then_follow_only_bound_cursors() -> TestResult
{
    for command in [PAGE_LIST, PAGE_SEARCH] {
        let setup = Setup::new()?;
        enable_reads(&setup)?;
        let issuer = issuer(SCOPES)?;
        let path = if command == PAGE_LIST {
            "/wiki/api/v2/pages"
        } else {
            "/wiki/rest/api/search"
        };
        let first = if command == PAGE_LIST {
            json!({"results":[page(101),page(102)],"_links":{"next":format!("{path}?cursor=opaque%2Fnext")}})
        } else {
            json!({"results":[search_item(101),search_item(102)],"size":2,"start":0,"limit":25,"_links":{"next":format!("{path}?cursor=opaque%2Fnext&next=true")}})
        };
        let last = if command == PAGE_LIST {
            json!({"results":[page(103)],"_links":{}})
        } else {
            json!({"results":[search_item(103)],"size":1,"start":0,"limit":25,"_links":{}})
        };
        let product = TlsServer::start(Reply::Sequence(vec![
            identity(),
            identity(),
            reply(first)?,
            identity(),
            identity(),
            reply(last)?,
        ]))?;
        let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
        let locked = fixture.lock(&setup.grant, &control()?)?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        let input = if command == PAGE_LIST {
            json!({})
        } else {
            json!({"cql":"type = page"})
        };
        let first = candidate(&setup, command, input.clone(), &[], None, 1, 16384)?.execute_cached(
            &setup.store,
            fixture.client(),
            &locked,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control()?,
        );
        assert!(first.envelope.ok, "{:?}", first.envelope.error);
        let cursor = first
            .envelope
            .page
            .ok_or("page")?
            .next_cursor
            .ok_or("cursor")?;
        let reopened = ProfileStore::open(&setup.root, &control()?)?;
        let second = candidate(&setup, command, input.clone(), &[], Some(&cursor), 1, 16384)?
            .execute_cached(
                &reopened,
                fixture.client(),
                &locked,
                TlsPolicy::AdditionalCaBundle(product.root()),
                control()?,
            );
        assert!(second.envelope.ok);
        assert_eq!(second.envelope.data.as_ref().ok_or("data")?["cached"], true);
        assert_eq!(
            second.envelope.data.as_ref().ok_or("data")?["items"][0]["id"],
            "102"
        );
        let next = second
            .envelope
            .page
            .ok_or("page")?
            .next_cursor
            .ok_or("cursor")?;
        let invalid = candidate(
            &setup,
            command,
            input.clone(),
            &["title"],
            Some(&next),
            1,
            16384,
        )?
        .execute_cached(
            &reopened,
            fixture.client(),
            &locked,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control()?,
        );
        assert!(!invalid.envelope.ok);
        assert_eq!(product.request_count(), 4);
        let third = candidate(&setup, command, input, &[], Some(&next), 1, 16384)?.execute_cached(
            &reopened,
            fixture.client(),
            &locked,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control()?,
        );
        assert!(third.envelope.ok, "{:?}", third.envelope.error);
        assert_eq!(third.envelope.data.ok_or("data")?["items"][0]["id"], "103");
        let requests = product.finish()?;
        assert_eq!(requests.len(), 6);
        assert!(requests[5].target.contains("cursor=opaque%2Fnext"));
        assert!(requests.iter().all(|r| r.method == "GET"));
        assert_eq!(issuer.finish()?.len(), 1);
    }
    Ok(())
}
#[test]
fn policy_revocation_bad_scope_and_native_refusal_preserve_renewal_without_returning_content()
-> TestResult {
    for mode in [
        "before_auth",
        "after_read",
        "scope",
        "wrong_id",
        "rate",
        "oversized",
    ] {
        let setup = Setup::new()?;
        let issuer = issuer(if mode == "scope" {
            "read:confluence-user"
        } else {
            SCOPES
        })?;
        let mut replies = vec![identity()];
        if mode != "before_auth" {
            replies.push(identity());
            if mode != "scope" {
                let mut raw = page(101);
                if mode == "wrong_id" {
                    raw["id"] = json!("102");
                }
                if mode == "oversized" {
                    raw["body"]["storage"]["value"] = json!("x".repeat(10000));
                }
                replies.push(if mode == "rate" {
                    // Advice above the 30 s retry cap is returned rather than waited on.
                    Reply::json(429, b"private-sentinel", &[("Retry-After", "60")])
                } else {
                    reply(raw)?
                });
            }
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
        let count = product.request_counter();
        let result = prepared(
            &setup.grant,
            PAGE_GET,
            json!({"id":"101"}),
            &["body_storage"],
            None,
            20,
            4096,
        )?
        .execute_cached_with_policy_check(
            fixture.client(),
            &locked,
            &setup.store,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control()?,
            || {
                if mode == "before_auth"
                    || (mode == "after_read" && count.load(Ordering::SeqCst) >= 3)
                {
                    return Err(CraftError::new(
                        ErrorCode::PolicyDenied,
                        "Confluence read policy revoked.",
                    ));
                }
                Ok(())
            },
        );
        assert!(!result.envelope.ok, "{mode}");
        let data = result.envelope.data.ok_or("data")?;
        assert!(data["item"].is_null());
        assert_eq!(
            data["credential_cache_update"].is_null(),
            mode == "before_auth"
        );
        if mode == "oversized" {
            assert_eq!(
                result.envelope.error.ok_or("error")?.code,
                ErrorCode::ItemTooLarge
            );
        }
        assert_eq!(
            product.finish()?.len(),
            if mode == "before_auth" {
                1
            } else if mode == "scope" {
                2
            } else {
                3
            }
        );
        assert_eq!(
            issuer.finish()?.len(),
            if mode == "before_auth" { 1 } else { 2 }
        );
        drop(locked);
        setup.no_secrets()?;
    }
    Ok(())
}
