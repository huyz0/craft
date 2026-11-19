//! Actual Trello host, static grant, policy, journal and paging against owned TLS servers.
// Shared fixture: not every helper is used by every suite.
#[allow(dead_code)]
#[path = "../../craft-runtime/src/transport/tls_fixture.rs"]
mod tls_fixture;
use craft_cli::profiles::InvocationOptions;
use craft_cli::trello_reads::{PreparedInvocation, TrelloReadArgs};
use craft_cli::trello_writes::{PreparedTrelloWrite, TrelloWriteEnvironment};
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
use craft_runtime::provider_policy::{ProviderPolicyUpdate, TRELLO_READ, TRELLO_WRITE};
use craft_runtime::synthetic_acceptance::{STATIC_SECRET, ServiceFixture};
use craft_runtime::transport::TlsPolicy;
use craft_test_support::state::ManualClock;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tls_fixture::{Reply, TlsServer};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const MEMBER: &str = "5f1d2c3b4a5968778695a4b3";
const LIST: &str = "5f1d2c3b4a5968778695a4b5";
const CARD: &str = "5f1d2c3b4a5968778695a4b6";

fn control() -> Result<InvocationControl, CraftError> {
    InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        30_000,
    )
}

fn account() -> ResourceRef {
    ResourceRef {
        provider: "trello".into(),
        deployment: "cloud".into(),
        tenant: MEMBER.into(),
        resource_type: "account".into(),
        id: MEMBER.into(),
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
            "trello-host",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos(),
        );
        let store = ProfileStore::open(&root, &control()?)?;
        let profile = store.create(
            &ProfileInput {
                name: "cards".into(),
                provider: "trello".into(),
                deployment: "cloud".into(),
                auth_mode: AuthMode::Service,
                grant: None,
                tenant: Some(MEMBER.into()),
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
            "cards",
            ProviderPolicyUpdate {
                class: class.into(),
                target: account(),
                expected_actor: MEMBER.into(),
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

fn create_request(max_bytes: u32) -> CanonicalRequest {
    CanonicalRequest {
        command: "trello.card.create".into(),
        target: account(),
        input: json!({"list_id": LIST, "name": "Fix login"}),
        selection: Selection {
            max_bytes,
            ..Selection::default()
        },
        preconditions: None,
    }
}

fn identity() -> Result<Reply, Box<dyn std::error::Error>> {
    reply(200, json!({"id": MEMBER, "username": "agent"}))
}

#[test]
fn a_confirmed_create_is_claimed_sent_once_recorded_and_free_of_secrets() -> TestResult {
    let setup = Setup::new()?;
    let product = TlsServer::start(Reply::Sequence(vec![
        identity()?,
        reply(
            200,
            json!({"id": CARD, "name": "Fix login", "idList": LIST, "closed": false}),
        )?,
    ]))?;
    let fixture = setup.fixture(&product)?;
    let locked = fixture.lock(&setup.grant, &control()?)?;
    let receipt = fixture.client().provision_static_grant(
        &locked,
        &account().tenant_ref(),
        fixture.static_credentials()?,
    )?;
    assert_eq!(receipt.actor, MEMBER);
    setup.policy(TRELLO_WRITE, &["trello.card.create"], true)?;
    let prepared = PreparedTrelloWrite::prepare(
        create_request(65_536),
        "create-card",
        true,
        "cards",
        &setup.store,
        &control()?,
    )?;
    let journal =
        NativeJournal::open(&setup.store, &control()?).map_err(|_| "journal unavailable")?;
    let (data, error) = prepared.execute(
        TrelloWriteEnvironment {
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
    assert_eq!(data.receipt.confirmed_resources[0].id, CARD);
    assert!(data.desired_fields_verified && !data.replay_authorized);
    let record = journal
        .get("create-card")
        .map_err(|_| "journal read")?
        .ok_or("journal record")?;
    assert_eq!(record.steps.len(), 1);
    let requests = product.finish()?;
    // One identity proof, then exactly one write.
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].method, "POST");
    assert_eq!(requests[1].target, "/1/cards");
    assert_eq!(
        requests[1].headers.get("authorization").map(String::as_str),
        Some(
            "OAuth oauth_consumer_key=\"craftsyntheticfixturekey\", oauth_token=\"craftsyntheticfixturetoken\""
        )
    );
    let body: Value = serde_json::from_slice(&requests[1].body)?;
    assert_eq!(body, json!({"idList": LIST, "name": "Fix login"}));
    setup.no_secrets()
}

#[test]
fn a_server_error_leaves_an_unknown_receipt_and_the_operation_cannot_be_replayed() -> TestResult {
    let setup = Setup::new()?;
    let product = TlsServer::start(Reply::Sequence(vec![
        identity()?,
        reply(500, json!({"message": "provider-body-sentinel"}))?,
    ]))?;
    let fixture = setup.fixture(&product)?;
    let locked = fixture.lock(&setup.grant, &control()?)?;
    fixture.client().provision_static_grant(
        &locked,
        &account().tenant_ref(),
        fixture.static_credentials()?,
    )?;
    setup.policy(TRELLO_WRITE, &["trello.card.create"], true)?;
    let journal =
        NativeJournal::open(&setup.store, &control()?).map_err(|_| "journal unavailable")?;
    let run = |id: &str| -> Result<_, Box<dyn std::error::Error>> {
        let prepared = PreparedTrelloWrite::prepare(
            create_request(65_536),
            id,
            true,
            "cards",
            &setup.store,
            &control()?,
        )?;
        Ok(prepared.execute(
            TrelloWriteEnvironment {
                store: &setup.store,
                client: fixture.client(),
                locked: &locked,
                journal: &journal,
                tls: TlsPolicy::AdditionalCaBundle(product.root()),
                control: control()?,
            },
            || Ok(()),
        ))
    };
    let (data, error) = run("unclear")?;
    let error = error.ok_or("expected an uncertain outcome")?;
    assert_eq!(error.code, ErrorCode::WriteOutcomeUnknown);
    assert_eq!(data.receipt.request_state, RequestState::SentUnknown);
    assert!(data.reconciliation_required && !data.replay_authorized);
    assert!(!serde_json::to_string(&data)?.contains("provider-body-sentinel"));
    // The same operation id is never sent again.
    let (_, second) = run("unclear")?;
    assert_eq!(
        second.ok_or("expected refusal")?.code,
        ErrorCode::OperationIdReused
    );
    assert_eq!(product.finish()?.len(), 2);
    Ok(())
}

#[test]
fn missing_apply_policy_operation_or_budget_refuse_before_any_request() -> TestResult {
    let setup = Setup::new()?;
    let control = control()?;
    let attempt = |request: CanonicalRequest, apply: bool| {
        PreparedTrelloWrite::prepare(request, "op", apply, "cards", &setup.store, &control)
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
    // No policy at all.
    assert_eq!(
        attempt(create_request(65_536), true),
        Some(ErrorCode::PolicyDenied)
    );
    // A policy for another command does not allow this one.
    setup.policy(TRELLO_WRITE, &["trello.comment.add"], true)?;
    assert_eq!(
        attempt(create_request(65_536), true),
        Some(ErrorCode::PolicyDenied)
    );
    Ok(())
}

#[test]
fn a_policy_disabled_during_the_call_stops_the_send_without_a_request() -> TestResult {
    let setup = Setup::new()?;
    let product = TlsServer::start(Reply::Sequence(vec![identity()?]))?;
    let fixture = setup.fixture(&product)?;
    let locked = fixture.lock(&setup.grant, &control()?)?;
    fixture.client().provision_static_grant(
        &locked,
        &account().tenant_ref(),
        fixture.static_credentials()?,
    )?;
    setup.policy(TRELLO_WRITE, &["trello.card.create"], true)?;
    let prepared = PreparedTrelloWrite::prepare(
        create_request(65_536),
        "disabled-mid-call",
        true,
        "cards",
        &setup.store,
        &control()?,
    )?;
    let journal =
        NativeJournal::open(&setup.store, &control()?).map_err(|_| "journal unavailable")?;
    // Disable the policy (revision 2) before the host rechecks it.
    setup.store.set_provider_policy(
        "cards",
        ProviderPolicyUpdate {
            class: TRELLO_WRITE.into(),
            target: account(),
            expected_actor: MEMBER.into(),
            operations: vec!["trello.card.create".into()],
            enabled: false,
            expected_revision: Some(1),
        },
        true,
        &control()?,
        |_| Ok(()),
    )?;
    let (data, error) = prepared.execute(
        TrelloWriteEnvironment {
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
fn a_read_pages_through_the_static_grant_and_policy() -> TestResult {
    let setup = Setup::new()?;
    let lists: Vec<Value> = (0..3)
        .map(|n| json!({"id": format!("5f1d2c3b4a5968778695a4c{n}"), "name": format!("L{n}"), "closed": false, "pos": n, "idBoard": LIST}))
        .collect();
    let product = TlsServer::start(Reply::Sequence(vec![
        identity()?,
        reply(200, json!(lists))?,
    ]))?;
    let fixture = setup.fixture(&product)?;
    let locked = fixture.lock(&setup.grant, &control()?)?;
    fixture.client().provision_static_grant(
        &locked,
        &account().tenant_ref(),
        fixture.static_credentials()?,
    )?;
    setup.policy(TRELLO_READ, &["trello.list.list"], true)?;
    let args: craft_cli::trello_reads::BoardArgs =
        serde_json::from_value(json!({"board_id": LIST, "limit": 2}))?;
    let args = TrelloReadArgs::ListList(args);
    let options = InvocationOptions {
        state_root: Some(&setup.root),
        profile: Some("cards"),
        request_id: "read-lists",
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
    let data = result.envelope.data.ok_or("data")?;
    assert_eq!(data["items"].as_array().map(Vec::len), Some(2));
    assert_eq!(data["items"][0]["name"], "L0");
    let page = result.envelope.page.ok_or("page")?;
    assert!(page.has_more && page.next_cursor.is_some());
    let requests = product.finish()?;
    assert_eq!(requests[1].method, "GET");
    assert!(
        requests[1]
            .target
            .starts_with(&format!("/1/boards/{LIST}/lists?"))
    );
    assert!(!requests[1].target.contains("sentinel"));
    setup.no_secrets()
}

#[test]
fn a_read_without_an_enabled_policy_is_refused_before_credentials_are_touched() -> TestResult {
    let setup = Setup::new()?;
    let args: craft_cli::trello_reads::BoardListArgs = serde_json::from_value(json!({}))?;
    let args = TrelloReadArgs::BoardList(args);
    let options = InvocationOptions {
        state_root: Some(&setup.root),
        profile: Some("cards"),
        request_id: "denied-read",
        max_bytes: 65_536,
    };
    let error = PreparedInvocation::prepare(&args, &[], &options, &setup.store, &control()?)
        .err()
        .ok_or("expected refusal")?;
    assert_eq!(error.code, ErrorCode::PolicyDenied);
    Ok(())
}
