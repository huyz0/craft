//! Actual Bitbucket host, static grant, policy, journal and paging against owned TLS servers.
// Shared fixture: not every helper is used by every suite.
#[allow(dead_code)]
#[path = "../../craft-runtime/src/transport/tls_fixture.rs"]
mod tls_fixture;
use craft_cli::bitbucket_reads::{BitbucketReadArgs, PreparedInvocation};
use craft_cli::bitbucket_writes::{
    BitbucketWriteArgs, BitbucketWriteEnvironment, PreparedBitbucketWrite,
};
use craft_cli::profiles::InvocationOptions;
use craft_core::contracts::{
    AuthMode, CanonicalRequest, GrantRef, RequestState, ResourceRef, Selection,
};
use craft_core::control::{CancellationToken, InvocationControl};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::profiles::ProfileInput;
use craft_core::storage::OperationJournal;
use craft_runtime::control::SystemClock;
use craft_runtime::journal::NativeJournal;
use craft_runtime::profiles::ProfileStore;
use craft_runtime::provider_policy::{BITBUCKET_READ, BITBUCKET_WRITE, ProviderPolicyUpdate};
use craft_runtime::synthetic_acceptance::{STATIC_SECRET, ServiceFixture};
use craft_runtime::transport::TlsPolicy;
use craft_test_support::state::ManualClock;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tls_fixture::{Reply, TlsServer};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const WORKSPACE: &str = "team";
const ACTOR: &str = "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0";

fn control() -> Result<InvocationControl, CraftError> {
    InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        30_000,
    )
}

fn workspace() -> ResourceRef {
    ResourceRef {
        provider: "bitbucket".into(),
        deployment: "cloud".into(),
        tenant: WORKSPACE.into(),
        resource_type: "workspace".into(),
        id: WORKSPACE.into(),
    }
}

fn reply(status: u16, body: Value) -> Result<Reply, Box<dyn std::error::Error>> {
    Ok(Reply::json(status, &serde_json::to_vec(&body)?, &[]))
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
            "bitbucket-host",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos(),
        );
        let store = ProfileStore::open(&root, &control()?)?;
        let profile = store.create(
            &ProfileInput {
                name: "code".into(),
                provider: "bitbucket".into(),
                deployment: "cloud".into(),
                auth_mode: AuthMode::Service,
                grant: None,
                tenant: Some(WORKSPACE.into()),
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

    fn fixture(&self, product: &TlsServer) -> Result<ServiceFixture, CraftError> {
        let port = product.address().port();
        ServiceFixture::new(
            &self.store,
            port,
            port,
            product.root(),
            control()?,
            Arc::new(ManualClock::new(1_000)),
        )
    }

    fn policy(&self, class: &str, operations: &[&str], enabled: bool) -> TestResult {
        self.store.set_provider_policy(
            "code",
            ProviderPolicyUpdate {
                class: class.into(),
                target: workspace(),
                expected_actor: ACTOR.into(),
                operations: operations.iter().map(|name| (*name).into()).collect(),
                enabled,
                expected_revision: None,
            },
            true,
            &control()?,
            |_| Ok(()),
        )?;
        Ok(())
    }

    fn no_secrets(&self) -> TestResult {
        for entry in std::fs::read_dir(&self.root)? {
            let path = entry?.path();
            if path.is_file() {
                // A database held open by this process cannot be read on Windows; skip it.
                let Ok(bytes) = std::fs::read(path) else {
                    continue;
                };
                for value in [
                    "craftsyntheticfixturekey",
                    "craftsyntheticfixturetoken",
                    STATIC_SECRET,
                ] {
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

fn identity() -> Result<Reply, Box<dyn std::error::Error>> {
    reply(
        200,
        json!({"slug": WORKSPACE, "uuid": format!("{{{ACTOR}}}")}),
    )
}

fn pull_request(id: u64, state: &str) -> Value {
    json!({
        "id": id, "title": "Fix login", "state": state,
        "source": {"branch": {"name": "feature/login"}},
        "destination": {"branch": {"name": "main"}},
        "author": {"display_name": "Agent"}
    })
}

fn write_request(command: &str, input: Value, max_bytes: u32) -> CanonicalRequest {
    CanonicalRequest {
        command: command.into(),
        target: workspace(),
        input,
        selection: Selection {
            max_bytes,
            ..Selection::default()
        },
        preconditions: None,
    }
}

fn create_request(max_bytes: u32) -> CanonicalRequest {
    write_request(
        "bitbucket.pullrequest.create",
        json!({"repo": "service", "title": "Fix login", "source_branch": "feature/login", "destination_branch": "main"}),
        max_bytes,
    )
}

/// Provisions the fixture credential and returns the locked grant for the run.
fn provision<'a>(
    setup: &'a Setup,
    fixture: &'a ServiceFixture,
) -> Result<craft_runtime::secrets::LockedCredentials<'a>, Box<dyn std::error::Error>> {
    let locked = fixture.lock(&setup.grant, &control()?)?;
    let receipt = fixture.client().provision_static_grant(
        &locked,
        &workspace().tenant_ref(),
        fixture.static_credentials()?,
    )?;
    assert_eq!(receipt.actor, ACTOR);
    Ok(locked)
}

#[test]
fn a_confirmed_create_is_claimed_sent_once_recorded_and_free_of_secrets() -> TestResult {
    let setup = Setup::new()?;
    let product = TlsServer::start(Reply::Sequence(vec![
        identity()?,
        reply(201, pull_request(7, "OPEN"))?,
    ]))?;
    let fixture = setup.fixture(&product)?;
    let locked = provision(&setup, &fixture)?;
    setup.policy(BITBUCKET_WRITE, &["bitbucket.pullrequest.create"], true)?;
    let prepared = PreparedBitbucketWrite::prepare(
        create_request(65_536),
        "open-pr",
        true,
        "code",
        &setup.store,
        &control()?,
    )?;
    let journal =
        NativeJournal::open(&setup.store, &control()?).map_err(|_| "journal unavailable")?;
    let (data, error) = prepared.execute(
        BitbucketWriteEnvironment {
            store: &setup.store,
            client: fixture.client(),
            locked: &locked,
            journal: &journal,
            tls: TlsPolicy::AdditionalCaBundle(product.root()),
            control: control()?,
        },
        || Ok(()),
    );
    assert!(error.is_none(), "{:?}", error.map(|e| e.message));
    assert_eq!(data.receipt.request_state, RequestState::Confirmed);
    assert_eq!(data.receipt.confirmed_resources[0].id, "7");
    assert!(data.desired_fields_verified && !data.replay_authorized);
    let record = journal
        .get("open-pr")
        .map_err(|_| "journal read")?
        .ok_or("journal record")?;
    assert_eq!(record.steps.len(), 1);
    let requests = product.finish()?;
    // One identity proof, then exactly one write.
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].method, "POST");
    assert_eq!(
        requests[1].target,
        "/2.0/repositories/team/service/pullrequests"
    );
    let authorization = requests[1]
        .headers
        .get("authorization")
        .ok_or("authorization header")?;
    assert!(authorization.starts_with("Basic "));
    let body: Value = serde_json::from_slice(&requests[1].body)?;
    assert_eq!(
        body,
        json!({
            "title": "Fix login",
            "source": {"branch": {"name": "feature/login"}},
            "destination": {"branch": {"name": "main"}}
        })
    );
    setup.no_secrets()
}

fn run_write(
    setup: &Setup,
    fixture: &ServiceFixture,
    locked: &craft_runtime::secrets::LockedCredentials<'_>,
    journal: &NativeJournal,
    product: &TlsServer,
    request: CanonicalRequest,
    operation_id: &str,
) -> Result<
    (
        craft_cli::bitbucket_writes::BitbucketWriteData,
        Option<CraftError>,
    ),
    Box<dyn std::error::Error>,
> {
    let prepared = PreparedBitbucketWrite::prepare(
        request,
        operation_id,
        true,
        "code",
        &setup.store,
        &control()?,
    )?;
    Ok(prepared.execute(
        BitbucketWriteEnvironment {
            store: &setup.store,
            client: fixture.client(),
            locked,
            journal,
            tls: TlsPolicy::AdditionalCaBundle(product.root()),
            control: control()?,
        },
        || Ok(()),
    ))
}

#[test]
fn a_server_error_leaves_an_unknown_receipt_and_the_operation_cannot_be_replayed() -> TestResult {
    let setup = Setup::new()?;
    let product = TlsServer::start(Reply::Sequence(vec![
        identity()?,
        reply(500, json!({"error": {"message": "provider-body-sentinel"}}))?,
    ]))?;
    let fixture = setup.fixture(&product)?;
    let locked = provision(&setup, &fixture)?;
    setup.policy(BITBUCKET_WRITE, &["bitbucket.pullrequest.create"], true)?;
    let journal =
        NativeJournal::open(&setup.store, &control()?).map_err(|_| "journal unavailable")?;
    let (data, error) = run_write(
        &setup,
        &fixture,
        &locked,
        &journal,
        &product,
        create_request(65_536),
        "unclear",
    )?;
    let error = error.ok_or("expected an uncertain outcome")?;
    assert_eq!(error.code, ErrorCode::WriteOutcomeUnknown);
    assert_eq!(data.receipt.request_state, RequestState::SentUnknown);
    assert!(data.reconciliation_required && !data.replay_authorized);
    assert!(!serde_json::to_string(&data)?.contains("provider-body-sentinel"));
    let (_, second) = run_write(
        &setup,
        &fixture,
        &locked,
        &journal,
        &product,
        create_request(65_536),
        "unclear",
    )?;
    assert_eq!(
        second.ok_or("expected refusal")?.code,
        ErrorCode::OperationIdReused
    );
    assert_eq!(product.finish()?.len(), 2);
    Ok(())
}

#[test]
fn a_merge_conflict_is_recorded_as_a_clean_failure_and_is_not_replayed() -> TestResult {
    let setup = Setup::new()?;
    let product = TlsServer::start(Reply::Sequence(vec![identity()?, reply(409, json!({}))?]))?;
    let fixture = setup.fixture(&product)?;
    let locked = provision(&setup, &fixture)?;
    setup.policy(BITBUCKET_WRITE, &["bitbucket.pullrequest.merge"], true)?;
    let journal =
        NativeJournal::open(&setup.store, &control()?).map_err(|_| "journal unavailable")?;
    let merge = write_request(
        "bitbucket.pullrequest.merge",
        json!({"repo": "service", "pull_request_id": 7, "merge_strategy": "squash"}),
        65_536,
    );
    let (data, error) = run_write(
        &setup,
        &fixture,
        &locked,
        &journal,
        &product,
        merge.clone(),
        "merge-7",
    )?;
    assert_eq!(
        error.ok_or("expected a refusal")?.code,
        ErrorCode::VersionConflict
    );
    assert_eq!(data.receipt.request_state, RequestState::Failed);
    assert!(!data.replay_authorized);
    let (_, second) = run_write(
        &setup, &fixture, &locked, &journal, &product, merge, "merge-7",
    )?;
    assert_eq!(
        second.ok_or("expected refusal")?.code,
        ErrorCode::OperationIdReused
    );
    let requests = product.finish()?;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].method, "POST");
    assert_eq!(
        requests[1].target,
        "/2.0/repositories/team/service/pullrequests/7/merge"
    );
    Ok(())
}

#[test]
fn withdrawing_an_approval_is_one_delete_and_an_empty_204_confirms_it() -> TestResult {
    let setup = Setup::new()?;
    let product = TlsServer::start(Reply::Sequence(vec![
        identity()?,
        Reply::json(204, b"", &[]),
    ]))?;
    let fixture = setup.fixture(&product)?;
    let locked = provision(&setup, &fixture)?;
    setup.policy(BITBUCKET_WRITE, &["bitbucket.pullrequest.unapprove"], true)?;
    let journal =
        NativeJournal::open(&setup.store, &control()?).map_err(|_| "journal unavailable")?;
    let request = write_request(
        "bitbucket.pullrequest.unapprove",
        json!({"repo": "service", "pull_request_id": 7}),
        65_536,
    );
    let (data, error) = run_write(
        &setup,
        &fixture,
        &locked,
        &journal,
        &product,
        request,
        "unapprove-7",
    )?;
    assert!(error.is_none(), "{:?}", error.map(|e| e.message));
    assert_eq!(data.receipt.request_state, RequestState::Confirmed);
    assert_eq!(
        data.receipt.confirmed_resources[0].resource_type,
        "pullrequest"
    );
    let requests = product.finish()?;
    assert_eq!(requests[1].method, "DELETE");
    assert_eq!(
        requests[1].target,
        "/2.0/repositories/team/service/pullrequests/7/approve"
    );
    assert!(requests[1].body.is_empty());
    Ok(())
}

#[test]
fn missing_apply_policy_operation_or_budget_refuse_before_any_request() -> TestResult {
    let setup = Setup::new()?;
    let control = control()?;
    let attempt = |request: CanonicalRequest, apply: bool| {
        PreparedBitbucketWrite::prepare(request, "op", apply, "code", &setup.store, &control)
            .err()
            .map(|e| e.code)
    };
    assert_eq!(
        attempt(create_request(65_536), false),
        Some(ErrorCode::ApplyRequired)
    );
    assert_eq!(
        attempt(create_request(512), true),
        Some(ErrorCode::OutputBudgetTooSmall)
    );
    assert_eq!(
        attempt(create_request(65_536), true),
        Some(ErrorCode::PolicyDenied)
    );
    // A policy for another command does not allow this one, and merge needs its own entry.
    setup.policy(BITBUCKET_WRITE, &["bitbucket.comment.add"], true)?;
    assert_eq!(
        attempt(create_request(65_536), true),
        Some(ErrorCode::PolicyDenied)
    );
    let merge = write_request(
        "bitbucket.pullrequest.merge",
        json!({"repo": "service", "pull_request_id": 7, "merge_strategy": "squash"}),
        65_536,
    );
    assert_eq!(attempt(merge, true), Some(ErrorCode::PolicyDenied));
    Ok(())
}

#[test]
fn a_policy_disabled_during_the_call_stops_the_send_without_a_request() -> TestResult {
    let setup = Setup::new()?;
    let product = TlsServer::start(Reply::Sequence(vec![identity()?]))?;
    let fixture = setup.fixture(&product)?;
    let locked = provision(&setup, &fixture)?;
    setup.policy(BITBUCKET_WRITE, &["bitbucket.pullrequest.create"], true)?;
    let prepared = PreparedBitbucketWrite::prepare(
        create_request(65_536),
        "disabled-mid-call",
        true,
        "code",
        &setup.store,
        &control()?,
    )?;
    let journal =
        NativeJournal::open(&setup.store, &control()?).map_err(|_| "journal unavailable")?;
    setup.store.set_provider_policy(
        "code",
        ProviderPolicyUpdate {
            class: BITBUCKET_WRITE.into(),
            target: workspace(),
            expected_actor: ACTOR.into(),
            operations: vec!["bitbucket.pullrequest.create".into()],
            enabled: false,
            expected_revision: Some(1),
        },
        true,
        &control()?,
        |_| Ok(()),
    )?;
    let (data, error) = prepared.execute(
        BitbucketWriteEnvironment {
            store: &setup.store,
            client: fixture.client(),
            locked: &locked,
            journal: &journal,
            tls: TlsPolicy::AdditionalCaBundle(product.root()),
            control: control()?,
        },
        || Ok(()),
    );
    assert_eq!(
        error.ok_or("expected refusal")?.code,
        ErrorCode::PolicyDenied
    );
    assert_eq!(data.receipt.request_state, RequestState::NotSent);
    // Only the provisioning identity proof reached the server.
    assert_eq!(product.finish()?.len(), 1);
    Ok(())
}

#[test]
fn a_read_pages_through_the_static_grant_policy_and_next_link() -> TestResult {
    let setup = Setup::new()?;
    // The synthetic fixture rewrites the origin, so the link is relative (same route).
    let next = "?state=OPEN&sort=-updated_on&pagelen=2&page=2";
    let product = TlsServer::start(Reply::Sequence(vec![
        identity()?,
        reply(
            200,
            json!({"pagelen": 2, "values": [pull_request(1, "OPEN"), pull_request(2, "OPEN")], "next": next}),
        )?,
    ]))?;
    let fixture = setup.fixture(&product)?;
    let locked = provision(&setup, &fixture)?;
    setup.policy(BITBUCKET_READ, &["bitbucket.pullrequest.list"], true)?;
    let args: craft_cli::bitbucket_reads::PullRequestListArgs =
        serde_json::from_value(json!({"repo": "service", "page_size": 2, "limit": 2}))?;
    let args = BitbucketReadArgs::PullRequestList(args);
    let options = InvocationOptions {
        state_root: Some(&setup.root),
        profile: Some("code"),
        request_id: "read-prs",
        max_bytes: 65_536,
    };
    let control = control()?;
    let prepared = PreparedInvocation::prepare(&args, &[], &options, &setup.store, &control)?;
    let result = prepared.execute(
        &setup.store,
        fixture.client(),
        &locked,
        TlsPolicy::AdditionalCaBundle(product.root()),
        control,
    );
    assert!(result.envelope.ok, "{:?}", result.envelope.error);
    let page = result.envelope.page.ok_or("page")?;
    assert!(page.has_more && page.next_cursor.is_some());
    let data = result.envelope.data.ok_or("data")?;
    let ids: Vec<_> = data["items"]
        .as_array()
        .ok_or("items")?
        .iter()
        .map(|item| item["id"].clone())
        .collect();
    assert_eq!(ids, [json!(1), json!(2)]);
    let requests = product.finish()?;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].method, "GET");
    assert!(
        requests[1]
            .target
            .starts_with("/2.0/repositories/team/service/pullrequests?")
    );
    assert!(requests[1].target.contains("state=OPEN"));
    assert!(!requests[1].target.contains("sentinel"));
    setup.no_secrets()
}

#[test]
fn a_read_without_an_enabled_policy_is_refused_before_credentials_are_touched() -> TestResult {
    let setup = Setup::new()?;
    let args: craft_cli::bitbucket_reads::PageArgs = serde_json::from_value(json!({}))?;
    let args = BitbucketReadArgs::RepositoryList(args);
    let options = InvocationOptions {
        state_root: Some(&setup.root),
        profile: Some("code"),
        request_id: "denied-read",
        max_bytes: 65_536,
    };
    let error = PreparedInvocation::prepare(&args, &[], &options, &setup.store, &control()?)
        .err()
        .ok_or("expected refusal")?;
    assert_eq!(error.code, ErrorCode::PolicyDenied);
    Ok(())
}

#[test]
fn flags_and_json_decode_to_the_same_write_input() -> TestResult {
    let write = BitbucketWriteArgs::decode(
        "bitbucket.comment.add",
        json!({"repo": "service", "pull_request_id": 7, "text": "ok", "operation_id": "c-1", "apply": true}),
    )?;
    assert_eq!(write.command(), "bitbucket.comment.add");
    assert_eq!(write.operation_id(), "c-1");
    assert!(write.apply());
    assert!(
        BitbucketWriteArgs::decode(
            "bitbucket.comment.add",
            json!({"repo": "service", "text": "ok", "operation_id": "c-1", "bogus": 1}),
        )
        .is_err()
    );
    assert!(BitbucketWriteArgs::decode("bitbucket.pullrequest.delete", json!({})).is_err());
    Ok(())
}
