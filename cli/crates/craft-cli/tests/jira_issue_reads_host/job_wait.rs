//! Recorded job handles polled through the real service access and TLS transport.
use super::*;
use craft_cli::job_wait::{self, PollRequest, StopReason, WaitEnvironment};
use craft_core::contracts::{JobState, RemoteJob};
use craft_core::input::content_digest;
use craft_core::storage::{
    JournalRecord, JournalStep, OperationBinding, OperationJournal, SubmissionState,
};
use craft_runtime::journal::NativeJournal;

fn handle(provider: &str, resource_type: &str, id: &str) -> RemoteJob {
    RemoteJob {
        resource: ResourceRef {
            provider: provider.into(),
            deployment: "cloud".into(),
            tenant: CLOUD.into(),
            resource_type: resource_type.into(),
            id: id.into(),
        },
        state: JobState::Accepted,
    }
}

fn seed(
    journal: &NativeJournal,
    grant: &GrantRef,
    actor: &str,
    job: &RemoteJob,
) -> Result<JournalRecord, Box<dyn std::error::Error>> {
    let mut record = journal.begin(&OperationBinding {
        operation_id: "async-submit".into(),
        request_digest: content_digest(b"logical"),
        descriptor_digest: content_digest(b"descriptor"),
        target: ResourceRef {
            provider: job.resource.provider.clone(),
            deployment: "cloud".into(),
            tenant: CLOUD.into(),
            resource_type: "site".into(),
            id: CLOUD.into(),
        },
        grant: grant.clone(),
        actor: actor.into(),
    })?;
    for state in [
        SubmissionState::Prepared,
        SubmissionState::SendStarted,
        SubmissionState::Confirmed,
    ] {
        let step = record.steps.entry("submit".into()).or_insert(JournalStep {
            request_digest: Some(content_digest(b"native")),
            ..Default::default()
        });
        step.state = state;
        if state == SubmissionState::Confirmed {
            step.job = Some(job.clone());
        }
        record.revision += 1;
        journal.compare_and_swap(record.revision - 1, &record)?;
    }
    Ok(record)
}

fn task(status: &str) -> Result<Reply, serde_json::Error> {
    reply(
        json!({"id":"77","status":status,"progress":50,"elapsedRuntime":10,"result":"private-sentinel","message":"private-sentinel"}),
    )
}

struct Run {
    envelope: Envelope,
    pauses: Vec<Duration>,
}

fn poll(
    setup: &Setup,
    fixture: &ServiceFixture,
    journal: &NativeJournal,
    wait_ms: u64,
    token: CancellationToken,
    cancel_on_pause: bool,
    product: &TlsServer,
) -> Result<Run, Box<dyn std::error::Error>> {
    let record = journal.get("async-submit")?.ok_or("record")?;
    let job_wait::Selected::Poll(target) = job_wait::select("async-submit", None, Some(record))?
    else {
        return Err("expected pollable record".into());
    };
    let job_wait::PollTarget {
        binding,
        step,
        job,
        submission,
    } = *target;
    let control = InvocationControl::new(Arc::new(SystemClock::default()), token.clone(), 30_000)?;
    let locked = fixture.lock(&setup.grant, &control)?;
    let pauses = Mutex::new(Vec::new());
    let pause = |duration: Duration, control: &InvocationControl| {
        pauses
            .lock()
            .map_err(|_| CraftError::new(ErrorCode::Internal, "lock"))?
            .push(duration);
        if cancel_on_pause {
            token.cancel();
        }
        control.check()
    };
    let result = job_wait::poll(
        PollRequest {
            binding,
            step,
            job,
            submission,
            grant: setup.grant.clone(),
            wait: Duration::from_millis(wait_ms),
            request_id: "wait-request".into(),
        },
        WaitEnvironment {
            journal,
            client: fixture.client(),
            locked: &locked,
            tls: TlsPolicy::AdditionalCaBundle(product.root()),
            control,
            pause: &pause,
        },
    );
    Ok(Run {
        envelope: result.envelope,
        pauses: pauses.into_inner().map_err(|_| "lock")?,
    })
}

fn provisioned(
    setup: &Setup,
    product: &TlsServer,
    scopes: &str,
) -> Result<(TlsServer, ServiceFixture), Box<dyn std::error::Error>> {
    let issuer = issuer(scopes)?;
    let fixture = setup.fixture(&issuer, product, Arc::new(ManualClock::new(1000)))?;
    let locked = fixture.lock(&setup.grant, &control()?)?;
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref()],
        Some("actor-one"),
    )?;
    drop(locked);
    Ok((issuer, fixture))
}

fn job_state(
    journal: &NativeJournal,
) -> Result<(SubmissionState, JobState), Box<dyn std::error::Error>> {
    let record = journal.get("async-submit")?.ok_or("record")?;
    let step = &record.steps["submit"];
    Ok((step.state, step.job.as_ref().ok_or("job")?.state))
}

fn validate(envelope: &Envelope) -> TestResult {
    let registry = craft_cli::catalog::registry()?;
    let schema = &registry.descriptor(job_wait::COMMAND)?.response_schema;
    assert!(jsonschema::validator_for(schema)?.is_valid(&json!(envelope)));
    Ok(())
}

#[test]
fn accepted_job_is_polled_to_failure_through_status_reads_only() -> TestResult {
    let setup = Setup::new()?;
    let journal = NativeJournal::open(&setup.store, &control()?)?;
    seed(
        &journal,
        &setup.grant,
        "actor-one",
        &handle("jira", "task", "77"),
    )?;
    let product = TlsServer::start(Reply::Sequence(vec![
        identity(),
        identity(),
        task("RUNNING")?,
        task("CANCELLED")?,
    ]))?;
    let (issuer, fixture) = provisioned(&setup, &product, "read:jira-user read:jira-work")?;
    let run = poll(
        &setup,
        &fixture,
        &journal,
        60_000,
        CancellationToken::default(),
        false,
        &product,
    )?;
    assert!(run.envelope.ok, "{:?}", run.envelope.error);
    let data = data(&run.envelope)?;
    assert_eq!(data["stop_reason"], json!(StopReason::Terminal));
    assert_eq!(data["polls"], 2);
    assert_eq!(data["job"]["state"], "failed");
    assert_eq!(data["observation"]["native_status"], "CANCELLED");
    assert_eq!(data["journal_current"], true);
    assert_eq!(data["replay_authorized"], false);
    assert_eq!(run.pauses, vec![Duration::from_secs(1)]);
    assert!(!serde_json::to_string(&run.envelope)?.contains("private-sentinel"));
    assert_eq!(
        job_state(&journal)?,
        (SubmissionState::Confirmed, JobState::Failed)
    );
    validate(&run.envelope)?;
    let observed = product.finish()?;
    let status_reads: Vec<_> = observed
        .iter()
        .filter(|r| r.target.contains("/task/"))
        .collect();
    assert_eq!(status_reads.len(), 2);
    assert!(
        status_reads
            .iter()
            .all(|r| r.target.ends_with("/rest/api/3/task/77"))
    );
    assert!(observed.iter().all(|r| r.method == "GET"));
    issuer.finish()?;
    setup.no_plaintext_credentials()
}

#[test]
fn pending_job_stays_pending_with_its_handle_when_the_wait_ends() -> TestResult {
    let setup = Setup::new()?;
    let journal = NativeJournal::open(&setup.store, &control()?)?;
    seed(
        &journal,
        &setup.grant,
        "actor-one",
        &handle("jira", "task", "77"),
    )?;
    let product = TlsServer::start(Reply::Sequence(vec![
        identity(),
        identity(),
        task("RUNNING")?,
    ]))?;
    let (_issuer, fixture) = provisioned(&setup, &product, "read:jira-user read:jira-work")?;
    let run = poll(
        &setup,
        &fixture,
        &journal,
        0,
        CancellationToken::default(),
        false,
        &product,
    )?;
    assert!(run.envelope.ok, "{:?}", run.envelope.error);
    let data = data(&run.envelope)?;
    assert_eq!(data["stop_reason"], json!(StopReason::WaitElapsed));
    assert_eq!(data["job"]["state"], "running");
    assert_eq!(data["job"]["resource"]["id"], "77");
    assert!(run.pauses.is_empty());
    assert_eq!(
        job_state(&journal)?,
        (SubmissionState::Confirmed, JobState::Running)
    );
    validate(&run.envelope)?;
    assert_eq!(product.finish()?.len(), 3);
    Ok(())
}

#[test]
fn rate_limited_or_missing_tasks_keep_the_recorded_state() -> TestResult {
    for (status_reply, stop, ok) in [
        (
            Reply::json(429, br#"{}"#, &[("Retry-After", "120")]),
            StopReason::RateLimited,
            false,
        ),
        (
            Reply::json(404, br#"{}"#, &[]),
            StopReason::NotVisible,
            true,
        ),
    ] {
        let setup = Setup::new()?;
        let journal = NativeJournal::open(&setup.store, &control()?)?;
        seed(
            &journal,
            &setup.grant,
            "actor-one",
            &handle("jira", "task", "77"),
        )?;
        let product =
            TlsServer::start(Reply::Sequence(vec![identity(), identity(), status_reply]))?;
        let (_issuer, fixture) = provisioned(&setup, &product, "read:jira-user read:jira-work")?;
        let run = poll(
            &setup,
            &fixture,
            &journal,
            5_000,
            CancellationToken::default(),
            false,
            &product,
        )?;
        assert_eq!(run.envelope.ok, ok, "{:?}", run.envelope.error);
        let data = data(&run.envelope)?;
        assert_eq!(data["stop_reason"], json!(stop));
        assert_eq!(data["job"]["state"], "accepted");
        if stop == StopReason::RateLimited {
            assert_eq!(code(&run.envelope), Some(ErrorCode::RateLimited));
            assert_eq!(
                data["retry_after"],
                json!({"kind":"delay_seconds","value":120})
            );
        }
        assert_eq!(
            job_state(&journal)?,
            (SubmissionState::Confirmed, JobState::Accepted)
        );
        validate(&run.envelope)?;
        assert_eq!(product.finish()?.len(), 3);
    }
    Ok(())
}

#[test]
fn cancellation_between_polls_keeps_the_observed_state() -> TestResult {
    let setup = Setup::new()?;
    let journal = NativeJournal::open(&setup.store, &control()?)?;
    seed(
        &journal,
        &setup.grant,
        "actor-one",
        &handle("jira", "task", "77"),
    )?;
    let product = TlsServer::start(Reply::Sequence(vec![
        identity(),
        identity(),
        task("RUNNING")?,
    ]))?;
    let (_issuer, fixture) = provisioned(&setup, &product, "read:jira-user read:jira-work")?;
    let run = poll(
        &setup,
        &fixture,
        &journal,
        60_000,
        CancellationToken::default(),
        true,
        &product,
    )?;
    assert!(!run.envelope.ok);
    assert_eq!(code(&run.envelope), Some(ErrorCode::Cancelled));
    let data = data(&run.envelope)?;
    assert_eq!(data["stop_reason"], json!(StopReason::Cancelled));
    assert_eq!(data["job"]["state"], "running");
    assert_eq!(
        job_state(&journal)?,
        (SubmissionState::Confirmed, JobState::Running)
    );
    validate(&run.envelope)?;
    assert_eq!(product.finish()?.len(), 3);
    Ok(())
}

#[test]
fn another_actor_or_unscoped_token_refuses_before_any_status_read() -> TestResult {
    for (actor, scopes, expected) in [
        (
            "someone-else",
            "read:jira-user read:jira-work",
            ErrorCode::PolicyDenied,
        ),
        ("actor-one", "read:jira-user", ErrorCode::PermissionDenied),
    ] {
        let setup = Setup::new()?;
        let journal = NativeJournal::open(&setup.store, &control()?)?;
        seed(&journal, &setup.grant, actor, &handle("jira", "task", "77"))?;
        let product = TlsServer::start(Reply::Sequence(vec![identity(), identity()]))?;
        let (_issuer, fixture) = provisioned(&setup, &product, scopes)?;
        let run = poll(
            &setup,
            &fixture,
            &journal,
            0,
            CancellationToken::default(),
            false,
            &product,
        )?;
        assert!(!run.envelope.ok);
        assert_eq!(code(&run.envelope), Some(expected));
        assert_eq!(data(&run.envelope)?["polls"], 0);
        let observed = product.finish()?;
        assert!(observed.iter().all(|r| !r.target.contains("/task/")));
        assert_eq!(
            job_state(&journal)?,
            (SubmissionState::Confirmed, JobState::Accepted)
        );
    }
    Ok(())
}

#[test]
fn confluence_long_task_reaches_success_through_its_own_route() -> TestResult {
    let setup = Setup::new()?;
    let confluence = setup.store.create(
        &ProfileInput {
            name: "wiki".into(),
            provider: "confluence".into(),
            deployment: "cloud".into(),
            auth_mode: AuthMode::Service,
            grant: Some(setup.grant.clone()),
            tenant: Some(CLOUD.into()),
            defaults: BTreeMap::new(),
        },
        &control()?,
        |_| Ok(()),
    )?;
    assert_eq!(confluence.grant, setup.grant);
    let journal = NativeJournal::open(&setup.store, &control()?)?;
    seed(
        &journal,
        &setup.grant,
        "actor-one",
        &handle("confluence", "longtask", "5e2c-41"),
    )?;
    let product = TlsServer::start(Reply::Sequence(vec![
        identity(),
        identity(),
        Reply::json(200, br#"{"accountId":"actor-one","type":"known"}"#, &[]),
        reply(
            json!({"id":"5e2c-41","finished":true,"successful":true,"percentageComplete":100,"messages":[{"translation":"private-sentinel"}]}),
        )?,
    ]))?;
    let issuer = issuer(
        "read:jira-user read:jira-work read:confluence-user read:content.metadata:confluence",
    )?;
    let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
    let locked = fixture.lock(&setup.grant, &control()?)?;
    let site = ResourceRef {
        provider: "confluence".into(),
        ..target()
    };
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref(), site.tenant_ref()],
        Some("actor-one"),
    )?;
    drop(locked);
    let run = poll(
        &setup,
        &fixture,
        &journal,
        0,
        CancellationToken::default(),
        false,
        &product,
    )?;
    assert!(run.envelope.ok, "{:?}", run.envelope.error);
    assert_eq!(data(&run.envelope)?["job"]["state"], "succeeded");
    assert!(!serde_json::to_string(&run.envelope)?.contains("private-sentinel"));
    assert_eq!(
        job_state(&journal)?,
        (SubmissionState::Confirmed, JobState::Succeeded)
    );
    let observed = product.finish()?;
    assert!(
        observed
            .iter()
            .any(|r| r.target.ends_with("/wiki/rest/api/longtask/5e2c-41"))
    );
    assert!(observed.iter().all(|r| r.method == "GET"));
    Ok(())
}
