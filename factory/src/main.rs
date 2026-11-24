mod bitbucket;
mod bitbucket_capture;
mod bitbucket_feedback;
mod bitbucket_publish;
mod bitbucket_read;
mod bitbucket_shadow;
mod build_capture;
mod ci_dispatch;
mod ci_verification;
mod claude_runner;
mod confluence_context;
mod dependency_compatibility;
mod dependency_update;
mod evidence_capture;
mod github;
mod github_capture;
mod github_issue_capture;
mod jira_capture;
mod jira_standing;
mod kubernetes_capture;
mod order;
mod process_tree;
mod queue_identity;
mod repository;
mod review;
mod runner;
mod series_comparison;
mod series_proposal;
mod setup;
mod shadow_merge;
mod shared_runner;
mod smoke;
mod snapshot;
mod store;
mod twg;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use craft_factory_evidence::{
    bitbucket_change, bitbucket_review, build_provenance, diagnosis, github_change,
    kubernetes_inventory, release_sources, release_trace, remediation,
};
use order::WorkOrder;
use review::{Draft, ReviewRecord, VerificationHandoff};
use store::{Attestation, Receipt, Store};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn reject<T>(message: impl Into<String>) -> Result<T> {
    Err(message.into().into())
}

#[derive(Parser)]
#[command(version, about = "Run supervised software factory workflows")]
struct Cli {
    /// Path to the local execution database.
    #[arg(long, global = true)]
    state: Option<PathBuf>,
    #[command(subcommand)]
    command: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Run the credential-free workflow in a new disposable directory.
    Smoke {
        #[arg(long)]
        destination: PathBuf,
    },
    /// Print a versioned JSON Schema.
    Schema { kind: SchemaKind },
    /// Write a fresh protected bearer token and print its digest-only queue identity record.
    IssueQueueIdentity(queue_identity::Options),
    /// Trace an observed resource back to the deployed artifact and approved change.
    TraceRelease(release_trace::Options),
    /// Build a release trace directly from a pinned archive of original evidence.
    TraceReleaseSources(release_sources::Options),
    /// Combine the three pinned collector outputs and write a verified release manifest.
    AssembleReleaseSources(release_sources::AssembleOptions),
    /// Add a running Kubernetes Pod observation to a release evidence bundle.
    ImportKubernetesInventory(kubernetes_inventory::Options),
    /// Verify the Pod and its ReplicaSet and add both to the release evidence.
    ImportKubernetesDeployment(kubernetes_inventory::DeploymentOptions),
    /// Capture one approved Kubernetes Pod and ReplicaSet into a source archive.
    CollectKubernetesRelease(kubernetes_capture::Options),
    /// Verify a signed build attestation and add its artifact-to-commit evidence.
    ImportBuildProvenance(build_provenance::Options),
    /// Capture one digest-pinned, signed build attestation from an approved evidence archive.
    CollectBuildProvenance(build_capture::Options),
    /// Sign trusted CI metadata and publish an immutable build envelope.
    PublishBuildProvenance(build_capture::PublishOptions),
    /// Bind a merged Bitbucket pull request to an approved change record.
    ImportBitbucketChange(bitbucket_change::Options),
    /// Capture the exact merged Bitbucket pull request named in a pinned approval record.
    CollectBitbucketChange(bitbucket_capture::Options),
    /// Collect the original merged GitHub PR and its pinned approval.
    CollectGithubChange(github_capture::Options),
    /// Capture a merged Bitbucket PR and its exact human approval comment.
    CollectBitbucketReview(bitbucket_capture::ReviewOptions),
    /// Check whether the approved remediation is running on every required target.
    VerifyRemediation(remediation::Options),
    /// Verify all required remediation targets from their original release evidence archives.
    VerifyRemediationSources(remediation::SourceOptions),
    /// Prepare a verified rollout index and a catalog entry for operator review.
    PrepareRolloutArchive(remediation::PrepareArchiveOptions),
    /// Evaluate Bitbucket merge evidence in shadow mode.
    ShadowMerge(shadow_merge::Options),
    /// Prepare a reviewed dependency update from pinned Git and OSV evidence.
    PrepareDependencyUpdate(dependency_update::IntakeOptions),
    /// Sign a complete OSV report and its lockfile binding in protected CI.
    SealDependencyScan(dependency_update::SealScanOptions),
    /// Evaluate paired CI compatibility evidence for a dependency update.
    VerifyDependencyCompatibility(dependency_compatibility::Options),
    /// Sign one CI compatibility receipt under the approved dependency policy.
    SealDependencyCompatibilityRun(dependency_compatibility::SealOptions),
    /// Sign one pre-draft CI repair result in the independent verifier.
    SealCiRepairRun(ci_verification::SealOptions),
    /// Publish a GitHub repair draft after checking paired independent results.
    PublishCiDraft(ci_verification::PublishOptions),
    /// Correlate approved redacted telemetry with the observed running release.
    DiagnoseRelease(diagnosis::Options),
    /// Diagnose approved telemetry using a pinned archive of original release evidence.
    DiagnoseReleaseSources(release_sources::DiagnosisOptions),
    /// Extract approved structured events and gauge samples from an OTLP archive.
    ImportDiagnosticOtlp(diagnosis::otlp::Options),
    /// View a shared diagnostic report using a scoped credential.
    DiagnoseSharedRelease(shared_runner::DiagnoseSharedOptions),
    /// Run a read-only Codex coordinator to draft a release proposal for review.
    DraftSeriesProposal(series_proposal::DraftOptions),
    /// Inspect the exact proposal and ordered work before approving a release series.
    ReviewSeriesProposal(series_proposal::ReviewOptions),
    /// Compare paired coordinator runs, usage, and proposal reviews from local evidence.
    CompareSeriesProfiles { manifest: PathBuf },
    /// Admit, inspect, or cancel shared work and control queue admission.
    SharedWork(shared_runner::WorkOptions),
    /// Authorize, send, or reconcile an exact GitHub merge-queue request.
    SharedGithubEnqueue(shared_runner::github_enqueue::Options),
    /// Register or inspect an ordered cross-repository release.
    SharedSeries(shared_runner::SeriesOptions),
    /// Check local prerequisites and optionally probe logged-in provider tools.
    Doctor {
        #[arg(long)]
        codex: Option<PathBuf>,
        #[arg(long)]
        codex_home: Option<PathBuf>,
        #[arg(long)]
        gh: Option<PathBuf>,
    },
    /// Validate and reserve a pinned work order.
    Delegate {
        order: PathBuf,
        /// Approved snapshot matching the order's SHA-256 tracker reference.
        #[arg(long, visible_alias = "issue-snapshot")]
        jira_snapshot: Option<PathBuf>,
    },
    /// Delegate a repair for a selected, failed Bitbucket branch step.
    DelegateBitbucketFailure {
        order: PathBuf,
        policy: PathBuf,
        #[arg(long)]
        build: u64,
        #[arg(long)]
        step: String,
        #[arg(long)]
        jira_snapshot: Option<PathBuf>,
        /// Evaluate admission without saving a work order or starting a runner.
        #[arg(long)]
        shadow: bool,
    },
    /// Delegate a repair for one approved, failed GitHub Actions job.
    DelegateGithubFailure {
        order: PathBuf,
        policy: PathBuf,
        #[arg(long)]
        gh: PathBuf,
        #[arg(long)]
        run: u64,
        #[arg(long)]
        attempt: u64,
        #[arg(long)]
        jira_snapshot: Option<PathBuf>,
    },
    /// Poll an allowlisted Bitbucket default pipeline for one approved repair step.
    PollBitbucketFailure {
        order: PathBuf,
        policy: PathBuf,
        #[arg(long)]
        step_name: String,
        #[arg(long)]
        jira_snapshot: Option<PathBuf>,
        /// Allow admission into the review slot. Otherwise record a shadow decision.
        #[arg(long)]
        admit: bool,
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=10))]
        daily_limit: u32,
    },
    /// Poll the newest GitHub Actions push run for one approved repair job.
    PollGithubFailure {
        order: PathBuf,
        policy: PathBuf,
        #[arg(long)]
        gh: PathBuf,
        #[arg(long)]
        jira_snapshot: Option<PathBuf>,
        /// Allow admission into the review slot. Otherwise record a shadow decision.
        #[arg(long)]
        admit: bool,
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=10))]
        daily_limit: u32,
    },
    /// Pause or resume recurring CI admission in this local database.
    SetCiIntake { setting: IntakeSetting },
    /// Read the most recent shadow and admission decisions.
    CiPollHistory {
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=200))]
        limit: u32,
    },
    /// Attach or verify an issue snapshot against an existing order's pin.
    #[command(visible_alias = "pin-issue-snapshot")]
    PinJiraSnapshot { id: String, snapshot: PathBuf },
    /// Compare a frozen supervised snapshot with the current Jira issue.
    CheckJiraSnapshot { id: String, policy: PathBuf },
    /// Compare a frozen supervised snapshot with the current GitHub issue.
    CheckGithubIssueSnapshot { id: String, policy: PathBuf },
    /// Inspect an order and its recorded events.
    Status { id: String },
    /// Print a short Markdown brief for the current candidate.
    Brief { id: String },
    /// Prepare a separate checkout at the pinned base or reviewed repair head.
    Prepare {
        id: String,
        #[arg(long)]
        destination: PathBuf,
        /// Write the pinned instructions and reviewed feedback beside the checkout.
        #[arg(long)]
        handoff: bool,
    },
    /// Create a clean checkout of the exact current candidate for verification.
    PrepareVerification {
        id: String,
        #[arg(long)]
        head: String,
        #[arg(long)]
        destination: PathBuf,
        /// Issue a verification packet bound to the current draft and trusted checks.
        #[arg(long)]
        handoff: bool,
    },
    /// Start a bounded attempt with the pinned agent adapter.
    Run {
        id: String,
        #[arg(long)]
        destination: PathBuf,
        #[command(flatten)]
        adapter: runner::AdapterOptions,
        /// Require the current Jira issue to match the approved snapshot before starting.
        #[arg(long)]
        jira_policy: Option<PathBuf>,
        /// Require the GitHub issue to match the approved snapshot before starting.
        #[arg(long, conflicts_with = "jira_policy")]
        github_issue_policy: Option<PathBuf>,
        #[arg(long, default_value_t = 900, value_parser = clap::value_parser!(u64).range(1..=3600))]
        timeout_seconds: u64,
    },
    /// Start or explicitly retry a scheduled CI repair after rechecking its failure.
    RunCiRepair(ci_dispatch::RepairOptions),
    /// Poll one approved CI job and optionally admit and start its repair.
    RunCiCycle(ci_dispatch::CycleOptions),
    /// Claim one shared order and run its pinned agent while holding the queue lease.
    RunShared(shared_runner::Options),
    /// Poll the shared queue and supervise one agent attempt at a time.
    RunSharedLoop(shared_runner::LoopOptions),
    /// Check a shared runner's local setup and repository enrollment.
    CheckSharedRunner(shared_runner::CheckRunnerOptions),
    /// Prepare a shared candidate in a clean checkout for independent checks.
    PrepareSharedVerification(shared_runner::VerificationOptions),
    /// Prepare the next candidate awaiting independent verification in a scoped repository.
    PrepareNextVerification(shared_runner::NextVerificationOptions),
    /// Record an independent verification result for a shared candidate.
    AttestSharedVerification(shared_runner::AttestationOptions),
    /// Verify pinned dependency evidence and record the exact shared candidate's verdict.
    AttestSharedDependencyCompatibility(shared_runner::DependencyAttestationOptions),
    /// Transfer a drained local database to the shared queue or roll it back.
    CutoverShared(shared_runner::CutoverOptions),
    /// Reconcile an uncertain publication using a read-only Bitbucket draft check.
    ReconcileSharedDraft(shared_runner::ReconcileDraftOptions),
    /// Confirm a manually opened GitHub draft against a verified shared candidate.
    BindSharedGithubReview(shared_runner::BindGithubReviewOptions),
    /// Reserve a draft creation or an update to the existing draft after repair.
    ReserveSharedDraft(shared_runner::ReserveDraftOptions),
    /// Submit a fresh ticket observation to a standing delegation rule.
    ObserveStandingTicket(shared_runner::ObserveStandingOptions),
    /// Read approved Jira tickets and relay current standing-delegation observations.
    PollStandingJira(jira_standing::Options),
    /// Prepare a digest-pinned snapshot of a reviewed Jira issue for supervised delegation.
    CaptureJiraSnapshot(jira_capture::Options),
    /// Prepare a digest-pinned snapshot of a reviewed GitHub issue.
    CaptureGithubIssueSnapshot(github_issue_capture::Options),
    /// Observe one pinned GitHub issue under a standing delegation rule.
    PollStandingGithubIssue(github_issue_capture::StandingOptions),
    /// Capture a published Confluence revision and prepare an approved context release.
    CaptureConfluenceContext(confluence_context::Options),
    /// Execute one reserved draft-only Bitbucket publication.
    PublishSharedDraft(shared_runner::PublishDraftOptions),
    /// Return a confirmed shared draft for repair on its exact reviewed revision.
    RequestSharedRepair(shared_runner::RequestRepairOptions),
    /// Read shared queue telemetry and audit evidence or manage restore recovery.
    SharedOps(shared_runner::OperationsOptions),
    #[command(hide = true)]
    RunnerChild,
    /// Submit a commit from an approved tool for verification.
    Submit {
        id: String,
        #[arg(long)]
        commit: String,
        /// Import the exact commit from this separate worktree before submitting it.
        #[arg(long)]
        checkout: Option<PathBuf>,
        /// Previous reviewed head when submitting a repair.
        #[arg(long)]
        replaces: Option<String>,
        #[arg(long)]
        note: String,
    },
    /// Advance an unverified draft to a content-identical child commit for a CI retry.
    RetryCi {
        id: String,
        #[arg(long)]
        previous: String,
        #[arg(long)]
        commit: String,
    },
    /// Record the published PR and head as observed by the operator.
    BindDraft {
        id: String,
        #[arg(long)]
        reference: String,
        #[arg(long)]
        head: String,
        /// Require the current Jira issue to match the approved snapshot before recording the draft.
        #[arg(long)]
        jira_policy: Option<PathBuf>,
        /// Require the GitHub issue to match the approved snapshot before recording the draft.
        #[arg(long, conflicts_with = "jira_policy")]
        github_issue_policy: Option<PathBuf>,
    },
    /// Hold the current candidate when the human publication outcome is unclear.
    HoldPublication {
        id: String,
        #[arg(long)]
        head: String,
        #[arg(long)]
        reason: String,
    },
    /// Clear a hold after recording evidence that publication did not occur.
    ClearPublication { id: String, record: PathBuf },
    /// Record an operator attestation of independent verification.
    AttestChecks {
        id: String,
        record: PathBuf,
        #[arg(long)]
        handoff: Option<PathBuf>,
    },
    /// Read trusted check outcomes for the current GitHub PR revision.
    CheckGithub {
        id: String,
        policy: PathBuf,
        #[arg(long)]
        gh: PathBuf,
    },
    /// Evaluate a GitHub merge using fresh read-only evidence.
    ShadowMergeGithub {
        id: String,
        policy: PathBuf,
        #[arg(long)]
        gh: PathBuf,
        #[arg(long)]
        approved_policy_digest: String,
        #[arg(long)]
        approved_protection_digest: String,
        #[arg(long)]
        require_eligible: bool,
    },
    /// Read a Bitbucket pipeline and its required steps using a saved Craft profile.
    CheckBitbucket {
        id: String,
        policy: PathBuf,
        #[arg(long)]
        pipeline: String,
    },
    /// Verify the Bitbucket PR, Git refs, and pipeline with a saved TWG OAuth profile.
    CheckBitbucketTwg {
        id: String,
        policy: PathBuf,
        #[arg(long)]
        build: u64,
        /// Repository policy for revision-bound approval evidence at closure.
        #[arg(long)]
        feedback_policy: Option<PathBuf>,
    },
    /// Record native feedback and open the same PR for a repair.
    RequestChanges { id: String, record: PathBuf },
    /// Open a repair from an approved GitHub review on the current PR head.
    RequestChangesGithub {
        id: String,
        policy: PathBuf,
        #[arg(long)]
        gh: PathBuf,
        #[arg(long)]
        review: u64,
        /// Local UTF-8 file with the operator-reviewed repair summary.
        #[arg(long)]
        summary: PathBuf,
    },
    /// Open a repair from an approved Bitbucket comment and active reviewer decision.
    RequestChangesBitbucket {
        id: String,
        policy: PathBuf,
        #[arg(long)]
        comment: u64,
        /// Local UTF-8 file with the operator-reviewed repair summary.
        #[arg(long)]
        summary: PathBuf,
    },
    /// Record the observed final outcome and free the review slot.
    Close { id: String, record: PathBuf },
    /// Read a completed GitHub merge for the verified revision and free the review slot.
    CloseGithub {
        id: String,
        policy: PathBuf,
        #[arg(long)]
        gh: PathBuf,
    },
    /// Read a completed Bitbucket merge for the verified revision and free the review slot.
    CloseBitbucket {
        id: String,
        policy: PathBuf,
        /// Approved PR comment ID. Repeat for distinct reviewers.
        #[arg(long = "approval-comment")]
        approval_comments: Vec<u64>,
    },
    /// Stop the local workflow and release its review slot.
    Cancel {
        id: String,
        #[arg(long)]
        reason: String,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum SchemaKind {
    VerificationHandoff,
    KubernetesCapturePolicy,
    BuildCapturePolicy,
    BuildArchivePolicy,
    BuildProvenanceInput,
    ReleaseSourceManifest,
    BitbucketChangeApproval,
    GithubChangeApproval,
    BitbucketReviewPolicy,
    BuildProvenancePolicy,
    ShadowMergePolicy,
    ShadowMergeObservation,
    SeriesProposal,
    SeriesComparison,
    ExecutionProfile,
    WorkOrder,
    ReleaseEvidence,
    RemediationPolicy,
    RemediationSourceIndex,
    DependencyUpdateInput,
    DependencyScanRefresh,
    DependencyCompatibilityPolicy,
    DependencyCompatibilityRun,
    CiVerificationPolicy,
    CiVerificationRun,
    DiagnosticPolicy,
    DiagnosticTelemetry,
    DiagnosticOtlpPolicy,
    Receipt,
    ReviewRecord,
    GithubPolicy,
    BitbucketPolicy,
    BitbucketFeedbackPolicy,
    TwgBitbucketPolicy,
}

#[derive(Clone, Copy, ValueEnum)]
enum IntakeSetting {
    Pause,
    Resume,
}

fn read_json<T: serde::de::DeserializeOwned>(path: impl AsRef<std::path::Path>) -> Result<T> {
    Ok(serde_json::from_reader(std::fs::File::open(path)?)?)
}

fn print_json(value: &impl serde::Serialize) -> Result<()> {
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, value)?;
    writeln!(stdout)?;
    Ok(())
}

fn observe_checks(
    path: &std::path::Path,
    id: &str,
    provider: &str,
    observe: impl FnOnce(&Receipt) -> Result<serde_json::Value>,
) -> Result<Receipt> {
    let mut store = Store::open(path)?;
    let receipt = store.status(id)?;
    let draft = receipt
        .draft
        .as_ref()
        .ok_or("bind the human-published pull request first")?;
    let sequence = store.start_check(id, draft)?;
    match observe(&receipt) {
        Ok(observation) => store.check_event(
            id,
            draft,
            sequence,
            &format!("{provider}_checks_observed"),
            &serde_json::to_string(&observation)?,
        ),
        Err(error) => {
            store.check_event(
                id,
                draft,
                sequence,
                &format!("{provider}_checks_unavailable"),
                &error.to_string(),
            )?;
            Err(error)
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    if let Action::Smoke { destination } = cli.command {
        return print_json(&smoke::run(destination)?);
    }
    if let Action::IssueQueueIdentity(options) = cli.command {
        return print_json(&queue_identity::issue(options)?);
    }
    if matches!(cli.command, Action::RunnerChild) {
        return runner::run_child();
    }
    if let Action::TraceRelease(options) = cli.command {
        return print_json(&release_trace::run(options).map_err(|error| error.to_string())?);
    }
    if let Action::TraceReleaseSources(options) = cli.command {
        let require_complete = options.require_complete;
        let report = release_sources::run(options).map_err(|error| error.to_string())?;
        print_json(&report)?;
        if require_complete && !report.trace.is_complete() {
            return reject("release evidence chain is incomplete");
        }
        return Ok(());
    }
    if let Action::AssembleReleaseSources(options) = cli.command {
        let (manifest, report) =
            release_sources::assemble(&options).map_err(|error| error.to_string())?;
        if !report.trace.is_complete() {
            print_json(&report)?;
            return reject("release evidence chain is incomplete; manifest was not written");
        }
        evidence_capture::write_new(&options.destination, &manifest)?;
        return print_json(&report);
    }
    if let Action::ImportKubernetesInventory(options) = cli.command {
        return print_json(&kubernetes_inventory::run(options).map_err(|error| error.to_string())?);
    }
    if let Action::ImportKubernetesDeployment(options) = cli.command {
        return print_json(
            &kubernetes_inventory::import_deployment(options).map_err(|error| error.to_string())?,
        );
    }
    if let Action::CollectKubernetesRelease(options) = cli.command {
        return print_json(&kubernetes_capture::run(options)?);
    }
    if let Action::ImportBuildProvenance(options) = cli.command {
        return print_json(&build_provenance::run(options).map_err(|error| error.to_string())?);
    }
    if let Action::CollectBuildProvenance(options) = cli.command {
        return print_json(&build_capture::run(options)?);
    }
    if let Action::PublishBuildProvenance(options) = cli.command {
        return print_json(&build_capture::publish(options)?);
    }
    if let Action::ImportBitbucketChange(options) = cli.command {
        return print_json(&bitbucket_change::run(options).map_err(|error| error.to_string())?);
    }
    if let Action::CollectBitbucketChange(options) = cli.command {
        return print_json(&bitbucket_capture::run(options)?);
    }
    if let Action::CollectGithubChange(options) = cli.command {
        return print_json(&github_capture::run(options)?);
    }
    if let Action::CollectBitbucketReview(options) = cli.command {
        return print_json(&bitbucket_capture::run_review(options)?);
    }
    if let Action::VerifyRemediation(options) = cli.command {
        let require_complete = options.require_complete;
        let report = remediation::run(options).map_err(|error| error.to_string())?;
        print_json(&report)?;
        if require_complete && !report.rollout_verified {
            return reject("remediation rollout remains open");
        }
        return Ok(());
    }
    if let Action::VerifyRemediationSources(options) = cli.command {
        let require_complete = options.require_complete;
        let report = remediation::run_sources(options).map_err(|error| error.to_string())?;
        print_json(&report)?;
        if require_complete && !report.rollout_verified {
            return reject("remediation rollout remains open");
        }
        return Ok(());
    }
    if let Action::PrepareRolloutArchive(options) = cli.command {
        let (index, catalog_entry, verification) =
            remediation::prepare_archive(&options).map_err(|error| error.to_string())?;
        if !verification.rollout_verified {
            print_json(&verification)?;
            return reject("rollout remains open; index was not written");
        }
        evidence_capture::write_new(&options.destination, &index)?;
        return print_json(
            &serde_json::json!({"catalog_entry": catalog_entry, "verification": verification}),
        );
    }
    if let Action::ShadowMerge(options) = cli.command {
        let require_eligible = options.require_eligible;
        let report = shadow_merge::run(options)?;
        print_json(&report)?;
        if require_eligible && report["shadow_eligible"] != true {
            return reject("shadow merge eligibility blocked");
        }
        return Ok(());
    }
    if let Action::PrepareDependencyUpdate(options) = cli.command {
        return print_json(&dependency_update::run(options)?);
    }
    if let Action::SealDependencyScan(options) = cli.command {
        return print_json(&dependency_update::seal_scan(options)?);
    }
    if let Action::VerifyDependencyCompatibility(options) = cli.command {
        let require_complete = options.require_complete;
        let report = dependency_compatibility::run(options.evidence)?;
        print_json(&report)?;
        if require_complete && !report.compatibility_verified {
            return reject("dependency compatibility remains open");
        }
        return Ok(());
    }
    if let Action::SealCiRepairRun(options) = cli.command {
        return print_json(&ci_verification::seal(options)?);
    }
    if let Action::SealDependencyCompatibilityRun(options) = cli.command {
        return print_json(&dependency_compatibility::seal(options)?);
    }
    if let Action::DiagnoseRelease(options) = cli.command {
        return print_json(&diagnosis::run(options).map_err(|error| error.to_string())?);
    }
    if let Action::DiagnoseReleaseSources(options) = cli.command {
        return print_json(&release_sources::diagnose(options).map_err(|error| error.to_string())?);
    }
    if let Action::ImportDiagnosticOtlp(options) = cli.command {
        return print_json(&diagnosis::otlp::run(options).map_err(|error| error.to_string())?);
    }
    if let Action::DiagnoseSharedRelease(options) = cli.command {
        return print_json(&shared_runner::diagnose(options)?);
    }
    if let Action::DraftSeriesProposal(options) = cli.command {
        let result = series_proposal::draft(options)?;
        print_json(&result)?;
        if result["state"] == "blocked" {
            return reject("coordinator needs more information; see the JSON report");
        }
        return Ok(());
    }
    if let Action::ReviewSeriesProposal(options) = cli.command {
        return series_proposal::review(options);
    }
    if let Action::CompareSeriesProfiles { manifest } = cli.command {
        return print_json(&series_comparison::compare(&manifest)?);
    }
    if let Action::SharedWork(options) = cli.command {
        let result = shared_runner::shared_work(options)?;
        print_json(&result)?;
        if result["cancelled"] == false {
            return reject("queue did not confirm cancellation; inspect the shared work status");
        }
        return Ok(());
    }
    if let Action::SharedGithubEnqueue(options) = cli.command {
        return print_json(&shared_runner::github_enqueue::run(options)?);
    }
    if let Action::SharedSeries(options) = cli.command {
        let result = shared_runner::series(options)?;
        print_json(&result)?;
        if result["preflight"]["evidence_ready"] == false {
            return reject("rollout evidence is blocked; see the JSON report");
        }
        return Ok(());
    }
    if let Action::RunShared(options) = cli.command {
        return print_json(&shared_runner::run(options)?);
    }
    if let Action::RunSharedLoop(options) = cli.command {
        return shared_runner::run_loop(options);
    }
    if let Action::CheckSharedRunner(options) = cli.command {
        let report = shared_runner::check_runner(options)?;
        print_json(&report)?;
        if report["ready"] != true {
            return reject("shared runner setup is incomplete; see the JSON report");
        }
        return Ok(());
    }
    if let Action::PrepareSharedVerification(options) = cli.command {
        return print_json(&shared_runner::prepare_verification(options)?);
    }
    if let Action::PrepareNextVerification(options) = cli.command {
        return print_json(&shared_runner::prepare_next_verification(options)?);
    }
    if let Action::AttestSharedVerification(options) = cli.command {
        return print_json(&shared_runner::attest_verification(options)?);
    }
    if let Action::AttestSharedDependencyCompatibility(options) = cli.command {
        let result = shared_runner::attest_dependency_verification(options)?;
        print_json(&result)?;
        if result["compatibility_verified"] == false {
            return reject("dependency compatibility remains open");
        }
        return Ok(());
    }
    if let Action::ReconcileSharedDraft(options) = cli.command {
        return print_json(&shared_runner::reconcile_draft(options)?);
    }
    if let Action::BindSharedGithubReview(options) = cli.command {
        return print_json(&shared_runner::bind_github_review(options)?);
    }
    if let Action::ReserveSharedDraft(options) = cli.command {
        return print_json(&shared_runner::reserve_draft(options)?);
    }
    if let Action::ObserveStandingTicket(options) = cli.command {
        return print_json(&shared_runner::observe_standing(options)?);
    }
    if let Action::PollStandingJira(options) = cli.command {
        let report = jira_standing::poll(options)?;
        print_json(&report)?;
        if report["errors"]
            .as_array()
            .is_some_and(|errors| !errors.is_empty())
        {
            return reject("one or more Jira relay operations failed; see the JSON report");
        }
        return Ok(());
    }
    if let Action::CaptureJiraSnapshot(options) = cli.command {
        return print_json(&jira_capture::capture(options)?);
    }
    if let Action::CaptureGithubIssueSnapshot(options) = cli.command {
        return print_json(&github_issue_capture::capture(options)?);
    }
    if let Action::PollStandingGithubIssue(options) = cli.command {
        return print_json(&github_issue_capture::poll_standing(options)?);
    }
    if let Action::CaptureConfluenceContext(options) = cli.command {
        return print_json(&confluence_context::capture(options)?);
    }
    if let Action::PublishSharedDraft(options) = cli.command {
        return print_json(&shared_runner::publish_draft(options)?);
    }
    if let Action::RequestSharedRepair(options) = cli.command {
        return print_json(&shared_runner::request_repair(options)?);
    }
    if let Action::SharedOps(options) = cli.command {
        return shared_runner::operations(options);
    }
    if let Action::Schema { kind } = cli.command {
        return print_json(&match kind {
            SchemaKind::ShadowMergePolicy => schemars::schema_for!(shadow_merge::Policy),
            SchemaKind::ShadowMergeObservation => schemars::schema_for!(shadow_merge::Observation),
            SchemaKind::SeriesProposal => schemars::schema_for!(series_proposal::SeriesProposal),
            SchemaKind::SeriesComparison => schemars::schema_for!(series_comparison::Comparison),
            SchemaKind::ExecutionProfile => schemars::schema_for!(order::ExecutionProfile),
            SchemaKind::WorkOrder => schemars::schema_for!(WorkOrder),
            SchemaKind::ReleaseEvidence => schemars::schema_for!(release_trace::Bundle),
            SchemaKind::ReleaseSourceManifest => schemars::schema_for!(release_sources::Manifest),
            SchemaKind::KubernetesCapturePolicy => {
                schemars::schema_for!(kubernetes_capture::Policy)
            }
            SchemaKind::BuildProvenancePolicy => schemars::schema_for!(build_provenance::Policy),
            SchemaKind::BuildCapturePolicy => schemars::schema_for!(build_capture::Policy),
            SchemaKind::BuildArchivePolicy => schemars::schema_for!(build_capture::ArchivePolicy),
            SchemaKind::BuildProvenanceInput => schemars::schema_for!(build_provenance::BuildInput),
            SchemaKind::BitbucketChangeApproval => {
                schemars::schema_for!(bitbucket_change::Approval)
            }
            SchemaKind::GithubChangeApproval => schemars::schema_for!(github_change::Approval),
            SchemaKind::BitbucketReviewPolicy => schemars::schema_for!(bitbucket_review::Policy),
            SchemaKind::RemediationPolicy => schemars::schema_for!(remediation::Policy),
            SchemaKind::RemediationSourceIndex => schemars::schema_for!(remediation::SourceIndex),
            SchemaKind::DependencyUpdateInput => schemars::schema_for!(dependency_update::Input),
            SchemaKind::DependencyScanRefresh => {
                schemars::schema_for!(dependency_update::ScanRefresh)
            }
            SchemaKind::DependencyCompatibilityPolicy => {
                schemars::schema_for!(dependency_compatibility::Policy)
            }
            SchemaKind::DependencyCompatibilityRun => {
                schemars::schema_for!(dependency_compatibility::RunReceipt)
            }
            SchemaKind::CiVerificationPolicy => {
                schemars::schema_for!(ci_verification::Policy)
            }
            SchemaKind::CiVerificationRun => {
                schemars::schema_for!(ci_verification::Run)
            }
            SchemaKind::DiagnosticPolicy => schemars::schema_for!(diagnosis::Policy),
            SchemaKind::DiagnosticTelemetry => schemars::schema_for!(diagnosis::Telemetry),
            SchemaKind::DiagnosticOtlpPolicy => schemars::schema_for!(diagnosis::otlp::Policy),
            SchemaKind::Receipt => schemars::schema_for!(Receipt),
            SchemaKind::ReviewRecord => schemars::schema_for!(ReviewRecord),
            SchemaKind::VerificationHandoff => schemars::schema_for!(VerificationHandoff),
            SchemaKind::GithubPolicy => schemars::schema_for!(github::Policy),
            SchemaKind::BitbucketPolicy => schemars::schema_for!(bitbucket::Policy),
            SchemaKind::BitbucketFeedbackPolicy => {
                schemars::schema_for!(bitbucket_feedback::Policy)
            }
            SchemaKind::TwgBitbucketPolicy => schemars::schema_for!(twg::Policy),
        });
    }
    let path = cli
        .state
        .ok_or("--state is required for workflow commands")?;
    let receipt = match cli.command {
        Action::CutoverShared(options) => {
            return print_json(&shared_runner::cutover(&path, options)?);
        }
        Action::Doctor {
            codex,
            codex_home,
            gh,
        } => {
            let readiness = setup::doctor(&path, codex, codex_home, gh)?;
            print_json(&readiness)?;
            if readiness["ready"] != true {
                return reject("a requested authentication probe failed");
            }
            return Ok(());
        }
        Action::Delegate {
            order,
            jira_snapshot,
        } => {
            let order: WorkOrder = read_json(order)?;
            if order.ci_failure.is_some() {
                return reject(
                    "use a provider-specific CI repair command to establish CI failure provenance",
                );
            }
            order.validate()?;
            repository::validate_base(&order)?;
            Store::open(&path)?.delegate(order, jira_snapshot.as_deref())?
        }
        Action::DelegateBitbucketFailure {
            order,
            policy,
            build,
            step,
            jira_snapshot,
            shadow,
        } => {
            let mut order: WorkOrder = read_json(order)?;
            let resolve = || -> Result<_> {
                if order.ci_failure.is_some() {
                    return reject("provide a work order template without CI observations");
                }
                order.validate()?;
                repository::validate_base(&order)?;
                let policy: twg::Policy = read_json(policy)?;
                let failure = twg::failed_step(&policy, build, &step, &order.base)?;
                repository::validate_bitbucket_remote(&order, &failure.repository)?;
                order.require_native_checks = true;
                order.ci_failure = Some(order::CiFailure::Bitbucket(failure));
                Ok(())
            };
            if shadow {
                let outcome = resolve().and_then(|()| {
                    Store::open(&path)?.shadow_delegation(&order, jira_snapshot.as_deref())
                });
                let (decision, reason) = match outcome {
                    Ok(decision) => (decision, None),
                    Err(error) => ("blocked", Some(error.to_string())),
                };
                return print_json(&serde_json::json!({
                    "schema_version": 1,
                    "mode": "shadow",
                    "decision": decision,
                    "reason": reason,
                    "work_order_id": order.id,
                    "ci_failure": order.ci_failure,
                }));
            }
            resolve()?;
            Store::open(&path)?.delegate(order, jira_snapshot.as_deref())?
        }
        Action::DelegateGithubFailure {
            order,
            policy,
            gh,
            run,
            attempt,
            jira_snapshot,
        } => {
            let mut order: WorkOrder = read_json(order)?;
            if order.ci_failure.is_some() {
                return reject("provide a work order template without CI observations");
            }
            order.validate()?;
            repository::validate_base(&order)?;
            let (policy, digest) = github::read_pinned_policy(&policy)?;
            repository::validate_github_remote(&order, &policy.repository)?;
            let failure = github::failed_job(&policy, &gh, run, attempt, &order.base)?;
            order.require_native_checks = true;
            order.ci_failure = Some(order::CiFailure::Github(failure));
            if order.ci_verification_policy_digest.is_some() {
                order.ci_policy_digest = Some(digest);
            }
            Store::open(&path)?.delegate(order, jira_snapshot.as_deref())?
        }
        Action::PollBitbucketFailure {
            order,
            policy,
            step_name,
            jira_snapshot,
            admit,
            daily_limit,
        } => {
            let receipt = ci_dispatch::poll(
                &path,
                &order,
                ci_dispatch::PollProvider::Bitbucket {
                    policy: &policy,
                    step_name: &step_name,
                },
                jira_snapshot.as_deref(),
                admit,
                daily_limit,
            )?;
            return print_json(&receipt);
        }
        Action::PollGithubFailure {
            order,
            policy,
            gh,
            jira_snapshot,
            admit,
            daily_limit,
        } => {
            let receipt = ci_dispatch::poll(
                &path,
                &order,
                ci_dispatch::PollProvider::Github {
                    policy: &policy,
                    gh: &gh,
                },
                jira_snapshot.as_deref(),
                admit,
                daily_limit,
            )?;
            return print_json(&receipt);
        }
        Action::SetCiIntake { setting } => {
            let paused = matches!(setting, IntakeSetting::Pause);
            Store::open(&path)?.set_ci_intake_paused(paused)?;
            return print_json(&serde_json::json!({"paused": paused}));
        }
        Action::CiPollHistory { limit } => {
            return print_json(&Store::open(&path)?.ci_poll_history(limit)?);
        }
        Action::PinJiraSnapshot { id, snapshot } => {
            Store::open(&path)?.pin_snapshot(&id, &snapshot)?
        }
        Action::CheckJiraSnapshot { id, policy } => {
            return print_json(&jira_capture::verify_live(&path, &id, &policy)?);
        }
        Action::CheckGithubIssueSnapshot { id, policy } => {
            return print_json(&github_issue_capture::verify_live(&path, &id, &policy)?);
        }
        Action::Status { id } => Store::open(&path)?.status(&id)?,
        Action::Brief { id } => {
            review::print_brief(&Store::open(&path)?.status(&id)?)?;
            return Ok(());
        }
        Action::Prepare {
            id,
            destination,
            handoff,
        } => {
            let mut store = Store::open(&path)?;
            store.verify_snapshot(&id)?;
            let receipt = store.status(&id)?;
            let base = match receipt.state {
                store::State::Delegated => &receipt.work_order.base,
                store::State::RepairRequested => {
                    receipt.candidate.as_ref().ok_or("missing repair head")?
                }
                _ => return reject("the order cannot start a new checkout in its current state"),
            };
            let handoff = handoff
                .then(|| runner::interactive_handoff(&receipt, base))
                .transpose()?;
            let checkout = repository::prepare_checkout(&receipt.work_order, &destination, base)?;
            let mut result = serde_json::json!({"checkout": checkout, "head": base});
            if let Some(bytes) = handoff {
                let path = destination.join("worker-handoff.json");
                evidence_capture::write_new(&path, &bytes)?;
                result["handoff"] = serde_json::json!(path);
                result["handoff_digest"] =
                    serde_json::json!(craft_factory_evidence::sha256(&bytes));
            }
            return print_json(&result);
        }
        Action::PrepareVerification {
            id,
            head,
            destination,
            handoff,
        } => {
            let mut store = Store::open(&path)?;
            store.verify_snapshot(&id)?;
            let receipt = store.status(&id)?;
            if !matches!(
                receipt.state,
                store::State::Candidate | store::State::VerificationAttested
            ) || receipt.candidate.as_deref() != Some(&head)
            {
                return reject("the requested head is not the current candidate awaiting review");
            }
            let packet = if handoff {
                let paths = repository::candidate_history_paths(
                    &receipt.work_order,
                    &receipt.work_order.base,
                    &head,
                )?;
                Some(store.issue_verification_handoff(&id, &head, paths.into_iter().collect())?)
            } else {
                None
            };
            let checkout = repository::prepare_checkout(&receipt.work_order, &destination, &head)?;
            if let Some(packet) = packet {
                let path = destination.join("verification-handoff.json");
                serde_json::to_writer_pretty(std::fs::File::create_new(&path)?, &packet)?;
                return print_json(
                    &serde_json::json!({"checkout": checkout, "head": head, "handoff": path, "handoff_digest": packet.digest()?}),
                );
            }
            return print_json(&serde_json::json!({"checkout": checkout, "head": head}));
        }
        Action::RunCiRepair(options) => return print_json(&ci_dispatch::run(&path, options)?),
        Action::RunCiCycle(options) => return print_json(&ci_dispatch::cycle(&path, options)?),
        Action::PublishCiDraft(options) => {
            return print_json(&ci_verification::publish(&path, options)?);
        }
        Action::Run {
            id,
            destination,
            adapter,
            timeout_seconds,
            jira_policy,
            github_issue_policy,
        } => {
            if let Some(policy) = jira_policy {
                jira_capture::verify_live(&path, &id, &policy)?;
            } else if let Some(policy) = github_issue_policy {
                github_issue_capture::verify_live(&path, &id, &policy)?;
            }
            runner::run(
                &path,
                &id,
                runner::Options {
                    destination,
                    runtime: adapter.resolve()?,
                    timeout: std::time::Duration::from_secs(timeout_seconds),
                    shared_repair: None,
                    team_skill: None,
                    project_context: Vec::new(),
                    collect_knowledge: false,
                },
            )?
        }
        Action::Submit {
            id,
            commit,
            checkout,
            replaces,
            note,
        } => {
            let mut store = Store::open(&path)?;
            if let Some(checkout) = checkout {
                store.submit_from_checkout(&id, &checkout, &commit, replaces.as_deref(), &note)?
            } else {
                store.submit(&id, &commit, replaces.as_deref(), &note)?
            }
        }
        Action::RetryCi {
            id,
            previous,
            commit,
        } => Store::open(&path)?.retry_ci(&id, &previous, &commit)?,
        Action::BindDraft {
            id,
            reference,
            head,
            jira_policy,
            github_issue_policy,
        } => {
            if let Some(policy) = jira_policy {
                jira_capture::verify_live(&path, &id, &policy)?;
            } else if let Some(policy) = github_issue_policy {
                github_issue_capture::verify_live(&path, &id, &policy)?;
            }
            Store::open(&path)?.bind_draft(&id, Draft { reference, head })?
        }
        Action::HoldPublication { id, head, reason } => {
            Store::open(&path)?.hold_publication(&id, &head, &reason)?
        }
        Action::ClearPublication { id, record } => {
            Store::open(&path)?.clear_publication(&id, read_json(record)?)?
        }
        Action::AttestChecks {
            id,
            record,
            handoff,
        } => Store::open(&path)?.attest_with_handoff(
            &id,
            Attestation::Verification,
            read_json(record)?,
            handoff.map(read_json).transpose()?,
        )?,
        Action::CheckGithub { id, policy, gh } => {
            let policy = read_json(policy)?;
            observe_checks(&path, &id, "github", |receipt| {
                let (head, checks) = github::observe(receipt, &policy, gh)?;
                Ok(serde_json::json!({
                    "pr": receipt.draft.as_ref().map(|draft| &draft.reference),
                    "head": head, "base": receipt.work_order.base, "checks": checks,
                    "merge_approval_policy": policy.merge_approval_policy()?
                }))
            })?
        }
        Action::ShadowMergeGithub {
            id,
            policy,
            gh,
            approved_policy_digest,
            approved_protection_digest,
            require_eligible,
        } => {
            let (policy, digest): (github::Policy, _) = shadow_merge::load(&policy)?;
            if digest != approved_policy_digest
                || !approved_protection_digest
                    .strip_prefix("sha256:")
                    .is_some_and(|hex| hex.len() == 64 && order::valid_oid(hex))
            {
                return reject("provide the approved GitHub policy and protection fingerprints");
            }
            let receipt = Store::open(&path)?.status(&id)?;
            let mut report = github::shadow_merge(
                &receipt,
                &policy,
                &gh,
                &approved_protection_digest,
                time::OffsetDateTime::now_utc().unix_timestamp(),
            )?;
            report["policy_digest"] = serde_json::json!(digest);
            report["approved_protection_digest"] = serde_json::json!(approved_protection_digest);
            print_json(&report)?;
            if require_eligible && report["shadow_eligible"] != true {
                return reject("GitHub shadow merge is blocked; see the JSON report");
            }
            return Ok(());
        }
        Action::CheckBitbucket {
            id,
            policy,
            pipeline,
        } => {
            let policy = read_json(policy)?;
            observe_checks(&path, &id, "bitbucket", |receipt| {
                bitbucket::observe(receipt, &policy, &pipeline)
            })?
        }
        Action::CheckBitbucketTwg {
            id,
            policy,
            build,
            feedback_policy,
        } => {
            let policy: twg::Policy = read_json(policy)?;
            let feedback_policy: Option<bitbucket_feedback::Policy> =
                feedback_policy.map(read_json).transpose()?;
            observe_checks(&path, &id, "bitbucket", |receipt| {
                let mut observation = twg::observe(receipt, &policy, build)?;
                if let Some(feedback) = &feedback_policy {
                    observation["merge_approval_policy"] =
                        serde_json::json!(feedback.merge_approval_policy(
                            receipt,
                            &policy.repository_name(),
                            &policy.base_branch,
                        )?);
                }
                Ok(observation)
            })?
        }
        Action::RequestChanges { id, record } => {
            Store::open(&path)?.attest(&id, Attestation::ChangesRequested, read_json(record)?)?
        }
        Action::RequestChangesGithub {
            id,
            policy,
            gh,
            review,
            summary,
        } => {
            let note = review::read_feedback_summary(&summary)?;
            let policy = read_json(policy)?;
            let mut store = Store::open(&path)?;
            let record =
                github::observe_change_request(&store.status(&id)?, &policy, &gh, review, note)?;
            store.attest(&id, Attestation::ChangesRequested, record)?
        }
        Action::RequestChangesBitbucket {
            id,
            policy,
            comment,
            summary,
        } => {
            let note = review::read_feedback_summary(&summary)?;
            let policy = read_json(policy)?;
            let mut store = Store::open(&path)?;
            let record = bitbucket_feedback::observe(&store.status(&id)?, &policy, comment, note)?;
            store.attest(&id, Attestation::ChangesRequested, record)?
        }
        Action::Close { id, record } => {
            Store::open(&path)?.attest(&id, Attestation::Close, read_json(record)?)?
        }
        Action::CloseGithub { id, policy, gh } => {
            let policy = read_json(policy)?;
            let mut store = Store::open(&path)?;
            let merge = github::observe_merge(&store.status(&id)?, &policy, &gh)?;
            store.close_with_native_merge(&id, merge)?
        }
        Action::CloseBitbucket {
            id,
            policy,
            approval_comments,
        } => {
            let policy = read_json(policy)?;
            let mut store = Store::open(&path)?;
            let merge = bitbucket_feedback::observe_merge(
                &store.status(&id)?,
                &policy,
                &approval_comments,
            )?;
            store.close_with_native_merge(&id, merge)?
        }
        Action::Cancel { id, reason } => Store::open(&path)?.cancel(&id, &reason)?,
        Action::Schema { .. }
        | Action::TraceRelease(_)
        | Action::TraceReleaseSources(_)
        | Action::AssembleReleaseSources(_)
        | Action::ImportKubernetesInventory(_)
        | Action::ImportKubernetesDeployment(_)
        | Action::CollectKubernetesRelease(_)
        | Action::ImportBuildProvenance(_)
        | Action::CollectBuildProvenance(_)
        | Action::PublishBuildProvenance(_)
        | Action::ImportBitbucketChange(_)
        | Action::CollectBitbucketChange(_)
        | Action::CollectGithubChange(_)
        | Action::CollectBitbucketReview(_)
        | Action::VerifyRemediation(_)
        | Action::VerifyRemediationSources(_)
        | Action::PrepareRolloutArchive(_)
        | Action::ShadowMerge(_)
        | Action::PrepareDependencyUpdate(_)
        | Action::SealDependencyScan(_)
        | Action::VerifyDependencyCompatibility(_)
        | Action::SealDependencyCompatibilityRun(_)
        | Action::SealCiRepairRun(_)
        | Action::DiagnoseRelease(_)
        | Action::DiagnoseReleaseSources(_)
        | Action::ImportDiagnosticOtlp(_)
        | Action::DiagnoseSharedRelease(_)
        | Action::DraftSeriesProposal(_)
        | Action::ReviewSeriesProposal(_)
        | Action::CompareSeriesProfiles { .. }
        | Action::SharedWork(_)
        | Action::SharedGithubEnqueue(_)
        | Action::SharedSeries(_)
        | Action::RunnerChild
        | Action::Smoke { .. }
        | Action::RunShared(_)
        | Action::RunSharedLoop(_)
        | Action::CheckSharedRunner(_)
        | Action::PrepareSharedVerification(_)
        | Action::PrepareNextVerification(_)
        | Action::AttestSharedVerification(_)
        | Action::AttestSharedDependencyCompatibility(_)
        | Action::ReconcileSharedDraft(_)
        | Action::BindSharedGithubReview(_)
        | Action::ReserveSharedDraft(_)
        | Action::ObserveStandingTicket(_)
        | Action::PollStandingJira(_)
        | Action::CaptureJiraSnapshot(_)
        | Action::CaptureGithubIssueSnapshot(_)
        | Action::PollStandingGithubIssue(_)
        | Action::CaptureConfluenceContext(_)
        | Action::PublishSharedDraft(_)
        | Action::RequestSharedRepair(_)
        | Action::SharedOps(_)
        | Action::IssueQueueIdentity(_) => unreachable!(),
    };
    print_json(&receipt)
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("craft-factory: {error}");
            ExitCode::FAILURE
        }
    }
}
