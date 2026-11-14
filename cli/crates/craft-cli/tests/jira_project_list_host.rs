//! Actual internal paging/auth/transport composition against owned servers and synthetic material.

#[path = "../../craft-runtime/src/transport/tls_fixture.rs"]
mod tls_fixture;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

use craft_cli::project_list::{PreparedProjectList, ProjectReadPolicy};
use craft_core::contracts::{AuthMode, CanonicalRequest, GrantRef, ResourceRef, Selection};
use craft_core::control::{CancellationToken, InvocationControl};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::profiles::ProfileInput;
use craft_core::storage::Clock;
use craft_runtime::control::SystemClock;
use craft_runtime::profiles::ProfileStore;
use craft_runtime::project_read_policy::ProjectReadPolicyUpdate;
use craft_runtime::synthetic_acceptance::{ACCESS_TOKEN, CLIENT_ID, CLIENT_SECRET, ServiceFixture};
use craft_runtime::transport::TlsPolicy;
use craft_test_support::state::ManualClock;
use serde_json::json;
use tls_fixture::{Reply, TlsServer};

type TestResult = Result<(), Box<dyn std::error::Error>>;
const CLOUD: &str = "00000000-1111-2222-3333-444444444444";
const SITE: &str = "https://craft-test.atlassian.net";
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
            &json!({"access_token":ACCESS_TOKEN,"expires_in":3600,"token_type":"Bearer","scope":scopes}),
        )?,
        &[],
    ))
}
fn page(start: u64, ids: &[u64], next: Option<u64>) -> Result<Reply, serde_json::Error> {
    let mut body = json!({"startAt":start,"maxResults":50,"isLast":next.is_none(),"total":ids.len(),"values":ids.iter().map(|id|json!({"id":id.to_string(),"key":format!("ED{id}"),"name":format!("Project {id}"),"projectTypeKey":"software"})).collect::<Vec<_>>()});
    if let Some(next) = next {
        body["nextPage"] = json!(format!(
            "{SITE}/rest/api/3/project/search?startAt={next}&maxResults=50"
        ));
    }
    Ok(Reply::json(200, &serde_json::to_vec(&body)?, &[]))
}
fn prepared(
    grant: &GrantRef,
    cursor: Option<&str>,
    limit: u16,
    max_bytes: u32,
    query: Option<&str>,
) -> Result<PreparedProjectList, CraftError> {
    PreparedProjectList::prepare(
        CanonicalRequest {
            command: "jira.project.list".into(),
            target: target(),
            input: json!({"page_size":50,"query":query}),
            selection: Selection {
                cursor: cursor.map(String::from),
                limit,
                max_bytes,
                ..Selection::default()
            },
            preconditions: None,
        },
        "original-request",
        ProjectReadPolicy {
            target: target(),
            grant: grant.clone(),
            expected_actor: "actor-one".into(),
            continuation_site: SITE.into(),
        },
    )
}
fn cursor(envelope: &craft_core::contracts::Envelope) -> Result<&str, Box<dyn std::error::Error>> {
    envelope
        .page
        .as_ref()
        .and_then(|page| page.next_cursor.as_deref())
        .ok_or_else(|| "Missing continuation".into())
}
struct ReadPolicyClock {
    base: ManualClock,
    store: Mutex<ProfileStore>,
    requests: Arc<AtomicUsize>,
    threshold: usize,
    disable: bool,
    changed: AtomicBool,
}
impl Clock for ReadPolicyClock {
    fn unix_millis(&self) -> u64 {
        if self.requests.load(Ordering::SeqCst) >= self.threshold
            && !self.changed.load(Ordering::SeqCst)
            && let Ok(store) = self.store.lock()
            && let Ok(control) = control()
        {
            let changed = store.set_project_read_policy(
                "factory",
                ProjectReadPolicyUpdate {
                    target: target(),
                    expected_actor: "actor-one".into(),
                    continuation_site: SITE.into(),
                    enabled: !self.disable,
                    expected_revision: Some(1),
                },
                true,
                &control,
                |_| Ok(()),
            );
            self.changed.store(changed.is_ok(), Ordering::SeqCst);
        }
        self.base.unix_millis()
    }
    fn monotonic_millis(&self) -> u64 {
        self.base.monotonic_millis()
    }
}

#[test]
fn typed_project_command_uses_private_configuration_and_refuses_revocation_before_auth_or_output()
-> TestResult {
    use clap::Parser;
    use craft_cli::profiles::InvocationOptions;
    use craft_cli::project_list::command::{PreparedInvocation, ProjectListArgs};
    #[derive(Parser)]
    struct Candidate {
        #[command(flatten)]
        args: ProjectListArgs,
    }
    for mode in [
        "success",
        "before_auth",
        "during_auth",
        "after_page",
        "changed_page",
        "buffered",
    ] {
        let setup = Setup::new()?;
        setup.store.set_project_read_policy(
            "factory",
            ProjectReadPolicyUpdate {
                target: target(),
                expected_actor: "actor-one".into(),
                continuation_site: SITE.into(),
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
        if matches!(mode, "success" | "after_page" | "changed_page" | "buffered") {
            replies.push(page(0, &[1, 2], None)?);
        }
        if mode == "buffered" {
            replies.push(identity());
        }
        let product = TlsServer::start(Reply::Sequence(replies))?;
        let clock = Arc::new(ReadPolicyClock {
            base: ManualClock::new(1000),
            store: Mutex::new(ProfileStore::open(&setup.files.root, &control()?)?),
            requests: product.request_counter(),
            threshold: match mode {
                "during_auth" => 2,
                "after_page" | "changed_page" => 3,
                "buffered" => 4,
                _ => usize::MAX,
            },
            changed: AtomicBool::new(false),
            disable: mode != "changed_page",
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
            state_root: Some(&setup.files.root),
            profile: Some("factory"),
            request_id: "typed-list-request",
            max_bytes: 16_384,
        };
        let args = Candidate::try_parse_from(["candidate", "--limit", "1"])?.args;
        let prepared = PreparedInvocation::prepare(args, &[], &options, &setup.store, &control()?)?;
        assert_eq!(prepared.grant(), &setup.grant);
        if mode == "before_auth" {
            setup.store.set_project_read_policy(
                "factory",
                ProjectReadPolicyUpdate {
                    target: target(),
                    expected_actor: "actor-one".into(),
                    continuation_site: SITE.into(),
                    enabled: false,
                    expected_revision: Some(1),
                },
                true,
                &control()?,
                |_| Ok(()),
            )?;
        }
        if matches!(mode, "during_auth" | "after_page" | "changed_page") {
            clock.base.advance(Duration::from_secs(3600))?;
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
            let args =
                Candidate::try_parse_from(["candidate", "--cursor", cursor(&result.envelope)?])?
                    .args;
            let prepared =
                PreparedInvocation::prepare(args, &[], &options, &setup.store, &control()?)?;
            result = prepared.execute_cached(
                &setup.store,
                fixture.client(),
                &locked,
                TlsPolicy::AdditionalCaBundle(product.root()),
                control()?,
            );
        }
        assert_eq!(result.envelope.request_id, "typed-list-request");
        if mode == "success" {
            assert!(result.envelope.ok, "{:?}", result.envelope.error);
            assert_eq!(
                result.envelope.data.as_ref().ok_or("Missing data")?["items"][0]["id"],
                "1"
            );
        } else {
            assert_eq!(
                result.envelope.error.as_ref().map(|error| error.code),
                Some(if mode == "changed_page" {
                    ErrorCode::VersionConflict
                } else {
                    ErrorCode::PolicyDenied
                }),
                "{mode}"
            );
            assert!(result.envelope.page.is_none());
            assert!(
                result
                    .envelope
                    .data
                    .as_ref()
                    .ok_or("Missing failure data")?["items"]
                    .as_array()
                    .ok_or("Missing items")?
                    .is_empty()
            );
            if matches!(mode, "during_auth" | "after_page" | "changed_page") {
                assert_eq!(
                    result
                        .envelope
                        .data
                        .as_ref()
                        .ok_or("Missing cache receipt")?["credential_cache_update"]["write_state"],
                    "confirmed"
                );
            }
            assert_eq!(
                setup
                    .store
                    .project_read_policy("factory", &control()?)?
                    .ok_or("Missing revoked policy")?
                    .enabled,
                mode == "changed_page"
            );
        }
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
            if matches!(mode, "during_auth" | "after_page" | "changed_page") {
                2
            } else {
                1
            }
        );
        let output = craft_cli::output::encode(result.envelope, 16_384, result.output_policy)?;
        assert!(output.bytes.len() <= 16_384);
    }
    Ok(())
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
    store: ProfileStore,
    grant: GrantRef,
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
            root: craft_test_support::state::fixture_root(&parent, "composed-projects", stamp),
            parent,
        };
        let store = ProfileStore::open(&files.root, &control()?)?;
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
            &self.store,
            issuer.address().port(),
            product.address().port(),
            [issuer.root(), product.root()].concat(),
            control()?,
            clock,
        )
    }
    fn no_plaintext(&self) -> TestResult {
        for entry in fs::read_dir(&self.files.root)? {
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

#[test]
fn project_pages_buffer_without_resource_fetch_and_site_links_stay_on_bound_gateway() -> TestResult
{
    let setup = Setup::new()?;
    let issuer = issuer("read:jira-user read:jira-work")?;
    let product = TlsServer::start(Reply::Sequence(vec![
        identity(),
        identity(),
        page(0, &[1, 2, 3], Some(50))?,
        identity(),
        identity(),
        page(50, &[], Some(75))?,
        identity(),
        page(75, &[4], None)?,
    ]))?;
    let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
    let locked = fixture.lock(&setup.grant, &control()?)?;
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref()],
        Some("actor-one"),
    )?;
    let first = prepared(&setup.grant, None, 1, 16384, None)?.execute_cached(
        fixture.client(),
        &locked,
        &setup.store,
        TlsPolicy::AdditionalCaBundle(product.root()),
        control()?,
    );
    assert!(first.envelope.ok, "{:?}", first.envelope.error);
    assert_eq!(
        first.envelope.data.as_ref().ok_or("Missing data")?["items"][0]["id"],
        "1"
    );
    let second = prepared(&setup.grant, Some(cursor(&first.envelope)?), 2, 16384, None)?
        .execute_cached(
            fixture.client(),
            &locked,
            &setup.store,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control()?,
        );
    assert!(second.envelope.ok, "{:?}", second.envelope.error);
    assert_eq!(
        second.envelope.data.as_ref().ok_or("Missing data")?["cached"],
        true
    );
    assert_eq!(
        second.envelope.data.as_ref().ok_or("Missing data")?["items"]
            .as_array()
            .ok_or("Missing items")?
            .len(),
        2
    );
    assert_eq!(
        first.envelope.data.as_ref().ok_or("Missing first")?["observation"],
        second.envelope.data.as_ref().ok_or("Missing second")?["observation"]
    );
    let third = prepared(
        &setup.grant,
        Some(cursor(&second.envelope)?),
        100,
        16384,
        None,
    )?
    .execute_cached(
        fixture.client(),
        &locked,
        &setup.store,
        TlsPolicy::AdditionalCaBundle(product.root()),
        control()?,
    );
    assert!(third.envelope.ok);
    assert!(
        third.envelope.data.as_ref().ok_or("Missing data")?["items"]
            .as_array()
            .ok_or("Missing items")?
            .is_empty()
    );
    let fourth = prepared(
        &setup.grant,
        Some(cursor(&third.envelope)?),
        100,
        16384,
        None,
    )?
    .execute_cached(
        fixture.client(),
        &locked,
        &setup.store,
        TlsPolicy::AdditionalCaBundle(product.root()),
        control()?,
    );
    assert!(fourth.envelope.ok);
    assert!(
        !fourth
            .envelope
            .page
            .as_ref()
            .ok_or("Missing page")?
            .has_more
    );
    let requests = product.finish()?;
    assert_eq!(requests.len(), 8);
    let pages: Vec<_> = requests
        .iter()
        .filter(|request| request.target.contains("/project/search?"))
        .collect();
    assert_eq!(pages.len(), 3);
    for (request, start) in pages.iter().zip([0, 50, 75]) {
        assert_eq!(request.method, "GET");
        assert!(
            request
                .target
                .starts_with(&format!("/ex/jira/{CLOUD}/rest/api/3/project/search?"))
        );
        assert!(request.target.contains(&format!("startAt={start}")));
        assert!(request.target.contains("action=view"));
        assert!(request.target.contains("orderBy=key"));
        assert!(!request.target.contains("expand"));
        assert!(request.body.is_empty());
        assert!(request.headers["host"].starts_with("localhost:"));
    }
    assert_eq!(issuer.finish()?.len(), 1);
    drop(locked);
    drop(fixture);
    setup.no_plaintext()?;
    Ok(())
}

#[test]
fn invalid_site_link_metadata_or_denial_preserves_renewal_receipt_without_following_link()
-> TestResult {
    for mode in [
        "foreign",
        "duplicate",
        "filter",
        "fragment",
        "offset",
        "ambiguous",
        "denied",
    ] {
        let setup = Setup::new()?;
        let issuer = issuer("read:jira-user read:jira-work")?;
        let mut body = json!({"startAt":0,"maxResults":50,"isLast":false,"values":[],"nextPage":format!("{SITE}/rest/api/3/project/search?startAt=50")});
        match mode {
            "foreign" => {
                body["nextPage"] =
                    json!("https://private-sentinel.invalid/rest/api/3/project/search?startAt=50")
            }
            "duplicate" => {
                body["nextPage"] = json!(format!(
                    "{SITE}/rest/api/3/project/search?startAt=50&startAt=100"
                ))
            }
            "filter" => {
                body["nextPage"] = json!(format!(
                    "{SITE}/rest/api/3/project/search?startAt=50&action=edit"
                ))
            }
            "fragment" => {
                body["nextPage"] = json!(format!(
                    "{SITE}/rest/api/3/project/search?startAt=50#private-sentinel"
                ))
            }
            "offset" => {
                body["nextPage"] = json!(format!("{SITE}/rest/api/3/project/search?startAt=0"))
            }
            "ambiguous" => {
                body.as_object_mut()
                    .ok_or("Expected object")?
                    .remove("nextPage");
            }
            _ => {}
        }
        let reply = Reply::json(
            if mode == "denied" { 403 } else { 200 },
            &serde_json::to_vec(&body)?,
            &[],
        );
        let product = TlsServer::start(Reply::Sequence(vec![identity(), identity(), reply]))?;
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
        let result = prepared(&setup.grant, None, 20, 16384, None)?.execute_cached(
            fixture.client(),
            &locked,
            &setup.store,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control()?,
        );
        assert!(!result.envelope.ok, "{mode}");
        assert!(result.envelope.page.is_none());
        assert_eq!(
            result.envelope.data.as_ref().ok_or("Missing effects")?["credential_cache_update"]["write_state"],
            "confirmed"
        );
        assert!(!serde_json::to_string(&result.envelope.error)?.contains("private-sentinel"));
        assert_eq!(product.finish()?.len(), 3);
        assert_eq!(issuer.finish()?.len(), 2);
        drop(locked);
        drop(fixture);
        setup.no_plaintext()?;
    }
    Ok(())
}

#[test]
fn mismatched_cursor_is_refused_before_cached_identity_or_acquisition() -> TestResult {
    let setup = Setup::new()?;
    let issuer = issuer("read:jira-user read:jira-work")?;
    let product = TlsServer::start(Reply::Sequence(vec![
        identity(),
        identity(),
        page(0, &[1, 2], None)?,
    ]))?;
    let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
    let locked = fixture.lock(&setup.grant, &control()?)?;
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref()],
        Some("actor-one"),
    )?;
    let first = prepared(&setup.grant, None, 1, 16384, None)?.execute_cached(
        fixture.client(),
        &locked,
        &setup.store,
        TlsPolicy::AdditionalCaBundle(product.root()),
        control()?,
    );
    assert!(first.envelope.ok);
    let wrong = prepared(
        &setup.grant,
        Some(cursor(&first.envelope)?),
        20,
        16384,
        Some("other"),
    )?
    .execute_cached(
        fixture.client(),
        &locked,
        &setup.store,
        TlsPolicy::AdditionalCaBundle(product.root()),
        control()?,
    );
    assert_eq!(
        wrong.envelope.error.as_ref().map(|error| error.code),
        Some(ErrorCode::CursorMismatch)
    );
    assert_eq!(product.finish()?.len(), 3);
    assert_eq!(issuer.finish()?.len(), 1);
    Ok(())
}

#[test]
fn scope_or_actor_denial_prevents_the_project_fetch() -> TestResult {
    for mode in ["scope", "actor"] {
        let setup = Setup::new()?;
        let issuer = issuer(if mode == "scope" {
            "read:jira-user"
        } else {
            "read:jira-user read:jira-work"
        })?;
        let product = TlsServer::start(Reply::Sequence(vec![identity(), identity()]))?;
        let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
        let locked = fixture.lock(&setup.grant, &control()?)?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        let host = if mode == "actor" {
            PreparedProjectList::prepare(
                CanonicalRequest {
                    command: "jira.project.list".into(),
                    target: target(),
                    input: json!({}),
                    selection: Selection::default(),
                    preconditions: None,
                },
                "request",
                ProjectReadPolicy {
                    target: target(),
                    grant: setup.grant.clone(),
                    expected_actor: "other".into(),
                    continuation_site: SITE.into(),
                },
            )?
        } else {
            prepared(&setup.grant, None, 20, 16384, None)?
        };
        let result = host.execute_cached(
            fixture.client(),
            &locked,
            &setup.store,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control()?,
        );
        assert_eq!(
            result.envelope.error.as_ref().map(|error| error.code),
            Some(if mode == "scope" {
                ErrorCode::PermissionDenied
            } else {
                ErrorCode::PolicyDenied
            })
        );
        assert_eq!(product.finish()?.len(), 2);
        assert_eq!(issuer.finish()?.len(), 1);
    }
    Ok(())
}

#[test]
fn oversized_project_retains_cursor_and_exact_output_budget_until_retry() -> TestResult {
    let setup = Setup::new()?;
    let issuer = issuer("read:jira-user read:jira-work")?;
    let reply = Reply::json(
        200,
        &serde_json::to_vec(
            &json!({"startAt":0,"maxResults":50,"isLast":true,"values":[{"id":"1","key":"ED1","name":"x".repeat(4000)},{"id":"2","key":"ED2","name":"Small"}]}),
        )?,
        &[],
    );
    let product = TlsServer::start(Reply::Sequence(vec![
        identity(),
        identity(),
        reply,
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
    let result = prepared(&setup.grant, None, 20, 3072, None)?.execute_cached(
        fixture.client(),
        &locked,
        &setup.store,
        TlsPolicy::AdditionalCaBundle(product.root()),
        control()?,
    );
    assert_eq!(
        result.envelope.error.as_ref().map(|error| error.code),
        Some(ErrorCode::ItemTooLarge)
    );
    let next = cursor(&result.envelope)?.to_owned();
    let output = craft_cli::output::encode(result.envelope, 3072, result.output_policy)?;
    assert!(output.bytes.len() <= 3072);
    assert_eq!(output.exit, 10);
    let recovered = prepared(&setup.grant, Some(&next), 20, 8192, None)?.execute_cached(
        fixture.client(),
        &locked,
        &setup.store,
        TlsPolicy::AdditionalCaBundle(product.root()),
        control()?,
    );
    assert!(recovered.envelope.ok);
    assert_eq!(recovered.envelope.request_id, "original-request");
    assert_eq!(
        recovered.envelope.data.as_ref().ok_or("Missing items")?["items"][0]["name"]
            .as_str()
            .ok_or("Missing name")?
            .len(),
        4000
    );
    assert_eq!(
        recovered
            .envelope
            .data
            .as_ref()
            .ok_or("Missing cache flag")?["cached"],
        true
    );
    assert_eq!(product.finish()?.len(), 4);
    assert_eq!(issuer.finish()?.len(), 1);
    Ok(())
}

#[test]
fn stalled_project_read_obeys_deadline_without_a_retry_or_continuation() -> TestResult {
    let setup = Setup::new()?;
    let issuer = issuer("read:jira-user read:jira-work")?;
    let product = TlsServer::start(Reply::Sequence(vec![identity(), identity(), Reply::Stall]))?;
    let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
    let locked = fixture.lock(&setup.grant, &control()?)?;
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref()],
        Some("actor-one"),
    )?;
    let host = prepared(&setup.grant, None, 20, 16384, None)?;
    let deadline_clock = Arc::new(ManualClock::new(1000));
    let deadline =
        InvocationControl::new(deadline_clock.clone(), CancellationToken::default(), 10_000)?;
    let requests = product.request_counter();
    let worker = std::thread::spawn(move || -> Result<(), String> {
        let started = std::time::Instant::now();
        while requests.load(std::sync::atomic::Ordering::SeqCst) < 3 {
            if started.elapsed() > Duration::from_secs(5) {
                return Err("Project request was not observed".into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        deadline_clock
            .advance(Duration::from_millis(10_000))
            .map_err(|error| error.to_string())
    });
    let result = host.execute_cached(
        fixture.client(),
        &locked,
        &setup.store,
        TlsPolicy::AdditionalCaBundle(product.root()),
        deadline,
    );
    worker
        .join()
        .map_err(|_| "Deadline worker panicked")?
        .map_err(|error| error.to_string())?;
    assert_eq!(
        result.envelope.error.as_ref().map(|error| error.code),
        Some(ErrorCode::DeadlineExceeded)
    );
    assert!(result.envelope.page.is_none());
    assert_eq!(product.request_count(), 3);
    assert_eq!(product.connections(), 3);
    assert_eq!(product.finish()?.len(), 3);
    assert_eq!(issuer.finish()?.len(), 1);
    Ok(())
}

#[test]
fn disconnected_project_read_stops_after_bounded_attempts_without_continuation() -> TestResult {
    let setup = Setup::new()?;
    let issuer = issuer("read:jira-user read:jira-work")?;
    let product = TlsServer::start(Reply::Sequence(vec![
        identity(),
        identity(),
        Reply::Disconnect,
        Reply::Disconnect,
        Reply::Disconnect,
    ]))?;
    let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
    let locked = fixture.lock(&setup.grant, &control()?)?;
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref()],
        Some("actor-one"),
    )?;
    let result = prepared(&setup.grant, None, 20, 16384, None)?.execute_cached(
        fixture.client(),
        &locked,
        &setup.store,
        TlsPolicy::AdditionalCaBundle(product.root()),
        control()?,
    );
    assert_eq!(
        result.envelope.error.as_ref().map(|error| error.code),
        Some(ErrorCode::RemoteUnavailable)
    );
    assert!(result.envelope.page.is_none());
    assert_eq!(product.finish()?.len(), 5);
    assert_eq!(issuer.finish()?.len(), 1);
    Ok(())
}

#[test]
fn invocation_deadline_also_bounds_cached_identity_before_any_project_request() -> TestResult {
    let setup = Setup::new()?;
    let issuer = issuer("read:jira-user read:jira-work")?;
    let product = TlsServer::start(Reply::Sequence(vec![identity(), Reply::Stall]))?;
    let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
    let locked = fixture.lock(&setup.grant, &control()?)?;
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref()],
        Some("actor-one"),
    )?;
    let host = prepared(&setup.grant, None, 20, 16384, None)?;
    let clock = Arc::new(ManualClock::new(1000));
    let deadline = InvocationControl::new(clock.clone(), CancellationToken::default(), 10_000)?;
    let counter = product.request_counter();
    let worker = std::thread::spawn(move || -> Result<(), String> {
        let start = std::time::Instant::now();
        while counter.load(std::sync::atomic::Ordering::SeqCst) < 2 {
            if start.elapsed() > Duration::from_secs(5) {
                return Err("Identity request was not observed".into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        clock
            .advance(Duration::from_millis(10_000))
            .map_err(|error| error.to_string())
    });
    let result = host.execute_cached(
        fixture.client(),
        &locked,
        &setup.store,
        TlsPolicy::AdditionalCaBundle(product.root()),
        deadline,
    );
    worker
        .join()
        .map_err(|_| "Identity deadline worker panicked")?
        .map_err(|error| error.to_string())?;
    assert_eq!(
        result.envelope.error.as_ref().map(|error| error.code),
        Some(ErrorCode::DeadlineExceeded)
    );
    assert!(result.envelope.page.is_none());
    assert_eq!(product.finish()?.len(), 2);
    assert_eq!(issuer.finish()?.len(), 1);
    Ok(())
}

#[test]
fn provider_clamped_page_size_uses_advertised_offset_without_changing_bound_requests() -> TestResult
{
    let setup = Setup::new()?;
    let issuer = issuer("read:jira-user read:jira-work")?;
    let first = Reply::json(
        200,
        &serde_json::to_vec(
            &json!({"startAt":0,"maxResults":2,"isLast":false,"nextPage":format!("{SITE}/rest/api/3/project/search?startAt=2&maxResults=2"),"values":[{"id":"1","key":"ED1","name":"One"},{"id":"2","key":"ED2","name":"Two"}]}),
        )?,
        &[],
    );
    let last=Reply::json(200,br#"{"startAt":2,"maxResults":2,"isLast":true,"values":[{"id":"3","key":"ED3","name":"Three"}]}"#,&[]);
    let product = TlsServer::start(Reply::Sequence(vec![
        identity(),
        identity(),
        first,
        identity(),
        last,
    ]))?;
    let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
    let locked = fixture.lock(&setup.grant, &control()?)?;
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref()],
        Some("actor-one"),
    )?;
    let first = prepared(&setup.grant, None, 20, 16384, None)?.execute_cached(
        fixture.client(),
        &locked,
        &setup.store,
        TlsPolicy::AdditionalCaBundle(product.root()),
        control()?,
    );
    assert!(first.envelope.ok, "{:?}", first.envelope.error);
    let last = prepared(
        &setup.grant,
        Some(cursor(&first.envelope)?),
        20,
        16384,
        None,
    )?
    .execute_cached(
        fixture.client(),
        &locked,
        &setup.store,
        TlsPolicy::AdditionalCaBundle(product.root()),
        control()?,
    );
    assert!(last.envelope.ok, "{:?}", last.envelope.error);
    assert_eq!(
        last.envelope.data.as_ref().ok_or("Missing data")?["items"][0]["id"],
        "3"
    );
    let requests = product.finish()?;
    assert_eq!(requests.len(), 5);
    for index in [2, 4] {
        assert!(requests[index].target.contains("maxResults=50"));
    }
    assert!(requests[4].target.contains("startAt=2"));
    assert_eq!(issuer.finish()?.len(), 1);
    Ok(())
}
