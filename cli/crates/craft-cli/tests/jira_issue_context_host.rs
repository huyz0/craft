//! Owned TLS evidence for issue-bound metadata, renewal receipts and immutable continuation.
#[path = "support/conformance.rs"]
mod conformance;
use conformance::Observed;
#[path = "../../craft-runtime/src/transport/tls_fixture.rs"]
mod tls_fixture;

use craft_cli::issue_context::{IssueContextPolicy, PreparedIssueContext};
use craft_core::contracts::{
    AuthMode, CanonicalRequest, Envelope, GrantRef, ResourceRef, Selection,
};
use craft_core::control::{CancellationToken, InvocationControl};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::profiles::ProfileInput;
use craft_core::storage::Clock;
use craft_provider_jira::issue_context::{EDIT, JiraIssueContext, TRANSITIONS};
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

struct MetadataDeadlineClock(Arc<AtomicUsize>);
impl Clock for MetadataDeadlineClock {
    fn unix_millis(&self) -> u64 {
        1000
    }
    fn monotonic_millis(&self) -> u64 {
        if self.0.load(Ordering::SeqCst) >= 4 {
            1001
        } else {
            0
        }
    }
}

fn site_target() -> ResourceRef {
    ResourceRef {
        resource_type: "site".into(),
        id: CLOUD.into(),
        ..target()
    }
}
struct ConfigClock {
    base: ManualClock,
    store: Mutex<ProfileStore>,
    requests: Arc<AtomicUsize>,
    threshold: usize,
    operation: craft_runtime::issue_read_policy::IssueReadOperation,
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
            let operation = if self.mode == "remove_operation" {
                if self.operation == IssueReadOperation::EditMetadata {
                    IssueReadOperation::Transitions
                } else {
                    IssueReadOperation::EditMetadata
                }
            } else {
                self.operation
            };
            let result = store.set_issue_read_policy(
                "factory",
                IssueReadPolicyUpdate {
                    target: site_target(),
                    expected_actor: "actor-one".into(),
                    operations: vec![operation],
                    enabled: matches!(self.mode, "changed_bracket" | "remove_operation"),
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
fn typed_metadata_candidates_stop_at_captured_policy_changes_across_each_resource_and_buffer()
-> TestResult {
    use clap::Parser;
    use craft_cli::issue_context::command::{
        EditMetadataArgs, IssueContextArgs, PreparedInvocation, TransitionListArgs,
    };
    use craft_cli::profiles::InvocationOptions;
    use craft_runtime::issue_read_policy::{IssueReadOperation, IssueReadPolicyUpdate};
    #[derive(Parser)]
    struct Edit {
        #[command(flatten)]
        args: EditMetadataArgs,
    }
    #[derive(Parser)]
    struct Transitions {
        #[command(flatten)]
        args: TransitionListArgs,
    }
    for operation in [
        IssueReadOperation::EditMetadata,
        IssueReadOperation::Transitions,
    ] {
        for mode in [
            "success",
            "before_auth",
            "during_auth",
            "after_identity",
            "after_metadata",
            "after_bracket",
            "changed_bracket",
            "remove_operation",
            "buffered",
        ] {
            let setup = Setup::new()?;
            let issuer = issuer()?;
            let product = TlsServer::start(Reply::Sequence(vec![
                identity(),
                identity(),
                reply(state())?,
                reply(metadata(operation.command()))?,
                reply(state())?,
                identity(),
            ]))?;
            let record = || IssueReadPolicyUpdate {
                target: site_target(),
                expected_actor: "actor-one".into(),
                operations: vec![operation],
                enabled: true,
                expected_revision: None,
            };
            setup.store.set_issue_read_policy(
                "factory",
                record(),
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
                    "after_identity" => 3,
                    "after_metadata" => 4,
                    "buffered" => 6,
                    _ => 5,
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
            let args = if operation == IssueReadOperation::EditMetadata {
                IssueContextArgs::Edit(
                    Edit::try_parse_from(["candidate", "--issue", "OLD-1", "--limit", "1"])?.args,
                )
            } else {
                IssueContextArgs::Transitions(
                    Transitions::try_parse_from(["candidate", "--issue", "OLD-1", "--limit", "1"])?
                        .args,
                )
            };
            let options = InvocationOptions {
                state_root: Some(&setup.root),
                profile: Some("factory"),
                request_id: "original-request",
                max_bytes: 16_384,
            };
            let prepared =
                PreparedInvocation::prepare(args, &[], &options, &setup.store, &control()?)?;
            if mode == "before_auth" {
                let mut update = record();
                update.enabled = false;
                update.expected_revision = Some(1);
                setup
                    .store
                    .set_issue_read_policy("factory", update, true, &control()?, |_| Ok(()))?;
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
                let args = if operation == IssueReadOperation::EditMetadata {
                    IssueContextArgs::Edit(
                        Edit::try_parse_from(["candidate", "--issue", "OLD-1", "--cursor", next])?
                            .args,
                    )
                } else {
                    IssueContextArgs::Transitions(
                        Transitions::try_parse_from([
                            "candidate",
                            "--issue",
                            "OLD-1",
                            "--cursor",
                            next,
                        ])?
                        .args,
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
            validate(&result.envelope, &[], 16_384)?;
            if mode == "success" {
                assert!(result.envelope.ok, "{:?}", result.envelope.error);
            } else {
                assert_eq!(
                    code(&result.envelope),
                    Some(if mode == "changed_bracket" {
                        ErrorCode::VersionConflict
                    } else {
                        ErrorCode::PolicyDenied
                    }),
                    "{} {mode}",
                    operation.command()
                );
                assert_eq!(data(&result.envelope)?["items"], json!([]));
                assert!(data(&result.envelope)?["issue_observation"].is_null());
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
                    .field_read_policy("factory", &control()?)?
                    .is_none()
            );
            assert!(
                setup
                    .store
                    .project_read_policy("factory", &control()?)?
                    .is_none()
            );
            let output = craft_cli::output::encode(result.envelope, 16_384, result.output_policy)?;
            assert!(output.bytes.len() <= 16_384);
            assert_eq!(
                product.finish()?.len(),
                match mode {
                    "before_auth" => 1,
                    "during_auth" => 2,
                    "after_identity" => 3,
                    "after_metadata" => 4,
                    "buffered" => 6,
                    _ => 5,
                },
                "{} {mode}",
                operation.command()
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
        resource_type: "issue".into(),
        id: "OLD-1".into(),
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
fn issuer() -> Result<TlsServer, Box<dyn std::error::Error>> {
    TlsServer::start(reply(
        json!({"access_token":ACCESS_TOKEN,"expires_in":120,"token_type":"Bearer","scope":"read:jira-user read:jira-work"}),
    )?)
}
fn reply(body: Value) -> Result<Reply, serde_json::Error> {
    Ok(Reply::json(200, &serde_json::to_vec(&body)?, &[]))
}
fn state() -> Value {
    json!({"id":"10000","key":"ED-7","fields":{"project":{"id":"100"},"issuetype":{"id":"101"},"status":{"id":"102"},"updated":"2026-11-10T01:00:00.000+0000"}})
}
fn field() -> Value {
    json!({"key":"native-key","name":"Field 文字","required":true,"operations":["set"],"schema":{"type":"string","configuration":{"private":"unselected-sentinel"}},"description":"unselected-sentinel"})
}
fn metadata(command: &str) -> Value {
    if command == EDIT {
        json!({"fields":{"customfield_1":field(),"customfield_2":field()}})
    } else {
        json!({"transitions":[{"id":"31","name":"Done","to":{"id":"103","name":"Done"},"hasScreen":false,"isAvailable":true},{"id":"41","name":"Reopen","hasScreen":true,"isAvailable":false}]})
    }
}
fn prepared(
    grant: &GrantRef,
    command: &str,
    fields: &[&str],
    cursor: Option<&str>,
    max_bytes: u32,
) -> Result<PreparedIssueContext, CraftError> {
    PreparedIssueContext::prepare(
        CanonicalRequest {
            command: command.into(),
            target: target(),
            input: json!({"issue":"OLD-1"}),
            selection: Selection {
                fields: fields.iter().map(|value| (*value).into()).collect(),
                cursor: cursor.map(String::from),
                limit: 1,
                max_bytes,
            },
            preconditions: None,
        },
        "original-request",
        IssueContextPolicy {
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
    registry.register_provider(&JiraIssueContext)?;
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
        let root =
            craft_test_support::state::fixture_root(&parent, "composed-issue-context", stamp);
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
fn edit_and_transition_pages_keep_issue_observation_across_reopen_and_immutable_replay()
-> TestResult {
    for command in [EDIT, TRANSITIONS] {
        let setup = Setup::new()?;
        let issuer = issuer()?;
        let product = TlsServer::start(Reply::Sequence(vec![
            identity(),
            identity(),
            reply(state())?,
            reply(metadata(command))?,
            reply(state())?,
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
        let first = prepared(&setup.grant, command, &[], None, 16_384)?
            .execute_cached(
                fixture.client(),
                &locked,
                &setup.store,
                TlsPolicy::AdditionalCaBundle(product.root()),
                control()?,
            )
            .observed();
        assert!(first.envelope.ok, "{:?}", first.envelope.error);
        validate(&first.envelope, &[], 16_384)?;
        assert_eq!(
            data(&first.envelope)?["credential_cache_update"]["write_state"],
            "confirmed"
        );
        assert_eq!(
            data(&first.envelope)?["issue_observation"]["resolution"],
            "resolved_key"
        );
        let reopened = ProfileStore::open(&setup.root, &control()?)?;
        for _ in 0..2 {
            let last = prepared(
                &setup.grant,
                command,
                &[],
                Some(cursor(&first.envelope)?),
                16_384,
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
            validate(&last.envelope, &[], 16_384)?;
            assert_eq!(data(&last.envelope)?["cached"], true);
            assert_eq!(
                data(&last.envelope)?["items"][0]["id"],
                if command == EDIT {
                    "customfield_2"
                } else {
                    "41"
                }
            );
            for field in ["observation", "issue_observation"] {
                assert_eq!(data(&first.envelope)?[field], data(&last.envelope)?[field]);
            }
            assert!(!last.envelope.page.as_ref().ok_or("Missing page")?.has_more);
        }
        drop(locked);
        setup.no_plaintext_credentials()?;
        let records = product.finish()?;
        assert_eq!(records.len(), 7);
        assert!(
            records[2]
                .target
                .starts_with(&format!("/ex/jira/{CLOUD}/rest/api/3/issue/OLD-1?"))
        );
        assert_eq!(
            records[3].target,
            format!(
                "/ex/jira/{CLOUD}/rest/api/3/issue/10000/{}{}",
                if command == EDIT {
                    "editmeta"
                } else {
                    "transitions"
                },
                if command == EDIT {
                    ""
                } else {
                    "?includeUnavailableTransitions=false&sortByOpsBarAndStatus=false"
                }
            )
        );
        assert!(
            records[4]
                .target
                .starts_with(&format!("/ex/jira/{CLOUD}/rest/api/3/issue/10000?"))
        );
        assert!(records.iter().all(|record| record.method == "GET"));
        for record in &records {
            assert!(record.body.is_empty());
            assert_eq!(
                record.headers.get("authorization").map(String::as_str),
                Some("Bearer craft-synthetic-access-token")
            );
        }
        assert_eq!(issuer.finish()?.len(), 2);
    }
    Ok(())
}

#[test]
fn empty_metadata_keeps_bracket_observation_without_claiming_absence_or_authority() -> TestResult {
    for command in [EDIT, TRANSITIONS] {
        let setup = Setup::new()?;
        let issuer = issuer()?;
        let empty = if command == EDIT {
            json!({"fields":{}})
        } else {
            json!({"transitions":[]})
        };
        let product = TlsServer::start(Reply::Sequence(vec![
            identity(),
            identity(),
            reply(state())?,
            reply(empty)?,
            reply(state())?,
        ]))?;
        let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
        let locked = fixture.lock(&setup.grant, &control()?)?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        let result = prepared(&setup.grant, command, &[], None, 16_384)?
            .execute_cached(
                fixture.client(),
                &locked,
                &setup.store,
                TlsPolicy::AdditionalCaBundle(product.root()),
                control()?,
            )
            .observed();
        assert!(result.envelope.ok, "{:?}", result.envelope.error);
        validate(&result.envelope, &[], 16_384)?;
        assert_eq!(data(&result.envelope)?["items"], json!([]));
        assert_eq!(
            data(&result.envelope)?["issue_observation"]["state"]["status_id"],
            "102"
        );
        assert!(
            !result
                .envelope
                .page
                .as_ref()
                .ok_or("Missing page")?
                .has_more
        );
        assert_eq!(product.finish()?.len(), 5);
        assert_eq!(issuer.finish()?.len(), 1);
    }
    Ok(())
}

#[test]
fn state_drift_and_remote_denial_suppress_items_but_keep_renewal_receipts() -> TestResult {
    for mode in ["drift", "denied"] {
        let setup = Setup::new()?;
        let issuer = issuer()?;
        let mut after = state();
        after["fields"]["status"]["id"] = json!("999");
        let mut replies = vec![identity(), identity(), reply(state())?];
        replies.push(if mode == "denied" {
            Reply::json(403, br#"{"errorMessages":["private-error-sentinel"]}"#, &[])
        } else {
            reply(metadata(EDIT))?
        });
        if mode == "drift" {
            replies.push(reply(after)?);
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
        let result = prepared(&setup.grant, EDIT, &[], None, 16_384)?
            .execute_cached(
                fixture.client(),
                &locked,
                &setup.store,
                TlsPolicy::AdditionalCaBundle(product.root()),
                control()?,
            )
            .observed();
        assert_eq!(
            code(&result.envelope),
            Some(if mode == "drift" {
                ErrorCode::VersionConflict
            } else {
                ErrorCode::PermissionDenied
            })
        );
        validate(&result.envelope, &[], 16_384)?;
        assert_eq!(data(&result.envelope)?["items"], json!([]));
        assert!(data(&result.envelope)?["issue_observation"].is_null());
        assert_eq!(
            data(&result.envelope)?["credential_cache_update"]["write_state"],
            "confirmed"
        );
        assert!(result.envelope.page.is_none());
        let output = craft_cli::output::encode(result.envelope, 16_384, result.output_policy)?;
        assert!(!String::from_utf8(output.bytes)?.contains("private-error-sentinel"));
        assert_eq!(product.finish()?.len(), if mode == "drift" { 5 } else { 4 });
        assert_eq!(issuer.finish()?.len(), 2);
    }
    Ok(())
}

#[test]
fn late_policy_denial_and_cancellation_suppress_results_after_bracket_reads() -> TestResult {
    for command in [EDIT, TRANSITIONS] {
        for mode in ["policy", "cancel"] {
            let setup = Setup::new()?;
            let issuer = issuer()?;
            let product = TlsServer::start(Reply::Sequence(vec![
                identity(),
                identity(),
                reply(state())?,
                reply(metadata(command))?,
                reply(state())?,
            ]))?;
            let count = product.request_counter();
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
            let cancellation = CancellationToken::default();
            let invocation = InvocationControl::new(
                Arc::new(SystemClock::default()),
                cancellation.clone(),
                30_000,
            )?;
            let result = prepared(&setup.grant, command, &[], None, 16_384)?
                .execute_cached_with_policy_check(
                    fixture.client(),
                    &locked,
                    &setup.store,
                    TlsPolicy::AdditionalCaBundle(product.root()),
                    invocation,
                    || {
                        if count.load(Ordering::SeqCst) >= 5 {
                            if mode == "cancel" {
                                cancellation.cancel();
                            } else {
                                return Err(CraftError::new(
                                    ErrorCode::PolicyDenied,
                                    "Policy changed.",
                                ));
                            }
                        }
                        Ok(())
                    },
                )
                .observed();
            assert!(!result.envelope.ok);
            validate(&result.envelope, &[], 16_384)?;
            assert_eq!(data(&result.envelope)?["items"], json!([]));
            assert_eq!(
                data(&result.envelope)?["credential_cache_update"]["write_state"],
                "confirmed"
            );
            assert!(result.envelope.page.is_none());
            assert_eq!(product.finish()?.len(), 5);
            assert_eq!(issuer.finish()?.len(), 2);
        }
    }
    Ok(())
}

#[test]
fn oversized_selected_metadata_recovers_from_buffer_with_full_issue_context() -> TestResult {
    let setup = Setup::new()?;
    let issuer = issuer()?;
    let mut body = metadata(EDIT);
    body["fields"]["customfield_1"]["defaultValue"] = json!("x".repeat(20_000));
    let product = TlsServer::start(Reply::Sequence(vec![
        identity(),
        identity(),
        reply(state())?,
        reply(body)?,
        reply(state())?,
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
    let fields = ["default_value"];
    let first = prepared(&setup.grant, EDIT, &fields, None, 16_384)?
        .execute_cached(
            fixture.client(),
            &locked,
            &setup.store,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control()?,
        )
        .observed();
    assert_eq!(code(&first.envelope), Some(ErrorCode::ItemTooLarge));
    validate(&first.envelope, &fields, 16_384)?;
    assert!(data(&first.envelope)?["issue_observation"].is_object());
    let recovered = prepared(
        &setup.grant,
        EDIT,
        &fields,
        Some(cursor(&first.envelope)?),
        65_536,
    )?
    .execute_cached(
        fixture.client(),
        &locked,
        &setup.store,
        TlsPolicy::AdditionalCaBundle(product.root()),
        control()?,
    )
    .observed();
    assert!(recovered.envelope.ok, "{:?}", recovered.envelope.error);
    validate(&recovered.envelope, &fields, 65_536)?;
    assert_eq!(
        data(&recovered.envelope)?["items"][0]["default_value"]
            .as_str()
            .ok_or("Missing value")?
            .len(),
        20_000
    );
    assert_eq!(data(&recovered.envelope)?["cached"], true);
    assert_eq!(
        data(&first.envelope)?["issue_observation"],
        data(&recovered.envelope)?["issue_observation"]
    );
    assert_eq!(product.finish()?.len(), 6);
    assert_eq!(issuer.finish()?.len(), 1);
    Ok(())
}

#[test]
fn interrupted_metadata_read_obeys_deadline_and_never_retries_or_publishes_partial_context()
-> TestResult {
    for command in [EDIT, TRANSITIONS] {
        for mode in ["disconnect", "deadline"] {
            let setup = Setup::new()?;
            let issuer = issuer()?;
            let product = TlsServer::start(Reply::Sequence(vec![
                identity(),
                identity(),
                reply(state())?,
                if mode == "deadline" {
                    Reply::Stall
                } else {
                    Reply::Disconnect
                },
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
            // Start the deadline fault only after the server receives the metadata request.
            let invocation = InvocationControl::new(
                Arc::new(MetadataDeadlineClock(product.request_counter())),
                CancellationToken::default(),
                1000,
            )?;
            let result = prepared(&setup.grant, command, &[], None, 16_384)?
                .execute_cached(
                    fixture.client(),
                    &locked,
                    &setup.store,
                    TlsPolicy::AdditionalCaBundle(product.root()),
                    invocation,
                )
                .observed();
            if mode == "deadline" {
                assert_eq!(code(&result.envelope), Some(ErrorCode::DeadlineExceeded));
            } else {
                // An abrupt TLS close can surface as EOF or wait until the request deadline.
                assert!(matches!(
                    code(&result.envelope),
                    Some(ErrorCode::RemoteUnavailable | ErrorCode::DeadlineExceeded)
                ));
            }
            validate(&result.envelope, &[], 16_384)?;
            assert_eq!(data(&result.envelope)?["items"], json!([]));
            assert!(data(&result.envelope)?["issue_observation"].is_null());
            assert_eq!(
                data(&result.envelope)?["credential_cache_update"]["write_state"],
                "confirmed"
            );
            assert!(result.envelope.page.is_none());
            assert_eq!(product.request_count(), 4);
            assert_eq!(product.connections(), 4);
            assert_eq!(product.finish()?.len(), 4);
            assert_eq!(issuer.finish()?.len(), 2);
        }
    }
    Ok(())
}
