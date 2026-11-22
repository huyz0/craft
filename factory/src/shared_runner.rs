use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use clap::{Args, Subcommand, ValueEnum};
use craft_factory_evidence::KnowledgeProposal;
pub(crate) use craft_factory_evidence::sha256;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use ureq::{
    Agent,
    tls::{Certificate, RootCerts, TlsConfig},
};

use crate::{
    Result, bitbucket_publish,
    order::{AgentAdapter, WorkOrder},
    read_json, reject, repository, runner, snapshot,
    store::{Receipt, Store},
    twg,
};

#[path = "shared_ci_lane.rs"]
mod ci_lane;
#[path = "shared_github_enqueue.rs"]
pub mod github_enqueue;

const PACK_LIMIT: u64 = 512 * 1024;
pub(crate) const QUEUE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Args)]
pub struct Options {
    #[command(flatten)]
    runner: RunnerOptions,
    #[arg(long)]
    request_id: String,
    #[arg(long)]
    destination: PathBuf,
}

#[derive(Args)]
pub struct LoopOptions {
    #[command(flatten)]
    runner: RunnerOptions,
    /// Existing directory dedicated to this runner and queue.
    #[arg(long)]
    attempt_root: PathBuf,
    #[arg(long, default_value_t = 15, value_parser = clap::value_parser!(u64).range(1..=60))]
    poll_seconds: u64,
    /// Exit after this many polls. Omit to keep polling until stopped.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    max_polls: Option<u64>,
    /// Keep a bounded runner status in .runner-status.json in the attempt root.
    #[arg(long)]
    write_status: bool,
}

#[derive(Args)]
pub struct CheckRunnerOptions {
    #[arg(long)]
    queue: String,
    #[arg(long)]
    ca: Option<PathBuf>,
    /// Read the queue runner token from this protected file.
    #[arg(long)]
    runner_token_file: Option<PathBuf>,
    #[arg(long)]
    repository_map: PathBuf,
    /// Existing directory dedicated to this runner and queue.
    #[arg(long)]
    attempt_root: PathBuf,
    #[command(flatten)]
    adapter: runner::AdapterOptions,
}

#[derive(Args)]
struct RunnerOptions {
    #[arg(long)]
    queue: String,
    #[arg(long)]
    ca: Option<PathBuf>,
    /// Read the queue runner token from this protected file.
    #[arg(long)]
    runner_token_file: Option<PathBuf>,
    #[arg(long)]
    repository_map: PathBuf,
    #[command(flatten)]
    adapter: runner::AdapterOptions,
    #[arg(long, default_value_t = 900, value_parser = clap::value_parser!(u64).range(1..=3600))]
    timeout_seconds: u64,
    #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u64).range(1..=60))]
    renew_seconds: u64,
    /// Invite one optional skill or lesson proposal from each successful worker.
    #[arg(long)]
    collect_knowledge: bool,
}

#[derive(Args)]
pub struct VerificationOptions {
    #[arg(long)]
    queue: String,
    #[arg(long)]
    ca: Option<PathBuf>,
    #[arg(long)]
    work_id: String,
    #[arg(long)]
    head: String,
    #[arg(long)]
    repository_map: PathBuf,
    #[arg(long)]
    destination: PathBuf,
    /// Use the independent verifier credential instead of the operator credential.
    #[arg(long)]
    verifier: bool,
    /// Read the verifier credential from a protected file.
    #[arg(long, requires = "verifier")]
    verifier_token_file: Option<PathBuf>,
}

#[derive(Args)]
pub struct NextVerificationOptions {
    #[arg(long)]
    queue: String,
    #[arg(long)]
    ca: Option<PathBuf>,
    #[arg(long)]
    repository: String,
    #[arg(long)]
    after_work_id: Option<String>,
    #[arg(long)]
    repository_map: PathBuf,
    #[arg(long)]
    destination: PathBuf,
    #[arg(long)]
    verifier_token_file: Option<PathBuf>,
}

#[derive(Deserialize)]
struct PendingCandidate {
    work_id: String,
    repository: String,
    generation: i64,
    head: String,
    pack_digest: String,
    pickup_id: String,
    pickup_expires_at: String,
}

#[derive(Args)]
pub struct AttestationOptions {
    #[arg(long)]
    queue: String,
    #[arg(long)]
    ca: Option<PathBuf>,
    /// Read the verifier credential from a protected file.
    #[arg(long)]
    verifier_token_file: Option<PathBuf>,
    #[arg(long)]
    handoff: PathBuf,
    #[arg(long)]
    result: VerificationResult,
    #[arg(long)]
    evidence: String,
    #[arg(long)]
    note: String,
    /// Optional review-facing summary of what was tested and observed, up to 240 characters.
    #[arg(long)]
    review_summary: Option<String>,
}

#[derive(Args)]
pub struct DependencyAttestationOptions {
    #[arg(long)]
    queue: String,
    #[arg(long)]
    ca: Option<PathBuf>,
    /// Read the verifier credential from a protected file.
    #[arg(long)]
    verifier_token_file: Option<PathBuf>,
    #[arg(long)]
    handoff: PathBuf,
    #[arg(long)]
    report: PathBuf,
    #[arg(long)]
    approval_id: String,
    #[command(flatten)]
    evidence: crate::dependency_compatibility::Inputs,
    #[command(flatten)]
    pins: crate::dependency_compatibility::Pins,
}

#[derive(Args)]
pub struct CutoverOptions {
    #[arg(long)]
    queue: String,
    #[arg(long)]
    ca: Option<PathBuf>,
    /// A stable UUID for this transfer. Reuse it after an interrupted request.
    #[arg(long)]
    epoch: String,
    /// Pause and release the shared queue, then return authority to this local database.
    #[arg(long)]
    rollback: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum VerificationResult {
    Passed,
    Failed,
}

#[derive(Clone, Copy, ValueEnum)]
enum KnowledgeDecision {
    Promote,
    Reject,
}

#[derive(Deserialize)]
struct Handoff {
    schema_version: u32,
    endpoint: String,
    work_id: String,
    generation: i64,
    head: String,
    pack_digest: String,
    verification_sequence: i64,
    status: String,
    pickup_id: Option<String>,
}

#[derive(Deserialize)]
struct PreparedVerification {
    #[serde(flatten)]
    handoff: Handoff,
    repository: String,
    checkout: PathBuf,
}

fn shadow_binding(handoff: &Handoff) -> Value {
    json!({"endpoint": handoff.endpoint, "generation": handoff.generation,
        "pack_digest": handoff.pack_digest, "verification_sequence": handoff.verification_sequence})
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkout {
    path: PathBuf,
    jira_snapshot: Option<PathBuf>,
    github_ci: Option<GithubCiSource>,
    bitbucket_ci: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GithubCiSource {
    policy: PathBuf,
    gh: PathBuf,
}

#[derive(Deserialize)]
struct Claim {
    work_id: String,
    repository: String,
    generation: i64,
    work_order: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RepairContext {
    head: String,
    native_url: String,
    note: String,
    evidence: String,
    digest: String,
    pack_base64: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CandidatePack {
    work_id: String,
    repository: String,
    generation: i64,
    commit: String,
    digest: String,
    pack_base64: String,
    work_order: Value,
    repair_head: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Submission {
    endpoint: String,
    request_id: String,
    work_id: String,
    generation: i64,
    commit: String,
    tokens: Option<u64>,
    digest: String,
    #[serde(default)]
    repaired: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    knowledge_proposal: Option<KnowledgeProposal>,
}

struct Queue {
    origin: String,
    endpoint: String,
    token: String,
    agent: Agent,
}

#[derive(Debug)]
struct QueueRequestError {
    status: Option<u16>,
}

impl std::fmt::Display for QueueRequestError {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.status {
            Some(code) => write!(output, "shared queue rejected the request (HTTP {code})"),
            None => output.write_str("could not reach the shared queue over verified HTTPS"),
        }
    }
}

impl std::error::Error for QueueRequestError {}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum RunnerFailureReason {
    SubmissionMissingOrIncomplete,
    SubmissionBindingMismatch,
    CandidatePackMismatch,
    WorkerFailed,
    LocalFailure,
    QueueUnreachable,
    QueueAuthenticationFailed,
    QueueAccessDenied,
    QueueConflict,
    QueueUnavailable,
    QueueRequestRejected,
}

#[derive(Debug)]
struct LocalRunnerFailure(RunnerFailureReason, Box<dyn std::error::Error>);

impl std::fmt::Display for LocalRunnerFailure {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.1, output)
    }
}

impl std::error::Error for LocalRunnerFailure {}

fn failure_reason(error: &(dyn std::error::Error + 'static)) -> RunnerFailureReason {
    use RunnerFailureReason::*;
    if let Some(queue) = error.downcast_ref::<QueueRequestError>() {
        return match queue.status {
            None => QueueUnreachable,
            Some(401) => QueueAuthenticationFailed,
            Some(403) => QueueAccessDenied,
            Some(409) => QueueConflict,
            Some(408 | 429 | 500..=599) => QueueUnavailable,
            Some(_) => QueueRequestRejected,
        };
    }
    error
        .downcast_ref::<LocalRunnerFailure>()
        .map_or(LocalFailure, |failure| failure.0)
}

#[derive(Args)]
pub struct ReconcileDraftOptions {
    #[arg(long)]
    queue: String,
    #[arg(long)]
    ca: Option<PathBuf>,
    #[arg(long)]
    action_key: String,
    #[arg(long)]
    policy: PathBuf,
    #[arg(long)]
    pull_request: u64,
}

#[derive(Args)]
pub struct BindGithubReviewOptions {
    #[arg(long)]
    queue: String,
    #[arg(long)]
    ca: Option<PathBuf>,
    #[arg(long)]
    handoff: PathBuf,
    #[arg(long)]
    policy: PathBuf,
    #[arg(long)]
    gh: PathBuf,
    #[arg(long)]
    pull_request: u64,
}

#[derive(Args)]
pub struct PublishDraftOptions {
    #[arg(long)]
    queue: String,
    #[arg(long)]
    ca: Option<PathBuf>,
    #[arg(long)]
    action_key: String,
    #[arg(long)]
    repository_map: PathBuf,
    #[arg(long)]
    destination: PathBuf,
    /// Advance the source branch of the existing draft after a verified repair.
    #[arg(long)]
    update_draft: bool,
}

#[derive(Args)]
pub struct ReserveDraftOptions {
    #[arg(long)]
    queue: String,
    #[arg(long)]
    ca: Option<PathBuf>,
    #[arg(long)]
    work_id: String,
    #[arg(long)]
    generation: i64,
    #[arg(long)]
    head: String,
    #[arg(long)]
    verification_sequence: i64,
    /// Bind the reservation to the existing draft for the reviewed revision.
    #[arg(long)]
    update_draft: bool,
}

#[derive(Args)]
pub struct ObserveStandingOptions {
    #[arg(long)]
    queue: String,
    #[arg(long)]
    ca: Option<PathBuf>,
    #[arg(long)]
    rule: String,
    /// Read the relay's queue intake token from a protected file.
    #[arg(long)]
    intake_token_file: Option<PathBuf>,
    /// Normalized source observation and its optional pinned work order.
    observation: PathBuf,
}

#[derive(Args)]
pub struct DiagnoseSharedOptions {
    #[arg(long)]
    queue: String,
    #[arg(long)]
    ca: Option<PathBuf>,
    #[arg(long)]
    case: String,
    #[arg(long)]
    from: u64,
    #[arg(long)]
    to: u64,
}

#[derive(Args)]
pub struct SeriesOptions {
    #[arg(long)]
    queue: String,
    #[arg(long)]
    ca: Option<PathBuf>,
    #[command(subcommand)]
    action: SeriesAction,
}

#[derive(Subcommand)]
enum SeriesAction {
    Register {
        plan: PathBuf,
    },
    /// Register the exact coordinator proposal approved by the release owner.
    RegisterProposal {
        proposal: PathBuf,
        #[arg(long)]
        proposal_digest: String,
        #[arg(long)]
        approval_reference: String,
        #[command(flatten)]
        seed: crate::series_proposal::SeedGuard,
    },
    Status {
        id: String,
    },
    /// Hand an assigned release member to another enrolled observer.
    TransferObserver {
        transfer_id: String,
        #[arg(long)]
        work_id: String,
        #[arg(long)]
        expected_revision: i64,
        #[arg(long)]
        observer_id: String,
        #[arg(long)]
        evidence: String,
    },
    /// Read one assigned release member using the observer credential.
    ObserverStatus {
        work_id: String,
    },
    /// Find work assigned to the authenticated release observer.
    ObserverInbox {
        #[arg(long)]
        repository: Option<String>,
        #[arg(long)]
        after: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: i32,
        #[arg(long)]
        include_closed: bool,
    },
    /// Reload an operator-reviewed replacement at the queue's configured catalog path.
    ReloadRolloutCatalog {
        #[arg(long)]
        digest: String,
    },
    Observe {
        receipt: PathBuf,
    },
    ApproveRollout {
        approval: PathBuf,
    },
    /// Approve a new archive index for an existing release receipt.
    ApproveRolloutRefresh {
        approval: PathBuf,
    },
    ObserveRollout {
        #[arg(long)]
        work_id: String,
        #[arg(long)]
        generation: i64,
        #[arg(long)]
        head: String,
        #[arg(long)]
        verification_sequence: i64,
        #[arg(long)]
        policy: PathBuf,
        #[arg(long)]
        release: PathBuf,
    },
    /// Ask the queue to verify the operator-registered release archive.
    ObserveArchivedRollout {
        #[arg(long)]
        work_id: String,
        #[arg(long)]
        generation: i64,
        #[arg(long)]
        head: String,
        #[arg(long)]
        verification_sequence: i64,
        /// Approved refresh ID when verifying new evidence for an existing release.
        #[arg(long)]
        refresh_id: Option<String>,
        /// Verify archive evidence and report blockers without recording a receipt.
        #[arg(long)]
        check_only: bool,
    },
    Withdraw {
        work_id: String,
        #[arg(long)]
        sequence: i64,
        #[arg(long)]
        evidence: String,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StandingObservation {
    snapshot: Value,
    work_order: Option<Value>,
}

#[derive(Args)]
pub struct RequestRepairOptions {
    #[arg(long)]
    queue: String,
    #[arg(long)]
    ca: Option<PathBuf>,
    #[arg(long)]
    work_id: String,
    #[arg(long)]
    generation: i64,
    #[arg(long)]
    head: String,
    #[arg(long)]
    draft: String,
    #[arg(long)]
    evidence: String,
    #[arg(long)]
    note: String,
}

#[derive(Args)]
pub struct OperationsOptions {
    #[arg(long)]
    queue: String,
    #[arg(long)]
    ca: Option<PathBuf>,
    #[command(subcommand)]
    action: OperationsAction,
}

#[derive(Args)]
pub struct WorkOptions {
    #[arg(long)]
    queue: String,
    #[arg(long)]
    ca: Option<PathBuf>,
    /// Read the operator token from this protected file.
    #[arg(long)]
    operator_token_file: Option<PathBuf>,
    #[command(subcommand)]
    action: WorkAction,
}

#[derive(Subcommand)]
enum WorkAction {
    /// Admit an exact, reviewed work order with a stable event key.
    Admit {
        order: PathBuf,
        #[arg(long)]
        order_digest: String,
        #[arg(long)]
        repository: String,
        #[arg(long)]
        event_key: String,
        #[arg(long, value_parser = clap::value_parser!(i64).range(1..))]
        token_limit: i64,
    },
    /// Admit an exact current GitHub failure into the shared queue after a native read.
    AdmitGithubFailure {
        order: PathBuf,
        #[arg(long)]
        order_digest: String,
        #[arg(long)]
        policy: PathBuf,
        #[arg(long)]
        policy_digest: String,
        #[arg(long)]
        gh: PathBuf,
        #[arg(long)]
        run: u64,
        #[arg(long)]
        attempt: u64,
        #[arg(long)]
        jira_snapshot: Option<PathBuf>,
        #[arg(long, value_parser = clap::value_parser!(i64).range(1..))]
        token_limit: i64,
    },
    /// Admit an exact current Bitbucket failure after reading the native pipeline step.
    AdmitBitbucketFailure {
        order: PathBuf,
        #[arg(long)]
        order_digest: String,
        #[arg(long)]
        policy: PathBuf,
        #[arg(long)]
        policy_digest: String,
        #[arg(long)]
        build: u64,
        #[arg(long)]
        step: String,
        #[arg(long)]
        jira_snapshot: Option<PathBuf>,
        #[arg(long, value_parser = clap::value_parser!(i64).range(1..))]
        token_limit: i64,
    },
    /// Register an immutable CI polling lane with the operator credential.
    ApproveCiLane {
        manifest: PathBuf,
        #[arg(long)]
        digest: String,
        #[arg(long)]
        evidence: String,
    },
    /// Read the approved CI source once using the intake credential.
    PollCiLane {
        manifest: PathBuf,
        #[arg(long)]
        digest: String,
        #[arg(long)]
        intake_token_file: Option<PathBuf>,
        #[arg(long)]
        admit: bool,
    },
    /// Stop new observations and admissions for an approved lane.
    DisableCiLane {
        lane: String,
        #[arg(long)]
        evidence: String,
    },
    /// Read the current state of a shared work order.
    Status { work_id: String },
    /// Cancel shared work and record the reason.
    Cancel {
        work_id: String,
        #[arg(long)]
        reason: String,
    },
    /// Pause new admissions and claims.
    Pause,
    /// Resume admissions and claims, subject to the queue's recovery state.
    Resume,
}

fn read_approved_order(path: &Path, digest: &str) -> Result<WorkOrder> {
    let bytes = craft_factory_evidence::release_trace::read_evidence(path, 64 * 1024)
        .map_err(|error| error.to_string())?;
    if sha256(&bytes) != digest {
        return reject("work order does not match the approved digest");
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn ci_template(path: &Path, digest: &str, jira_snapshot: Option<&Path>) -> Result<WorkOrder> {
    let order = read_approved_order(path, digest)?;
    if order.ci_failure.is_some() || order.dependency_intake_digest.is_some() {
        return reject("provide a CI repair template without CI or dependency observations");
    }
    order.validate()?;
    repository::validate_base(&order)?;
    if snapshot::reference(&order)?.is_some() {
        snapshot::verify(
            &order,
            jira_snapshot.ok_or("provide the approved Jira snapshot for the shared CI order")?,
        )?;
    }
    Ok(order)
}

pub fn shared_work(options: WorkOptions) -> Result<Value> {
    use craft_factory_evidence::queue_identity::{identifier, repository_key};
    let request = match options.action {
        WorkAction::Admit {
            order,
            order_digest,
            repository,
            event_key,
            token_limit,
        } => {
            if !repository_key(&repository) || !work_text(&event_key, 300) {
                return reject(
                    "admission requires a repository key in owner/repo form and a stable event key",
                );
            }
            let mut work_order = read_approved_order(&order, &order_digest)?;
            work_order.validate_shared()?;
            if work_order.ci_failure.is_some() {
                return reject("use the provider-specific shared CI failure admission command");
            }
            if !work_order.repository.is_absolute()
                && work_order.repository.to_str() != Some(repository.as_str())
            {
                return reject("the work order does not match the admission repository");
            }
            admission(&mut work_order, &repository, event_key, token_limit)
        }
        WorkAction::AdmitGithubFailure {
            order,
            order_digest,
            policy,
            policy_digest,
            gh,
            run,
            attempt,
            jira_snapshot,
            token_limit,
        } => {
            let order = ci_template(&order, &order_digest, jira_snapshot.as_deref())?;
            let policy: crate::github::Policy = load_shared_ci_policy(&policy, &policy_digest)?;
            if !repository_key(&policy.repository) {
                return reject("GitHub policy needs a queue repository key in owner/repo form");
            }
            repository::validate_github_remote(&order, &policy.repository)?;
            let failure = crate::order::CiFailure::Github(crate::github::failed_job(
                &policy,
                &gh,
                run,
                attempt,
                &order.base,
            )?);
            ci_admission(order, failure, policy_digest, token_limit)?
        }
        WorkAction::ApproveCiLane {
            manifest,
            digest,
            evidence,
        } => ci_lane::approve(&manifest, &digest, evidence)?,
        WorkAction::PollCiLane {
            manifest,
            digest,
            intake_token_file,
            admit,
        } => {
            return ci_lane::poll(
                &options.queue,
                options.ca.as_deref(),
                intake_token_file.as_deref(),
                &manifest,
                &digest,
                admit,
            );
        }
        WorkAction::DisableCiLane { lane, evidence } => {
            if !identifier(&lane) || !work_text(&evidence, 500) {
                return reject("disabling CI intake requires a valid lane and decision reference");
            }
            json!({"action":"disable_ci_lane", "lane":lane, "evidence":evidence})
        }
        WorkAction::AdmitBitbucketFailure {
            order,
            order_digest,
            policy,
            policy_digest,
            build,
            step,
            jira_snapshot,
            token_limit,
        } => {
            let order = ci_template(&order, &order_digest, jira_snapshot.as_deref())?;
            let policy: crate::twg::Policy = load_shared_ci_policy(&policy, &policy_digest)?;
            let repository = policy.repository_name();
            if !repository_key(&repository) {
                return reject(
                    "Bitbucket policy needs a queue repository key in workspace/repo form",
                );
            }
            repository::validate_bitbucket_remote(&order, &repository)?;
            let failure = crate::order::CiFailure::Bitbucket(crate::twg::failed_step(
                &policy,
                build,
                &step,
                &order.base,
            )?);
            ci_admission(order, failure, policy_digest, token_limit)?
        }
        WorkAction::Status { work_id } => {
            if !identifier(&work_id) {
                return reject("invalid shared work ID");
            }
            json!({"action": "status", "work_id": work_id})
        }
        WorkAction::Cancel { work_id, reason } => {
            if !identifier(&work_id) || !work_text(&reason, 500) {
                return reject("cancellation requires a valid work ID and a reason");
            }
            json!({"action": "cancel", "work_id": work_id, "reason": reason})
        }
        WorkAction::Pause => json!({"action": "set_paused", "paused": true}),
        WorkAction::Resume => json!({"action": "set_paused", "paused": false}),
    };
    if serde_json::to_vec(&request)?.len() > 64 * 1024 {
        return reject("shared work request exceeds the queue's byte limit");
    }
    Queue::open_with_token_file(
        &options.queue,
        options.ca.as_deref(),
        QueueRole::Operator,
        options.operator_token_file.as_deref(),
    )?
    .request_bounded(&request, 256 * 1024)
}

fn admission(
    order: &mut WorkOrder,
    repository: &str,
    event_key: String,
    token_limit: i64,
) -> Value {
    order.repository = PathBuf::from(repository);
    json!({"action": "admit", "repository": repository, "event_key": event_key,
        "work_order": order, "token_limit": token_limit})
}

fn ci_admission(
    mut order: WorkOrder,
    failure: crate::order::CiFailure,
    policy_digest: String,
    token_limit: i64,
) -> Result<Value> {
    let digest = failure.poll_digest()?;
    let repository = failure.repository().to_owned();
    order.id = format!("{}-ci-{digest}", &order.id[..order.id.len().min(32)]);
    order.ci_failure = Some(failure);
    order.ci_policy_digest = Some(policy_digest);
    order.require_native_checks = true;
    order.validate()?;
    Ok(admission(
        &mut order,
        &repository,
        format!("ci:{digest}"),
        token_limit,
    ))
}

fn load_shared_ci_policy<T: serde::de::DeserializeOwned>(path: &Path, digest: &str) -> Result<T> {
    let (policy, actual) = crate::shadow_merge::load(path)?;
    if actual != digest {
        return reject("shared CI policy does not match the approved digest");
    }
    Ok(policy)
}

fn work_text(text: &str, maximum: usize) -> bool {
    !text.trim().is_empty() && text.len() <= maximum && !text.chars().any(char::is_control)
}

#[derive(Subcommand)]
enum OperationsAction {
    Summary,
    /// Read the current draft policy and its recorded operator decision.
    DraftPolicy {
        repository: String,
    },
    /// Configure a draft policy or change its enabled state at the current revision.
    ConfigureDraftPolicy {
        repository: String,
        #[arg(long)]
        base_branch: String,
        #[arg(long)]
        publisher: String,
        #[arg(long)]
        forge: String,
        #[arg(long)]
        expected_revision: i64,
        #[arg(long, action = clap::ArgAction::Set)]
        enabled: bool,
        #[arg(long)]
        evidence: String,
    },
    /// Register approved Markdown for an exact team skill revision.
    RegisterTeamSkill {
        location: String,
        markdown: PathBuf,
        #[arg(long)]
        revision: String,
        #[arg(long)]
        owner: String,
        #[arg(long, required = true)]
        repository: Vec<String>,
        #[arg(long)]
        evidence: String,
    },
    /// Inspect metadata for one exact team skill revision.
    TeamSkillRelease {
        location: String,
        revision: String,
    },
    /// Withdraw one exact team skill revision.
    WithdrawTeamSkill {
        location: String,
        revision: String,
        #[arg(long)]
        evidence: String,
    },
    /// Enable a repository-scoped standing rule from an approved policy.
    #[command(visible_alias = "approve-jira-standing-rule")]
    ApproveStandingRule {
        policy: PathBuf,
        #[arg(long)]
        digest: String,
    },
    /// Withdraw a standing tracker rule and record why.
    #[command(visible_alias = "disable-jira-standing-rule")]
    DisableStandingRule {
        rule_id: String,
        #[arg(long)]
        evidence: String,
    },
    /// Approve the exact replacement intent returned by a tracker relay.
    #[command(visible_alias = "approve-jira-standing-intent")]
    ApproveStandingIntent {
        rule_id: String,
        #[arg(long)]
        issue: String,
        #[arg(long)]
        source_revision: i64,
        #[arg(long)]
        review_token: String,
        #[arg(long)]
        evidence: String,
    },
    /// Read queue health, waiting work, review decisions, and token usage in one snapshot.
    OperatorBrief {
        #[arg(long)]
        repository: Option<String>,
        #[arg(long, default_value_t = 24, value_parser = clap::value_parser!(i32).range(1..=720))]
        usage_hours: i32,
        #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(i32).range(1..=20))]
        limit: i32,
        #[arg(long)]
        json: bool,
    },
    /// Activate a validated replacement of the queue's configured identity file.
    ReloadIdentities {
        #[arg(long)]
        digest: String,
    },
    /// Activate the approved replacement diagnostic catalog and retire superseded grants.
    ReloadDiagnosticCatalog {
        #[arg(long)]
        digest: String,
        #[arg(long)]
        evidence: String,
    },
    /// Approve the exact policy and CI receipts for a shared dependency candidate.
    ApproveDependencyEvidence {
        approval: PathBuf,
    },
    ApproveShadowPolicy {
        repository: String,
        #[arg(long)]
        policy_digest: String,
        #[arg(long)]
        require_native: bool,
        #[arg(long)]
        evidence: String,
        #[arg(long)]
        valid_seconds: i32,
    },
    /// Approve the GitHub policy file and branch protection as one pin.
    ApproveGithubShadowPolicy {
        #[arg(long)]
        policy: PathBuf,
        #[arg(long)]
        approved_protection_digest: String,
        #[arg(long)]
        evidence: String,
        #[arg(long)]
        valid_seconds: i32,
    },
    /// Assess a confirmed shared GitHub PR using the exact candidate pack.
    AssessGithubShadow {
        #[arg(long)]
        work_id: String,
        #[arg(long)]
        head: String,
        #[arg(long)]
        repository_map: PathBuf,
        #[arg(long)]
        destination: PathBuf,
        #[arg(long)]
        policy: PathBuf,
        #[arg(long)]
        gh: PathBuf,
        #[arg(long)]
        approved_protection_digest: String,
    },
    RecordShadowMerge {
        receipt_id: String,
        #[arg(long)]
        handoff: PathBuf,
        #[arg(long)]
        report: PathBuf,
    },
    /// Assess and record one ready GitHub revision with scoped verifier credentials.
    PollGithubReadiness(GithubReadinessOptions),
    RecordShadowOutcome {
        receipt_id: String,
        #[arg(long)]
        head: String,
        #[arg(long)]
        report_digest: String,
        #[arg(long)]
        basis: ShadowBasis,
        #[arg(long)]
        expected: ShadowExpectation,
        #[arg(long)]
        evidence_file: PathBuf,
        #[arg(long)]
        evidence: String,
        #[arg(long)]
        note: String,
    },
    /// Record a GitHub merge observed by an independent verifier.
    ConfirmGithubShadowMerge {
        receipt_id: String,
        #[arg(long)]
        head: String,
        #[arg(long)]
        report_digest: String,
        #[arg(long)]
        policy: PathBuf,
        #[arg(long)]
        gh: PathBuf,
    },
    /// Attach the actual GitHub check runs to a passing shared CI repair verdict.
    AttestGithubCiChecks {
        #[arg(long)]
        handoff: PathBuf,
        #[arg(long)]
        policy: PathBuf,
        #[arg(long)]
        gh: PathBuf,
    },
    /// Attach successful Bitbucket Pipelines steps to a passing shared CI repair verdict.
    AttestBitbucketCiChecks {
        #[arg(long)]
        handoff: PathBuf,
        #[arg(long)]
        policy: PathBuf,
        #[arg(long)]
        build: u64,
    },
    /// Record the exact Bitbucket merge with the independent verifier credential.
    ConfirmBitbucketWorkMerge {
        work_id: String,
        #[arg(long)]
        head: String,
        #[arg(long)]
        policy: PathBuf,
    },
    /// List current drafts awaiting an independent forge merge check.
    PendingMergeChecks {
        repository: String,
        #[arg(long)]
        after_work_id: Option<String>,
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(i32).range(1..=50))]
        limit: i32,
    },
    /// List current candidates awaiting their first independent verdict in one repository.
    PendingVerifications {
        repository: String,
        #[arg(long)]
        after_work_id: Option<String>,
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(i32).range(1..=50))]
        limit: i32,
        #[arg(long)]
        verifier_token_file: Option<PathBuf>,
    },
    /// Return an automatic verification pickup to the queue.
    ReleaseVerificationPickup {
        repository: String,
        pickup_id: String,
        #[arg(long)]
        verifier_token_file: Option<PathBuf>,
    },
    /// Extend an active automatic verification pickup.
    RenewVerificationPickup {
        repository: String,
        pickup_id: String,
        #[arg(long)]
        verifier_token_file: Option<PathBuf>,
    },
    /// Check one bounded page of drafts under a single GitHub policy.
    CheckGithubMerges {
        #[arg(long)]
        policy: PathBuf,
        #[arg(long)]
        gh: PathBuf,
        /// Close standalone work whose acknowledged enqueue opted in under standing policy.
        #[arg(long)]
        close_scheduled: bool,
        #[command(flatten)]
        batch: MergeBatchOptions,
    },
    /// Check one bounded page of drafts under a single Bitbucket policy.
    CheckBitbucketMerges {
        #[arg(long)]
        policy: PathBuf,
        #[arg(long)]
        bitbucket_token_file: Option<PathBuf>,
        #[arg(long)]
        bitbucket_email: Option<String>,
        #[command(flatten)]
        batch: MergeBatchOptions,
    },
    ShadowMergeSummary {
        repository: String,
        /// Include receipts recorded since this Unix timestamp.
        #[arg(long)]
        since: i64,
    },
    ReviewInbox {
        #[arg(long)]
        repository: Option<String>,
        #[arg(long)]
        owner: Option<String>,
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(i32).range(1..=50))]
        limit: i32,
        /// Print a compact human-readable inbox.
        #[arg(long)]
        brief: bool,
        /// Use the repository-scoped reviewer credential.
        #[arg(long)]
        reviewer: bool,
        /// Show only reviews assigned to the authenticated reviewer.
        #[arg(long, requires = "reviewer")]
        assigned_only: bool,
        /// Continue an assigned-only inbox from a previously returned handoff ID.
        #[arg(long, requires = "assigned_only")]
        after: Option<String>,
    },
    /// Register an approved context release or replace its exact active predecessor.
    RegisterProjectContext {
        prepared: PathBuf,
        #[arg(long)]
        replace: Option<String>,
    },
    /// Recheck an approved Confluence excerpt and register it in one step.
    RegisterConfluenceContext {
        policy: PathBuf,
        #[arg(long)]
        excerpt: PathBuf,
        #[arg(long)]
        approval: PathBuf,
        /// Exact active revision to replace, if any.
        #[arg(long)]
        replace: Option<String>,
    },
    /// Compare the published Confluence revision with the active context pin.
    CheckConfluenceContext {
        policy: PathBuf,
        /// Exit unsuccessfully if the source no longer matches the active pin.
        #[arg(long)]
        require_current: bool,
        /// Use the repository-scoped context reader identity.
        #[arg(long)]
        context_reader: bool,
        #[arg(long, requires = "context_reader")]
        context_reader_token_file: Option<PathBuf>,
        /// Keep the last completed check as JSON.
        #[arg(long)]
        report_file: Option<PathBuf>,
    },
    /// List active context metadata or inspect one exact historical revision.
    InspectProjectContext {
        repository: String,
        #[arg(long)]
        location: Option<String>,
        #[arg(long, requires = "location")]
        revision: Option<String>,
    },
    /// Withdraw one exact context revision and record the reason.
    WithdrawProjectContext {
        repository: String,
        location: String,
        revision: String,
        #[arg(long)]
        evidence: String,
    },
    GrantReviewInbox {
        grant_id: String,
        #[arg(long)]
        reviewer_id: String,
        #[arg(long)]
        repository: String,
        #[arg(long)]
        evidence: String,
        #[arg(long)]
        valid_seconds: i32,
    },
    RevokeReviewInbox {
        grant_id: String,
        #[arg(long)]
        evidence: String,
    },
    GrantDiagnosticAccess {
        grant_id: String,
        #[arg(long)]
        diagnostician_id: String,
        #[arg(long)]
        case_id: String,
        #[arg(long)]
        policy_digest: String,
        #[arg(long)]
        from: i64,
        #[arg(long)]
        to: i64,
        #[arg(long)]
        valid_seconds: i32,
        #[arg(long)]
        max_reads: i32,
        #[arg(long)]
        evidence: String,
    },
    RevokeDiagnosticAccess {
        grant_id: String,
        #[arg(long)]
        evidence: String,
    },
    AuditDiagnosticAccess {
        grant_id: String,
    },
    /// List grant metadata for a diagnostic case, with an optional exact policy filter.
    DiagnosticCaseAccess {
        case_id: String,
        #[arg(long)]
        policy_digest: Option<String>,
        #[arg(long)]
        after: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: i32,
    },
    /// Retire an exact diagnostic case policy and revoke all its grants.
    RetireDiagnosticCase {
        case_id: String,
        #[arg(long)]
        policy_digest: String,
        #[arg(long)]
        evidence: String,
    },
    AssignReview {
        handoff_id: String,
        #[arg(long)]
        work_id: String,
        #[arg(long)]
        generation: i64,
        #[arg(long)]
        head: String,
        #[arg(long)]
        verification_sequence: i64,
        #[arg(long)]
        reviewer_id: String,
        #[arg(long)]
        evidence: String,
        /// Queue a repair when this reviewer requests changes on the assigned revision.
        #[arg(long)]
        queue_repair: bool,
    },
    /// Replace one pending assignment on the exact reviewed head.
    ReassignReview {
        handoff_id: String,
        replacement_handoff_id: String,
        #[arg(long)]
        head: String,
        #[arg(long)]
        reviewer_id: String,
        #[arg(long)]
        evidence: String,
        /// Authorize this replacement reviewer to queue a repair.
        #[arg(long)]
        queue_repair: bool,
    },
    /// Assign the current draft to the least-loaded eligible reviewer on an approved roster.
    RouteReview {
        handoff_id: String,
        #[arg(long)]
        work_id: String,
        #[arg(long)]
        generation: i64,
        #[arg(long)]
        head: String,
        #[arg(long)]
        verification_sequence: i64,
        #[arg(long = "reviewer-id", required = true)]
        roster: Vec<String>,
        #[arg(long, value_parser = clap::value_parser!(i32).range(1..=50))]
        max_pending: i32,
        #[arg(long)]
        evidence: String,
        #[arg(long)]
        queue_repair: bool,
    },
    RecordReviewDecision {
        handoff_id: String,
        #[arg(long)]
        head: String,
        #[arg(long)]
        decision: HumanReviewDecision,
        #[arg(long)]
        evidence: String,
        #[arg(long)]
        note: String,
    },
    /// Import your active GitHub change request for an assigned draft.
    ImportGithubReview {
        handoff_id: String,
        #[arg(long)]
        review: u64,
        #[arg(long)]
        policy: PathBuf,
        #[arg(long)]
        gh: PathBuf,
    },
    /// Import your active Bitbucket change request for an assigned draft.
    ImportBitbucketReview {
        handoff_id: String,
        #[arg(long)]
        comment: u64,
        #[arg(long)]
        policy: PathBuf,
    },
    ApplyReviewRepair {
        handoff_id: String,
    },
    RunnerStatus {
        runner_id: String,
    },
    /// Inspect a runner's current enrollment and last decision for one repository.
    RunnerEnrollment {
        runner_id: String,
        repository: String,
    },
    /// Grant or revoke one repository enrollment at its expected revision.
    SetRunnerEnrollment {
        runner_id: String,
        repository: String,
        #[arg(long, value_parser = clap::value_parser!(i64).range(0..i64::MAX))]
        expected_revision: i64,
        #[arg(long)]
        decision: EnrollmentDecision,
        #[arg(long)]
        evidence: String,
    },
    /// Explain what the queue can observe about one work order waiting for a claim.
    DispatchStatus {
        work_id: String,
    },
    /// Requeue an exact CI preflight hold after reviewing why it was held.
    RetryCiPreflight {
        work_id: String,
        #[arg(long)]
        generation: i64,
        #[arg(long)]
        evidence: String,
    },
    /// List current review assignments in order of how long they have waited.
    ReviewWait {
        #[arg(long)]
        repository: Option<String>,
        #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(i32).range(1..=50))]
        limit: i32,
    },
    /// Change the dispatch priority of a waiting work order at its expected revision.
    SetDispatchPriority {
        work_id: String,
        #[arg(long, value_parser = clap::value_parser!(i64).range(0..i64::MAX))]
        expected_revision: i64,
        #[arg(long)]
        priority: DispatchPriority,
        #[arg(long)]
        evidence: String,
    },
    SetRunnerMode {
        runner_id: String,
        #[arg(long)]
        expected_revision: i64,
        #[arg(long)]
        mode: RunnerMode,
        #[arg(long)]
        evidence: String,
    },
    HeldPublications,
    RecoveryStatus,
    Audit {
        work_id: String,
    },
    CompareProfiles {
        pairs: PathBuf,
    },
    ProfileUsage {
        work_id: String,
    },
    /// Close standalone work after an independent verifier records the exact GitHub merge.
    CloseGithubWork {
        work_id: String,
        #[arg(long)]
        merge_receipt_id: String,
        #[arg(long)]
        head: String,
        #[arg(long)]
        evidence: String,
    },
    /// Close standalone work after an independent verifier records the exact Bitbucket merge.
    CloseBitbucketWork {
        work_id: String,
        #[arg(long)]
        publication_key: String,
        #[arg(long)]
        head: String,
        #[arg(long)]
        evidence: String,
    },
    /// Report recent attempts and token usage by repository, adapter, model, and effort.
    QueueUsage {
        #[arg(long)]
        repository: Option<String>,
        #[arg(long, default_value_t = 24, value_parser = clap::value_parser!(i32).range(1..=720))]
        since_hours: i32,
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(i32).range(1..=100))]
        limit: i32,
    },
    EscalateProfile {
        work_id: String,
        #[arg(long)]
        generation: i64,
        #[arg(long)]
        replacement_id: String,
        #[arg(long)]
        profile: PathBuf,
        #[arg(long)]
        evidence: String,
    },
    ApproveProfileFallback {
        work_id: String,
        #[arg(long)]
        replacement_id: String,
        #[arg(long)]
        profile: PathBuf,
        #[arg(long)]
        evidence: String,
        #[arg(long)]
        valid_seconds: i32,
    },
    RevokeProfileFallback {
        work_id: String,
    },
    ApproveClaudeRepository {
        repository: String,
        #[arg(long)]
        approval_id: String,
        #[arg(long)]
        evidence: String,
        #[arg(long)]
        valid_seconds: i32,
    },
    RevokeClaudeRepository {
        approval_id: String,
        #[arg(long)]
        evidence: String,
    },
    ApplyProfileFallbacks,
    ProposeKnowledge {
        proposal_id: String,
        #[arg(long)]
        work_id: String,
        #[arg(long)]
        generation: i64,
        #[arg(long)]
        head: String,
        #[arg(long)]
        proposal: PathBuf,
    },
    PendingKnowledge {
        repository: String,
    },
    InspectKnowledge {
        proposal_id: String,
    },
    ReviewKnowledge {
        proposal_id: String,
        #[arg(long)]
        proposal_digest: String,
        #[arg(long)]
        decision: KnowledgeDecision,
        #[arg(long)]
        owner: String,
        #[arg(long)]
        evidence: String,
        #[arg(long)]
        verification_sequence: Option<i64>,
        /// Approved scope including the originating repository. Repeat for each repository.
        #[arg(long)]
        repository: Vec<String>,
    },
    /// Withdraw the exact knowledge release across its approved repositories.
    WithdrawKnowledge {
        proposal_id: String,
        #[arg(long)]
        release_revision: String,
        #[arg(long)]
        evidence: String,
    },
    BeginRecovery {
        epoch: String,
    },
    FinishRecovery {
        epoch: String,
        #[arg(long)]
        evidence: String,
    },
}

#[derive(Clone, Copy, ValueEnum, Serialize)]
#[serde(rename_all = "snake_case")]
enum RunnerMode {
    Active,
    Draining,
    Stopped,
}

#[derive(Clone, Copy, ValueEnum, Serialize)]
#[serde(rename_all = "snake_case")]
enum DispatchPriority {
    Normal,
    Expedited,
}

#[derive(Clone, Copy, ValueEnum)]
enum EnrollmentDecision {
    Grant,
    Revoke,
}

#[derive(Clone, Copy, ValueEnum, Serialize)]
#[serde(rename_all = "snake_case")]
enum HumanReviewDecision {
    Acknowledged,
    ChangesRequested,
}

#[derive(Clone, Copy, ValueEnum, Serialize)]
#[serde(rename_all = "snake_case")]
enum ShadowBasis {
    Fixture,
    ForgeObserved,
}

#[derive(Clone, Copy, ValueEnum, PartialEq, Eq)]
enum ShadowExpectation {
    Eligible,
    Blocked,
}

enum QueueRole {
    Runner,
    Operator,
    Verifier,
    Reviewer,
    Cutover,
    Publication,
    Publisher,
    Operations,
    ProfileScheduler,
    ContextReader,
    Intake,
    Diagnostics,
    SeriesOperator,
    ReleaseObserver,
    VerifiedRollout,
}

#[derive(Args)]
struct MergeBatchOptions {
    #[arg(long, conflicts_with = "cursor_db")]
    after_work_id: Option<String>,
    /// Require the exact SHA-256 digest of the policy bytes before contacting the queue.
    #[arg(long)]
    policy_digest: Option<String>,
    #[arg(long)]
    verifier_token_file: Option<PathBuf>,
    /// Save the next page position and prevent overlapping use of this file.
    #[arg(long)]
    cursor_db: Option<PathBuf>,
    /// Atomically replace the most recent completed or partial batch report.
    #[arg(long)]
    report_file: Option<PathBuf>,
    #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(i32).range(1..=50))]
    limit: i32,
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(6..=300))]
    budget_seconds: u64,
    #[arg(long, default_value_t = 15, value_parser = clap::value_parser!(u64).range(1..=60))]
    check_seconds: u64,
}

#[derive(Args)]
struct GithubReadinessOptions {
    #[arg(long)]
    repository_map: PathBuf,
    #[arg(long)]
    checkout_root: PathBuf,
    #[arg(long)]
    policy: PathBuf,
    #[arg(long)]
    gh: PathBuf,
    #[arg(long)]
    approved_protection_digest: String,
    #[arg(long)]
    verifier_token_file: Option<PathBuf>,
    #[arg(long)]
    report_file: Option<PathBuf>,
}

impl MergeBatchOptions {
    fn check_policy_digest(&self, digest: &str) -> Result<()> {
        if self
            .policy_digest
            .as_deref()
            .is_some_and(|expected| expected != digest)
        {
            return reject("merge check policy differs from the supplied digest");
        }
        Ok(())
    }
}

impl Queue {
    fn open(origin: &str, ca: Option<&Path>, role: QueueRole) -> Result<Self> {
        Self::open_with_token_file(origin, ca, role, None)
    }

    fn open_with_token_file(
        origin: &str,
        ca: Option<&Path>,
        role: QueueRole,
        token_file: Option<&Path>,
    ) -> Result<Self> {
        let uri: ureq::http::Uri = origin.parse()?;
        if uri.scheme_str() != Some("https")
            || uri
                .authority()
                .is_none_or(|authority| authority.as_str().contains('@'))
            || uri.path() != "/"
            || uri.query().is_some()
            || origin.contains('#')
        {
            return reject("provide a queue HTTPS origin without a path or embedded credentials");
        }
        let (route, variable) = match role {
            QueueRole::Operator => ("operator", "CRAFT_FACTORY_OPERATOR_TOKEN"),
            QueueRole::Runner => ("runner", "CRAFT_FACTORY_RUNNER_TOKEN"),
            QueueRole::Verifier => ("verifier", "CRAFT_FACTORY_VERIFIER_TOKEN"),
            QueueRole::Reviewer => ("reviewer", "CRAFT_FACTORY_REVIEWER_TOKEN"),
            QueueRole::Cutover => ("cutover", "CRAFT_FACTORY_OPERATOR_TOKEN"),
            QueueRole::Publication => ("publication", "CRAFT_FACTORY_OPERATOR_TOKEN"),
            QueueRole::Publisher => ("publisher", "CRAFT_FACTORY_PUBLISHER_TOKEN"),
            QueueRole::Operations => ("operations", "CRAFT_FACTORY_OPERATOR_TOKEN"),
            QueueRole::ProfileScheduler => {
                ("profile-scheduler", "CRAFT_FACTORY_PROFILE_SCHEDULER_TOKEN")
            }
            QueueRole::ContextReader => ("context-reader", "CRAFT_FACTORY_CONTEXT_READER_TOKEN"),
            QueueRole::Intake => ("intake", "CRAFT_FACTORY_INTAKE_TOKEN"),
            QueueRole::Diagnostics => ("diagnostics", "CRAFT_FACTORY_DIAGNOSTIC_TOKEN"),
            QueueRole::SeriesOperator => ("series", "CRAFT_FACTORY_OPERATOR_TOKEN"),
            QueueRole::ReleaseObserver => {
                ("release-observer", "CRAFT_FACTORY_RELEASE_OBSERVER_TOKEN")
            }
            QueueRole::VerifiedRollout => {
                ("series-rollout", "CRAFT_FACTORY_RELEASE_OBSERVER_TOKEN")
            }
        };
        let token = match token_file {
            Some(path) => validate_queue_token(read_queue_token_file(path, variable)?)?,
            None => queue_token(variable)?,
        };
        let mut config = Agent::config_builder()
            .https_only(true)
            .max_redirects(0)
            .proxy(None)
            .timeout_global(Some(QUEUE_TIMEOUT));
        if let Some(ca) = ca {
            let bytes = fs::read(ca)?;
            let root = Certificate::from_pem(&bytes)?;
            config = config.tls_config(
                TlsConfig::builder()
                    .root_certs(RootCerts::new_with_certs(&[root]))
                    .build(),
            );
        }
        let origin = format!("https://{}", uri.authority().unwrap());
        Ok(Self {
            endpoint: format!("{origin}/v1/{route}"),
            origin,
            token,
            agent: config.build().into(),
        })
    }

    fn request(&self, body: &Value) -> Result<Value> {
        self.request_bounded(body, 64 * 1024)
    }

    fn request_bounded(&self, body: &Value, limit: u64) -> Result<Value> {
        let mut response = self
            .agent
            .post(&self.endpoint)
            .header("authorization", format!("Bearer {}", self.token))
            .send_json(body)
            .map_err(|error| QueueRequestError {
                status: match error {
                    ureq::Error::StatusCode(code) => Some(code),
                    _ => None,
                },
            })?;
        if response.status() != 200 {
            return reject("shared queue returned an unexpected status");
        }
        let result: Value = response.body_mut().with_config().limit(limit).read_json()?;
        if result["schema_version"] != 1 {
            return reject("unsupported shared queue response version");
        }
        Ok(result)
    }

    fn upload(&self, directory: &Path, submission: &Submission, pending: bool) -> Result<Value> {
        let pack = read_pack(directory, submission.repaired)?;
        if sha256(&pack) != submission.digest {
            return Err(LocalRunnerFailure(
                RunnerFailureReason::CandidatePackMismatch,
                "local candidate pack changed since the runner prepared the submission".into(),
            )
            .into());
        }
        if pending {
            let committed = directory.join("submission.json");
            if let Err(error) = fs::rename(directory.join("submission.pending"), &committed) {
                // Another launcher may have promoted this same submission first.
                if error.kind() != std::io::ErrorKind::NotFound || !committed.is_file() {
                    return Err(error.into());
                }
            }
        }
        let reply = self.request(&json!({
            "action": "submit_pack", "work_id": submission.work_id, "generation": submission.generation,
            "commit": submission.commit, "pack_base64": STANDARD.encode(pack), "tokens": submission.tokens
        }))?;
        if reply["candidate_evidence"] != submission.digest {
            return reject("shared queue returned an unexpected candidate digest");
        }
        let mut receipt = json!({
            "schema_version": 1, "work_id": submission.work_id, "generation": submission.generation,
            "candidate": submission.commit, "candidate_evidence": submission.digest,
        });
        if let Some(proposal) = &submission.knowledge_proposal {
            let key = format!(
                "{}\n{}\n{}",
                submission.work_id, submission.generation, submission.commit
            );
            let proposal_id = format!("auto-{}", &sha256(key.as_bytes())[7..]);
            let result = self.request(&json!({
                "action": "propose_knowledge", "proposal_id": proposal_id,
                "work_id": submission.work_id, "generation": submission.generation,
                "head": submission.commit, "proposal": proposal,
            }));
            receipt["knowledge_proposal"] = match result {
                Ok(reply) => {
                    let proposal = &reply["proposal"];
                    if proposal["proposal_id"] == proposal_id
                        && proposal["work_id"] == submission.work_id
                        && proposal["generation"] == submission.generation
                        && proposal["head"] == submission.commit
                        && proposal["pack_digest"] == submission.digest
                        && proposal["proposal_digest"]
                            .as_str()
                            .and_then(|digest| digest.strip_prefix("sha256:"))
                            .is_some_and(|hash| hash.len() == 64 && crate::order::valid_oid(hash))
                    {
                        json!({"id": proposal_id, "state": "recorded", "digest": proposal["proposal_digest"]})
                    } else {
                        json!({"id": proposal_id, "state": "unconfirmed"})
                    }
                }
                Err(error)
                    if let Some(status @ (400 | 401 | 403 | 404 | 409 | 413 | 422)) = error
                        .downcast_ref::<QueueRequestError>()
                        .and_then(|error| error.status) =>
                {
                    json!({"id": proposal_id, "state": "rejected", "http_status": status})
                }
                _ => json!({"id": proposal_id, "state": "unconfirmed"}),
            };
        }
        Ok(receipt)
    }
}

fn queue_token(variable: &str) -> Result<String> {
    let token = match std::env::var(variable) {
        Ok(token) => token,
        Err(std::env::VarError::NotUnicode(_)) => return reject("invalid shared queue credential"),
        Err(std::env::VarError::NotPresent) => {
            let file_variable = format!("{variable}_FILE");
            let path = crate::evidence_capture::credential_path(variable).ok_or_else(|| format!("{variable}, {file_variable}, or the corresponding service credential is required"))?;
            read_queue_token_file(&path, variable)?
        }
    };
    validate_queue_token(token)
}

fn read_queue_token_file(path: &Path, variable: &str) -> Result<String> {
    let bytes = craft_factory_evidence::release_trace::read_evidence(path, 258)
        .map_err(|_| format!("could not read a bounded credential file for {variable}"))?;
    let text = std::str::from_utf8(&bytes).map_err(|_| "invalid shared queue credential")?;
    Ok(text
        .strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text)
        .to_owned())
}

fn validate_queue_token(token: String) -> Result<String> {
    if !(32..=256).contains(&token.len()) || !token.bytes().all(|byte| byte.is_ascii_graphic()) {
        return reject("invalid shared queue credential");
    }
    Ok(token)
}

pub fn run(options: Options) -> Result<Value> {
    let queue = Queue::open_with_token_file(
        &options.runner.queue,
        options.runner.ca.as_deref(),
        QueueRole::Runner,
        options.runner.runner_token_file.as_deref(),
    )?;
    run_attempt(
        &queue,
        &options.runner,
        &options.request_id,
        &options.destination,
    )
}

pub fn run_loop(options: LoopOptions) -> Result<()> {
    let stopping = Arc::new(AtomicBool::new(false));
    #[cfg(unix)]
    {
        signal_hook::flag::register(signal_hook::consts::SIGTERM, stopping.clone())?;
        signal_hook::flag::register(signal_hook::consts::SIGINT, stopping.clone())?;
    }
    let checkouts: BTreeMap<String, Checkout> = read_json(options.runner.repository_map.clone())?;
    let root = validate_attempt_root(&options.attempt_root, &checkouts)?;
    let lock = File::options()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join(".runner-loop.lock"))?;
    lock.try_lock()
        .map_err(|_| "a runner loop already owns this attempt root")?;
    let mut status = options.write_status.then(|| RunnerStatus {
        path: root.join(".runner-status.json"),
        started_at: time::OffsetDateTime::now_utc().unix_timestamp(),
        last_poll: Value::Null,
        last_failed_poll: None,
        last_failed_run: None,
    });
    if let Some(status) = &mut status {
        status.load_previous()?;
        status.save("running", None)?;
    }
    let result = poll_loop(&options, &root, &stopping, status.as_mut());
    if let Some(status) = &status {
        let final_report = match &result {
            Ok(()) => status.save("stopped", None),
            Err(error) => status.save("failed", Some(error.as_ref())),
        };
        return result.and(final_report);
    }
    result
}

struct RunnerStatus {
    path: PathBuf,
    started_at: i64,
    last_poll: Value,
    last_failed_poll: Option<FailedPoll>,
    last_failed_run: Option<FailedRun>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum FailedPollState {
    AttemptFailed,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FailedPoll {
    state: FailedPollState,
    request_id: String,
    http_status: Option<u16>,
    reason: RunnerFailureReason,
    completed_at: i64,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FailedRun {
    http_status: Option<u16>,
    reason: RunnerFailureReason,
    failed_at: i64,
}

fn valid_http_status(status: Option<u16>) -> bool {
    status.is_none_or(|code| (100..=599).contains(&code))
}

impl RunnerStatus {
    fn load_previous(&mut self) -> Result<()> {
        match fs::symlink_metadata(&self.path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
            Ok(metadata) => {
                if metadata.is_file() && metadata.len() <= 4096 {
                    let previous =
                        serde_json::from_reader::<_, Value>(File::open(&self.path)?.take(4096));
                    if let Ok(report) = previous
                        && report["kind"] == "runner_status"
                    {
                        if report["schema_version"] == 1 {
                            self.last_failed_poll = serde_json::from_value::<FailedPoll>(
                                report["last_failed_poll"].clone(),
                            )
                            .ok()
                            .filter(|failure| {
                                valid_request_id(&failure.request_id)
                                    && valid_http_status(failure.http_status)
                                    && failure.completed_at > 0
                            });
                            // Older reports recorded the most recent process failure only at the top level.
                            let failure = if report["state"] == "failed" {
                                json!({
                                    "http_status": report["http_status"],
                                    "reason": report["failure_reason"],
                                    "failed_at": report["updated_at"],
                                })
                            } else {
                                report["last_failed_run"].clone()
                            };
                            self.last_failed_run = serde_json::from_value::<FailedRun>(failure)
                                .ok()
                                .filter(|failure| {
                                    valid_http_status(failure.http_status) && failure.failed_at > 0
                                });
                        }
                        return Ok(());
                    }
                }
                reject(".runner-status.json already exists and is not a runner status report")
            }
        }
    }

    fn save(&self, state: &str, error: Option<&(dyn std::error::Error + 'static)>) -> Result<()> {
        let updated_at = time::OffsetDateTime::now_utc().unix_timestamp();
        let current_failure = error.map(|error| FailedRun {
            http_status: queue_status(error),
            reason: failure_reason(error),
            failed_at: updated_at,
        });
        let report = json!({
            "schema_version": 1,
            "kind": "runner_status",
            "state": state,
            "started_at": self.started_at,
            "updated_at": updated_at,
            "http_status": current_failure.as_ref().and_then(|failure| failure.http_status),
            "failure_reason": current_failure.as_ref().map(|failure| failure.reason),
            "last_poll": self.last_poll,
            "last_failed_poll": self.last_failed_poll,
            "last_failed_run": current_failure.as_ref().or(self.last_failed_run.as_ref()),
        });
        crate::evidence_capture::replace_json(&self.path, &report)
    }
}

fn queue_status(error: &(dyn std::error::Error + 'static)) -> Option<u16> {
    error
        .downcast_ref::<QueueRequestError>()
        .and_then(|error| error.status)
}

fn poll_loop(
    options: &LoopOptions,
    root: &Path,
    stopping: &AtomicBool,
    mut status: Option<&mut RunnerStatus>,
) -> Result<()> {
    options.runner.adapter.resolve()?;
    let queue = Queue::open_with_token_file(
        &options.runner.queue,
        options.runner.ca.as_deref(),
        QueueRole::Runner,
        options.runner.runner_token_file.as_deref(),
    )?;
    let pending = root.join(".pending");
    fs::create_dir_all(&pending)?;

    for poll in 1..=options.max_polls.unwrap_or(u64::MAX) {
        if stopping.load(Ordering::Relaxed) || root.join(".stop").try_exists()? {
            break;
        }
        let (request_id, marker) = pending_request(&pending)?;
        let destination = root.join(&request_id);
        let result = run_attempt(&queue, &options.runner, &request_id, &destination);
        let (keep_pending, event) = match result {
            Ok(receipt) if receipt["candidate"].is_string() => {
                (false, json!({"state": "submitted", "receipt": receipt}))
            }
            Ok(_) => (false, json!({"state": "idle"})),
            Err(error) => {
                let status = error
                    .downcast_ref::<QueueRequestError>()
                    .map(|error| error.status);
                if matches!(status, Some(Some(401 | 403))) {
                    return Err(error);
                }
                let prepared = destination.join("submission.json").try_exists()?;
                let retryable = matches!(status, Some(None | Some(408 | 429 | 500..=599)));
                let reason = failure_reason(error.as_ref());
                if retryable && (prepared || !destination.try_exists()?) {
                    (
                        true,
                        json!({"state": "retrying", "status": status.flatten(), "reason": reason}),
                    )
                } else if prepared && status != Some(Some(409)) {
                    return Err(error);
                } else {
                    (
                        false,
                        json!({"state": "attempt_failed", "status": status.flatten(), "reason": reason}),
                    )
                }
            }
        };
        if !keep_pending {
            fs::remove_dir(marker)?;
        }
        let submitted = event["state"] == "submitted";
        let mut record = event;
        record["schema_version"] = json!(1);
        record["request_id"] = json!(request_id);
        if let Some(status) = status.as_deref_mut() {
            status.last_poll = json!({
                "state": record["state"],
                "request_id": request_id,
                "http_status": record["status"],
                "reason": record["reason"],
                "completed_at": time::OffsetDateTime::now_utc().unix_timestamp(),
            });
            if record["state"] == "attempt_failed" {
                status.last_failed_poll = Some(serde_json::from_value(status.last_poll.clone())?);
            }
            status.save("running", None)?;
        }
        {
            let mut output = std::io::stdout().lock();
            serde_json::to_writer(&mut output, &record)?;
            writeln!(output)?;
            output.flush()?;
        }
        if stopping.load(Ordering::Relaxed) {
            break;
        }
        if !submitted && options.max_polls != Some(poll) {
            std::thread::sleep(Duration::from_secs(options.poll_seconds));
        }
    }
    Ok(())
}

fn validate_attempt_root(root: &Path, checkouts: &BTreeMap<String, Checkout>) -> Result<PathBuf> {
    if !root.is_absolute() {
        return reject("the runner requires an absolute attempt root");
    }
    let root = root.canonicalize()?;
    if !root.is_dir() {
        return reject("the runner requires an existing attempt directory");
    }
    if checkouts.values().any(|checkout| {
        checkout
            .path
            .canonicalize()
            .is_ok_and(|source| root.starts_with(source))
    }) {
        return reject("the attempt root must be outside all mapped checkouts");
    }
    Ok(root)
}

pub fn check_runner(options: CheckRunnerOptions) -> Result<Value> {
    let checkouts: BTreeMap<String, Checkout> = read_json(options.repository_map)?;
    validate_attempt_root(&options.attempt_root, &checkouts)?;
    let adapter = options.adapter.resolve()?.adapter();
    let queue = Queue::open_with_token_file(
        &options.queue,
        options.ca.as_deref(),
        QueueRole::Runner,
        options.runner_token_file.as_deref(),
    )?;
    let response = queue.request_bounded(
        &json!({
            "action": "check_enrollment",
            "mapped_repositories": checkouts.keys().collect::<Vec<_>>(),
        }),
        256 * 1024,
    )?;
    let setup = &response["setup"];
    let enrolled = setup["enrolled_repositories"]
        .as_array()
        .ok_or("queue did not return the runner's enrolled repositories")?;
    let claude_approved = match adapter {
        AgentAdapter::Codex => None,
        AgentAdapter::Claude => Some(
            setup["claude_approved_repositories"]
                .as_array()
                .ok_or("queue did not return Claude approval status; upgrade the shared queue")?,
        ),
    };
    let mut usable = 0;
    let repositories = checkouts
        .iter()
        .map(|(name, checkout)| {
            let enrolled = enrolled.iter().any(|repo| repo == name.as_str());
            let available = repository::available_worktree(&checkout.path);
            let adapter_approved = claude_approved
                .is_none_or(|approved| approved.iter().any(|repo| repo == name.as_str()));
            usable += usize::from(enrolled && available && adapter_approved);
            json!({
                "repository": name,
                "enrolled": enrolled,
                "checkout_available": available,
                "adapter_approved": adapter_approved,
                "jira_snapshot_file_available": checkout.jira_snapshot.as_ref()
                    .map(|path| path.is_absolute() && path.is_file()),
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "schema_version": 1, "ready": setup["mode"] == "active" && usable > 0,
        "runner_id": setup["runner_id"], "runner_mode": setup["mode"], "queue_paused": setup["queue_paused"],
        "adapter": adapter.as_str(), "usable_repositories": usable, "repositories": repositories,
    }))
}

fn valid_request_id(request: &str) -> bool {
    request.split('-').map(str::len).eq([8, 4, 4, 4, 12])
        && request
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
}

fn pending_request(pending: &Path) -> Result<(String, PathBuf)> {
    let mut entries = fs::read_dir(pending)?;
    if let Some(entry) = entries.next().transpose()? {
        let request = entry
            .file_name()
            .into_string()
            .map_err(|_| "invalid pending request ID")?;
        if entries.next().transpose()?.is_some()
            || !entry.file_type()?.is_dir()
            || !valid_request_id(&request)
        {
            return reject("invalid runner loop journal");
        }
        return Ok((request, entry.path()));
    }
    let mut bytes = [0; 16];
    getrandom::getrandom(&mut bytes).map_err(|_| "could not generate a queue request ID")?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = format!("{:032x}", u128::from_be_bytes(bytes));
    let request = format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    );
    let marker = pending.join(&request);
    fs::create_dir(&marker)?;
    Ok((request, marker))
}

fn run_attempt(
    queue: &Queue,
    options: &RunnerOptions,
    request_id: &str,
    destination: &Path,
) -> Result<Value> {
    if !destination.is_absolute() {
        return reject("shared runner destination must be absolute");
    }
    let submission_path = destination.join("submission.json");
    if destination.exists() {
        let pending = !submission_path.try_exists()?;
        let submission: Submission = if pending {
            read_json(destination.join("submission.pending"))
        } else {
            read_json(submission_path)
        }
        .map_err(|_| LocalRunnerFailure(
            RunnerFailureReason::SubmissionMissingOrIncomplete,
            "existing attempt has no prepared submission; use a new request and directory after lease expiry".into(),
        ))?;
        if submission.request_id != request_id || submission.endpoint != queue.endpoint {
            return Err(LocalRunnerFailure(
                RunnerFailureReason::SubmissionBindingMismatch,
                "submission replay must use the original queue and request ID".into(),
            )
            .into());
        }
        return queue.upload(destination, &submission, pending);
    }
    let checkouts: BTreeMap<String, Checkout> = read_json(options.repository_map.clone())?;
    let runtime = options.adapter.resolve()?;
    let adapter = runtime.adapter();
    let available: Vec<_> = checkouts
        .iter()
        .filter(|(_, checkout)| repository::available_worktree(&checkout.path))
        .map(|(repository, _)| repository)
        .collect();
    let response = queue.request(&json!({
        "action": "claim", "request_id": request_id,
        "context_capable": true, "profile_capable": true, "available_repositories": available,
        "agent_adapter": adapter.as_str(),
    }))?;
    let claim: Option<Claim> = serde_json::from_value(
        response
            .get("claim")
            .ok_or("shared queue response did not contain a claim decision")?
            .clone(),
    )?;
    let Some(mut claim) = claim else {
        return Ok(json!({"schema_version": 1, "claim": null}));
    };
    if !available.contains(&&claim.repository) {
        return reject("queue returned a repository outside the available checkout scope");
    }
    let checkout = checkouts
        .get(&claim.repository)
        .ok_or("no local checkout is mapped for the claimed repository")?;
    let repair = claim
        .work_order
        .as_object_mut()
        .ok_or("shared work order is not a JSON object")?
        .remove("shared_repair")
        == Some(Value::Bool(true));
    let mut order = bind_checkout(
        claim.work_order,
        checkout,
        &claim.work_id,
        &claim.repository,
        claim.generation,
    )?;
    if order.agent_adapter != adapter {
        return reject("queue returned an incompatible agent adapter");
    }
    let team_skill = if order.skill.location.starts_with("team:") {
        let context = queue.request_bounded(
            &json!({"action": "team_skill", "work_id": claim.work_id, "generation": claim.generation}),
            224 * 1024,
        )?;
        let skill: runner::TeamSkill = serde_json::from_value(context["skill"].clone())?;
        if skill.location != order.skill.location
            || skill.revision != order.skill.revision
            || skill.markdown.len() > 32 * 1024
            || skill.markdown.trim().is_empty()
            || sha256(skill.markdown.as_bytes()) != skill.revision
        {
            return reject("approved skill content did not match the work order pin");
        }
        Some(skill)
    } else {
        None
    };
    let mut expected_context: BTreeMap<_, _> = order
        .context
        .iter()
        .filter(|entry| entry.location.starts_with("project:"))
        .map(|entry| (entry.location.as_str(), entry.revision.as_str()))
        .collect();
    let project_context: Vec<runner::ProjectContext> = if expected_context.is_empty() {
        Vec::new()
    } else {
        let response = queue.request_bounded(
            &json!({"action": "project_context", "work_id": claim.work_id, "generation": claim.generation}),
            128 * 1024,
        )?;
        let entries: Vec<runner::ProjectContext> =
            serde_json::from_value(response["context"].clone())?;
        for entry in &entries {
            let canonical = [
                entry.kind.as_str(),
                &entry.source_reference,
                &entry.source_revision,
                &entry.approval_reference,
                &entry.excerpt,
            ]
            .join("\n");
            if expected_context.remove(entry.location.as_str()) != Some(entry.revision.as_str())
                || entry.excerpt.len() > 4096
                || entry.excerpt.trim().is_empty()
                || sha256(canonical.as_bytes()) != entry.revision
            {
                return reject("approved project context did not match the work order pins");
            }
        }
        if !expected_context.is_empty() {
            return reject("approved project context is incomplete");
        }
        entries
    };
    let source = order.repository.canonicalize()?;
    let parent = destination
        .parent()
        .ok_or("attempt must have a parent directory")?
        .canonicalize()?;
    if parent.starts_with(&source) {
        return reject("shared runner destination must be outside the trusted checkout");
    }
    if let Err(error) = recheck_shared_ci(&order, checkout) {
        let hold = error
            .downcast_ref::<crate::github::ObsoleteFailure>()
            .map(crate::github::ObsoleteFailure::reason)
            .or_else(|| {
                error
                    .downcast_ref::<crate::twg::ObsoleteFailure>()
                    .map(crate::twg::ObsoleteFailure::reason)
            });
        if let Some(reason) = hold {
            let report = queue.request(&json!({
                "action": "hold_obsolete_ci", "work_id": claim.work_id, "generation": claim.generation,
                "reason": reason,
            }));
            return match report {
                Ok(_) => reject(format!("shared CI work {} is held. {error}", claim.work_id)),
                Err(report_error) => reject(format!(
                    "shared CI work {} could not record its hold. {error}. {report_error}",
                    claim.work_id,
                )),
            };
        }
        return Err(error);
    }
    fs::create_dir(destination)?;
    let shared_repair = if repair {
        let context = queue.request_bounded(&json!({
            "action": "repair_context", "work_id": claim.work_id, "generation": claim.generation,
        }), 800 * 1024)?;
        let previous: RepairContext = serde_json::from_value(context["repair"].clone())?;
        let pack = STANDARD.decode(previous.pack_base64)?;
        if pack.len() as u64 > PACK_LIMIT
            || !pack.starts_with(b"PACK")
            || sha256(&pack) != previous.digest
        {
            return reject("reviewed predecessor pack did not match the repair context");
        }
        order.repository = repository::prepare_pack_for_verification(
            &order,
            &destination.join("reviewed-revision"),
            &previous.head,
            &pack,
            None,
        )?;
        Some(runner::SharedRepair {
            head: previous.head.clone(),
            feedback: json!({"head": previous.head, "draft": previous.native_url, "evidence": previous.evidence, "note": previous.note}),
        })
    } else {
        None
    };
    let state = destination.join("state.db");
    Store::open(&state)?.delegate(order, checkout.jira_snapshot.as_deref())?;
    let mut owns_start = false;
    let mut last_renewal = None::<Instant>;
    let renew_interval = Duration::from_secs(options.renew_seconds);
    let result = runner::run_guarded(
        &state,
        &claim.work_id,
        runner::Options {
            destination: destination.join("work"),
            runtime,
            timeout: Duration::from_secs(options.timeout_seconds),
            shared_repair,
            team_skill,
            project_context,
            collect_knowledge: options.collect_knowledge,
        },
        |force_check| {
            if force_check || last_renewal.is_none_or(|time| time.elapsed() >= renew_interval) {
                let sent_at = Instant::now();
                if !owns_start {
                    queue.request(&json!({
                        "action": "start_attempt",
                        "work_id": claim.work_id, "generation": claim.generation
                    }))?;
                    owns_start = true;
                }
                queue.request(&json!({
                    "action": "renew", "work_id": claim.work_id, "generation": claim.generation
                }))?;
                last_renewal = Some(sent_at);
            }
            Ok(())
        },
    );
    let (receipt, proposal) = match result {
        Ok(value) => value,
        Err(error) => {
            // A launcher that lost the start race must not stop the owner's lease.
            if owns_start {
                let tokens = Store::open(&state)
                    .and_then(|mut store| store.status(&claim.work_id))
                    .and_then(|receipt| observed_tokens(&receipt))
                    .ok()
                    .flatten()
                    .and_then(|tokens| i64::try_from(tokens).ok());
                let _ = queue.request(&json!({
                    "action": "finish_failed_attempt", "work_id": claim.work_id,
                    "generation": claim.generation, "tokens": tokens,
                }));
            }
            return if error.is::<QueueRequestError>() {
                Err(error)
            } else {
                Err(LocalRunnerFailure(RunnerFailureReason::WorkerFailed, error).into())
            };
        }
    };
    let tokens = observed_tokens(&receipt)?;
    let candidate = receipt
        .candidate
        .ok_or("shared runner did not produce a candidate")?;
    if repair {
        repository::write_candidate_pack(
            &receipt.work_order,
            &candidate,
            &receipt.work_order.base,
            &destination.join("repair.pack"),
        )?;
    }
    let submission = Submission {
        endpoint: queue.endpoint.clone(),
        request_id: request_id.to_owned(),
        work_id: claim.work_id,
        generation: claim.generation,
        commit: candidate,
        tokens,
        digest: sha256(&read_pack(destination, repair)?),
        repaired: repair,
        knowledge_proposal: proposal,
    };
    let temporary = destination.join("submission.pending");
    let file = File::create_new(&temporary)?;
    serde_json::to_writer(&file, &submission)?;
    file.sync_all()?;
    drop(file);
    queue.upload(destination, &submission, true)
}

fn observed_tokens(receipt: &Receipt) -> Result<Option<u64>> {
    let usage = receipt
        .events
        .iter()
        .rev()
        .find(|event| event.kind == "runner_observed")
        .map(|event| serde_json::from_str::<Value>(&event.note))
        .transpose()?;
    Ok(usage.and_then(|usage| {
        usage["input_tokens"]
            .as_u64()?
            .checked_add(usage["output_tokens"].as_u64()?)
    }))
}

pub fn prepare_verification(options: VerificationOptions) -> Result<Value> {
    let role = if options.verifier {
        QueueRole::Verifier
    } else {
        QueueRole::Operator
    };
    let queue = Queue::open_with_token_file(
        &options.queue,
        options.ca.as_deref(),
        role,
        options.verifier_token_file.as_deref(),
    )?;
    prepare_verification_at(
        &queue,
        &options.work_id,
        &options.head,
        options.repository_map,
        options.destination,
        None,
    )
}

pub fn prepare_next_verification(options: NextVerificationOptions) -> Result<Value> {
    if !options.destination.is_absolute() {
        return reject("prepare verification with an absolute new directory");
    }
    let queue = Queue::open_with_token_file(
        &options.queue,
        options.ca.as_deref(),
        QueueRole::Verifier,
        options.verifier_token_file.as_deref(),
    )?;
    let response = queue.request(&json!({
        "action": "claim_pending_verification", "repository": options.repository,
        "after_work_id": options.after_work_id,
    }))?;
    let picked = response
        .get("verification_pickup")
        .ok_or("missing verification pickup response")?;
    if picked.is_null() {
        return Ok(
            json!({"schema_version": 1, "status": "idle", "repository": options.repository}),
        );
    }
    let target: PendingCandidate = serde_json::from_value(picked.clone())?;
    if target.repository != options.repository {
        return reject("the verification pickup did not match the requested repository");
    }
    let handoff = prepare_verification_at(
        &queue,
        &target.work_id,
        &target.head,
        options.repository_map,
        options.destination,
        Some(&target),
    );
    if handoff.is_err() {
        let _ = queue.request(&json!({"action": "release_verification_pickup",
            "repository": target.repository, "pickup_id": target.pickup_id}));
    }
    handoff
}

fn prepare_verification_at(
    queue: &Queue,
    work_id: &str,
    head: &str,
    repository_map: PathBuf,
    destination: PathBuf,
    pending: Option<&PendingCandidate>,
) -> Result<Value> {
    if !destination.is_absolute() || !crate::order::valid_oid(head) {
        return reject(
            "prepare verification with an absolute new directory and a full candidate SHA",
        );
    }
    let checkouts: BTreeMap<String, Checkout> = read_json(repository_map)?;
    let response = queue.request_bounded(
        &json!({"action": "download_pack", "work_id": work_id}),
        800 * 1024,
    )?;
    let artifact: CandidatePack = serde_json::from_value(response["candidate"].clone())?;
    if artifact.work_id != work_id
        || artifact.commit != head
        || pending.is_some_and(|target| {
            artifact.repository != target.repository
                || artifact.generation != target.generation
                || artifact.digest != target.pack_digest
        })
    {
        return reject("the requested candidate is no longer current");
    }
    let mapped = checkouts
        .get(&artifact.repository)
        .ok_or("no trusted checkout is mapped for the candidate repository")?;
    let mut order = bind_checkout(
        artifact.work_order,
        mapped,
        &artifact.work_id,
        &artifact.repository,
        artifact.generation,
    )?;
    repository::validate_ci_anchor(&order)?;
    if snapshot::reference(&order)?.is_some() {
        snapshot::verify(
            &order,
            mapped
                .jira_snapshot
                .as_deref()
                .ok_or("map the approved Jira snapshot before verification")?,
        )?;
    }
    let pack = STANDARD.decode(&artifact.pack_base64)?;
    if pack.len() as u64 > PACK_LIMIT
        || !pack.starts_with(b"PACK")
        || sha256(&pack) != artifact.digest
    {
        return reject("downloaded candidate pack failed size, format, or digest validation");
    }
    let checkout = repository::prepare_pack_for_verification(
        &order,
        &destination,
        &artifact.commit,
        &pack,
        artifact.repair_head.as_deref(),
    )?;
    order.repository = checkout.clone();
    let mut brief = Vec::new();
    crate::review::write_candidate_summary(&mut brief, &order, &artifact.commit)?;
    if let Some(prior) = &artifact.repair_head {
        writeln!(brief, "\n### Repair comparison\n")?;
        crate::review::write_repair_comparison(&mut brief, &order, prior, &artifact.commit)?;
    }
    writeln!(
        brief,
        "\n### Independent checks\n\nRun the approved checks in the prepared checkout and record observed evidence with `handoff.json`.\n\nQueue generation {} · candidate pack `{}`",
        artifact.generation, artifact.digest
    )?;
    let current = queue.request(&json!({"action": "status", "work_id": work_id}))?;
    let current = &current["work"];
    if current["state"] != "candidate"
        || current["id"] != artifact.work_id
        || current["repository"] != artifact.repository
        || current["generation"] != artifact.generation
        || current["candidate"] != artifact.commit
        || current["candidate_evidence"] != artifact.digest
        || (pending.is_some() && !current["verification"].is_null())
    {
        return reject("the candidate changed while preparing verification");
    }
    let work_order_path = destination.join("work-order.json");
    serde_json::to_writer_pretty(File::create_new(&work_order_path)?, &order)?;
    let review_brief_path = destination.join("review-brief.md");
    File::create_new(&review_brief_path)?.write_all(&brief)?;
    let mut handoff = json!({
        "schema_version": 1, "work_id": artifact.work_id, "repository": artifact.repository,
        "generation": artifact.generation, "base": order.base, "head": artifact.commit,
        "pack_digest": artifact.digest, "checkout": checkout, "work_order": work_order_path,
        "review_brief": review_brief_path, "review_brief_digest": sha256(&brief),
        "endpoint": queue.origin,
        "verification_sequence": current["verification"]["sequence"].as_i64().unwrap_or(0),
        "status": "ready_for_independent_checks",
    });
    if let Some(target) = pending {
        handoff["pickup_id"] = json!(target.pickup_id);
        handoff["pickup_expires_at"] = json!(target.pickup_expires_at);
    }
    serde_json::to_writer_pretty(
        File::create_new(destination.join("handoff.json"))?,
        &handoff,
    )?;
    Ok(handoff)
}

pub fn attest_verification(options: AttestationOptions) -> Result<Value> {
    let queue = Queue::open_with_token_file(
        &options.queue,
        options.ca.as_deref(),
        QueueRole::Verifier,
        options.verifier_token_file.as_deref(),
    )?;
    let prepared: PreparedVerification = read_json(options.handoff)?;
    let handoff = prepared.handoff;
    if handoff.schema_version != 1
        || handoff.endpoint != queue.origin
        || handoff.status != "ready_for_independent_checks"
    {
        return reject("verification requires a current handoff from this shared queue");
    }
    repository::validate_verification_checkout(&prepared.checkout, &handoff.head)?;
    let mut request = json!({
        "action": "record_verification", "work_id": handoff.work_id, "generation": handoff.generation,
        "head": handoff.head, "digest": handoff.pack_digest, "previous_sequence": handoff.verification_sequence,
        "passed": matches!(options.result, VerificationResult::Passed),
        "evidence": options.evidence, "note": options.note,
    });
    if let Some(summary) = options.review_summary {
        request["review_summary"] = json!(summary);
    }
    if let Some(pickup_id) = handoff.pickup_id {
        request["pickup_id"] = json!(pickup_id);
    }
    queue.request(&request)
}

pub fn attest_dependency_verification(options: DependencyAttestationOptions) -> Result<Value> {
    let queue = Queue::open_with_token_file(
        &options.queue,
        options.ca.as_deref(),
        QueueRole::Verifier,
        options.verifier_token_file.as_deref(),
    )?;
    let prepared: PreparedVerification = read_json(options.handoff)?;
    let handoff = prepared.handoff;
    if handoff.schema_version != 1
        || handoff.endpoint != queue.origin
        || handoff.status != "ready_for_independent_checks"
    {
        return reject("dependency verification requires a current handoff from this shared queue");
    }
    repository::validate_verification_checkout(&prepared.checkout, &handoff.head)?;
    let report = crate::dependency_compatibility::run_pinned(
        options.evidence,
        &prepared.checkout,
        &options.pins,
    )?;
    if report.work_order_id != handoff.work_id
        || report.candidate.commit != handoff.head
        || report.repository != prepared.repository
    {
        return reject("dependency evidence does not match the candidate handoff");
    }
    let bytes = serde_json::to_vec_pretty(&report)?;
    match File::create_new(&options.report) {
        Ok(mut file) => file.write_all(&bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let saved =
                craft_factory_evidence::release_trace::read_evidence(&options.report, 256 * 1024)
                    .map_err(|error| error.to_string())?;
            if saved != bytes {
                return reject("the report path already holds different evidence");
            }
        }
        Err(error) => return Err(error.into()),
    }
    let report_digest = sha256(&bytes);
    let mut proof = json!({
        "intake_digest": report.intake_digest, "policy_digest": report.policy_digest,
        "baseline_run_digest": report.baseline.receipt_digest,
        "candidate_run_digest": report.candidate.receipt_digest, "report_digest": report_digest,
    });
    if let Some(fresh_through) = report.scanner_fresh_through {
        proof["scanner_fresh_through"] = json!(fresh_through);
    }
    if let Some(digest) = report.scanner_refresh_digest {
        proof["scanner_refresh_digest"] = json!(digest);
    }
    let mut request = json!({
        "action": "record_dependency_verification", "work_id": handoff.work_id,
        "generation": handoff.generation, "head": handoff.head, "digest": handoff.pack_digest,
        "previous_sequence": handoff.verification_sequence, "passed": report.compatibility_verified,
        "approval_id": options.approval_id,
        "proof": proof,
    });
    if let Some(pickup_id) = handoff.pickup_id {
        request["pickup_id"] = json!(pickup_id);
    }
    let attestation = queue.request(&request)?;
    Ok(
        json!({"compatibility_verified": report.compatibility_verified, "report": options.report,
        "approval_id": options.approval_id, "report_digest": report_digest,
        "verification_sequence": attestation["verification_sequence"]}),
    )
}

pub fn cutover(state: &Path, options: CutoverOptions) -> Result<Value> {
    let epoch = options.epoch.to_ascii_lowercase();
    if !epoch.split('-').map(str::len).eq([8, 4, 4, 4, 12])
        || !epoch
            .bytes()
            .all(|byte| byte == b'-' || byte.is_ascii_hexdigit())
    {
        return reject("cutover epoch must be a UUID");
    }
    if !state.is_file() {
        return reject("pass the existing local queue database with --state");
    }
    let queue = Queue::open(&options.queue, options.ca.as_deref(), QueueRole::Cutover)?;
    if !options.rollback {
        let status =
            queue.request_bounded(&json!({"action": "status", "epoch": epoch}), 800 * 1024)?;
        if status["cutover"]["phase"] == "released" {
            return reject("this cutover epoch was already released");
        }
    }
    let _lock = runner::lock_local_slot(state)?;
    let mut store = Store::open(state)?;
    let archive = if options.rollback {
        store.local_cutover_archive(&epoch, &queue.origin)?
    } else {
        store.freeze_for_cutover(&epoch, &queue.origin)?
    };
    let digest = sha256(archive.as_bytes());
    let request = if options.rollback {
        json!({"action": "release", "epoch": epoch})
    } else {
        json!({"action": "activate", "epoch": epoch, "archive": archive})
    };
    let reply = queue.request_bounded(&request, 800 * 1024)?;
    let transfer = &reply["cutover"];
    if transfer["epoch"] != epoch || transfer["digest"] != digest {
        return reject("shared queue did not confirm this local cutover archive");
    }
    let authority = match (options.rollback, transfer["phase"].as_str()) {
        (false, Some("shared")) => "shared",
        (true, Some("draining")) => "shared_draining",
        (true, Some("released")) => {
            let returned_work: Vec<String> =
                serde_json::from_value(transfer["returned_work"].clone())?;
            store.resume_after_cutover(&epoch, &queue.origin, &returned_work)?;
            "local"
        }
        _ => {
            return reject(
                "shared queue reported an unexpected cutover phase; inspect it before continuing",
            );
        }
    };
    Ok(json!({"schema_version": 1, "authority": authority, "cutover": transfer}))
}

pub fn reserve_draft(options: ReserveDraftOptions) -> Result<Value> {
    let queue = Queue::open(
        &options.queue,
        options.ca.as_deref(),
        QueueRole::Publication,
    )?;
    queue.request(&json!({
        "action": "reserve", "work_id": options.work_id, "generation": options.generation,
        "head": options.head, "verification_sequence": options.verification_sequence,
        "update_draft": options.update_draft,
    }))
}

pub fn observe_standing(options: ObserveStandingOptions) -> Result<Value> {
    let observation: StandingObservation = read_json(options.observation)?;
    submit_standing(
        &options.queue,
        options.ca.as_deref(),
        options.intake_token_file.as_deref(),
        &options.rule,
        &observation.snapshot,
        observation.work_order.as_ref(),
    )
}

pub fn diagnose(options: DiagnoseSharedOptions) -> Result<Value> {
    Queue::open(
        &options.queue,
        options.ca.as_deref(),
        QueueRole::Diagnostics,
    )?
    .request_bounded(
        &json!({"case_id": options.case, "from": options.from, "to": options.to}),
        512 * 1024,
    )
}

pub fn series(options: SeriesOptions) -> Result<Value> {
    let response_limit = if matches!(&options.action, SeriesAction::ObserverInbox { .. }) {
        256 * 1024
    } else {
        128 * 1024
    };
    let (role, request) = match options.action {
        SeriesAction::Register { plan } => {
            (QueueRole::SeriesOperator, series_file(plan, "register")?)
        }
        SeriesAction::RegisterProposal {
            proposal,
            proposal_digest,
            approval_reference,
            seed,
        } => (
            QueueRole::SeriesOperator,
            crate::series_proposal::registration(
                &proposal,
                &proposal_digest,
                &approval_reference,
                &seed,
            )?,
        ),
        SeriesAction::Status { id } => (
            QueueRole::SeriesOperator,
            json!({"action":"status", "id":id}),
        ),
        SeriesAction::TransferObserver {
            transfer_id,
            work_id,
            expected_revision,
            observer_id,
            evidence,
        } => (
            QueueRole::SeriesOperator,
            json!({"action":"transfer_observer", "transfer_id":transfer_id, "work_id":work_id,
                "expected_revision":expected_revision, "observer_id":observer_id, "evidence":evidence}),
        ),
        SeriesAction::ObserverStatus { work_id } => (
            QueueRole::ReleaseObserver,
            json!({"action":"status", "work_id":work_id}),
        ),
        SeriesAction::ObserverInbox {
            repository,
            after,
            limit,
            include_closed,
        } => (
            QueueRole::ReleaseObserver,
            json!({"action":"inbox", "repository":repository, "after":after, "limit":limit,
                "include_closed":include_closed}),
        ),
        SeriesAction::ReloadRolloutCatalog { digest } => (
            QueueRole::SeriesOperator,
            json!({"action":"reload_rollout_catalog", "digest":digest}),
        ),
        SeriesAction::Observe { receipt } => {
            (QueueRole::ReleaseObserver, series_file(receipt, "observe")?)
        }
        SeriesAction::ApproveRollout { approval } => (
            QueueRole::SeriesOperator,
            series_file(approval, "approve_rollout")?,
        ),
        SeriesAction::ApproveRolloutRefresh { approval } => (
            QueueRole::SeriesOperator,
            series_file(approval, "approve_rollout_refresh")?,
        ),
        SeriesAction::ObserveRollout {
            work_id,
            generation,
            head,
            verification_sequence,
            policy,
            release,
        } => {
            let read = |path: &Path, limit| {
                craft_factory_evidence::release_trace::read_evidence(path, limit)
                    .map_err(|error| error.to_string())
            };
            (
                QueueRole::VerifiedRollout,
                json!({
                    "work_id":work_id, "generation":generation, "head":head,
                    "verification_sequence":verification_sequence,
                    "policy_base64": STANDARD.encode(read(&policy, 64 * 1024)?),
                    "evidence_base64": STANDARD.encode(read(&release, 512 * 1024)?),
                }),
            )
        }
        SeriesAction::ObserveArchivedRollout {
            work_id,
            generation,
            head,
            verification_sequence,
            refresh_id,
            check_only,
        } => {
            let mut request = json!({"work_id": work_id, "generation": generation, "head": head,
                "verification_sequence": verification_sequence, "archived": true, "refresh_id": refresh_id});
            if check_only {
                request["check_only"] = json!(true);
            }
            (QueueRole::VerifiedRollout, request)
        }
        SeriesAction::Withdraw {
            work_id,
            sequence,
            evidence,
        } => (
            QueueRole::ReleaseObserver,
            json!({"action":"withdraw", "work_id":work_id, "sequence":sequence, "evidence":evidence}),
        ),
    };
    Queue::open(&options.queue, options.ca.as_deref(), role)?
        .request_bounded(&request, response_limit)
}

fn series_file(path: PathBuf, action: &str) -> Result<Value> {
    let mut value: Value = read_json(path)?;
    let object = value
        .as_object_mut()
        .ok_or("series input must be a JSON object")?;
    if object.contains_key("action") {
        return reject("the series command defines the action");
    }
    object.insert("action".into(), json!(action));
    Ok(value)
}

pub fn submit_standing(
    origin: &str,
    ca: Option<&Path>,
    token_file: Option<&Path>,
    rule: &str,
    snapshot: &Value,
    work_order: Option<&Value>,
) -> Result<Value> {
    Queue::open_with_token_file(origin, ca, QueueRole::Intake, token_file)?.request(&json!({
        "action":"observe", "rule":rule, "snapshot":snapshot, "work_order":work_order,
    }))
}

fn draft_update(
    publication: &Value,
    target: &twg::DraftTarget,
) -> Result<Option<bitbucket_publish::DraftUpdate>> {
    match (
        publication["repair_head"].as_str(),
        publication["expected_draft_url"].as_str(),
    ) {
        (Some(head), Some(url)) => Ok(Some(bitbucket_publish::DraftUpdate::new(
            target, head, url,
        )?)),
        (None, None) if publication["prior_action_key"].is_null() => Ok(None),
        _ => reject("publication returned an incomplete draft update binding"),
    }
}

pub fn reconcile_draft(options: ReconcileDraftOptions) -> Result<Value> {
    let queue = Queue::open(
        &options.queue,
        options.ca.as_deref(),
        QueueRole::Publication,
    )?;
    let status = queue.request(&json!({"action": "status", "action_key": options.action_key}))?;
    let publication = &status["publication"];
    if publication["action_key"] != options.action_key
        || !matches!(
            publication["state"].as_str(),
            Some("sending" | "uncertain" | "confirmed")
        )
    {
        return reject("publication is not awaiting a Bitbucket draft observation");
    }
    let target: twg::DraftTarget = serde_json::from_value(publication.clone())?;
    if draft_update(publication, &target)?
        .is_some_and(|update| update.pull_request != options.pull_request)
    {
        return reject("the observation must use the existing draft's pull request number");
    }
    let policy = read_json(options.policy)?;
    let url = twg::observe_draft(&policy, &target, options.pull_request)?;
    queue.request(&json!({
        "action": "reconcile", "action_key": options.action_key, "observation": {
            "outcome": "confirmed", "head": target.head, "native_url": url,
            "source_branch": target.source_branch, "base_branch": target.base_branch,
            "evidence": format!("TWG observed open draft {url} and confirmed the pinned source and base"),
        }
    }))
}

pub fn bind_github_review(options: BindGithubReviewOptions) -> Result<Value> {
    let queue = Queue::open(
        &options.queue,
        options.ca.as_deref(),
        QueueRole::Publication,
    )?;
    let handoff: Handoff = read_json(options.handoff)?;
    if handoff.schema_version != 1
        || handoff.endpoint != queue.origin
        || handoff.status != "ready_for_independent_checks"
    {
        return reject("GitHub draft binding requires a candidate handoff from this queue");
    }
    let mut request = json!({
        "action": "bind_github_review", "work_id": handoff.work_id,
        "generation": handoff.generation, "head": handoff.head,
        "verification_sequence": handoff.verification_sequence,
    });
    let binding = queue.request(&request)?;
    let target: twg::DraftTarget = serde_json::from_value(binding["publication"].clone())?;
    if target.head != handoff.head || binding["publication"]["pack_digest"] != handoff.pack_digest {
        return reject("the shared candidate no longer matches the handoff");
    }
    let policy = read_json(options.policy)?;
    let url =
        crate::github::observe_review_draft(&policy, &options.gh, &target, options.pull_request)?;
    if binding["publication"]["expected_draft_url"]
        .as_str()
        .is_some_and(|expected| expected != url)
    {
        return reject("the repair must use the existing GitHub pull request");
    }
    request["observation"] = json!({
        "native_url": url, "head": target.head, "base": target.base,
        "source_branch": target.source_branch, "base_branch": target.base_branch,
    });
    queue.request(&request)
}

pub fn publish_draft(options: PublishDraftOptions) -> Result<Value> {
    let queue = Queue::open(&options.queue, options.ca.as_deref(), QueueRole::Publisher)?;
    let publisher = bitbucket_publish::Publisher::open()?;
    let maps: BTreeMap<String, Checkout> = read_json(options.repository_map)?;
    let prepared = queue.request_bounded(
        &json!({"action": "prepare", "action_key": options.action_key, "update_draft": options.update_draft}),
        800 * 1024,
    )?;
    let publication = &prepared["publication"];
    let work_id = publication["work_id"]
        .as_str()
        .ok_or("publication is missing a work order")?;
    let generation = publication["generation"]
        .as_i64()
        .ok_or("publication is missing its generation")?;
    let target: twg::DraftTarget = serde_json::from_value(publication.clone())?;
    let update = draft_update(publication, &target)?;
    if publication["action_key"] != options.action_key
        || publication["state"] != "reserved"
        || target.source_branch != format!("craft-factory/{work_id}")
        || update.is_some() != options.update_draft
    {
        return reject("queue returned a different draft reservation");
    }
    let mapped = maps
        .get(&target.repository)
        .ok_or("no trusted checkout is mapped for the draft repository")?;
    let order = bind_checkout(
        publication["work_order"].clone(),
        mapped,
        work_id,
        &target.repository,
        generation,
    )?;
    if order.base != target.base {
        return reject("draft reservation disagrees with the work order base");
    }
    if snapshot::reference(&order)?.is_some() {
        snapshot::verify(
            &order,
            mapped
                .jira_snapshot
                .as_deref()
                .ok_or("map the approved Jira snapshot before publication")?,
        )?;
    }
    let encoded = publication["pack_base64"]
        .as_str()
        .ok_or("publication has no candidate pack")?;
    let pack = STANDARD.decode(encoded)?;
    if pack.len() as u64 > PACK_LIMIT
        || !pack.starts_with(b"PACK")
        || publication["pack_digest"] != sha256(&pack)
    {
        return reject("publication pack failed size, format, or digest validation");
    }
    let checkout = repository::prepare_pack_for_verification(
        &order,
        &options.destination,
        &target.head,
        &pack,
        update.as_ref().map(|update| update.previous_head.as_str()),
    )?;
    if let Some(update) = &update {
        publisher.assert_update_target(&target, update)?;
    } else {
        publisher.assert_base(&target)?;
        publisher.assert_source_absent(&target)?;
    }
    let permit = queue.request(&json!({
        "action": "begin", "action_key": options.action_key, "update_draft": options.update_draft
    }))?;
    let permit = &permit["publication"];
    if permit["action_key"] != options.action_key
        || permit["state"] != "sending"
        || permit["work_id"] != work_id
        || permit["generation"] != generation
        || permit["verification_sequence"] != publication["verification_sequence"]
        || permit["pack_digest"] != publication["pack_digest"]
        || permit["prior_action_key"] != publication["prior_action_key"]
        || serde_json::from_value::<twg::DraftTarget>(permit.clone())? != target
        || draft_update(permit, &target)? != update
    {
        return reject("queue returned a different send permit; leave publication held");
    }
    let send = match &update {
        Some(update) => publisher.update(&target, &checkout, update),
        None => publisher.create(&target, &checkout, work_id),
    };
    let observation = match send {
        Ok(confirmed) => confirmed,
        Err(_) => {
            let _ = queue.request(&json!({
                "action": "record", "action_key": options.action_key, "observation": {
                    "outcome": "uncertain", "head": target.head,
                    "evidence": "Bitbucket route did not confirm both the source branch and draft; reconcile before any further action",
                }
            }));
            return reject("Bitbucket draft outcome is uncertain; reconcile the reserved action");
        }
    };
    queue.request(&json!({"action": "record", "action_key": options.action_key, "observation": observation}))
        .map_err(|_| "Bitbucket returned a draft but the queue did not confirm its receipt; reconcile the reserved action".into())
}

fn import_review(
    queue: &Queue,
    handoff_id: &str,
    observe: impl FnOnce(&crate::review::FeedbackTarget) -> Result<crate::review::ReviewRecord>,
) -> Result<Value> {
    let response = queue.request(&json!({
        "action": "review_feedback_target", "handoff_id": handoff_id
    }))?;
    let target = serde_json::from_value(response["target"].clone())?;
    let feedback = observe(&target)?;
    Ok(json!({
        "action": "record_review_decision", "handoff_id": handoff_id,
        "head": target.draft.head, "decision": "changes_requested",
        "evidence": feedback.evidence, "note": feedback.note
    }))
}

fn github_merge_observation(
    target: &Value,
    head: &str,
    policy: &crate::github::Policy,
    digest: &str,
    gh: &Path,
    deadline: Instant,
) -> Result<Option<craft_factory_evidence::GithubMergeOutcome>> {
    if target["head"] != head {
        return reject("the native merge target differs from the evaluated candidate");
    }
    if target["policy_file_digest"] != digest
        || target["repository"] != policy.repository
        || target["base_branch"] != policy.base_branch
    {
        return reject("the GitHub policy differs from the policy used for this evaluation");
    }
    crate::github::observe_shared_merge_until(
        &crate::review::Draft {
            head: head.to_owned(),
            reference: target["pull_request"]
                .as_str()
                .ok_or("missing bound GitHub PR")?
                .to_owned(),
        },
        target["base"]
            .as_str()
            .ok_or("missing pinned GitHub base")?,
        policy,
        gh,
        target["as_of"]
            .as_i64()
            .ok_or("missing shadow evaluation time")?,
        deadline,
    )
}

#[derive(Clone, Copy)]
struct MergeCheckBudget {
    receipt_deadline: Instant,
    native_timeout: Duration,
}

impl MergeCheckBudget {
    fn native_deadline(self) -> Instant {
        self.receipt_deadline
            .min(Instant::now() + self.native_timeout)
    }
}

fn check_github_merge(
    queue: &Queue,
    item: &Value,
    policy: &crate::github::Policy,
    digest: &str,
    gh: &Path,
    budget: MergeCheckBudget,
) -> &'static str {
    if item["forge"] != "github"
        || item["policy_file_digest"] != digest
        || item["base_branch"] != policy.base_branch
    {
        return "outside_policy";
    }
    let Some(head) = item["head"].as_str() else {
        return "invalid_target";
    };
    let mut request = json!({"action": "confirm_github_shadow_merge", "receipt_id": item["receipt_id"],
                             "head": head, "report_digest": item["report_digest"]});
    let Ok(response) = queue.request(&request) else {
        return "target_unavailable";
    };
    let target = &response["github_merge"];
    if target["head"] != head || target["pull_request"] != item["pull_request"] {
        return "invalid_target";
    }
    if !target["recorded"].is_null() {
        return "already_recorded";
    }
    let deadline = budget.native_deadline();
    if Instant::now() >= deadline {
        return "check_timed_out";
    }
    let outcome = github_merge_observation(target, head, policy, digest, gh, deadline);
    finish_merge_check(
        queue,
        &mut request,
        outcome,
        deadline,
        "github_merge",
        "receipt_id",
        &item["receipt_id"],
    )
}

#[derive(Deserialize)]
struct BitbucketMergeTarget {
    publication_key: String,
    repository: String,
    base: String,
    base_branch: String,
    pull_request: String,
}

impl BitbucketMergeTarget {
    fn observe(
        &self,
        head: &str,
        policy: &crate::bitbucket_feedback::Policy,
        client: &crate::bitbucket_read::ReadClient,
        deadline: Instant,
    ) -> Result<Option<craft_factory_evidence::BitbucketMergeOutcome>> {
        crate::bitbucket_feedback::observe_shared_merge_until(
            &crate::review::Draft {
                head: head.to_owned(),
                reference: self.pull_request.clone(),
            },
            &self.repository,
            &self.base,
            &self.base_branch,
            policy,
            client,
            deadline,
        )
    }
}

fn check_bitbucket_merge(
    queue: &Queue,
    item: &Value,
    policy: &crate::bitbucket_feedback::Policy,
    client: &crate::bitbucket_read::ReadClient,
    budget: MergeCheckBudget,
) -> &'static str {
    if item["forge"] != "bitbucket" || item["base_branch"] != policy.base_branch() {
        return "outside_policy";
    }
    let Some(head) = item["head"].as_str() else {
        return "invalid_target";
    };
    let mut request =
        json!({"action": "confirm_bitbucket_work_merge", "work_id": item["work_id"], "head": head});
    let Ok(response) = queue.request(&request) else {
        return "target_unavailable";
    };
    let target = &response["bitbucket_merge"];
    if target["head"] != head {
        return "invalid_target";
    }
    if !target["recorded"].is_null() {
        return if target["recorded"]["publication_key"] == item["publication_key"] {
            "already_recorded"
        } else {
            "invalid_target"
        };
    }
    if target["publication_key"] != item["publication_key"]
        || target["pull_request"] != item["pull_request"]
    {
        return "invalid_target";
    }
    let Ok(target) = serde_json::from_value::<BitbucketMergeTarget>(target.clone()) else {
        return "invalid_target";
    };
    let deadline = budget.native_deadline();
    if Instant::now() >= deadline {
        return "check_timed_out";
    }
    let outcome = target.observe(head, policy, client, deadline);
    request["publication_key"] = json!(target.publication_key);
    finish_merge_check(
        queue,
        &mut request,
        outcome,
        deadline,
        "bitbucket_merge",
        "publication_key",
        &item["publication_key"],
    )
}

fn finish_merge_check(
    queue: &Queue,
    request: &mut Value,
    observation: Result<Option<impl Serialize>>,
    deadline: Instant,
    response_key: &str,
    binding_field: &str,
    expected: &Value,
) -> &'static str {
    let observed = match observation {
        Ok(Some(observed)) => observed,
        Ok(None) => return "not_merged",
        Err(_) if Instant::now() >= deadline => return "check_timed_out",
        Err(_) => return "check_failed",
    };
    if Instant::now() >= deadline {
        return "check_timed_out";
    }
    request["observation"] = json!(observed);
    match queue.request(request) {
        Ok(response) if response[response_key][binding_field] == *expected => "recorded",
        Err(error)
            if error
                .downcast_ref::<QueueRequestError>()
                .and_then(|error| error.status)
                .is_some_and(|status| (400..500).contains(&status)) =>
        {
            "receipt_rejected"
        }
        _ => "receipt_write_uncertain",
    }
}

fn check_merge_output_paths(batch: &MergeBatchOptions, inputs: &[Option<&Path>]) -> Result<()> {
    let credential = crate::evidence_capture::credential_path("CRAFT_FACTORY_VERIFIER_TOKEN");
    crate::evidence_capture::check_output_paths(
        &[batch.cursor_db.as_deref(), batch.report_file.as_deref()],
        inputs
            .iter()
            .flatten()
            .copied()
            .chain(batch.verifier_token_file.as_deref())
            .chain(credential.as_deref()),
        "merge batch cursor and report paths must be distinct from each other and from the inputs",
    )
}

fn run_merge_checks(
    queue: &Queue,
    scope: (&str, &str, &str),
    batch: MergeBatchOptions,
    scheduled_closure: Option<Value>,
    check: impl Fn(&Value, MergeCheckBudget) -> &'static str,
) -> Result<()> {
    use rusqlite::{Connection, OptionalExtension, TransactionBehavior};

    let (forge, repository, policy_digest) = scope;
    let scope_key = sha256(&serde_json::to_vec(&(
        &queue.origin,
        forge,
        repository,
        policy_digest,
    ))?);
    if batch.cursor_db.is_some() && batch.budget_seconds <= QUEUE_TIMEOUT.as_secs() * 3 {
        return reject(
            "a recurring merge batch needs more than 15 seconds for the page, target, and receipt requests",
        );
    }
    let mut db = batch
        .cursor_db
        .as_deref()
        .map(Connection::open)
        .transpose()?;
    let tx = db
        .as_mut()
        .map(|db| -> rusqlite::Result<_> {
            db.busy_timeout(Duration::ZERO)?;
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS merge_batch_cursor (
            id INTEGER PRIMARY KEY CHECK (id = 1), scope_key TEXT NOT NULL, after_work_id TEXT
        );",
            )?;
            Ok(tx)
        })
        .transpose()?;
    let after = if let Some(tx) = &tx {
        tx.query_row(
            "SELECT after_work_id FROM merge_batch_cursor WHERE id = 1 AND scope_key = ?1",
            [&scope_key],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten()
    } else {
        batch.after_work_id.clone()
    };
    let deadline = Instant::now() + Duration::from_secs(batch.budget_seconds);
    let observation_deadline = if scheduled_closure.is_some() {
        deadline - QUEUE_TIMEOUT
    } else {
        deadline
    };
    let check_budget = MergeCheckBudget {
        receipt_deadline: observation_deadline - QUEUE_TIMEOUT,
        native_timeout: Duration::from_secs(batch.check_seconds),
    };
    let page = queue.request(
        &json!({"action": "pending_merge_checks", "repository": repository,
        "after_work_id": after, "limit": batch.limit}),
    )?;
    let page = &page["merge_checks"];
    let items = page["items"]
        .as_array()
        .ok_or("merge check list is missing")?;
    let mut results = Vec::new();
    for item in items {
        let now = Instant::now();
        // Reserve one queue read before native inspection and one receipt write after it.
        if observation_deadline.saturating_duration_since(now) <= QUEUE_TIMEOUT * 2 {
            break;
        }
        let outcome = check(item, check_budget);
        results.push(json!({"work_id": item["work_id"], "head": item["head"],
            "pull_request": item["pull_request"], "outcome": outcome}));
    }
    let deferred = items.len() - results.len();
    let next = if deferred == 0 {
        page["next_after_work_id"].clone()
    } else {
        results
            .last()
            .map_or_else(|| json!(after), |result| result["work_id"].clone())
    };
    let mut report = json!({"schema_version": 1, "merge_check_report": {
        "repository": repository, "items": results,
        "deferred_in_page": deferred, "next_after_work_id": next,
    }});
    if let Some(request) = scheduled_closure {
        report["merge_check_report"]["scheduled_closure"] =
            if deadline.saturating_duration_since(Instant::now()) <= QUEUE_TIMEOUT {
                json!({"outcome": "deferred"})
            } else {
                match queue.request_bounded(&request, 64 * 1024) {
                    Ok(response) if response["closures"].is_array() => {
                        json!({"outcome": "checked", "items": response["closures"]})
                    }
                    _ => json!({"outcome": "request_failed"}),
                }
            };
    }
    if let Some(path) = &batch.report_file {
        crate::evidence_capture::replace_json(path, &report)?;
    }
    if let Some(tx) = tx {
        tx.execute("INSERT INTO merge_batch_cursor (id, scope_key, after_work_id) VALUES (1, ?1, ?2)
            ON CONFLICT(id) DO UPDATE SET scope_key = excluded.scope_key, after_work_id = excluded.after_work_id",
            rusqlite::params![scope_key, next.as_str()])?;
        tx.commit()?;
    }
    crate::print_json(&report)
}

fn github_shadow_report(
    order: &WorkOrder,
    draft: &crate::review::Draft,
    policy: &crate::github::Policy,
    gh: &Path,
    pin: craft_factory_evidence::GithubShadowPolicy,
    policy_digest: String,
    handoff: &Handoff,
) -> Result<Value> {
    let mut report = crate::github::shadow_merge_candidate(
        order,
        draft,
        true,
        policy,
        gh,
        &pin.protection_digest,
        time::OffsetDateTime::now_utc().unix_timestamp(),
    )?;
    report["github_policy"] = serde_json::to_value(pin)?;
    report["policy_digest"] = json!(policy_digest);
    report["shared_binding"] = shadow_binding(handoff);
    if report["shadow_eligible"] == true && policy.require_merge_queue {
        match repository::github_enqueue_change_evidence(order, &draft.head) {
            Ok(evidence) => report["enqueue_risk"] = serde_json::to_value(evidence)?,
            Err(error) => report["enqueue_risk_blocker"] = json!(error.to_string()),
        }
    }
    Ok(report)
}

fn shadow_receipt(
    endpoint: &str,
    receipt_id: String,
    handoff: &Handoff,
    report: &Value,
    report_digest: String,
) -> Result<Value> {
    if handoff.schema_version != 1
        || handoff.endpoint != endpoint
        || handoff.status != "ready_for_independent_checks"
        || report["schema_version"] != 1
        || report["work_id"] != handoff.work_id
        || report["head"] != handoff.head
    {
        return reject("shadow report does not match the candidate handoff");
    }
    let github = report["provider"] == "github";
    let mut native_verified = report["native_preflight_requested"] == true
        && report["native_preflight_completed_at"].is_u64()
        && report["native_preflight"]["blockers"]
            .as_array()
            .is_some_and(Vec::is_empty);
    let mut request = json!({"receipt_id": receipt_id,
        "work_id": handoff.work_id, "generation": handoff.generation, "head": handoff.head,
        "pack_digest": handoff.pack_digest, "verification_sequence": handoff.verification_sequence,
        "report_digest": report_digest, "assessment": {
            "repository": report["repository"], "base": report["base"], "pull_request": report["pull_request"],
            "policy_digest": report["policy_digest"], "observation_digest": report["observation_digest"],
            "as_of": report["as_of"], "eligible": report["shadow_eligible"],
            "native_verified": native_verified,
            "blockers": report["blocking_reasons"],
        }
    });
    if github {
        if report["shared_binding"] != shadow_binding(handoff) {
            return reject("GitHub shadow report does not match the exact queue verdict");
        }
        let pin: craft_factory_evidence::GithubShadowPolicy =
            serde_json::from_value(report["github_policy"].clone())?;
        native_verified = report["shadow_eligible"] == true
            && report["observed_protection_digest"] == pin.protection_digest
            && ["checks", "approvals"].iter().all(|key| {
                report["evidence"][key]
                    .as_array()
                    .is_some_and(|rows| !rows.is_empty())
            });
        let assessment = &mut request["assessment"];
        assessment["github"] = serde_json::to_value(pin)?;
        assessment["native_verified"] = json!(native_verified);
        assessment["observation_digest"] = json!(report_digest);
        assessment["blockers"] = report["blocking_codes"].clone();
        if report["merge_queue_required"] == true && report["shadow_eligible"] == true {
            let readiness: craft_factory_evidence::GithubMergeQueueReadiness =
                serde_json::from_value(report["evidence"]["merge_queue"].clone())?;
            if !readiness.valid() {
                return reject("GitHub merge queue report lacks complete readiness evidence");
            }
            assessment["github_queue"] = serde_json::to_value(readiness)?;
            if !report["enqueue_risk"].is_null() {
                let risk: craft_factory_evidence::github_risk::ChangeEvidence =
                    serde_json::from_value(report["enqueue_risk"].clone())?;
                if !risk.valid() {
                    return reject("GitHub enqueue report has invalid change evidence");
                }
                assessment["enqueue_risk"] = serde_json::to_value(risk)?;
            }
        }
    }
    Ok(request)
}

fn poll_github_readiness(queue: &Queue, options: GithubReadinessOptions) -> Result<Value> {
    crate::github::validate_gh(&options.gh)?;
    let (policy, policy_file_digest): (crate::github::Policy, _) =
        crate::shadow_merge::load(&options.policy)?;
    policy.validate()?;
    if !policy.require_merge_queue || !options.checkout_root.is_absolute() {
        return reject(
            "readiness relay requires a native queue policy and an absolute private checkout root",
        );
    }
    let root = options.checkout_root.canonicalize()?;
    if !root.is_dir() {
        return reject("readiness relay requires an existing private checkout root");
    }
    let pin = craft_factory_evidence::GithubShadowPolicy {
        policy_file_digest,
        protection_digest: options.approved_protection_digest,
    };
    let policy_digest = pin
        .digest()
        .ok_or("provide the approved GitHub protection fingerprint")?;
    let response = queue.request(&json!({
        "action": "next_github_readiness", "repository": policy.repository,
        "base_branch": policy.base_branch, "policy_digest": policy_digest,
    }))?;
    let target = response
        .get("github_readiness_target")
        .ok_or("readiness response missing target")?;
    if target.is_null() {
        return Ok(json!({"state": "idle", "repository": policy.repository}));
    }
    let draft = github_enqueue::bound_draft(target)?;
    let work_id = target["work_id"]
        .as_str()
        .ok_or("missing readiness work ID")?;
    github_enqueue::with_temporary_checkout(&root, |destination| {
        let prepared = prepare_verification_at(
            queue,
            work_id,
            &draft.head,
            options.repository_map,
            destination,
            None,
        )?;
        let handoff: Handoff = serde_json::from_value(prepared.clone())?;
        if handoff.work_id != work_id
            || prepared["repository"] != policy.repository
            || target["generation"] != handoff.generation
            || target["pack_digest"] != handoff.pack_digest
            || target["verification_sequence"] != handoff.verification_sequence
        {
            return reject("candidate binding changed before GitHub readiness inspection");
        }
        let order: WorkOrder = read_json(PathBuf::from(
            prepared["work_order"]
                .as_str()
                .ok_or("missing prepared work order")?,
        ))?;
        let report = github_shadow_report(
            &order,
            &draft,
            &policy,
            &options.gh,
            pin,
            policy_digest,
            &handoff,
        )?;
        let report_digest = sha256(&serde_json::to_vec_pretty(&report)?);
        let receipt_id = format!("readiness-{}", &report_digest["sha256:".len()..]);
        let receipt = shadow_receipt(&queue.origin, receipt_id, &handoff, &report, report_digest)?;
        queue.request_bounded(
            &json!({"action": "record_github_readiness", "receipt": receipt}),
            64 * 1024,
        )
    })
}

pub fn operations(options: OperationsOptions) -> Result<()> {
    let role = if matches!(options.action, OperationsAction::ProposeKnowledge { .. }) {
        QueueRole::Runner
    } else if matches!(options.action, OperationsAction::ApplyProfileFallbacks) {
        QueueRole::ProfileScheduler
    } else if matches!(
        options.action,
        OperationsAction::CheckConfluenceContext {
            context_reader: true,
            ..
        }
    ) {
        QueueRole::ContextReader
    } else if matches!(
        options.action,
        OperationsAction::RecordShadowOutcome { .. }
            | OperationsAction::ConfirmGithubShadowMerge { .. }
            | OperationsAction::AttestGithubCiChecks { .. }
            | OperationsAction::AttestBitbucketCiChecks { .. }
            | OperationsAction::ConfirmBitbucketWorkMerge { .. }
            | OperationsAction::PendingMergeChecks { .. }
            | OperationsAction::PendingVerifications { .. }
            | OperationsAction::ReleaseVerificationPickup { .. }
            | OperationsAction::RenewVerificationPickup { .. }
            | OperationsAction::PollGithubReadiness(_)
            | OperationsAction::CheckGithubMerges { .. }
            | OperationsAction::CheckBitbucketMerges { .. }
    ) {
        QueueRole::Verifier
    } else if matches!(
        options.action,
        OperationsAction::ReviewInbox { reviewer: true, .. }
            | OperationsAction::RecordReviewDecision { .. }
            | OperationsAction::ImportGithubReview { .. }
            | OperationsAction::ImportBitbucketReview { .. }
    ) {
        QueueRole::Reviewer
    } else {
        QueueRole::Operations
    };
    let credential_file = match &options.action {
        OperationsAction::CheckConfluenceContext {
            context_reader_token_file,
            ..
        } => context_reader_token_file.as_deref(),
        OperationsAction::PendingVerifications {
            verifier_token_file,
            ..
        }
        | OperationsAction::ReleaseVerificationPickup {
            verifier_token_file,
            ..
        }
        | OperationsAction::RenewVerificationPickup {
            verifier_token_file,
            ..
        } => verifier_token_file.as_deref(),
        OperationsAction::PollGithubReadiness(poll) => poll.verifier_token_file.as_deref(),
        OperationsAction::CheckGithubMerges { batch, .. }
        | OperationsAction::CheckBitbucketMerges { batch, .. } => {
            batch.verifier_token_file.as_deref()
        }
        _ => None,
    };
    let queue =
        Queue::open_with_token_file(&options.queue, options.ca.as_deref(), role, credential_file)?;
    let brief = matches!(
        &options.action,
        OperationsAction::ReviewInbox { brief: true, .. }
    );
    let response_limit = if matches!(&options.action, OperationsAction::ReviewInbox { .. }) {
        1024 * 1024
    } else {
        256 * 1024
    };
    let request = match options.action {
        OperationsAction::Summary => json!({"action": "summary"}),
        OperationsAction::DraftPolicy { repository } => {
            json!({"action": "draft_policy", "repository": repository})
        }
        OperationsAction::ConfigureDraftPolicy {
            repository,
            base_branch,
            publisher,
            forge,
            expected_revision,
            enabled,
            evidence,
        } => json!({"action": "configure_draft_policy", "repository": repository,
            "base_branch": base_branch, "publisher_id": publisher, "forge": forge,
            "expected_revision": expected_revision, "enabled": enabled, "evidence": evidence,
        }),
        OperationsAction::RegisterTeamSkill {
            location,
            markdown,
            revision,
            owner,
            repository,
            evidence,
        } => {
            let bytes = craft_factory_evidence::release_trace::read_evidence(&markdown, 32768)
                .map_err(|error| error.to_string())?;
            if sha256(&bytes) != revision {
                return reject("team skill Markdown differs from the approved revision");
            }
            json!({"action": "register_team_skill", "release": {
                "location": location, "markdown": String::from_utf8(bytes)?, "revision": revision,
                "owner_name": owner, "repositories": repository, "approval_reference": evidence,
            }})
        }
        OperationsAction::TeamSkillRelease { location, revision } => {
            json!({"action": "team_skill_release", "location": location, "revision": revision})
        }
        OperationsAction::WithdrawTeamSkill {
            location,
            revision,
            evidence,
        } => {
            json!({"action": "withdraw_team_skill", "location": location, "revision": revision, "evidence": evidence})
        }
        OperationsAction::ApproveStandingRule { policy, digest } => {
            let policy: Value = load_shared_ci_policy(&policy, &digest)?;
            json!({"action": "approve_jira_standing_rule", "policy": policy, "digest": digest})
        }
        OperationsAction::DisableStandingRule { rule_id, evidence } => {
            json!({"action": "disable_jira_standing_rule", "rule_id": rule_id, "evidence": evidence})
        }
        OperationsAction::ApproveStandingIntent {
            rule_id,
            issue,
            source_revision,
            review_token,
            evidence,
        } => {
            json!({"action": "approve_jira_standing_intent", "rule_id": rule_id, "issue": issue,
                "source_revision": source_revision, "review_token": review_token, "evidence": evidence})
        }
        OperationsAction::OperatorBrief {
            repository,
            usage_hours,
            limit,
            json,
        } => {
            let snapshot = queue.request_bounded(&json!({
                "action": "operator_brief", "repository": repository, "usage_hours": usage_hours, "limit": limit,
            }), 256 * 1024)?;
            return if json {
                crate::print_json(&snapshot)
            } else {
                print_operator_brief(&snapshot["operator_brief"], usage_hours)
            };
        }
        OperationsAction::ReloadIdentities { digest } => {
            json!({"action": "reload_identities", "digest": digest})
        }
        OperationsAction::ReloadDiagnosticCatalog { digest, evidence } => {
            json!({"action": "reload_diagnostic_catalog", "digest": digest, "evidence": evidence})
        }
        OperationsAction::ApproveDependencyEvidence { approval } => {
            let approval: Value = read_json(approval)?;
            json!({"action": "approve_dependency_evidence", "approval": approval})
        }
        OperationsAction::ApproveShadowPolicy {
            repository,
            policy_digest,
            require_native,
            evidence,
            valid_seconds,
        } => {
            json!({"action": "approve_shadow_policy", "repository": repository, "policy_digest": policy_digest,
                "require_native": require_native, "evidence": evidence, "valid_seconds": valid_seconds})
        }
        OperationsAction::ApproveGithubShadowPolicy {
            policy,
            approved_protection_digest,
            evidence,
            valid_seconds,
        } => {
            let (policy, policy_file_digest): (crate::github::Policy, _) =
                crate::shadow_merge::load(&policy)?;
            let pin = craft_factory_evidence::GithubShadowPolicy {
                policy_file_digest,
                protection_digest: approved_protection_digest,
            };
            let policy_digest = pin
                .digest()
                .ok_or("provide the approved GitHub protection fingerprint")?;
            json!({"action": "approve_shadow_policy", "repository": policy.repository, "policy_digest": policy_digest,
                "require_native": true, "evidence": evidence, "valid_seconds": valid_seconds})
        }
        OperationsAction::AssessGithubShadow {
            work_id,
            head,
            repository_map,
            destination,
            policy,
            gh,
            approved_protection_digest,
        } => {
            let (policy, policy_file_digest): (crate::github::Policy, _) =
                crate::shadow_merge::load(&policy)?;
            let pin = craft_factory_evidence::GithubShadowPolicy {
                policy_file_digest,
                protection_digest: approved_protection_digest,
            };
            let policy_digest = pin
                .digest()
                .ok_or("provide the approved GitHub protection fingerprint")?;
            let report_path = destination.join("github-shadow-report.json");
            let prepared = prepare_verification(VerificationOptions {
                queue: options.queue,
                ca: options.ca,
                work_id,
                head,
                repository_map,
                destination,
                verifier: false,
                verifier_token_file: None,
            })?;
            let handoff: Handoff = serde_json::from_value(prepared.clone())?;
            let status = queue.request_bounded(
                &json!({"action": "audit", "work_id": handoff.work_id}),
                512 * 1024,
            )?;
            let audit = &status["audit"];
            let work = &audit["work"];
            if work["generation"] != handoff.generation
                || work["candidate"] != handoff.head
                || work["candidate_evidence"] != handoff.pack_digest
                || work["verification"]["sequence"] != handoff.verification_sequence
                || work["independent_checks_passed"] != true
                || work["repository"] != policy.repository
            {
                return reject(
                    "GitHub shadow assessment requires the latest passing candidate verdict",
                );
            }
            let publication = audit["publications"]
                .as_array()
                .and_then(|rows| {
                    rows.iter().find(|row| {
                        row["generation"] == handoff.generation
                            && row["head"] == handoff.head
                            && row["state"] == "confirmed"
                    })
                })
                .ok_or("bind the shared GitHub draft before checking merge readiness")?;
            let draft = crate::review::Draft {
                head: handoff.head.clone(),
                reference: publication["result_url"]
                    .as_str()
                    .ok_or("publication has no PR reference")?
                    .to_owned(),
            };
            let order: WorkOrder = read_json(PathBuf::from(
                prepared["work_order"]
                    .as_str()
                    .ok_or("missing prepared work order")?,
            ))?;
            let report =
                github_shadow_report(&order, &draft, &policy, &gh, pin, policy_digest, &handoff)?;
            serde_json::to_writer_pretty(File::create_new(report_path)?, &report)?;
            return crate::print_json(&report);
        }
        OperationsAction::RecordShadowMerge {
            receipt_id,
            handoff,
            report,
        } => {
            let handoff: Handoff = read_json(handoff)?;
            let (report, report_digest): (Value, _) = crate::shadow_merge::load(&report)?;
            let mut request =
                shadow_receipt(&queue.origin, receipt_id, &handoff, &report, report_digest)?;
            request["action"] = json!("record_shadow_merge");
            request
        }
        OperationsAction::PollGithubReadiness(readiness) => {
            let credential =
                crate::evidence_capture::credential_path("CRAFT_FACTORY_VERIFIER_TOKEN");
            crate::evidence_capture::check_output_paths(
                &[readiness.report_file.as_deref()],
                [
                    Some(readiness.repository_map.as_path()),
                    Some(readiness.policy.as_path()),
                    Some(readiness.gh.as_path()),
                    readiness.verifier_token_file.as_deref(),
                    options.ca.as_deref(),
                    credential.as_deref(),
                ]
                .into_iter()
                .flatten(),
                "GitHub readiness report path must be distinct from the inputs",
            )?;
            let path = readiness.report_file.clone();
            let report = poll_github_readiness(&queue, readiness)?;
            if let Some(path) = path {
                crate::evidence_capture::replace_json(&path, &report)?;
            }
            return crate::print_json(&report);
        }
        OperationsAction::ShadowMergeSummary { repository, since } => {
            json!({"action": "shadow_merge_summary", "repository": repository, "since": since})
        }
        action @ (OperationsAction::AttestGithubCiChecks { .. }
        | OperationsAction::AttestBitbucketCiChecks { .. }) => {
            let (handoff_path, policy_path, key, action_name) = match &action {
                OperationsAction::AttestGithubCiChecks {
                    handoff, policy, ..
                } => (
                    handoff,
                    policy,
                    "github_ci_checks",
                    "record_github_ci_checks",
                ),
                OperationsAction::AttestBitbucketCiChecks {
                    handoff, policy, ..
                } => (
                    handoff,
                    policy,
                    "bitbucket_ci_checks",
                    "record_bitbucket_ci_checks",
                ),
                _ => unreachable!(),
            };
            #[derive(Deserialize)]
            struct Prepared {
                #[serde(flatten)]
                handoff: Handoff,
                checkout: PathBuf,
            }
            let Prepared { handoff, checkout } = read_json(handoff_path)?;
            if handoff.schema_version != 1
                || handoff.endpoint != queue.origin
                || handoff.status != "ready_for_independent_checks"
                || handoff.verification_sequence < 1
                || !checkout.is_absolute()
            {
                return reject(
                    "use a current shared verification handoff prepared after the independent pass",
                );
            }
            let mut request = json!({"action": action_name, "work_id": handoff.work_id,
                "generation": handoff.generation, "head": handoff.head, "pack_digest": handoff.pack_digest,
                "verification_sequence": handoff.verification_sequence});
            let response = queue.request(&request)?;
            let target = &response[key];
            if !target["recorded"].is_null() {
                return crate::print_json(&json!({(key): target["recorded"]}));
            }
            let mut order: WorkOrder = serde_json::from_value(target["work_order"].clone())?;
            order.repository = checkout;
            order.validate()?;
            repository::validate_base(&order)?;
            let pin = order
                .ci_policy_digest
                .as_deref()
                .ok_or("missing shared CI policy digest")?;
            let failure = order
                .ci_failure
                .as_ref()
                .ok_or("missing shared CI failure")?;
            if target["head"] != handoff.head
                || target["base"] != order.base
                || target["repository"] != failure.repository()
            {
                return reject("the policy and handoff do not match the confirmed shared CI draft");
            }
            let draft = crate::review::Draft {
                head: handoff.head,
                reference: target["pull_request"]
                    .as_str()
                    .ok_or("missing bound PR")?
                    .to_owned(),
            };
            let mut proof = match (&action, failure) {
                (
                    OperationsAction::AttestGithubCiChecks { gh, .. },
                    crate::order::CiFailure::Github(_),
                ) => {
                    let policy: crate::github::Policy = load_shared_ci_policy(policy_path, pin)?;
                    if target["repository"] != policy.repository
                        || target["base_branch"] != policy.base_branch
                    {
                        return reject("the GitHub policy does not match the confirmed draft");
                    }
                    json!({"checks": crate::github::observe_shared_ci_checks(&order, &draft, &policy, gh)?})
                }
                (
                    OperationsAction::AttestBitbucketCiChecks { build, .. },
                    crate::order::CiFailure::Bitbucket(_),
                ) => {
                    let policy: crate::twg::Policy = load_shared_ci_policy(policy_path, pin)?;
                    if target["repository"] != policy.repository_name()
                        || target["base_branch"] != policy.base_branch
                    {
                        return reject("the Bitbucket policy does not match the confirmed draft");
                    }
                    crate::twg::observe_candidate_checks(&order, &draft, &policy, *build)?
                }
                _ => return reject("the native check command does not match the provider"),
            };
            proof["publication_key"] = target["publication_key"].clone();
            proof["pull_request"] = json!(draft.reference);
            proof["head"] = json!(draft.head);
            proof["policy_digest"] = json!(pin);
            request["proof"] = proof;
            request
        }
        OperationsAction::ConfirmGithubShadowMerge {
            receipt_id,
            head,
            report_digest,
            policy,
            gh,
        } => {
            let mut request = json!({"action": "confirm_github_shadow_merge",
                "receipt_id": receipt_id, "head": head, "report_digest": report_digest});
            let response = queue.request(&request)?;
            let target = &response["github_merge"];
            if target["head"] != head {
                return reject("the native merge target differs from the evaluated candidate");
            }
            if !target["recorded"].is_null() {
                return crate::print_json(&json!({"github_merge": target["recorded"]}));
            }
            let (policy, digest): (crate::github::Policy, _) = crate::shadow_merge::load(&policy)?;
            let observed = github_merge_observation(
                target,
                &head,
                &policy,
                &digest,
                &gh,
                Instant::now() + Duration::from_secs(45),
            )?.ok_or("GitHub has not reported a merge of the exact reviewed head into the approved destination")?;
            request["observation"] = serde_json::to_value(observed)?;
            request
        }
        OperationsAction::ConfirmBitbucketWorkMerge {
            work_id,
            head,
            policy,
        } => {
            let mut request =
                json!({"action": "confirm_bitbucket_work_merge", "work_id": work_id, "head": head});
            let response = queue.request(&request)?;
            let target = &response["bitbucket_merge"];
            if target["head"] != head {
                return reject("the Bitbucket merge target differs from the requested candidate");
            }
            if !target["recorded"].is_null() {
                return crate::print_json(&json!({"bitbucket_merge": target["recorded"]}));
            }
            let target: BitbucketMergeTarget = serde_json::from_value(target.clone())?;
            let observed = target
                .observe(
                    &head,
                    &read_json(policy)?,
                    &crate::bitbucket_feedback::read_client()?,
                    Instant::now() + Duration::from_secs(45),
                )?
                .ok_or(crate::bitbucket_feedback::UNCONFIRMED_MERGE)?;
            request["publication_key"] = json!(target.publication_key);
            request["observation"] = serde_json::to_value(observed)?;
            request
        }
        OperationsAction::PendingVerifications {
            repository,
            after_work_id,
            limit,
            ..
        } => {
            json!({"action": "pending_verifications", "repository": repository,
                "after_work_id": after_work_id, "limit": limit})
        }
        OperationsAction::PendingMergeChecks {
            repository,
            after_work_id,
            limit,
        } => json!({"action": "pending_merge_checks", "repository": repository,
                    "after_work_id": after_work_id, "limit": limit}),
        OperationsAction::CheckGithubMerges {
            policy,
            gh,
            close_scheduled,
            batch,
        } => {
            let inputs = [
                Some(policy.as_path()),
                Some(gh.as_path()),
                options.ca.as_deref(),
            ];
            check_merge_output_paths(&batch, &inputs)?;
            crate::github::validate_gh(&gh)?;
            let (policy, digest): (crate::github::Policy, _) = crate::shadow_merge::load(&policy)?;
            policy.validate()?;
            batch.check_policy_digest(&digest)?;
            if close_scheduled
                && (!policy.require_merge_queue
                    || batch.budget_seconds <= QUEUE_TIMEOUT.as_secs() * 4)
            {
                return reject(
                    "scheduled closeout requires a native merge-queue policy and more than 20 seconds",
                );
            }
            let closure = close_scheduled.then(|| json!({
                "action": "close_scheduled_github_work", "repository": policy.repository,
                "base_branch": policy.base_branch, "policy_file_digest": digest, "limit": batch.limit.min(20),
            }));
            return run_merge_checks(
                &queue,
                ("github", &policy.repository, &digest),
                batch,
                closure,
                |item, budget| check_github_merge(&queue, item, &policy, &digest, &gh, budget),
            );
        }
        OperationsAction::ReleaseVerificationPickup {
            repository,
            pickup_id,
            ..
        } => {
            json!({"action": "release_verification_pickup", "repository": repository, "pickup_id": pickup_id})
        }
        OperationsAction::RenewVerificationPickup {
            repository,
            pickup_id,
            ..
        } => {
            json!({"action": "renew_verification_pickup", "repository": repository, "pickup_id": pickup_id})
        }
        OperationsAction::CheckBitbucketMerges {
            policy,
            bitbucket_token_file,
            bitbucket_email,
            batch,
        } => {
            let mounted_token =
                crate::evidence_capture::credential_path(crate::bitbucket_feedback::EVIDENCE_TOKEN);
            let inputs = [
                Some(policy.as_path()),
                options.ca.as_deref(),
                bitbucket_token_file.as_deref(),
                mounted_token.as_deref(),
            ];
            check_merge_output_paths(&batch, &inputs)?;
            let (policy, digest): (crate::bitbucket_feedback::Policy, _) =
                crate::shadow_merge::load(&policy)?;
            let repository = policy.shared_merge_repository()?;
            batch.check_policy_digest(&digest)?;
            let client = crate::bitbucket_feedback::read_client_with_credentials(
                bitbucket_token_file.as_deref(),
                bitbucket_email,
            )?;
            return run_merge_checks(
                &queue,
                ("bitbucket", &repository, &digest),
                batch,
                None,
                |item, budget| check_bitbucket_merge(&queue, item, &policy, &client, budget),
            );
        }
        OperationsAction::RecordShadowOutcome {
            receipt_id,
            head,
            report_digest,
            basis,
            expected,
            evidence_file,
            evidence,
            note,
        } => {
            const LIMIT: u64 = 512 * 1024;
            let mut bytes = Vec::new();
            File::open(evidence_file)?
                .take(LIMIT + 1)
                .read_to_end(&mut bytes)?;
            if bytes.is_empty() || bytes.len() as u64 > LIMIT {
                return reject("shadow outcome evidence must contain 1 to 524288 bytes");
            }
            json!({"action": "record_shadow_outcome", "receipt_id": receipt_id, "head": head,
                "report_digest": report_digest, "basis": basis, "expected_eligible": expected == ShadowExpectation::Eligible,
                "evidence_digest": sha256(&bytes), "evidence": evidence, "note": note,
            })
        }
        OperationsAction::ReviewInbox {
            repository,
            owner,
            limit,
            assigned_only,
            after,
            ..
        } => {
            let mut request = json!({"action": "review_inbox", "repository": repository, "owner": owner, "limit": limit});
            if assigned_only {
                request["assigned_only"] = json!(true);
            }
            if let Some(after) = after {
                request["after"] = json!(after);
            }
            request
        }
        OperationsAction::RegisterProjectContext { prepared, replace } => {
            let prepared: Value = read_json(prepared)?;
            if prepared["schema_version"] != 1 || !prepared["release"].is_object() {
                return reject("provide a prepared project context release with recorded approval");
            }
            json!({"action": "register_project_context", "release": prepared["release"],
                "expected_revision": replace})
        }
        OperationsAction::RegisterConfluenceContext {
            policy,
            excerpt,
            approval,
            replace,
        } => {
            let release = crate::confluence_context::prepare_release(policy, excerpt, approval)?;
            json!({"action": "register_project_context", "release": release, "expected_revision": replace})
        }
        OperationsAction::CheckConfluenceContext {
            policy,
            require_current,
            context_reader,
            context_reader_token_file,
            report_file,
        } => {
            let variable = if context_reader {
                "CRAFT_FACTORY_CONTEXT_READER_TOKEN"
            } else {
                "CRAFT_FACTORY_OPERATOR_TOKEN"
            };
            let mounted = crate::evidence_capture::credential_path(variable);
            crate::evidence_capture::check_output_paths(
                &[report_file.as_deref()],
                [
                    options.ca.as_deref(),
                    context_reader_token_file.as_deref(),
                    mounted.as_deref(),
                ]
                .into_iter()
                .flatten(),
                "context check report path must be distinct from the inputs",
            )?;
            let observed =
                crate::confluence_context::observe_source(&policy, report_file.as_deref())?;
            let target = &observed["destination"];
            let mut request =
                json!({"repository": target["repository"], "location": target["location"]});
            if !context_reader {
                request["action"] = json!("inspect_project_context");
            }
            let response = queue.request(&request)?;
            let report = crate::confluence_context::assess_registered_source(
                &observed,
                &response["catalog"],
            )?;
            if let Some(path) = report_file {
                crate::evidence_capture::replace_json(&path, &report)?;
            }
            crate::print_json(&report)?;
            if require_current && report["status"] != "current" {
                return reject(
                    "the Confluence source does not match an active project context pin",
                );
            }
            return Ok(());
        }
        OperationsAction::InspectProjectContext {
            repository,
            location,
            revision,
        } => {
            json!({"action": "inspect_project_context", "repository": repository,
                "location": location, "revision": revision})
        }
        OperationsAction::WithdrawProjectContext {
            repository,
            location,
            revision,
            evidence,
        } => {
            json!({"action": "withdraw_project_context", "repository": repository,
                "location": location, "revision": revision, "evidence": evidence})
        }
        OperationsAction::GrantReviewInbox {
            grant_id,
            reviewer_id,
            repository,
            evidence,
            valid_seconds,
        } => {
            json!({"action": "grant_review_inbox", "grant_id": grant_id, "reviewer_id": reviewer_id,
                "repository": repository, "evidence": evidence, "valid_seconds": valid_seconds})
        }
        OperationsAction::RevokeReviewInbox { grant_id, evidence } => {
            json!({"action": "revoke_review_inbox", "grant_id": grant_id, "evidence": evidence})
        }
        OperationsAction::GrantDiagnosticAccess {
            grant_id,
            diagnostician_id,
            case_id,
            policy_digest,
            from,
            to,
            valid_seconds,
            max_reads,
            evidence,
        } => {
            json!({"action": "grant_diagnostic_access", "grant_id": grant_id,
                "diagnostician_id": diagnostician_id, "case_id": case_id, "policy_digest": policy_digest,
                "from": from, "to": to, "valid_seconds": valid_seconds, "max_reads": max_reads, "evidence": evidence})
        }
        OperationsAction::RevokeDiagnosticAccess { grant_id, evidence } => {
            json!({"action": "revoke_diagnostic_access", "grant_id": grant_id, "evidence": evidence})
        }
        OperationsAction::AuditDiagnosticAccess { grant_id } => {
            json!({"action": "audit_diagnostic_access", "grant_id": grant_id})
        }
        OperationsAction::DiagnosticCaseAccess {
            case_id,
            policy_digest,
            after,
            limit,
        } => {
            json!({"action": "diagnostic_case_access", "case_id": case_id,
                "policy_digest": policy_digest, "after": after, "limit": limit})
        }
        OperationsAction::RetireDiagnosticCase {
            case_id,
            policy_digest,
            evidence,
        } => {
            json!({"action": "retire_diagnostic_case", "case_id": case_id,
                "policy_digest": policy_digest, "evidence": evidence})
        }
        OperationsAction::AssignReview {
            handoff_id,
            work_id,
            generation,
            head,
            verification_sequence,
            reviewer_id,
            evidence,
            queue_repair,
        } => {
            json!({"action": "assign_review", "handoff_id": handoff_id, "work_id": work_id,
                "generation": generation, "head": head, "verification_sequence": verification_sequence,
                "reviewer_id": reviewer_id, "evidence": evidence, "queue_repair": queue_repair})
        }
        OperationsAction::ReassignReview {
            handoff_id,
            replacement_handoff_id,
            head,
            reviewer_id,
            evidence,
            queue_repair,
        } => {
            json!({"action": "reassign_review", "handoff_id": handoff_id,
                "replacement_handoff_id": replacement_handoff_id, "head": head,
                "reviewer_id": reviewer_id, "evidence": evidence, "queue_repair": queue_repair})
        }
        OperationsAction::RouteReview {
            handoff_id,
            work_id,
            generation,
            head,
            verification_sequence,
            roster,
            max_pending,
            evidence,
            queue_repair,
        } => {
            json!({"action": "route_review", "handoff_id": handoff_id, "work_id": work_id,
                "generation": generation, "head": head, "verification_sequence": verification_sequence,
                "roster": roster, "max_pending": max_pending, "evidence": evidence, "queue_repair": queue_repair})
        }
        OperationsAction::RecordReviewDecision {
            handoff_id,
            head,
            decision,
            evidence,
            note,
        } => {
            json!({"action": "record_review_decision", "handoff_id": handoff_id, "head": head,
                "decision": decision, "evidence": evidence, "note": note})
        }
        OperationsAction::ImportGithubReview {
            handoff_id,
            review,
            policy,
            gh,
        } => import_review(&queue, &handoff_id, |target| {
            crate::github::observe_change_request_at(
                &target.draft,
                &target.base,
                &read_json(policy)?,
                &gh,
                review,
                None,
            )
        })?,
        OperationsAction::ImportBitbucketReview {
            handoff_id,
            comment,
            policy,
        } => import_review(&queue, &handoff_id, |target| {
            crate::bitbucket_feedback::observe_assigned(target, &read_json(policy)?, comment)
        })?,
        OperationsAction::ApplyReviewRepair { handoff_id } => {
            json!({"action": "apply_review_repair", "handoff_id": handoff_id})
        }
        OperationsAction::RunnerStatus { runner_id } => {
            json!({"action": "runner_status", "runner_id": runner_id})
        }
        OperationsAction::RunnerEnrollment {
            runner_id,
            repository,
        } => {
            json!({"action": "runner_enrollment", "runner_id": runner_id, "repository": repository})
        }
        OperationsAction::SetRunnerEnrollment {
            runner_id,
            repository,
            expected_revision,
            decision,
            evidence,
        } => {
            json!({"action": "set_runner_enrollment", "runner_id": runner_id, "repository": repository,
                "expected_revision": expected_revision, "enrolled": matches!(decision, EnrollmentDecision::Grant),
                "evidence": evidence})
        }
        OperationsAction::RetryCiPreflight {
            work_id,
            generation,
            evidence,
        } => {
            json!({"action": "retry_ci_preflight", "work_id": work_id, "generation": generation, "evidence": evidence})
        }
        OperationsAction::DispatchStatus { work_id } => {
            json!({"action": "dispatch_status", "work_id": work_id})
        }
        OperationsAction::ReviewWait { repository, limit } => {
            json!({"action": "review_wait", "repository": repository, "limit": limit})
        }
        OperationsAction::SetDispatchPriority {
            work_id,
            expected_revision,
            priority,
            evidence,
        } => {
            json!({"action": "set_dispatch_priority", "work_id": work_id,
                "expected_revision": expected_revision, "priority": priority, "evidence": evidence})
        }
        OperationsAction::SetRunnerMode {
            runner_id,
            expected_revision,
            mode,
            evidence,
        } => {
            json!({"action": "set_runner_mode", "runner_id": runner_id,
                "expected_revision": expected_revision, "mode": mode, "evidence": evidence})
        }
        OperationsAction::HeldPublications => json!({"action": "held_publications"}),
        OperationsAction::RecoveryStatus => json!({"action": "recovery_status"}),
        OperationsAction::Audit { work_id } => json!({"action": "audit", "work_id": work_id}),
        OperationsAction::ProfileUsage { work_id } => {
            json!({"action": "profile_usage", "work_id": work_id})
        }
        OperationsAction::CloseGithubWork {
            work_id,
            merge_receipt_id,
            head,
            evidence,
        } => {
            json!({"action": "close_github_work", "work_id": work_id,
                "merge_receipt_id": merge_receipt_id, "head": head, "evidence": evidence})
        }
        OperationsAction::CloseBitbucketWork {
            work_id,
            publication_key,
            head,
            evidence,
        } => {
            json!({"action": "close_bitbucket_work", "work_id": work_id,
                "publication_key": publication_key, "head": head, "evidence": evidence})
        }
        OperationsAction::QueueUsage {
            repository,
            since_hours,
            limit,
        } => {
            json!({"action": "queue_usage", "repository": repository, "since_hours": since_hours, "limit": limit})
        }
        OperationsAction::CompareProfiles { pairs } => {
            let pairs: Vec<Value> = read_json(pairs)?;
            json!({"action": "compare_profiles", "pairs": pairs})
        }
        OperationsAction::EscalateProfile {
            work_id,
            generation,
            replacement_id,
            profile,
            evidence,
        } => {
            let execution_profile: crate::order::ExecutionProfile = read_json(profile)?;
            if !execution_profile.valid() {
                return reject("invalid replacement execution profile");
            }
            json!({"action": "escalate_profile", "work_id": work_id, "generation": generation,
                "replacement_id": replacement_id, "execution_profile": execution_profile, "evidence": evidence})
        }
        OperationsAction::ApproveProfileFallback {
            work_id,
            replacement_id,
            profile,
            evidence,
            valid_seconds,
        } => {
            let execution_profile: crate::order::ExecutionProfile = read_json(profile)?;
            if !execution_profile.valid() {
                return reject("invalid fallback execution profile");
            }
            json!({"action": "approve_profile_fallback", "work_id": work_id, "replacement_id": replacement_id,
                "execution_profile": execution_profile, "evidence": evidence, "valid_seconds": valid_seconds})
        }
        OperationsAction::RevokeProfileFallback { work_id } => {
            json!({"action": "revoke_profile_fallback", "work_id": work_id})
        }
        OperationsAction::ApproveClaudeRepository {
            repository,
            approval_id,
            evidence,
            valid_seconds,
        } => {
            json!({"action": "approve_claude_repository", "repository": repository, "approval_id": approval_id,
                "evidence": evidence, "valid_seconds": valid_seconds})
        }
        OperationsAction::RevokeClaudeRepository {
            approval_id,
            evidence,
        } => {
            json!({"action": "revoke_claude_repository", "approval_id": approval_id, "evidence": evidence})
        }
        OperationsAction::ApplyProfileFallbacks => json!({"action": "apply_profile_fallbacks"}),
        OperationsAction::ProposeKnowledge {
            proposal_id,
            work_id,
            generation,
            head,
            proposal,
        } => {
            let mut data = Vec::new();
            File::open(proposal)?
                .take(224 * 1024 + 1)
                .read_to_end(&mut data)?;
            if data.len() > 224 * 1024 {
                return reject("knowledge proposal file exceeds 224 KiB");
            }
            let proposal: Value = serde_json::from_slice(&data)?;
            json!({"action": "propose_knowledge", "proposal_id": proposal_id, "work_id": work_id,
                "generation": generation, "head": head, "proposal": proposal})
        }
        OperationsAction::PendingKnowledge { repository } => {
            json!({"action": "pending_knowledge", "repository": repository})
        }
        OperationsAction::InspectKnowledge { proposal_id } => {
            json!({"action": "inspect_knowledge", "proposal_id": proposal_id})
        }
        OperationsAction::ReviewKnowledge {
            proposal_id,
            proposal_digest,
            decision,
            owner,
            evidence,
            verification_sequence,
            repository,
        } => {
            json!({"action": "review_knowledge", "proposal_id": proposal_id, "proposal_digest": proposal_digest,
                "promote": matches!(decision, KnowledgeDecision::Promote), "owner": owner,
                "evidence": evidence, "verification_sequence": verification_sequence,
                "repositories": (!repository.is_empty()).then_some(repository)})
        }
        OperationsAction::WithdrawKnowledge {
            proposal_id,
            release_revision,
            evidence,
        } => {
            json!({"action": "withdraw_knowledge", "proposal_id": proposal_id,
                "release_revision": release_revision, "evidence": evidence})
        }
        OperationsAction::BeginRecovery { epoch } => {
            json!({"action": "begin_recovery", "epoch": epoch})
        }
        OperationsAction::FinishRecovery { epoch, evidence } => {
            json!({"action": "finish_recovery", "epoch": epoch, "evidence": evidence})
        }
    };
    let response = queue.request_bounded(&request, response_limit)?;
    if brief {
        return print_review_inbox(&response["inbox"]);
    }
    crate::print_json(&response)
}

fn print_operator_brief(snapshot: &Value, usage_hours: i32) -> Result<()> {
    let mut output = std::io::stdout().lock();
    let field = |value: &Value, key: &str| -> Result<u64> {
        value[key]
            .as_u64()
            .ok_or_else(|| format!("operator brief is missing {key}").into())
    };
    let queue = &snapshot["queue"];
    let scope = snapshot["repository"]
        .as_str()
        .unwrap_or("All repositories");
    let mode = match (
        queue["paused"].as_bool(),
        queue["recovery_active"].as_bool(),
    ) {
        (Some(_), Some(true)) => "recovery in progress",
        (Some(true), Some(false)) => "paused",
        (Some(false), Some(false)) => "open",
        _ => return reject("operator brief is missing queue state"),
    };
    writeln!(
        output,
        "{} | {}",
        terminal_text(&json!(scope)),
        terminal_text(&snapshot["as_of"])
    )?;
    writeln!(
        output,
        "Shared queue {mode} | review slots {}/{} | token budget {}/{}",
        field(queue, "review_reserved")?,
        field(queue, "review_limit")?,
        field(queue, "tokens_charged_or_reserved")?,
        field(queue, "token_budget")?
    )?;

    let reference =
        |output: &mut std::io::StdoutLock<'_>, item: &Value, keys: &[&str]| -> Result<()> {
            for key in keys {
                let value = terminal_text(&item[*key]);
                if !value.is_empty() {
                    writeln!(output, "  {value}")?;
                }
            }
            Ok(())
        };
    for (key, label, rows, count) in [
        (
            "github_queue",
            "GitHub merge queue",
            "items",
            "unresolved_total",
        ),
        ("github_readiness", "GitHub readiness", "items", "total"),
        (
            "standing_intent_reviews",
            "Standing issue approvals",
            "items",
            "total",
        ),
        ("review", "Review and operator decisions", "items", "total"),
        (
            "assignments",
            "Outstanding reviewer assignments",
            "assignments",
            "total",
        ),
        (
            "backlog",
            "Work waiting on runners",
            "items",
            "queued_total",
        ),
    ] {
        let section = &snapshot[key];
        if matches!(key, "github_readiness" | "standing_intent_reviews") && section.is_null() {
            continue;
        }
        let items = section[rows]
            .as_array()
            .ok_or("operator brief is missing a results section")?;
        let total = field(section, count)?;
        match key {
            "github_queue" => writeln!(
                output,
                "\n{label} | {total} unresolved | {} acknowledged",
                field(section, "queued_total")?
            )?,
            "backlog" => writeln!(
                output,
                "\n{label} | {total} queued | {} expired leases",
                field(section, "expired_lease_total")?
            )?,
            "standing_intent_reviews" => writeln!(
                output,
                "\n{label} | {} ready | {} of {total} shown",
                field(section, "ready")?,
                items.len()
            )?,
            _ => writeln!(output, "\n{label} | {} of {total} shown", items.len())?,
        }
        for item in items {
            let work = terminal_text(&item["work_id"]);
            let repository = terminal_text(&item["repository"]);
            match key {
                "github_queue" => {
                    writeln!(
                        output,
                        "  {} | {work} | {repository} | {}",
                        if item["state"] == "queued" {
                            "Await merge"
                        } else {
                            "Reconcile enqueue"
                        },
                        terminal_text(&item["intent_id"])
                    )?;
                    reference(&mut output, item, &["pull_request"])?;
                }
                "github_readiness" => {
                    let state = match item["state"].as_str() {
                        Some("check_overdue") => "Check did not report",
                        Some("blocked") => "Blocked",
                        Some("waiting_for_check") => "Waiting for check",
                        Some("check_in_progress") => "Awaiting check result",
                        _ => return reject("operator brief has an invalid readiness state"),
                    };
                    writeln!(output, "  {state} | {work} | {repository}")?;
                    reference(&mut output, item, &["pull_request"])?;
                    if let Some(attempted_at) = item["attempted_at"].as_str() {
                        writeln!(output, "  Selected {}", terminal_text(&json!(attempted_at)))?;
                    }
                    if let Some(receipt_id) = item["receipt_id"].as_str() {
                        writeln!(
                            output,
                            "  Last check {} | {}",
                            terminal_text(&item["checked_at"]),
                            terminal_text(&json!(receipt_id))
                        )?;
                    }
                    if let Some(codes) = item["blockers"]
                        .as_array()
                        .filter(|codes| !codes.is_empty())
                    {
                        let codes = codes
                            .iter()
                            .take(3)
                            .map(terminal_text)
                            .collect::<Vec<_>>()
                            .join(", ");
                        writeln!(output, "  Blockers {codes}")?;
                    }
                }
                "review" => {
                    let pickup = &item["verification_pickup"];
                    writeln!(
                        output,
                        "  {} | {work} | {repository} | {}",
                        if pickup.is_null() {
                            review_action(&item["next_action"])
                        } else {
                            "Verification in progress"
                        },
                        terminal_text(&item["issue"])
                    )?;
                    if !pickup.is_null() {
                        writeln!(
                            output,
                            "  Verifier {} | pickup expires {}",
                            terminal_text(&pickup["verifier"]),
                            terminal_text(&pickup["expires_at"])
                        )?;
                    }
                    reference(
                        &mut output,
                        item,
                        &[
                            "draft_url",
                            "review_evidence",
                            "verification_evidence",
                            "merge_receipt_id",
                        ],
                    )?;
                }
                "standing_intent_reviews" => {
                    let state = match item["state"].as_str() {
                        Some("ready_for_approval") => "Approve standing intent",
                        Some("confirm_relay_input") => "Confirm reviewed input in the relay",
                        Some("refresh_observation") => "Refresh issue observation",
                        _ => return reject("operator brief has an invalid standing intent state"),
                    };
                    writeln!(
                        output,
                        "  {state} | {} | {repository} | rule {} | revision {}",
                        terminal_text(&item["issue"]),
                        terminal_text(&item["rule_id"]),
                        field(item, "source_revision")?,
                    )?;
                    if item["tracker"] == "jira" {
                        writeln!(
                            output,
                            "  Jira Cloud ID {}",
                            terminal_text(&item["cloud_id"])
                        )?;
                    }
                    reference(&mut output, item, &["issue_url", "review_token"])?;
                }
                "assignments" => {
                    writeln!(
                        output,
                        "  {work} | {repository} | reviewer {} | {}s",
                        terminal_text(&item["reviewer_id"]),
                        field(item, "wait_seconds")?
                    )?;
                    reference(&mut output, item, &["draft_url"])?;
                }
                _ => {
                    let state = if item["kind"] == "expired_lease" {
                        "Lease expired"
                    } else {
                        "Queued"
                    };
                    writeln!(
                        output,
                        "  {state} | {work} | {repository} | since {}",
                        terminal_text(&item["since"])
                    )?;
                    if let Some(blockers) = item["blockers"]
                        .as_array()
                        .filter(|codes| !codes.is_empty())
                    {
                        let labels = blockers
                            .iter()
                            .take(3)
                            .map(dispatch_blocker)
                            .collect::<Vec<_>>()
                            .join(", ");
                        if blockers.len() > 3 {
                            writeln!(
                                output,
                                "  Blocked by {labels} (+{} more)",
                                blockers.len() - 3
                            )?;
                        } else {
                            writeln!(output, "  Blocked by {labels}")?;
                        }
                    }
                }
            }
        }
    }
    let totals = &snapshot["usage"]["totals"];
    writeln!(
        output,
        "\nUsage over {usage_hours}h | {} attempts | usage reported for {}",
        field(totals, "attempts")?,
        field(totals, "attempts_with_usage")?
    )?;
    writeln!(
        output,
        "  {} reported tokens | {} reserved for attempts without usage",
        field(totals, "reported_tokens")?,
        field(totals, "reserved_tokens_without_usage")?
    )?;
    for group in snapshot["usage"]["groups"]
        .as_array()
        .ok_or("operator brief is missing usage groups")?
    {
        let profile = ["adapter", "model", "reasoning_effort"]
            .iter()
            .map(|key| terminal_text(&group[*key]))
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        writeln!(
            output,
            "  {} | {profile} | {} attempts | {} reported tokens",
            terminal_text(&group["repository"]),
            field(group, "attempts")?,
            field(group, "reported_tokens")?
        )?;
    }
    if totals.get("overrun_attempts").is_some() {
        let total = field(totals, "overrun_attempts")?;
        if total > 0 {
            let overruns = snapshot["usage"]["overruns"]
                .as_array()
                .ok_or("operator brief is missing token overruns")?;
            writeln!(
                output,
                "\nToken overruns ({}/{total}) | {} tokens above reservation",
                overruns.len(),
                field(totals, "tokens_above_reservation")?
            )?;
            for attempt in overruns {
                writeln!(
                    output,
                    "  {} | {} | generation {} | {} reported / {} reserved",
                    terminal_text(&attempt["repository"]),
                    terminal_text(&attempt["work_id"]),
                    field(attempt, "generation")?,
                    field(attempt, "reported_tokens")?,
                    field(attempt, "token_limit")?
                )?;
            }
        }
    }
    Ok(())
}

fn dispatch_blocker(code: &Value) -> String {
    match code.as_str() {
        Some("queue_paused") => "paused queue",
        Some("no_enrolled_runner") => "no enrolled runner",
        Some("no_active_enrolled_runner") => "no active enrolled runner",
        Some("waiting_for_standing_runner") => "standing runner preference",
        Some("standing_delegation_not_current") => "stale delegation",
        Some("release_predecessor_not_ready") => "release prerequisite",
        Some("attempt_limit_reached") => "attempt limit",
        Some("claude_repository_approval_required") => "Claude repository approval",
        Some("token_budget") => "token budget",
        Some("review_capacity_at_snapshot") => "review capacity",
        _ => return terminal_text(code),
    }
    .to_owned()
}

fn print_review_inbox(inbox: &Value) -> Result<()> {
    if !inbox
        .as_object()
        .is_some_and(|inbox| inbox.contains_key("items"))
    {
        let assignments = inbox["assignments"]
            .as_array()
            .ok_or("review assignments are missing")?;
        let mut output = std::io::stdout().lock();
        writeln!(output, "Assigned reviews   {} shown", assignments.len())?;
        for assignment in assignments {
            writeln!(
                output,
                "\n{} | {} | {}",
                terminal_text(&assignment["id"]),
                terminal_text(&assignment["work_id"]),
                terminal_text(&assignment["repository"])
            )?;
            for (label, key) in [
                ("Issue", "issue"),
                ("Intent", "intent_excerpt"),
                ("Candidate", "head"),
                ("Draft", "draft_url"),
                ("Test summary", "review_summary"),
                ("Independent verification", "verification_evidence"),
                ("Review request", "request_evidence"),
            ] {
                let value = terminal_text(&assignment[key]);
                if !value.is_empty() {
                    writeln!(output, "  {label}  {value}")?;
                }
            }
            if let Some(criteria) = assignment["acceptance_excerpts"].as_array() {
                for criterion in criteria {
                    writeln!(output, "  Acceptance  {}", terminal_text(criterion))?;
                }
            }
            if assignment["queue_repair"] == true {
                writeln!(output, "  A change request can queue a repair.")?;
            }
        }
        let after = terminal_text(&inbox["next_after"]);
        if !after.is_empty() {
            writeln!(
                output,
                "\nMore assigned reviews. Continue with --after {after}"
            )?;
        }
        return Ok(());
    }
    let items = inbox["items"]
        .as_array()
        .ok_or("review inbox entries are missing")?;
    let total = inbox["total"]
        .as_u64()
        .ok_or("review inbox count is missing")?;
    let mut output = std::io::stdout().lock();
    if let Some(assignments) = inbox["assignments"].as_array() {
        for assignment in assignments {
            writeln!(
                output,
                "Assigned review | {} | {}",
                terminal_text(&assignment["id"]),
                terminal_text(&assignment["work_id"])
            )?;
            writeln!(
                output,
                "  Candidate  {}",
                terminal_text(&assignment["head"])
            )?;
            writeln!(
                output,
                "  Draft  {}",
                terminal_text(&assignment["draft_url"])
            )?;
            if assignment["queue_repair"] == true {
                writeln!(output, "  A change request can queue a repair.")?;
            }
        }
        if !assignments.is_empty() {
            writeln!(output)?;
        }
    }
    writeln!(output, "Review inbox   {} of {total} shown", items.len())?;
    for item in items {
        let action = review_action(&item["next_action"]);
        writeln!(
            output,
            "\n{action} | {} | {}",
            terminal_text(&item["work_id"]),
            terminal_text(&item["repository"])
        )?;
        if let Some(criteria) = item["acceptance_excerpts"].as_array() {
            for criterion in criteria {
                let criterion = terminal_text(criterion);
                if !criterion.is_empty() {
                    writeln!(output, "  Acceptance  {criterion}")?;
                }
            }
        }
        for (label, value) in [
            ("Issue", &item["issue"]),
            ("Intent", &item["intent_excerpt"]),
            ("Candidate", &item["head"]),
            ("Evidence", &item["independent_verification"]["evidence"]),
            ("Draft", &item["publication"]["result_url"]),
            ("Merge result", &item["merge_closure"]["result_commit"]),
            ("Merge receipt", &item["merge_closure"]["merge_receipt_id"]),
            ("Publication key", &item["merge_closure"]["publication_key"]),
            ("Prior draft", &item["prior_draft_url"]),
            ("Waiting on", &item["waiting_on"]),
            ("Review handoff", &item["pending_repair"]["handoff_id"]),
            ("Feedback", &item["pending_repair"]["feedback_excerpt"]),
            ("Feedback evidence", &item["pending_repair"]["evidence"]),
        ] {
            let text = terminal_text(value);
            if !text.is_empty() {
                writeln!(output, "  {label}  {text}")?;
            }
        }
        match item["pending_repair"]["blocker"].as_str() {
            Some("queue_paused") => writeln!(output, "  Admission is paused.")?,
            Some("attempt_cap") => writeln!(output, "  The approved attempt limit is exhausted.")?,
            _ => (),
        }
        if let Some(count) = item["pending_repair"]["outstanding_assignments"].as_u64() {
            let noun = if count == 1 {
                "assignment is"
            } else {
                "assignments are"
            };
            writeln!(output, "  {count} review {noun} awaiting a decision.")?;
        }
        if let Some(count) = item["pending_repair"]["decision_count"]
            .as_u64()
            .filter(|count| *count > 1)
        {
            writeln!(
                output,
                "  {count} change requests on this revision. See the work audit."
            )?;
        }
    }
    Ok(())
}

fn review_action(action: &Value) -> &'static str {
    match action.as_str() {
        Some("reconcile_publication") => "Reconcile publication",
        Some("close_merged_work") => "Close merged work",
        Some("resolve_failed_verification") => "Resolve failed verification",
        Some("resolve_attempt_cap") => "Resolve attempt cap",
        Some("check_standing_delegation") => "Check standing delegation",
        Some("resolve_review_changes") => "Resolve requested changes",
        Some("inspect_draft") => "Inspect draft",
        Some("verify_candidate") => "Verify candidate",
        Some("wait_for_predecessor") => "Wait for predecessor",
        Some("choose_publication") => "Choose publication",
        Some("inspect_reservation") => "Inspect publication reservation",
        Some("inspect_absent_publication") => "Inspect absent publication",
        _ => "Inspect work order",
    }
}

fn terminal_text(value: &Value) -> String {
    let Some(value) = value.as_str() else {
        return String::new();
    };
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|character| !character.is_control())
        .collect()
}

fn bind_checkout(
    mut payload: Value,
    checkout: &Checkout,
    work_id: &str,
    repository: &str,
    generation: i64,
) -> Result<WorkOrder> {
    payload
        .as_object_mut()
        .ok_or("shared work order is not a JSON object")?
        .insert("repository".into(), serde_json::to_value(&checkout.path)?);
    let order: WorkOrder = serde_json::from_value(payload)?;
    order.validate()?;
    if order.id != work_id || generation < 1 {
        return reject("shared queue returned inconsistent claim identity");
    }
    repository::validate_base(&order)?;
    if order
        .ci_failure
        .as_ref()
        .is_some_and(|ci| ci.repository() != repository)
    {
        return reject("claimed CI work refers to a different repository");
    }
    Ok(order)
}

fn recheck_shared_ci(order: &WorkOrder, checkout: &Checkout) -> Result<()> {
    repository::validate_ci_anchor(order)?;
    match &order.ci_failure {
        None => Ok(()),
        Some(crate::order::CiFailure::Bitbucket(original)) => {
            let path = checkout
                .bitbucket_ci
                .as_ref()
                .ok_or("map the approved Bitbucket CI policy before claiming shared CI work")?;
            let pin = order
                .ci_policy_digest
                .as_deref()
                .ok_or("shared Bitbucket CI work requires an approved policy digest")?;
            let policy: crate::twg::Policy = load_shared_ci_policy(path, pin)?;
            repository::validate_bitbucket_remote(order, &policy.repository_name())?;
            let current =
                crate::twg::failed_step(&policy, original.build, &original.step_uuid, &order.base)?;
            if &current != original {
                return reject(
                    "the admitted Bitbucket failure changed; review the repair intent again",
                );
            }
            Ok(())
        }
        Some(crate::order::CiFailure::Github(original)) => {
            let source = checkout.github_ci.as_ref().ok_or(
                "map the approved GitHub CI policy and CLI before claiming shared CI work",
            )?;
            let pin = order
                .ci_policy_digest
                .as_deref()
                .ok_or("shared GitHub CI work requires an approved policy digest")?;
            let policy: crate::github::Policy = load_shared_ci_policy(&source.policy, pin)?;
            repository::validate_github_remote(order, &policy.repository)?;
            let current = crate::github::failed_job(
                &policy,
                &source.gh,
                original.run_id,
                original.run_attempt,
                &order.base,
            )?;
            if &current != original {
                return reject(
                    "the admitted GitHub failure changed; review the repair intent again",
                );
            }
            Ok(())
        }
    }
}

pub fn request_repair(options: RequestRepairOptions) -> Result<Value> {
    let queue = Queue::open(&options.queue, options.ca.as_deref(), QueueRole::Operator)?;
    queue.request(&json!({
        "action": "request_repair", "work_id": options.work_id, "generation": options.generation,
        "head": options.head, "native_url": options.draft, "evidence": options.evidence, "note": options.note,
    }))
}

fn read_pack(directory: &Path, repaired: bool) -> Result<Vec<u8>> {
    let mut pack = Vec::new();
    let relative = if repaired {
        "repair.pack"
    } else {
        "work/candidate.pack"
    };
    File::open(directory.join(relative))?
        .take(PACK_LIMIT + 1)
        .read_to_end(&mut pack)?;
    if pack.len() as u64 > PACK_LIMIT || !pack.starts_with(b"PACK") {
        return reject("candidate pack is invalid or exceeds 512 KiB");
    }
    Ok(pack)
}
