use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use craft_core::contracts::{
    AuthMode, CanonicalRequest, JobState, RemoteJob, ResourceRef, SCHEMA_VERSION, Selection,
};
use craft_core::control::CancellationToken;
use craft_core::execution::{OperationHandler, RouteSegment};
use craft_core::input::canonical_digest;
use craft_core::profiles::ProfileInput;
use craft_core::registry::{OperationDescriptor, ReconciliationClass, RetryClass, response_schema};
use craft_core::storage::OperationJournal;
use craft_test_support::state::FaultPoint;
use serde_json::{Value, json};

use super::*;
use crate::control::SystemClock;
use crate::profiles::ProfileStore;
use crate::tls_fixture::{Reply, TlsServer};

const CLOUD: &str = "00000000-1111-2222-3333-444444444444";
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[test]
fn subtask_and_assignee_proofs_guard_the_final_tls_create_send()
-> Result<(), Box<dyn std::error::Error>> {
    use craft_core::registry::schema_for;
    use craft_provider_jira::{
        create_metadata::{self, CreateInput, JiraMetadata, MetadataPrincipal},
        created_issue,
    };
    for mode in ["fresh", "expired_account", "wrong_parent_project"] {
        let fixture = Fixture::new()?;
        let (profiles, grant, journal) = fixture.setup()?;
        let field = |id: &str, kind: &str, required: bool| json!({"fieldId":id,"key":id,"name":id,"operations":["set"],"required":required,"schema":{"type":kind,"system":id}});
        let parent_project = if mode == "wrong_parent_project" {
            "10099"
        } else {
            "10000"
        };
        let server=TlsServer::start(Reply::Sequence(vec![
            Reply::json(200,br#"{"id":"10000","key":"ED"}"#,&[]),
            Reply::json(200,br#"{"startAt":0,"maxResults":50,"total":1,"issueTypes":[{"id":"10001","name":"Subtask","subtask":true}]}"#,&[]),
            Reply::json(200,&serde_json::to_vec(&json!({"startAt":0,"maxResults":50,"total":5,"fields":[field("project","project",true),field("issuetype","issuetype",true),field("summary","string",true),field("parent","issuelink",true),field("assignee","user",false)]}))?,&[]),
            Reply::json(200,&serde_json::to_vec(&json!({"id":"20000","fields":{"project":{"id":parent_project},"issuetype":{"id":"10002","subtask":false}}}))?,&[]),
            Reply::json(200,br#"[{"accountId":"account-two","active":true,"emailAddress":"private-email-sentinel"}]"#,&[]),
            Reply::json(201,br#"{"id":"10003","key":"ED-25"}"#,&[]),
        ]))?;
        let clock = Arc::new(craft_test_support::state::ManualClock::new(100));
        let account_clock = Arc::new(craft_test_support::state::ManualClock::new(100));
        let mut registry = Registry::default();
        registry.register_provider(&JiraMetadata::new(clock.clone()))?;
        let (base, _) = prepared()?;
        let mut descriptor = base.descriptor("jira.fixture.mutate")?.clone();
        descriptor.id = created_issue::COMMAND.into();
        descriptor.input_schema = schema_for::<CreateInput>()?;
        descriptor.example = json!({"issue_type_id":"10001","summary":"Example"});
        descriptor.routes[0].id = created_issue::ROUTE.into();
        descriptor.routes[0].path.pop();
        registry.register_typed_handler::<CreateInput>(descriptor, Arc::new(Handler))?;
        let project = ResourceRef {
            resource_type: "project".into(),
            id: "10000".into(),
            ..target()
        };
        let request = |command: &str, input: Value| CanonicalRequest {
            command: command.into(),
            target: project.clone(),
            input,
            selection: Selection::default(),
            preconditions: None,
        };
        let read = registry.prepare(request(
            create_metadata::COMMAND,
            json!({"issue_type_id":"10001","parent_issue_id":"20000"}),
        ))?;
        let mut context = HttpContext::for_operation(
            &registry,
            &read,
            &grant,
            session(&grant)?,
            TlsPolicy::AdditionalCaBundle(server.root()),
            control(10_000)?,
        )?;
        context
            .endpoint
            .prefix
            .set_host(Some("localhost"))
            .map_err(|_| "Invalid fixture host")?;
        context
            .endpoint
            .prefix
            .set_port(Some(server.address().port()))
            .map_err(|_| "Invalid fixture port")?;
        let principal = MetadataPrincipal {
            grant: grant.clone(),
            actor: "account-one".into(),
        };
        let metadata = create_metadata::read_context(&read, &mut context, principal.clone(), clock);
        if mode == "wrong_parent_project" {
            assert_eq!(
                metadata.err().ok_or("Expected parent refusal")?.code,
                ErrorCode::PolicyDenied
            );
            assert!(journal.get("publish-summary")?.is_none());
            assert_eq!(server.finish()?.len(), 4);
            continue;
        }
        let metadata = metadata?;
        let lookup = registry.prepare(request(
            create_metadata::ASSIGNEE_COMMAND,
            json!({"account_id":"account-two"}),
        ))?;
        let mut accounts = HttpContext::for_operation(
            &registry,
            &lookup,
            &grant,
            session(&grant)?,
            TlsPolicy::AdditionalCaBundle(server.root()),
            control(10_000)?,
        )?;
        accounts
            .endpoint
            .prefix
            .set_host(Some("localhost"))
            .map_err(|_| "Invalid fixture host")?;
        accounts
            .endpoint
            .prefix
            .set_port(Some(server.address().port()))
            .map_err(|_| "Invalid fixture port")?;
        let assignee = create_metadata::read_assignable(
            &lookup,
            &mut accounts,
            principal.clone(),
            account_clock.clone(),
        )?;
        assert_eq!(server.request_count(), 5);
        let create=registry.prepare(request(created_issue::COMMAND,json!({"issue_type_id":"10001","summary":"Subtask","parent_issue_id":"20000","assignee_account_id":"account-two"})))?;
        let frozen = metadata.prepare_create_with_assignee(&create, &principal, Some(&assignee))?;
        let attempt = local_request(
            &server,
            &registry,
            &create,
            &grant,
            control(10_000)?,
            frozen.call(),
        )?
        .send_checked(&journal, &authority(&grant), created_issue::ROUTE, || {
            if mode == "expired_account" {
                account_clock
                    .advance(Duration::from_secs(60))
                    .map_err(|_| internal())?;
            }
            frozen.verify(&create, &principal)
        })?;
        if mode == "expired_account" {
            assert_eq!(
                attempt
                    .response
                    .as_ref()
                    .err()
                    .ok_or("Expected account expiry")?
                    .code,
                ErrorCode::VersionConflict
            );
            assert_eq!(
                attempt
                    .pending_evidence
                    .as_ref()
                    .map_err(|_| "Expected unknown marker")?
                    .steps[created_issue::ROUTE]
                    .state,
                SubmissionState::Unknown
            );
            assert_eq!(server.finish()?.len(), 5);
            continue;
        }
        let observed = created_issue::interpret(
            &project,
            attempt
                .response
                .as_ref()
                .map_err(|_| "Expected subtask response")?,
        );
        assert!(observed.error.is_none());
        let receipt = attempt.record_outcome(&StepOutcome {
            state: observed.state,
            confirmed_resources: observed.resources,
            job: None,
        })?;
        let fresh = NativeJournal::open(&profiles, &control(5_000)?)?;
        assert_eq!(fresh.get("publish-summary")?, Some(receipt));
        let requests = server.finish()?;
        assert_eq!(requests.len(), 6);
        assert!(requests[..5].iter().all(|request| request.method == "GET"));
        assert_eq!(requests[5].method, "POST");
        let parent_url = url::Url::parse(&format!("https://localhost{}", requests[3].target))?;
        assert_eq!(
            parent_url.path(),
            format!("/ex/jira/{CLOUD}/rest/api/3/issue/20000")
        );
        let query = parent_url.query_pairs().collect::<BTreeMap<_, _>>();
        assert_eq!(
            query.get("fields").map(|value| value.as_ref()),
            Some("project,issuetype")
        );
        assert_eq!(
            query.get("updateHistory").map(|value| value.as_ref()),
            Some("false")
        );
        let account_url = url::Url::parse(&format!("https://localhost{}", requests[4].target))?;
        let query = account_url.query_pairs().collect::<BTreeMap<_, _>>();
        assert_eq!(
            query.get("accountId").map(|value| value.as_ref()),
            Some("account-two")
        );
        assert_eq!(
            query.get("project").map(|value| value.as_ref()),
            Some("10000")
        );
        assert!(!query.contains_key("query"));
        let body: Value = serde_json::from_slice(&requests[5].body)?;
        assert_eq!(
            body,
            json!({"fields":{"project":{"id":"10000"},"issuetype":{"id":"10001"},"summary":"Subtask","parent":{"id":"20000"},"assignee":{"accountId":"account-two"}}})
        );
        assert!(
            !fs::read(fixture.root.join("state.sqlite3"))?
                .windows(b"private-email-sentinel".len())
                .any(|bytes| bytes == b"private-email-sentinel")
        );
    }
    Ok(())
}

#[test]
fn metadata_tls_reads_qualify_a_frozen_create_before_the_single_claimed_post()
-> Result<(), Box<dyn std::error::Error>> {
    for stale in [false, true] {
        use craft_core::registry::schema_for;
        use craft_provider_jira::create_metadata::{
            self, CreateInput, JiraMetadata, MetadataPrincipal,
        };
        let fixture = Fixture::new()?;
        let (profiles, grant, journal) = fixture.setup()?;
        let field = |id: &str, kind: &str| json!({"fieldId":id,"key":id,"name":id,"operations":["set"],"required":true,"schema":{"type":kind,"system":id}});
        let server=TlsServer::start(Reply::Sequence(vec![
        Reply::json(200,br#"{"id":"10000","key":"ED"}"#,&[]),
        Reply::json(200,br#"{"startAt":0,"maxResults":50,"total":1,"issueTypes":[{"id":"10001","name":"Task","subtask":false}]}"#,&[]),
        Reply::json(200,&serde_json::to_vec(&json!({"startAt":0,"maxResults":50,"total":3,"fields":[field("project","project"),field("issuetype","issuetype"),field("summary","string")]}))?,&[]),
        Reply::json(201,br#"{"id":"10002","key":"ED-24"}"#,&[]),
    ]))?;
        let clock = Arc::new(craft_test_support::state::ManualClock::new(100));
        let mut registry = Registry::default();
        registry.register_provider(&JiraMetadata::new(clock.clone()))?;
        let (base, _) = prepared()?;
        let mut descriptor = base.descriptor("jira.fixture.mutate")?.clone();
        descriptor.id = craft_provider_jira::created_issue::COMMAND.into();
        descriptor.input_schema = schema_for::<CreateInput>()?;
        descriptor.example = json!({"issue_type_id":"10001","summary":"Example"});
        descriptor.routes[0].id = craft_provider_jira::created_issue::ROUTE.into();
        descriptor.routes[0].path.pop();
        registry.register_typed_handler::<CreateInput>(descriptor, Arc::new(Handler))?;
        let project = ResourceRef {
            resource_type: "project".into(),
            id: "10000".into(),
            ..target()
        };
        let request = |command: &str, input: Value| CanonicalRequest {
            command: command.into(),
            target: project.clone(),
            input,
            selection: Selection::default(),
            preconditions: None,
        };
        let read = registry.prepare(request(
            create_metadata::COMMAND,
            json!({"issue_type_id":"10001"}),
        ))?;
        let mut context = HttpContext::for_operation(
            &registry,
            &read,
            &grant,
            session(&grant)?,
            TlsPolicy::AdditionalCaBundle(server.root()),
            control(5_000)?,
        )?;
        context
            .endpoint
            .prefix
            .set_host(Some("localhost"))
            .map_err(|_| "Invalid fixture host")?;
        context
            .endpoint
            .prefix
            .set_port(Some(server.address().port()))
            .map_err(|_| "Invalid fixture port")?;
        let principal = MetadataPrincipal {
            grant: grant.clone(),
            actor: "account-one".into(),
        };
        let proof =
            create_metadata::read_context(&read, &mut context, principal.clone(), clock.clone())?;
        assert_eq!(server.request_count(), 3);
        assert!(journal.get("publish-summary")?.is_none());
        let invalid = registry.prepare(request(
            craft_provider_jira::created_issue::COMMAND,
            json!({"issue_type_id":"10001","summary":"Example","custom_fields":{"customfield_999":"invented"}}),
        ))?;
        assert!(proof.prepare_create(&invalid, &principal).is_err());
        assert_eq!(server.request_count(), 3);
        let create = registry.prepare(request(
            craft_provider_jira::created_issue::COMMAND,
            json!({"issue_type_id":"10001","summary":"Unicode 世界"}),
        ))?;
        let frozen = proof.prepare_create(&create, &principal)?;
        frozen.verify(&create, &principal)?;
        let attempt = local_request(
            &server,
            &registry,
            &create,
            &grant,
            control(5_000)?,
            frozen.call(),
        )?
        .send_checked(
            &journal,
            &authority(&grant),
            craft_provider_jira::created_issue::ROUTE,
            || {
                if stale {
                    clock
                        .advance(Duration::from_secs(60))
                        .map_err(|_| internal())?;
                }
                frozen.verify(&create, &principal)
            },
        )?;
        if stale {
            assert_eq!(
                attempt
                    .response
                    .as_ref()
                    .err()
                    .ok_or("Expected stale-context refusal")?
                    .code,
                ErrorCode::VersionConflict
            );
            assert_eq!(
                attempt
                    .pending_evidence
                    .as_ref()
                    .map_err(|_| "Expected durable uncertainty")?
                    .steps[craft_provider_jira::created_issue::ROUTE]
                    .state,
                SubmissionState::Unknown
            );
            assert_eq!(server.request_count(), 3);
            assert_eq!(server.finish()?.len(), 3);
            continue;
        }
        let observed = craft_provider_jira::created_issue::interpret(
            &project,
            attempt
                .response
                .as_ref()
                .map_err(|_| "Expected create response")?,
        );
        assert!(observed.error.is_none());
        let receipt = attempt.record_outcome(&StepOutcome {
            state: observed.state,
            confirmed_resources: observed.resources,
            job: None,
        })?;
        let fresh = NativeJournal::open(&profiles, &control(5_000)?)?;
        assert_eq!(fresh.get("publish-summary")?, Some(receipt));
        let requests = server.finish()?;
        assert_eq!(requests.len(), 4);
        assert!(requests[..3].iter().all(|request| request.method == "GET"));
        assert_eq!(requests[3].method, "POST");
        let body: Value = serde_json::from_slice(&requests[3].body)?;
        assert_eq!(
            body,
            json!({"fields":{"project":{"id":"10000"},"issuetype":{"id":"10001"},"summary":"Unicode 世界"}})
        );
        assert_eq!(
            requests[0].target,
            format!("/ex/jira/{CLOUD}/rest/api/3/project/10000")
        );
        assert!(requests[2].target.starts_with(&format!(
            "/ex/jira/{CLOUD}/rest/api/3/issue/createmeta/10000/issuetypes/10001?"
        )));
    }
    Ok(())
}

#[test]
fn native_create_response_flows_through_jira_interpretation_and_durable_partial_receipt()
-> Result<(), Box<dyn std::error::Error>> {
    use craft_provider_jira::created_issue;
    let fixture = Fixture::new()?;
    let (profiles, grant, journal) = fixture.setup()?;
    let server = TlsServer::start(Reply::json(201, br#"{"id":"10001","key":"ED-24","watchers":{"status":403,"errorCollection":{"errorMessages":["raw-provider-sentinel"]}}}"#, &[]))?;
    let (base, _) = prepared()?;
    let mut descriptor = base.descriptor("jira.fixture.mutate")?.clone();
    descriptor.id = created_issue::COMMAND.into();
    descriptor.routes[0].id = created_issue::ROUTE.into();
    descriptor.routes[0].path.pop();
    let mut registry = Registry::default();
    registry.register_typed_handler::<Value>(descriptor, Arc::new(Handler))?;
    let project = ResourceRef {
        resource_type: "project".into(),
        id: "10000".into(),
        ..target()
    };
    let operation = registry.prepare(CanonicalRequest {
        command: created_issue::COMMAND.into(),
        target: project.clone(),
        input: json!({}),
        selection: Selection::default(),
        preconditions: None,
    })?;
    let native = NativeCall {
        repeated_query: std::collections::BTreeMap::new(),
        route: created_issue::ROUTE.into(),
        path_parameters: BTreeMap::new(),
        query: BTreeMap::new(),
        body: Some(json!({"fields":{"summary":"synthetic native create"}})),
    };
    let attempt = local_request(
        &server,
        &registry,
        &operation,
        &grant,
        control(5_000)?,
        &native,
    )?
    .send(&journal, &authority(&grant), created_issue::ROUTE)?;
    assert_eq!(attempt.command(), created_issue::COMMAND);
    assert_eq!(attempt.route(), created_issue::ROUTE);
    assert_eq!(attempt.step(), created_issue::ROUTE);
    assert_eq!(attempt.binding().target, project);
    let observed = created_issue::interpret(
        &attempt.binding().target,
        attempt
            .response
            .as_ref()
            .map_err(|_| "Expected create response")?,
    );
    assert_eq!(
        observed.error.as_ref().map(|error| error.code),
        Some(ErrorCode::PartialApply)
    );
    assert_eq!(observed.resources[0].id, "10001");
    assert_eq!(observed.state, SubmissionState::Confirmed);
    let receipt = attempt.record_outcome(&StepOutcome {
        state: observed.state,
        confirmed_resources: observed.resources.clone(),
        job: None,
    })?;
    let fresh = NativeJournal::open(&profiles, &control(5_000)?)?;
    assert_eq!(fresh.get("publish-summary")?, Some(receipt));
    let requests = server.finish()?;
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].target,
        format!("/ex/jira/{CLOUD}/rest/api/3/issue")
    );
    assert_eq!(requests[0].method, "POST");
    assert!(
        !fs::read(fixture.root.join("state.sqlite3"))?
            .windows(b"raw-provider-sentinel".len())
            .any(|bytes| bytes == b"raw-provider-sentinel")
    );
    Ok(())
}

#[test]
fn host_preparation_uses_fixed_origin_and_rejects_read_or_mismatched_session()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let (_profiles, grant, journal) = fixture.setup()?;
    let (registry, operation) = prepared()?;
    let request = PreparedHttpMutation::prepare(
        &registry,
        &operation,
        session(&grant)?,
        TlsPolicy::Platform,
        control(5_000)?,
        &call(),
    )?;
    assert_eq!(
        request.request.url().as_str(),
        format!("https://api.atlassian.com/ex/jira/{CLOUD}/rest/api/3/issue/PAY-1")
    );
    assert_eq!(request.request.method(), Method::POST);
    assert!(request.request.headers()[AUTHORIZATION].is_sensitive());
    drop(request);
    let wrong = HostSession::new(
        TenantRef {
            tenant: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into(),
            ..target().tenant_ref()
        },
        grant.clone(),
        SecretBytes::new(b"synthetic-bearer-sentinel".to_vec())?,
    )?;
    assert!(
        PreparedHttpMutation::prepare(
            &registry,
            &operation,
            wrong,
            TlsPolicy::Platform,
            control(5_000)?,
            &call()
        )
        .is_err()
    );
    for read_effect in [false, true] {
        let mut descriptor = registry.descriptor("jira.fixture.mutate")?.clone();
        if read_effect {
            descriptor.effect = EffectClass::Read;
            descriptor.retry_class = RetryClass::SafeRead;
            descriptor.reconciliation = ReconciliationClass::NotApplicable;
        } else {
            descriptor.routes[0].method = HttpMethod::Get;
        }
        let mut other = Registry::default();
        other.register_typed_handler::<Value>(descriptor, Arc::new(Handler))?;
        let altered = other.prepare(serde_json::from_value(serde_json::to_value(
            operation.request(),
        )?)?)?;
        assert!(
            PreparedHttpMutation::prepare(
                &other,
                &altered,
                session(&grant)?,
                TlsPolicy::Platform,
                control(5_000)?,
                &call()
            )
            .is_err()
        );
    }
    assert!(journal.get("publish-summary")?.is_none());
    Ok(())
}

#[test]
fn competing_prepared_calls_yield_one_committed_claim_and_one_complete_http_request()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let (_profiles, grant, journal) = fixture.setup()?;
    let server = TlsServer::start(Reply::json(201, br#"{"id":"PAY-1"}"#, &[]))?;
    let (registry, operation) = prepared()?;
    let original = call();
    let mut changed = call();
    changed.body = Some(json!({"title":"different-native-payload"}));
    let requests = [
        local_request(
            &server,
            &registry,
            &operation,
            &grant,
            control(5_000)?,
            &original,
        )?,
        local_request(
            &server,
            &registry,
            &operation,
            &grant,
            control(5_000)?,
            &changed,
        )?,
    ];
    let results = std::thread::scope(|scope| -> Result<Vec<_>, Box<dyn std::error::Error>> {
        let (ready_send, ready_receive) = std::sync::mpsc::sync_channel(2);
        let mut workers = Vec::new();
        let mut releases = Vec::new();
        for request in requests {
            let (release, wait) = std::sync::mpsc::sync_channel(1);
            releases.push(release);
            let ready = ready_send.clone();
            let journal = &journal;
            let grant = &grant;
            workers.push(scope.spawn(move || {
                ready.send(()).map_err(|_| internal())?;
                wait.recv_timeout(Duration::from_secs(3))
                    .map_err(|_| internal())?;
                request.send(journal, &authority(grant), "update")
            }));
        }
        for _ in 0..2 {
            ready_receive.recv_timeout(Duration::from_secs(3))?;
        }
        for release in releases {
            release.send(())?;
        }
        workers
            .into_iter()
            .map(|worker| worker.join().map_err(|_| "Contender panicked".into()))
            .collect()
    })?;
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let winner = results
        .into_iter()
        .find_map(Result::ok)
        .ok_or("Missing winner")?;
    assert_eq!(
        winner
            .response
            .as_ref()
            .map_err(|_| "Expected winner response")?
            .status,
        201
    );
    let stored = journal
        .get("publish-summary")?
        .ok_or("Missing durable record")?;
    assert_eq!(stored.steps.len(), 1);
    assert_eq!(stored.steps["update"].state, SubmissionState::Unknown);
    let observed = server.finish()?;
    assert_eq!(observed.len(), 1);
    let body: Value = serde_json::from_slice(&observed[0].body)?;
    assert_eq!(
        stored.steps["update"].request_digest,
        Some(canonical_digest(
            &json!({"route":"update","method":"post","parameters":call().path_parameters,"query":{},"body":body})
        )?)
    );
    Ok(())
}

#[test]
fn cancellation_after_claim_and_nested_runtime_refusal_never_poll_the_exchange()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let (profiles, grant, journal) = fixture.setup()?;
    let server = TlsServer::start(Reply::json(200, b"{}", &[]))?;
    let (registry, operation) = prepared()?;
    let request = local_request(
        &server,
        &registry,
        &operation,
        &grant,
        control(5_000)?,
        &call(),
    )?;
    let nested = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let result = nested.block_on(async { request.send(&journal, &authority(&grant), "update") });
    assert_eq!(
        result.err().ok_or("Expected runtime refusal")?.code,
        ErrorCode::Internal
    );
    assert!(journal.get("publish-summary")?.is_none());
    let token = CancellationToken::default();
    let invocation =
        InvocationControl::new(Arc::new(SystemClock::default()), token.clone(), 5_000)?;
    let mut request = local_request(&server, &registry, &operation, &grant, invocation, &call())?;
    request.cancel_before_exchange = Some(token);
    let attempt = request.send(&journal, &authority(&grant), "update")?;
    let error = attempt
        .response
        .as_ref()
        .err()
        .ok_or("Expected cancellation")?;
    assert_eq!(error.code, ErrorCode::Cancelled);
    assert_eq!(error.details.write_state, RequestState::SentUnknown);
    let pending = attempt
        .pending_evidence
        .as_ref()
        .map_err(|_| "Expected durable uncertainty")?;
    assert_eq!(pending.steps["update"].state, SubmissionState::Unknown);
    let fresh = NativeJournal::open(&profiles, &control(5_000)?)?;
    assert_eq!(fresh.get("publish-summary")?.as_ref(), Some(pending));
    assert_eq!(server.connections(), 0);
    assert!(server.finish()?.is_empty());
    Ok(())
}

#[test]
fn deadline_and_ready_confirmed_cancellation_after_send_keep_durable_unknown()
-> Result<(), Box<dyn std::error::Error>> {
    for cancel in [false, true] {
        let fixture = Fixture::new()?;
        let (_profiles, grant, journal) = fixture.setup()?;
        let server = TlsServer::start(Reply::Stall)?;
        let (registry, operation) = prepared()?;
        let token = CancellationToken::default();
        let invocation = InvocationControl::new(
            Arc::new(SystemClock::default()),
            token.clone(),
            if cancel { 5_000 } else { 1_000 },
        )?;
        let request = local_request(&server, &registry, &operation, &grant, invocation, &call())?;
        let result = std::thread::scope(|scope| {
            let counter = server.request_counter();
            let canceller = cancel.then(|| {
                scope.spawn(move || {
                    let start = Instant::now();
                    while counter.load(Ordering::SeqCst) == 0 {
                        if start.elapsed() > Duration::from_secs(3) {
                            return false;
                        }
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    token.cancel();
                    true
                })
            });
            let result = request.send(&journal, &authority(&grant), "update");
            if let Some(canceller) = canceller {
                assert!(canceller.join().is_ok_and(|ready| ready));
            }
            result
        })?;
        let error = result
            .response
            .as_ref()
            .err()
            .ok_or("Expected interrupted exchange")?;
        assert_eq!(
            error.code,
            if cancel {
                ErrorCode::Cancelled
            } else {
                ErrorCode::DeadlineExceeded
            }
        );
        assert_eq!(error.details.write_state, RequestState::SentUnknown);
        assert_eq!(
            result
                .pending_evidence
                .as_ref()
                .map_err(|_| "Expected unknown")?
                .steps["update"]
                .state,
            SubmissionState::Unknown
        );
        assert_eq!(server.request_count(), 1);
        assert_eq!(server.finish()?.len(), 1);
    }
    Ok(())
}

struct Fixture {
    parent: PathBuf,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        fs::create_dir_all(&parent)?;
        let parent = parent.canonicalize()?;
        let stamp = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_nanos();
        let root = parent.join(format!(
            "claimed-http-{}-{stamp}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        Ok(Self { parent, root })
    }
    fn setup(&self) -> Result<(ProfileStore, GrantRef, NativeJournal), Box<dyn std::error::Error>> {
        let control = control(60_000)?;
        let profiles = ProfileStore::open(&self.root, &control)?;
        let profile = profiles.create(
            &ProfileInput {
                name: "team".into(),
                provider: "jira".into(),
                deployment: "cloud".into(),
                auth_mode: AuthMode::Delegated,
                grant: None,
                tenant: Some(CLOUD.into()),
                defaults: BTreeMap::new(),
            },
            &control,
            |_| Ok(()),
        )?;
        let journal = NativeJournal::open(&profiles, &control)?;
        Ok((profiles, profile.grant, journal))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(root) = self.root.canonicalize()
            && root.starts_with(&self.parent)
            && root != self.parent
        {
            let _ = fs::remove_dir_all(root);
        }
    }
}

fn control(timeout: u64) -> Result<InvocationControl, CraftError> {
    InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        timeout,
    )
}
fn target() -> ResourceRef {
    ResourceRef {
        provider: "jira".into(),
        deployment: "cloud".into(),
        tenant: CLOUD.into(),
        resource_type: "issue".into(),
        id: "PAY-1".into(),
    }
}
fn session(grant: &GrantRef) -> Result<HostSession, CraftError> {
    HostSession::new(
        target().tenant_ref(),
        grant.clone(),
        SecretBytes::new(b"synthetic-bearer-sentinel".to_vec()).map_err(|_| internal())?,
    )
}
fn authority(grant: &GrantRef) -> MutationAuthority<'_> {
    MutationAuthority {
        apply: true,
        operation_id: "publish-summary",
        grant,
        actor: "account-one",
    }
}
fn call() -> NativeCall {
    NativeCall {
        repeated_query: std::collections::BTreeMap::new(),
        route: "update".into(),
        path_parameters: BTreeMap::from([("id".into(), "PAY-1".into())]),
        query: BTreeMap::new(),
        body: Some(json!({"title":"private-native-body-sentinel"})),
    }
}
struct Handler;
impl OperationHandler for Handler {
    fn validate(&self, _: &CanonicalRequest) -> Result<(), CraftError> {
        Ok(())
    }
    fn execute(
        &self,
        _: &PreparedOperation,
        _: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        Err(denied())
    }
}
fn prepared() -> Result<(Registry, PreparedOperation), CraftError> {
    let mut registry = Registry::default();
    registry.register_typed_handler::<Value>(
        OperationDescriptor {
            id: "jira.fixture.mutate".into(),
            provider: "jira".into(),
            contract_version: SCHEMA_VERSION.into(),
            api_revision: "fixture".into(),
            description: "Synthetic journal-bound JSON mutation.".into(),
            effect: EffectClass::Write,
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            response_schema: response_schema::<Value>()?,
            example: json!({}),
            response_example: serde_json::to_value(craft_core::contracts::Envelope::success(
                "jira.fixture.mutate",
                "example".into(),
                json!({}),
            ))
            .map_err(|_| internal())?,
            projection: None,
            scope_alternatives: vec![vec!["write:issue".into()]],
            permissions: vec!["update".into()],
            tenant_conditions: vec![],
            routes: vec![NativeRoute {
                repeated_query_parameters: Vec::new(),
                id: "update".into(),
                method: HttpMethod::Post,
                path: vec![
                    RouteSegment::Literal("rest".into()),
                    RouteSegment::Literal("api".into()),
                    RouteSegment::Literal("3".into()),
                    RouteSegment::Literal("issue".into()),
                    RouteSegment::Parameter("id".into()),
                ],
                query_parameters: vec![],
                continuation_parameters: vec![],
                response_max_bytes: 512,
            }],
            retry_class: RetryClass::ReconcileBeforeReplay,
            reconciliation: ReconciliationClass::KnownResource,
        },
        Arc::new(Handler),
    )?;
    let operation = registry.prepare(CanonicalRequest {
        command: "jira.fixture.mutate".into(),
        target: target(),
        input: json!({}),
        selection: Selection::default(),
        preconditions: None,
    })?;
    Ok((registry, operation))
}
fn local_request<'a>(
    server: &TlsServer,
    registry: &'a Registry,
    operation: &'a PreparedOperation,
    grant: &GrantRef,
    control: InvocationControl,
    call: &NativeCall,
) -> Result<PreparedHttpMutation<'a>, Box<dyn std::error::Error>> {
    let mut context = HttpContext::bound(
        registry,
        operation,
        grant,
        session(grant)?,
        TlsPolicy::AdditionalCaBundle(server.root()),
        control,
    )?;
    context
        .endpoint
        .prefix
        .set_host(Some("localhost"))
        .map_err(|_| "Invalid fixture host")?;
    context
        .endpoint
        .prefix
        .set_port(Some(server.address().port()))
        .map_err(|_| "Invalid fixture port")?;
    Ok(PreparedHttpMutation::freeze(
        registry, operation, context, call,
    )?)
}
fn confirmed() -> StepOutcome {
    StepOutcome {
        state: SubmissionState::Confirmed,
        confirmed_resources: vec![target()],
        job: Some(RemoteJob {
            resource: ResourceRef {
                resource_type: "job".into(),
                id: "job-one".into(),
                ..target()
            },
            state: JobState::Accepted,
        }),
    }
}

#[test]
fn frozen_request_is_sent_once_and_semantic_receipts_require_explicit_recording()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let (profiles, grant, journal) = fixture.setup()?;
    let server = TlsServer::start(Reply::json(
        201,
        br#"{"id":"PAY-1"}"#,
        &[("atl-traceid", "trace-one")],
    ))?;
    let (registry, operation) = prepared()?;
    let mut native = call();
    let request = local_request(
        &server,
        &registry,
        &operation,
        &grant,
        control(5_000)?,
        &native,
    )?;
    native.body = Some(json!({"title":"changed-after-preflight"}));
    let attempt = request.send(&journal, &authority(&grant), "update")?;
    let response = attempt.response.as_ref().map_err(|_| "Expected response")?;
    assert_eq!(response.status, 201);
    assert_eq!(response.body["id"], "PAY-1");
    assert_eq!(response.request_id.as_deref(), Some("trace-one"));
    let pending = attempt
        .pending_evidence
        .as_ref()
        .map_err(|_| "Expected unknown record")?;
    assert_eq!(pending.steps["update"].state, SubmissionState::Unknown);
    assert!(pending.steps["update"].confirmed_resources.is_empty());
    assert_eq!(
        pending.steps["update"].request_digest,
        Some(canonical_digest(
            &json!({"route":"update","method":"post","parameters":call().path_parameters,"query":{},"body":call().body})
        )?)
    );
    let receipt = attempt.record_outcome(&confirmed())?;
    assert_eq!(receipt.steps["update"].confirmed_resources, vec![target()]);
    assert_eq!(
        receipt.steps["update"]
            .job
            .as_ref()
            .ok_or("Missing job")?
            .state,
        JobState::Accepted
    );
    let fresh = NativeJournal::open(&profiles, &control(5_000)?)?;
    assert_eq!(fresh.get("publish-summary")?, Some(receipt));
    assert!(
        local_request(
            &server,
            &registry,
            &operation,
            &grant,
            control(5_000)?,
            &call()
        )?
        .send(&journal, &authority(&grant), "update")
        .is_err()
    );
    let observed = server.finish()?;
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].method, "POST");
    assert_eq!(
        observed[0].target,
        format!("/ex/jira/{CLOUD}/rest/api/3/issue/PAY-1")
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&observed[0].body)?,
        call().body.ok_or("Missing body")?
    );
    assert_eq!(
        observed[0].headers.get("authorization").map(String::as_str),
        Some("Bearer synthetic-bearer-sentinel")
    );
    let bytes = fs::read(fixture.root.join("state.sqlite3"))?;
    for sentinel in [
        b"private-native-body-sentinel".as_slice(),
        b"synthetic-bearer-sentinel",
    ] {
        assert!(
            !bytes
                .windows(sentinel.len())
                .any(|window| window == sentinel)
        );
    }
    Ok(())
}

#[test]
fn disconnect_invalid_response_and_redirect_preserve_unknown_without_replay()
-> Result<(), Box<dyn std::error::Error>> {
    for reply in [
        Reply::Disconnect,
        Reply::json(200, br#"{"id":1,"id":2}"#, &[]),
        Reply::json(200, &vec![b'x'; 513], &[]),
        Reply::json(302, b"{}", &[("Location", "https://untrusted.invalid/")]),
        Reply::Raw(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}".to_vec()),
    ] {
        let fixture = Fixture::new()?;
        let (_profiles, grant, journal) = fixture.setup()?;
        let server = TlsServer::start(reply)?;
        let (registry, operation) = prepared()?;
        let attempt = local_request(&server, &registry, &operation, &grant, control(5_000)?, &call())?.send(&journal, &authority(&grant), "update")?;
        let error = attempt.response.as_ref().err().ok_or("Expected exchange error")?;
        assert_eq!(error.details.write_state, RequestState::SentUnknown);
        assert!(!error.retryable);
        let serialized = serde_json::to_string(error)?;
        for sentinel in ["synthetic-bearer-sentinel", "untrusted.invalid", "private-native-body-sentinel"] { assert!(!serialized.contains(sentinel)); }
        assert_eq!(attempt.pending_evidence.as_ref().map_err(|_| "Expected unknown")?.steps["update"].state, SubmissionState::Unknown);
        assert!(local_request(&server, &registry, &operation, &grant, control(5_000)?, &call())?.send(&journal, &authority(&grant), "update").is_err());
        assert_eq!(server.finish()?.len(), 1);
    }
    Ok(())
}

#[test]
fn native_error_status_and_retry_advice_are_data_without_confirmation_or_retry()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let (_profiles, grant, journal) = fixture.setup()?;
    let server = TlsServer::start(Reply::json(
        429,
        br#"{"secret":"provider-error-sentinel"}"#,
        &[("Retry-After", "7")],
    ))?;
    let (registry, operation) = prepared()?;
    let attempt = local_request(
        &server,
        &registry,
        &operation,
        &grant,
        control(5_000)?,
        &call(),
    )?
    .send(&journal, &authority(&grant), "update")?;
    let response = attempt
        .response
        .as_ref()
        .map_err(|_| "Expected native status")?;
    assert_eq!(response.status, 429);
    assert_eq!(response.retry_after, Some(RetryAfter::DelaySeconds(7)));
    assert_eq!(response.body, Value::Null);
    assert_eq!(
        attempt
            .pending_evidence
            .as_ref()
            .map_err(|_| "Expected unknown")?
            .steps["update"]
            .state,
        SubmissionState::Unknown
    );
    assert_eq!(server.finish()?.len(), 1);
    Ok(())
}

#[test]
fn preflight_apply_and_authority_refusals_create_no_claim_or_connection()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let (_profiles, grant, journal) = fixture.setup()?;
    let server = TlsServer::start(Reply::json(200, b"{}", &[]))?;
    let (registry, operation) = prepared()?;
    for kind in ["query", "path", "body", "route"] {
        let mut native = call();
        match kind {
            "query" => {
                native
                    .query
                    .insert("authorization".into(), "private-query".into());
            }
            "path" => {
                native
                    .path_parameters
                    .insert("id".into(), "../escape".into());
            }
            "body" => native.body = Some(json!("x".repeat(1_048_577))),
            _ => native.route = "unknown".into(),
        }
        assert!(
            local_request(
                &server,
                &registry,
                &operation,
                &grant,
                control(5_000)?,
                &native
            )
            .is_err()
        );
    }
    let mut no_apply = authority(&grant);
    no_apply.apply = false;
    let error = local_request(
        &server,
        &registry,
        &operation,
        &grant,
        control(5_000)?,
        &call(),
    )?
    .send(&journal, &no_apply, "update")
    .err()
    .ok_or("Expected apply refusal")?;
    assert_eq!(error.code, ErrorCode::ApplyRequired);
    let wrong_grant = GrantRef {
        id: "gr_00000000000000000000000000000000".into(),
    };
    assert!(
        local_request(
            &server,
            &registry,
            &operation,
            &grant,
            control(5_000)?,
            &call()
        )?
        .send(&journal, &authority(&wrong_grant), "update")
        .is_err()
    );
    assert!(journal.get("publish-summary")?.is_none());
    assert_eq!(server.connections(), 0);
    assert!(server.finish()?.is_empty());
    Ok(())
}

#[test]
fn lost_send_marker_acknowledgment_never_reaches_http_or_reopens_the_claim()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let (profiles, grant, journal) = fixture.setup()?;
    let server = TlsServer::start(Reply::json(200, b"{}", &[]))?;
    let (registry, operation) = prepared()?;
    journal.fail_on(FaultPoint::JournalUpdateAfterCommit, 2)?;
    let error = local_request(
        &server,
        &registry,
        &operation,
        &grant,
        control(5_000)?,
        &call(),
    )?
    .send(&journal, &authority(&grant), "update")
    .err()
    .ok_or("Expected claim uncertainty")?;
    assert_eq!(error.code, ErrorCode::WriteOutcomeUnknown);
    let fresh = NativeJournal::open(&profiles, &control(5_000)?)?;
    assert_eq!(
        fresh.get("publish-summary")?.ok_or("Missing marker")?.steps["update"].state,
        SubmissionState::SendStarted
    );
    assert!(
        local_request(
            &server,
            &registry,
            &operation,
            &grant,
            control(5_000)?,
            &call()
        )?
        .send(&fresh, &authority(&grant), "update")
        .is_err()
    );
    assert_eq!(server.connections(), 0);
    assert!(server.finish()?.is_empty());
    Ok(())
}

#[test]
fn receipt_persistence_failures_preserve_response_and_allow_qualified_evidence_after_cancellation()
-> Result<(), Box<dyn std::error::Error>> {
    for point in [
        FaultPoint::JournalUpdateBeforeCommit,
        FaultPoint::JournalUpdateAfterCommit,
    ] {
        let fixture = Fixture::new()?;
        let (profiles, grant, journal) = fixture.setup()?;
        let server = TlsServer::start(Reply::json(201, br#"{"id":"PAY-1"}"#, &[]))?;
        let (registry, operation) = prepared()?;
        let token = CancellationToken::default();
        let invocation =
            InvocationControl::new(Arc::new(SystemClock::default()), token.clone(), 5_000)?;
        journal.fail_on(point, 3)?;
        let attempt = local_request(&server, &registry, &operation, &grant, invocation, &call())?
            .send(&journal, &authority(&grant), "update")?;
        assert!(attempt.pending_evidence.is_err());
        assert_eq!(
            attempt
                .response
                .as_ref()
                .map_err(|_| "Lost known response")?
                .body["id"],
            "PAY-1"
        );
        let fresh = NativeJournal::open(&profiles, &control(5_000)?)?;
        let prior = fresh
            .get("publish-summary")?
            .ok_or("Missing durable marker")?;
        assert!(prior.steps["update"].state.requires_reconciliation());
        token.cancel();
        let confirmed = attempt.record_outcome(&confirmed())?;
        assert_eq!(fresh.get("publish-summary")?, Some(confirmed));
        assert_eq!(server.finish()?.len(), 1);
    }
    Ok(())
}

#[test]
fn journal_contention_uses_http_deadline_and_cancelled_preflight_sends_nothing()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let (_profiles, grant, journal) = fixture.setup()?;
    let server = TlsServer::start(Reply::json(200, b"{}", &[]))?;
    let (registry, operation) = prepared()?;
    let blocker = rusqlite::Connection::open(fixture.root.join("state.sqlite3"))?;
    blocker.execute_batch("PRAGMA journal_mode=PERSIST; BEGIN IMMEDIATE;")?;
    let start = Instant::now();
    let error = local_request(
        &server,
        &registry,
        &operation,
        &grant,
        control(150)?,
        &call(),
    )?
    .send(&journal, &authority(&grant), "update")
    .err()
    .ok_or("Expected deadline")?;
    assert_eq!(error.code, ErrorCode::DeadlineExceeded);
    assert!(start.elapsed() < Duration::from_secs(2));
    blocker.execute_batch("ROLLBACK")?;
    assert!(journal.get("publish-summary")?.is_none());
    let token = CancellationToken::default();
    let invocation =
        InvocationControl::new(Arc::new(SystemClock::default()), token.clone(), 5_000)?;
    let request = local_request(&server, &registry, &operation, &grant, invocation, &call())?;
    token.cancel();
    assert_eq!(
        request
            .send(&journal, &authority(&grant), "update")
            .err()
            .ok_or("Expected cancellation")?
            .code,
        ErrorCode::Cancelled
    );
    assert!(journal.get("publish-summary")?.is_none());
    assert_eq!(server.connections(), 0);
    assert!(server.finish()?.is_empty());
    Ok(())
}

mod multipart;
