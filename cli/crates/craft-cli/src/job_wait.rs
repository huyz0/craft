//! Bounded polling of a recorded provider job handle. The host registers only task-status GET
//! routes, so it cannot submit or resubmit work; every state it learns is appended to the
//! journal as a one-way job transition.
pub mod command;

use std::time::{Duration, Instant};

use craft_core::contracts::{
    CanonicalRequest, CredentialCacheUpdate, Envelope, GrantRef, JobObservation, JobState,
    RemoteJob, RequestState, ResourceRef, Selection,
};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{NativeResponse, PreparedOperation, RetryAfter};
use craft_core::registry::Registry;
use craft_core::storage::{JournalRecord, OperationBinding, SubmissionState};
use craft_provider_confluence::longtasks;
use craft_provider_jira::tasks;
use craft_runtime::auth::{ServiceOAuthClient, VerifiedServiceAccess};
use craft_runtime::journal::NativeJournal;
use craft_runtime::secrets::LockedCredentials;
use craft_runtime::transport::TlsPolicy;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::dispatch::CommandResult;
use crate::output::OutputPolicy;

pub const COMMAND: &str = "operation.wait";
/// Longest single wait between polls; provider advice above the window stops the wait instead.
const MAX_BACKOFF: Duration = Duration::from_secs(10);
/// Kept free at the end of the invocation so the final journal append and output still fit.
const RESERVE: Duration = Duration::from_millis(1_500);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The recorded or observed job is succeeded or failed.
    Terminal,
    /// The requested wait or the invocation budget ended with the job still pending.
    WaitElapsed,
    RateLimited,
    Cancelled,
    /// The provider no longer shows the task; tasks expire, so this is not a failure.
    NotVisible,
    /// There is no recorded job handle to poll.
    NotPollable,
    Error,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaitData {
    pub operation_id: String,
    pub found: bool,
    pub step: Option<String>,
    pub submission_state: Option<SubmissionState>,
    /// Latest known job evidence: the recorded handle, advanced by any observation this run.
    pub job: Option<RemoteJob>,
    /// The last provider status this invocation observed, without native result or messages.
    pub observation: Option<JobObservation>,
    #[schemars(range(max = 10_000))]
    pub polls: u32,
    pub stop_reason: StopReason,
    pub retry_after: Option<RetryAfter>,
    /// Whether every observed state change is in the journal. False keeps `job` as the only
    /// copy of a newer observation.
    pub journal_current: bool,
    /// Null when no local record exists, so nothing can be inferred.
    pub reconciliation_required: Option<bool>,
    pub credential_cache_update: Option<CredentialCacheUpdate>,
    #[schemars(extend("const" = false))]
    pub replay_authorized: bool,
}

/// The step whose job this wait follows, decided from local evidence alone.
pub enum Selected {
    /// Nothing to poll; the data is final.
    Local(Box<WaitData>),
    Poll(Box<PollTarget>),
}

pub struct PollTarget {
    pub binding: OperationBinding,
    pub step: String,
    pub job: RemoteJob,
    pub submission: SubmissionState,
}

fn uncertain(record: &JournalRecord) -> bool {
    record.steps.values().any(|step| {
        step.state.requires_reconciliation()
            || step
                .job
                .as_ref()
                .is_some_and(|job| job.state == JobState::Unknown)
    })
}

pub fn terminal(state: JobState) -> bool {
    matches!(state, JobState::Succeeded | JobState::Failed)
}

fn base(operation_id: &str, record: Option<&JournalRecord>) -> WaitData {
    WaitData {
        operation_id: operation_id.into(),
        found: record.is_some(),
        step: None,
        submission_state: None,
        job: None,
        observation: None,
        polls: 0,
        stop_reason: StopReason::NotPollable,
        retry_after: None,
        journal_current: true,
        reconciliation_required: record.map(uncertain),
        credential_cache_update: None,
        replay_authorized: false,
    }
}

/// Chooses the job step before any profile or credential access.
pub fn select(
    operation_id: &str,
    step: Option<&str>,
    record: Option<JournalRecord>,
) -> Result<Selected, CraftError> {
    let Some(record) = record else {
        return Ok(Selected::Local(Box::new(base(operation_id, None))));
    };
    let mut data = base(operation_id, Some(&record));
    let name = match step {
        Some(name) => {
            if !record.steps.contains_key(name) {
                return Err(CraftError::field("/step", "step_not_recorded"));
            }
            name.to_owned()
        }
        None => {
            let jobs: Vec<_> = record
                .steps
                .iter()
                .filter(|(_, step)| step.job.is_some())
                .map(|(name, _)| name.clone())
                .collect();
            match jobs.as_slice() {
                [only] => only.clone(),
                [] => {
                    // A lone step is still reported so a lost submission receipt is visible.
                    if let [(name, step)] = record.steps.iter().collect::<Vec<_>>().as_slice() {
                        data.step = Some((*name).clone());
                        data.submission_state = Some(step.state);
                    }
                    return Ok(Selected::Local(Box::new(data)));
                }
                _ => return Err(CraftError::field("/step", "step_required_for_several_jobs")),
            }
        }
    };
    let recorded = &record.steps[&name];
    data.step = Some(name.clone());
    data.submission_state = Some(recorded.state);
    data.job = recorded.job.clone();
    match &recorded.job {
        None => Ok(Selected::Local(Box::new(data))),
        Some(job) if terminal(job.state) => {
            data.stop_reason = StopReason::Terminal;
            Ok(Selected::Local(Box::new(data)))
        }
        Some(job) => Ok(Selected::Poll(Box::new(PollTarget {
            binding: record.binding.clone(),
            step: name,
            job: job.clone(),
            submission: recorded.state,
        }))),
    }
}

/// The provider operation that reads one handle's status, or a refusal for an unknown handle.
pub fn status_operation(job: &RemoteJob) -> Result<(Registry, PreparedOperation), CraftError> {
    let mut registry = Registry::default();
    let (command, input) = match (
        job.resource.provider.as_str(),
        job.resource.resource_type.as_str(),
    ) {
        ("jira", tasks::RESOURCE_TYPE) if tasks::valid_task_id(&job.resource.id) => {
            tasks::register(&mut registry)?;
            (tasks::COMMAND, json!({"task_id":job.resource.id}))
        }
        ("confluence", longtasks::RESOURCE_TYPE) if longtasks::valid_task_id(&job.resource.id) => {
            longtasks::register(&mut registry)?;
            (longtasks::COMMAND, json!({"task_id":job.resource.id}))
        }
        _ => {
            return Err(CraftError::new(
                ErrorCode::UnsupportedCapability,
                "The recorded job handle has no supported status read.",
            ));
        }
    };
    let operation = registry.prepare(CanonicalRequest {
        command: command.into(),
        target: site(&job.resource),
        input,
        selection: Selection::default(),
        preconditions: None,
    })?;
    Ok((registry, operation))
}

pub fn site(resource: &ResourceRef) -> ResourceRef {
    ResourceRef {
        provider: resource.provider.clone(),
        deployment: resource.deployment.clone(),
        tenant: resource.tenant.clone(),
        resource_type: "site".into(),
        id: resource.tenant.clone(),
    }
}

fn observe(job: &RemoteJob, response: &NativeResponse) -> Result<JobObservation, CraftError> {
    if job.resource.provider == "jira" {
        tasks::observe(&response.body, &job.resource.id)
    } else {
        longtasks::observe(&response.body, &job.resource.id)
    }
}

pub struct WaitEnvironment<'a, 'credentials> {
    pub journal: &'a NativeJournal,
    pub client: &'a ServiceOAuthClient,
    pub locked: &'a LockedCredentials<'credentials>,
    pub tls: TlsPolicy,
    pub control: InvocationControl,
    /// Waits for at most the given duration and returns early with an error on cancellation.
    pub pause: &'a dyn Fn(Duration, &InvocationControl) -> Result<(), CraftError>,
}

/// Cancellation-aware sleep in short slices; never extends the invocation deadline.
pub fn pause(duration: Duration, control: &InvocationControl) -> Result<(), CraftError> {
    let end = Instant::now() + duration;
    loop {
        let remaining = control.remaining()?;
        let now = Instant::now();
        if now >= end {
            return Ok(());
        }
        std::thread::sleep((end - now).min(remaining).min(Duration::from_millis(50)));
    }
}

fn advice(retry_after: Option<RetryAfter>, control: &InvocationControl) -> Option<Duration> {
    match retry_after? {
        RetryAfter::DelaySeconds(seconds) => Some(Duration::from_secs(seconds)),
        RetryAfter::AtUnixMillis(at) => Some(Duration::from_millis(
            at.saturating_sub(control.unix_millis()),
        )),
    }
}

fn denied(message: &str) -> CraftError {
    CraftError::new(ErrorCode::PolicyDenied, message)
}

pub struct PollRequest {
    pub binding: OperationBinding,
    pub step: String,
    pub job: RemoteJob,
    pub submission: SubmissionState,
    pub grant: GrantRef,
    pub wait: Duration,
    pub request_id: String,
}

/// Polls one recorded handle. The returned data always carries the latest known job state,
/// including on failure, so a caller can never lose an observation.
pub fn poll(request: PollRequest, environment: WaitEnvironment<'_, '_>) -> CommandResult {
    let WaitEnvironment {
        journal,
        client,
        locked,
        tls,
        control,
        pause,
    } = environment;
    let mut data = base(&request.binding.operation_id, None);
    data.found = true;
    data.step = Some(request.step.clone());
    data.submission_state = Some(request.submission);
    data.job = Some(request.job.clone());
    data.reconciliation_required = Some(
        request.submission.requires_reconciliation() || request.job.state == JobState::Unknown,
    );
    let started = Instant::now();
    let result = (|| -> Result<(), CraftError> {
        if request.grant != request.binding.grant || locked.grant() != request.binding.grant {
            return Err(denied(
                "The selected profile does not hold the grant that recorded this job.",
            ));
        }
        let (registry, operation) = status_operation(&request.job)?;
        let client = client.for_invocation(control.clone())?;
        let tenant = site(&request.job.resource).tenant_ref();
        if tenant != request.binding.target.tenant_ref() {
            return Err(denied("The job handle belongs to another site."));
        }
        let access: VerifiedServiceAccess = match client.open_service_access(locked, &tenant) {
            Ok(access) => access,
            Err(failure) => {
                data.credential_cache_update = failure
                    .credential_update
                    .map(|receipt| CredentialCacheUpdate {
                        grant: locked.grant(),
                        write_state: RequestState::Confirmed,
                        cleanup_pending: Some(receipt.cleanup_pending),
                    })
                    .or_else(|| {
                        failure
                            .credential_write_state
                            .map(|state| CredentialCacheUpdate {
                                grant: locked.grant(),
                                write_state: state,
                                cleanup_pending: None,
                            })
                    });
                return Err(failure.error);
            }
        };
        data.credential_cache_update = access.cache_update().map(|receipt| CredentialCacheUpdate {
            grant: access.grant().clone(),
            write_state: RequestState::Confirmed,
            cleanup_pending: Some(receipt.cleanup_pending),
        });
        if access.identity().actor() != request.binding.actor {
            return Err(denied(
                "The verified actor differs from the actor that recorded this job.",
            ));
        }
        access.check_operation(&registry, &operation)?;
        let window = request
            .wait
            .min(control.remaining()?.saturating_sub(RESERVE));
        let mut backoff = Duration::from_secs(1);
        loop {
            control.check()?;
            let mut context =
                access.read_context(&registry, &operation, tls.clone(), control.clone())?;
            let response = if request.job.resource.provider == "jira" {
                tasks::read_status(&operation, &mut context)?
            } else {
                longtasks::read_status(&operation, &mut context)?
            };
            data.polls = data.polls.saturating_add(1);
            data.retry_after = response.retry_after;
            let elapsed = started.elapsed();
            match response.status {
                200 => {
                    let observed = observe(&request.job, &response)?;
                    let current = data.job.clone().ok_or_else(|| denied("Missing job."))?;
                    if !current.state.allows(observed.state) {
                        data.observation = Some(observed);
                        return Err(CraftError::new(
                            ErrorCode::RemoteUnavailable,
                            "Provider status would move the recorded job backwards; the journal was not changed.",
                        ));
                    }
                    let next = RemoteJob {
                        resource: current.resource.clone(),
                        state: observed.state,
                    };
                    if next.state != current.state {
                        data.job = Some(next.clone());
                        data.journal_current = journal
                            .record_job_state(&request.binding.operation_id, &request.step, &next)
                            .is_ok();
                        data.reconciliation_required =
                            Some(request.submission.requires_reconciliation());
                    }
                    data.observation = Some(observed);
                    if terminal(next.state) {
                        data.stop_reason = StopReason::Terminal;
                        return Ok(());
                    }
                    if !data.journal_current {
                        // Further polling could observe states the journal never saw.
                        data.stop_reason = StopReason::Error;
                        return Err(CraftError::new(
                            ErrorCode::Internal,
                            "The observed job state could not be persisted; it is retained only in this response.",
                        ));
                    }
                    let wait = backoff;
                    if elapsed + wait >= window {
                        data.stop_reason = StopReason::WaitElapsed;
                        return Ok(());
                    }
                    pause(wait, &control)?;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                }
                404 => {
                    data.stop_reason = StopReason::NotVisible;
                    return Ok(());
                }
                429 | 503 => {
                    let wait = advice(response.retry_after, &control).unwrap_or(backoff);
                    if elapsed + wait >= window {
                        data.stop_reason = StopReason::RateLimited;
                        let mut error = CraftError::new(
                            ErrorCode::RateLimited,
                            "The provider limited task status reads; the job remains pending with its handle.",
                        );
                        error.next_action = "Wait for the reported retry time, then run operation wait again. Nothing was resubmitted.".into();
                        return Err(error);
                    }
                    pause(wait, &control)?;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                }
                401 => {
                    return Err(CraftError::new(
                        ErrorCode::AuthRequired,
                        "Task status read was not authorized.",
                    ));
                }
                403 => {
                    return Err(CraftError::new(
                        ErrorCode::PermissionDenied,
                        "Task status read was refused.",
                    ));
                }
                _ => {
                    return Err(CraftError::new(
                        ErrorCode::RemoteUnavailable,
                        "Task status read failed; the recorded job state was not changed.",
                    ));
                }
            }
        }
    })();
    let envelope = match result {
        Ok(()) => Envelope::success(COMMAND, request.request_id.clone(), json!(data)),
        Err(error) => {
            if data.stop_reason != StopReason::RateLimited {
                data.stop_reason = if matches!(
                    error.code,
                    ErrorCode::Cancelled | ErrorCode::DeadlineExceeded
                ) {
                    StopReason::Cancelled
                } else {
                    StopReason::Error
                };
            }
            Envelope::failure_with_data(COMMAND, request.request_id.clone(), error, json!(data))
        }
    };
    CommandResult {
        envelope,
        output_policy: OutputPolicy::PreserveEvidence,
    }
}

/// Returns final local data without any profile, credential or network access.
pub fn local(data: WaitData, request_id: &str) -> CommandResult {
    CommandResult {
        envelope: Envelope::success(COMMAND, request_id.into(), json!(data)),
        output_policy: OutputPolicy::PreserveEvidence,
    }
}
