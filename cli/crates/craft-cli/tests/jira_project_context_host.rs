//! Owned TLS acceptance for project context/auth/cache/continuation composition.

#[path = "../../craft-runtime/src/transport/tls_fixture.rs"]
mod tls_fixture;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::{Mutex, atomic::AtomicBool};
use std::time::Duration;

use craft_cli::project_context::{PreparedProjectContext, ProjectContextPolicy};
use craft_core::contracts::{
    AuthMode, CanonicalRequest, Envelope, GrantRef, ResourceRef, Selection,
};
use craft_core::control::{CancellationToken, InvocationControl};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::profiles::ProfileInput;
use craft_core::storage::Clock;
use craft_provider_jira::project_context::{GET, STATUSES};
use craft_runtime::control::SystemClock;
use craft_runtime::profiles::ProfileStore;
use craft_runtime::project_read_policy::{
    ProjectContextOperation, ProjectContextPermissionUpdate, ProjectReadPolicyUpdate,
};
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
        resource_type: "project".into(),
        id: "10000".into(),
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
fn project() -> Value {
    json!({"id":"10000","key":"ED","name":"Project 文字","projectTypeKey":"software","description":"private-sentinel","lead":{"emailAddress":"private-sentinel"},"self":"https://untrusted.invalid"})
}
fn group(id: u64) -> Value {
    json!({"id":id.to_string(),"name":format!("Type {id}"),"subtask":false,"statuses":[{"id":"1","name":"Open","description":"private-sentinel","statusCategory":{"id":2,"name":"New"},"self":"https://untrusted.invalid"}]})
}
fn reply(body: Value) -> Result<Reply, serde_json::Error> {
    Ok(Reply::json(200, &serde_json::to_vec(&body)?, &[]))
}
fn prepared(
    grant: &GrantRef,
    command: &str,
    selection: Selection,
) -> Result<PreparedProjectContext, CraftError> {
    PreparedProjectContext::prepare(
        CanonicalRequest {
            command: command.into(),
            target: target(),
            input: json!({}),
            selection,
            preconditions: None,
        },
        "original-request",
        ProjectContextPolicy {
            target: target(),
            grant: grant.clone(),
            expected_actor: "actor-one".into(),
        },
    )
}
fn selection(fields: &[&str], cursor: Option<&str>, limit: u16, max_bytes: u32) -> Selection {
    Selection {
        fields: fields.iter().map(|field| (*field).into()).collect(),
        cursor: cursor.map(String::from),
        limit,
        max_bytes,
    }
}
fn schema(envelope: &Envelope, fields: &[&str]) -> TestResult {
    use craft_core::registry::Registry;
    let mut registry = Registry::default();
    registry.register_provider(&craft_provider_jira::project_context::JiraProjectContext)?;
    let descriptor = registry.descriptor(&envelope.command)?;
    let projection = registry
        .projection(
            &envelope.command,
            &fields
                .iter()
                .map(|field| (*field).into())
                .collect::<Vec<_>>(),
        )?
        .ok_or("Missing projection")?;
    let schema = projection.response_schema(&descriptor.response_schema)?;
    assert!(jsonschema::validator_for(&schema)?.is_valid(&serde_json::to_value(envelope)?));
    Ok(())
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
        let root =
            craft_test_support::state::fixture_root(&parent, "composed-project-context", stamp);
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

struct PermissionClock {
    base: ManualClock,
    store: Mutex<ProfileStore>,
    requests: Arc<std::sync::atomic::AtomicUsize>,
    threshold: usize,
    operation: ProjectContextOperation,
    mode: &'static str,
    changed: AtomicBool,
}
impl Clock for PermissionClock {
    fn unix_millis(&self) -> u64 {
        if self.requests.load(Ordering::SeqCst) >= self.threshold
            && !self.changed.load(Ordering::SeqCst)
            && let Ok(store) = self.store.lock()
            && let Ok(control) = control()
        {
            let operations = if self.mode == "remove_operation" {
                vec![if self.operation == ProjectContextOperation::Get {
                    ProjectContextOperation::Statuses
                } else {
                    ProjectContextOperation::Get
                }]
            } else {
                vec![
                    ProjectContextOperation::Get,
                    ProjectContextOperation::Statuses,
                ]
            };
            let result = store.set_project_context_permission(
                "factory",
                ProjectContextPermissionUpdate {
                    target: target(),
                    operations,
                    enabled: matches!(self.mode, "changed_native" | "remove_operation"),
                    expected_revision: 2,
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
fn typed_project_context_candidates_use_explicit_permissions_and_stop_on_captured_changes()
-> TestResult {
    use clap::Parser;
    use craft_cli::profiles::InvocationOptions;
    use craft_cli::project_context::command::{
        PreparedInvocation, ProjectContextArgs, ProjectGetArgs, ProjectStatusesArgs,
    };
    #[derive(Parser)]
    struct Get {
        #[command(flatten)]
        args: ProjectGetArgs,
    }
    #[derive(Parser)]
    struct Statuses {
        #[command(flatten)]
        args: ProjectStatusesArgs,
    }
    for operation in [
        ProjectContextOperation::Get,
        ProjectContextOperation::Statuses,
    ] {
        for mode in [
            "success",
            "before_auth",
            "during_auth",
            "after_native",
            "changed_native",
            "remove_operation",
            "buffered",
        ] {
            if operation == ProjectContextOperation::Get && mode == "buffered" {
                continue;
            }
            let setup = Setup::new()?;
            setup.store.set_project_read_policy(
                "factory",
                ProjectReadPolicyUpdate {
                    target: ResourceRef {
                        resource_type: "site".into(),
                        id: CLOUD.into(),
                        ..target()
                    },
                    expected_actor: "actor-one".into(),
                    continuation_site: "https://craft-test.atlassian.net".into(),
                    enabled: false,
                    expected_revision: None,
                },
                true,
                &control()?,
                |_| Ok(()),
            )?;
            setup.store.set_project_context_permission(
                "factory",
                ProjectContextPermissionUpdate {
                    target: target(),
                    operations: vec![
                        ProjectContextOperation::Get,
                        ProjectContextOperation::Statuses,
                    ],
                    enabled: true,
                    expected_revision: 1,
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
                replies.push(reply(if operation == ProjectContextOperation::Get {
                    project()
                } else {
                    json!([group(1), group(2)])
                })?);
            }
            if mode == "buffered" {
                replies.push(identity());
            }
            let product = TlsServer::start(Reply::Sequence(replies))?;
            let clock = Arc::new(PermissionClock {
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
            let args = if operation == ProjectContextOperation::Get {
                ProjectContextArgs::Get(
                    Get::try_parse_from(["candidate", "--project", "10000"])?.args,
                )
            } else {
                ProjectContextArgs::Statuses(
                    Statuses::try_parse_from(["candidate", "--project", "10000", "--limit", "1"])?
                        .args,
                )
            };
            let prepared =
                PreparedInvocation::prepare(args, &[], &options, &setup.store, &control()?)?;
            assert_eq!(prepared.grant(), &setup.grant);
            if mode == "before_auth" {
                setup.store.set_project_context_permission(
                    "factory",
                    ProjectContextPermissionUpdate {
                        target: target(),
                        operations: vec![operation],
                        enabled: false,
                        expected_revision: 2,
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
                let args = ProjectContextArgs::Statuses(
                    Statuses::try_parse_from([
                        "candidate",
                        "--project",
                        "10000",
                        "--cursor",
                        cursor(&result.envelope)?,
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
                if operation == ProjectContextOperation::Get {
                    assert!(data(&result.envelope)?["project"].is_null());
                } else {
                    assert!(result.envelope.page.is_none());
                    assert_eq!(data(&result.envelope)?["items"], json!([]));
                }
                if mode != "before_auth" {
                    assert!(clock.changed.load(Ordering::SeqCst));
                    assert_eq!(
                        data(&result.envelope)?["credential_cache_update"]["write_state"],
                        "confirmed"
                    );
                }
            }
            assert!(
                !setup
                    .store
                    .project_read_policy("factory", &control()?)?
                    .ok_or("Missing configuration")?
                    .enabled
            );
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
fn project_get_uses_one_declared_read_and_preserves_identity_schema_and_renewal() -> TestResult {
    for renew in [false, true] {
        let setup = Setup::new()?;
        let issuer = issuer("read:jira-user read:jira-work")?;
        let product = TlsServer::start(Reply::Sequence(vec![
            identity(),
            identity(),
            reply(project())?,
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
            clock.advance(Duration::from_secs(120))?;
        }
        let result = prepared(&setup.grant, GET, selection(&[], None, 20, 16_384))?.execute_cached(
            fixture.client(),
            &locked,
            &setup.store,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control()?,
        );
        assert!(result.envelope.ok, "{:?}", result.envelope.error);
        schema(&result.envelope, &[])?;
        assert_eq!(data(&result.envelope)?["project"]["id"], "10000");
        assert_eq!(data(&result.envelope)?["identity_resolution"], "exact_id");
        assert_eq!(
            data(&result.envelope)?["credential_cache_update"].is_null(),
            !renew
        );
        assert_eq!(result.envelope.request_id, "original-request");
        let bytes = craft_cli::output::encode(result.envelope, 16_384, result.output_policy)?.bytes;
        assert!(!String::from_utf8(bytes)?.contains("private-sentinel"));
        assert_eq!(product.connections(), 3);
        assert_eq!(product.request_count(), 3);
        let requests = product.finish()?;
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[2].method, "GET");
        assert_eq!(
            requests[2].target,
            format!("/ex/jira/{CLOUD}/rest/api/3/project/10000")
        );
        assert!(requests[2].body.is_empty());
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
fn grouped_status_remainders_resume_after_renewal_without_refetching_or_flattening() -> TestResult {
    let setup = Setup::new()?;
    let issuer = issuer("read:jira-user read:jira-work")?;
    let product = TlsServer::start(Reply::Sequence(vec![
        identity(),
        identity(),
        reply(json!([group(1), group(2)]))?,
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
    let first = prepared(&setup.grant, STATUSES, selection(&[], None, 1, 16_384))?.execute_cached(
        fixture.client(),
        &locked,
        &setup.store,
        TlsPolicy::AdditionalCaBundle(product.root()),
        control()?,
    );
    assert!(first.envelope.ok, "{:?}", first.envelope.error);
    schema(&first.envelope, &[])?;
    assert_eq!(data(&first.envelope)?["items"][0]["id"], "1");
    clock.advance(Duration::from_secs(120))?;
    let second = prepared(
        &setup.grant,
        STATUSES,
        selection(&[], Some(cursor(&first.envelope)?), 20, 16_384),
    )?
    .execute_cached(
        fixture.client(),
        &locked,
        &setup.store,
        TlsPolicy::AdditionalCaBundle(product.root()),
        control()?,
    );
    assert!(second.envelope.ok, "{:?}", second.envelope.error);
    schema(&second.envelope, &[])?;
    assert_eq!(data(&second.envelope)?["items"][0]["id"], "2");
    assert_eq!(
        data(&second.envelope)?["items"][0]["statuses"],
        json!([{"id":"1","name":"Open"}])
    );
    assert_eq!(data(&second.envelope)?["cached"], true);
    assert_eq!(
        data(&second.envelope)?["observation"],
        data(&first.envelope)?["observation"]
    );
    assert_eq!(
        data(&second.envelope)?["credential_cache_update"]["write_state"],
        "confirmed"
    );
    assert!(
        !second
            .envelope
            .page
            .as_ref()
            .ok_or("Missing page")?
            .has_more
    );
    let requests = product.finish()?;
    assert_eq!(requests.len(), 4);
    assert_eq!(
        requests[2].target,
        format!("/ex/jira/{CLOUD}/rest/api/3/project/10000/statuses")
    );
    assert_eq!(issuer.finish()?.len(), 2);
    drop(locked);
    drop(fixture);
    setup.no_plaintext_credentials()?;
    Ok(())
}

#[test]
fn changed_selector_fields_or_actor_refuse_status_cursor_before_auth() -> TestResult {
    let setup = Setup::new()?;
    let issuer = issuer("read:jira-user read:jira-work")?;
    let product = TlsServer::start(Reply::Sequence(vec![
        identity(),
        identity(),
        reply(json!([group(1), group(2)]))?,
    ]))?;
    let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
    let locked = fixture.lock(&setup.grant, &control()?)?;
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref()],
        Some("actor-one"),
    )?;
    let first = prepared(&setup.grant, STATUSES, selection(&[], None, 1, 16_384))?.execute_cached(
        fixture.client(),
        &locked,
        &setup.store,
        TlsPolicy::AdditionalCaBundle(product.root()),
        control()?,
    );
    assert!(first.envelope.ok, "{:?}", first.envelope.error);
    for mode in ["selector", "fields", "actor"] {
        let mut target = target();
        if mode == "selector" {
            target.id = "ED".into();
        }
        let prepared = PreparedProjectContext::prepare(
            CanonicalRequest {
                command: STATUSES.into(),
                target: target.clone(),
                input: json!({}),
                selection: selection(
                    if mode == "fields" { &["id"] } else { &[] },
                    Some(cursor(&first.envelope)?),
                    20,
                    16_384,
                ),
                preconditions: None,
            },
            "request",
            ProjectContextPolicy {
                target,
                grant: setup.grant.clone(),
                expected_actor: if mode == "actor" {
                    "other"
                } else {
                    "actor-one"
                }
                .into(),
            },
        )?;
        let result = prepared.execute_cached(
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
    }
    assert_eq!(product.finish()?.len(), 3);
    assert_eq!(issuer.finish()?.len(), 1);
    Ok(())
}

#[test]
fn oversized_project_has_no_partial_body_and_status_group_recovery_is_lossless() -> TestResult {
    for command in [GET, STATUSES] {
        let setup = Setup::new()?;
        let issuer = issuer("read:jira-user read:jira-work")?;
        let mut large = if command == GET { project() } else { group(1) };
        if command == GET {
            large["description"] = json!("文字".repeat(1500));
        } else {
            large["statuses"][0]["description"] = json!("文字".repeat(1500));
        }
        let fields = if command == GET {
            vec!["description"]
        } else {
            vec!["statuses.description"]
        };
        let body = if command == GET {
            large
        } else {
            json!([large])
        };
        let mut replies = vec![identity(), identity(), reply(body.clone())?, identity()];
        if command == GET {
            replies.push(reply(body)?);
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
        clock.advance(Duration::from_secs(120))?;
        let first = prepared(&setup.grant, command, selection(&fields, None, 20, 4096))?
            .execute_cached(
                fixture.client(),
                &locked,
                &setup.store,
                TlsPolicy::AdditionalCaBundle(product.root()),
                control()?,
            );
        assert_eq!(code(&first.envelope), Some(ErrorCode::ItemTooLarge));
        schema(&first.envelope, &fields)?;
        assert_eq!(
            data(&first.envelope)?["credential_cache_update"]["write_state"],
            "confirmed"
        );
        let next = if command == STATUSES {
            assert_eq!(data(&first.envelope)?["items"], json!([]));
            Some(cursor(&first.envelope)?)
        } else {
            assert!(first.envelope.page.is_none());
            assert!(data(&first.envelope)?["project"].is_null());
            None
        };
        let last = prepared(&setup.grant, command, selection(&fields, next, 20, 16_384))?
            .execute_cached(
                fixture.client(),
                &locked,
                &setup.store,
                TlsPolicy::AdditionalCaBundle(product.root()),
                control()?,
            );
        assert!(last.envelope.ok, "{:?}", last.envelope.error);
        schema(&last.envelope, &fields)?;
        if command == STATUSES {
            assert_eq!(data(&last.envelope)?["cached"], true);
        }
        let bytes = craft_cli::output::encode(first.envelope, 4096, first.output_policy)?.bytes;
        assert!(bytes.len() <= 4096);
        assert_eq!(product.finish()?.len(), if command == GET { 5 } else { 4 });
        assert_eq!(issuer.finish()?.len(), 2);
    }
    Ok(())
}

#[test]
fn scope_actor_http_and_response_failures_preserve_independent_cache_evidence() -> TestResult {
    for mode in [
        "scope",
        "actor",
        "403",
        "404",
        "redirect",
        "disconnect",
        "duplicate",
        "bad_get",
    ] {
        let setup = Setup::new()?;
        let issuer = issuer(if mode == "scope" {
            "read:jira-user"
        } else {
            "read:jira-user read:jira-work"
        })?;
        let mut replies = vec![identity(), identity()];
        if !matches!(mode, "scope" | "actor") {
            replies.push(match mode {
                "403" => Reply::json(403, b"{}", &[]),
                "404" => Reply::json(404, b"{}", &[]),
                "redirect" => Reply::json(302, b"{}", &[("Location", "https://untrusted.invalid")]),
                "disconnect" => Reply::Disconnect,
                "duplicate" => reply(json!([group(1), group(1)]))?,
                _ => reply(json!({"id":"other","key":"ED"}))?,
            });
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
        clock.advance(Duration::from_secs(120))?;
        let command = if mode == "bad_get" { GET } else { STATUSES };
        let prepared = if mode == "actor" {
            PreparedProjectContext::prepare(
                CanonicalRequest {
                    command: command.into(),
                    target: target(),
                    input: json!({}),
                    selection: Selection::default(),
                    preconditions: None,
                },
                "request",
                ProjectContextPolicy {
                    target: target(),
                    grant: setup.grant.clone(),
                    expected_actor: "other".into(),
                },
            )?
        } else {
            prepared(&setup.grant, command, selection(&[], None, 20, 16_384))?
        };
        let result = prepared.execute_cached(
            fixture.client(),
            &locked,
            &setup.store,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control()?,
        );
        assert!(!result.envelope.ok, "{mode}");
        schema(&result.envelope, &[])?;
        assert!(result.envelope.page.is_none());
        assert_eq!(
            data(&result.envelope)?["credential_cache_update"]["write_state"],
            "confirmed"
        );
        if command == STATUSES {
            assert_eq!(data(&result.envelope)?["items"], json!([]));
        } else {
            assert!(data(&result.envelope)?["project"].is_null());
        }
        assert_eq!(
            product.finish()?.len(),
            if matches!(mode, "scope" | "actor") {
                2
            } else {
                3
            }
        );
        assert_eq!(issuer.finish()?.len(), 2);
    }
    Ok(())
}

struct RevocationClock {
    base: ManualClock,
    requests: Arc<std::sync::atomic::AtomicUsize>,
    threshold: usize,
    revoked: std::sync::atomic::AtomicBool,
}
impl Clock for RevocationClock {
    fn unix_millis(&self) -> u64 {
        if self.requests.load(Ordering::SeqCst) >= self.threshold {
            self.revoked.store(true, Ordering::SeqCst);
        }
        self.base.unix_millis()
    }
    fn monotonic_millis(&self) -> u64 {
        self.base.monotonic_millis()
    }
}
#[test]
fn revocation_before_auth_during_auth_and_after_native_or_buffered_work_suppresses_output()
-> TestResult {
    for command in [GET, STATUSES] {
        for mode in ["before_auth", "during_auth", "after_native", "buffered"] {
            if command == GET && mode == "buffered" {
                continue;
            }
            let setup = Setup::new()?;
            let issuer = issuer("read:jira-user read:jira-work")?;
            let mut replies = vec![identity()];
            if mode != "before_auth" {
                replies.push(identity());
            }
            if matches!(mode, "after_native" | "buffered") {
                replies.push(reply(if command == GET {
                    project()
                } else {
                    json!([group(1), group(2)])
                })?);
            }
            if mode == "buffered" {
                replies.push(identity());
            }
            let product = TlsServer::start(Reply::Sequence(replies))?;
            let clock = Arc::new(RevocationClock {
                base: ManualClock::new(1000),
                requests: product.request_counter(),
                threshold: match mode {
                    "during_auth" => 2,
                    "after_native" => 3,
                    "buffered" => 4,
                    _ => usize::MAX,
                },
                revoked: std::sync::atomic::AtomicBool::new(false),
            });
            let fixture = setup.fixture(&issuer, &product, clock.clone())?;
            let locked = fixture.lock(&setup.grant, &control()?)?;
            fixture.client().provision_grant(
                &locked,
                fixture.service_credentials()?,
                &[target().tenant_ref()],
                Some("actor-one"),
            )?;
            if mode == "before_auth" {
                clock.revoked.store(true, Ordering::SeqCst);
            } else if mode != "buffered" {
                clock.base.advance(Duration::from_secs(120))?;
            }
            let check = || {
                if clock.revoked.load(Ordering::SeqCst) {
                    Err(CraftError::new(
                        ErrorCode::PolicyDenied,
                        "Project context policy revoked.",
                    ))
                } else {
                    Ok(())
                }
            };
            let mut result = prepared(&setup.grant, command, selection(&[], None, 1, 16_384))?
                .execute_cached_with_policy_check(
                    fixture.client(),
                    &locked,
                    &setup.store,
                    TlsPolicy::AdditionalCaBundle(product.root()),
                    control()?,
                    check,
                );
            if mode == "buffered" {
                assert!(result.envelope.ok, "{:?}", result.envelope.error);
                let prepared = prepared(
                    &setup.grant,
                    command,
                    selection(&[], Some(cursor(&result.envelope)?), 20, 16_384),
                )?;
                clock.base.advance(Duration::from_secs(120))?;
                result = prepared.execute_cached_with_policy_check(
                    fixture.client(),
                    &locked,
                    &setup.store,
                    TlsPolicy::AdditionalCaBundle(product.root()),
                    control()?,
                    check,
                );
            }
            assert_eq!(
                code(&result.envelope),
                Some(ErrorCode::PolicyDenied),
                "{command} {mode}"
            );
            assert!(result.envelope.page.is_none());
            if command == GET {
                assert!(data(&result.envelope)?["project"].is_null());
            } else {
                assert_eq!(data(&result.envelope)?["items"], json!([]));
            }
            if mode != "before_auth" {
                assert_eq!(
                    data(&result.envelope)?["credential_cache_update"]["write_state"],
                    "confirmed"
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
                if mode == "before_auth" { 1 } else { 2 }
            );
        }
    }
    Ok(())
}

#[test]
fn cancelled_or_expired_resource_io_does_not_retry_or_publish_project_context() -> TestResult {
    for cancelled in [false, true] {
        let setup = Setup::new()?;
        let issuer = issuer("read:jira-user read:jira-work")?;
        let product =
            TlsServer::start(Reply::Sequence(vec![identity(), identity(), Reply::Stall]))?;
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
        let worker = std::thread::spawn(move || -> Result<(), String> {
            let start = std::time::Instant::now();
            while counter.load(Ordering::SeqCst) < 3 {
                if start.elapsed() > Duration::from_secs(5) {
                    return Err("Resource request not observed".into());
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            if cancelled {
                cancel.cancel();
            } else {
                clock
                    .advance(Duration::from_secs(10))
                    .map_err(|error| error.to_string())?;
            }
            Ok(())
        });
        let result = prepared(&setup.grant, STATUSES, selection(&[], None, 20, 16_384))?
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
            Some(if cancelled {
                ErrorCode::Cancelled
            } else {
                ErrorCode::DeadlineExceeded
            })
        );
        assert!(result.envelope.page.is_none());
        assert_eq!(product.finish()?.len(), 3);
        assert_eq!(issuer.finish()?.len(), 1);
    }
    Ok(())
}
