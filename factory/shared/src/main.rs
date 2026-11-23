mod ci_lanes;
mod diagnostics;
mod github_enqueue;
mod identities;
mod operations;
mod preflight;
mod publication;
mod series;

use std::{fs::File, io::BufReader, net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use base64::Engine;
use clap::Parser;
use craft_factory_evidence::queue_identity::{Role, identifier};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder,
    service::TowerToHyperService,
};
use identities::{IdentityStore, authenticate};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tokio::{net::TcpListener, sync::Semaphore, task::JoinSet, time::timeout};
use tokio_rustls::{TlsAcceptor, rustls};

type Error = Box<dyn std::error::Error + Send + Sync>;
type Api = Result<Json<Value>, ApiError>;

#[derive(Parser)]
struct Settings {
    #[arg(long, default_value = "127.0.0.1:8443")]
    bind: SocketAddr,
    #[arg(long)]
    certificate: PathBuf,
    #[arg(long)]
    private_key: PathBuf,
    #[arg(long)]
    identities: PathBuf,
    /// Trusted manifest of case IDs and their approved diagnostic files.
    #[arg(long)]
    diagnostic_cases: Option<PathBuf>,
    /// Operator-managed cases for archive-only verified rollouts.
    #[arg(long)]
    rollout_archives: Option<PathBuf>,
    /// Validate startup inputs and read queue status, then exit.
    #[arg(long, conflicts_with = "recover_restored_database")]
    check_config: bool,
    /// Fence a restored database before opening the HTTPS listener.
    #[arg(long)]
    recover_restored_database: Option<String>,
}

#[derive(Clone)]
struct App {
    database: PgPool,
    identities: IdentityStore,
    diagnostics: diagnostics::Catalog,
    rollout_verifiers: Arc<Semaphore>,
    rollout_archives: Option<series::ArchiveCatalog>,
    metrics_scrape: Arc<Semaphore>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TicketSnapshot {
    source: String,
    issue: String,
    source_revision: i64,
    observed_at: i64,
    assignee: Option<String>,
    status: String,
    labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    intent_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    intent_approved: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_issue_id: Option<u64>,
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum IntakeRequest {
    ObserveCi {
        lane: String,
        manifest_digest: String,
        failure: Option<craft_factory_evidence::ci::CiFailure>,
        admit: bool,
    },
    Observe {
        rule: String,
        snapshot: TicketSnapshot,
        work_order: Option<Value>,
    },
}

async fn intake(
    State(app): State<App>,
    headers: HeaderMap,
    Json(request): Json<IntakeRequest>,
) -> Api {
    let actor = authenticate(&app, &headers, Role::Intake)?;
    let (rule, mut snapshot, work_order) = match request {
        IntakeRequest::Observe {
            rule,
            snapshot,
            work_order,
        } => (rule, snapshot, work_order),
        IntakeRequest::ObserveCi {
            lane,
            manifest_digest,
            failure,
            admit,
        } => return ci_lanes::observe(&app, actor, lane, manifest_digest, failure, admit).await,
    };
    if work_order
        .as_ref()
        .is_some_and(|order| !order["ci_failure"].is_null())
    {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "ci_failure_requires_operator_admission",
        ));
    }
    if work_order.as_ref().is_some_and(|order| {
        order
            .get("execution_profile")
            .is_some_and(|profile| !profile.is_null())
    }) {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "execution_profile_requires_operator_admission",
        ));
    }
    if !identifier(&rule)
        || snapshot.source.len() > 200
        || snapshot.issue.len() > 50
        || snapshot
            .assignee
            .as_ref()
            .is_some_and(|value| value.len() > 200)
        || snapshot.status.is_empty()
        || snapshot.status.len() > 100
        || snapshot.labels.len() > 100
        || snapshot.labels.iter().any(|label| label.len() > 100)
        || snapshot.source_issue_id == Some(0)
        || snapshot.intent_digest.is_some() != snapshot.intent_approved.is_some()
        || snapshot.intent_digest.as_deref().is_some_and(|digest| {
            !digest.strip_prefix("sha256:").is_some_and(|hash| {
                hash.len() == 64
                    && hash
                        .bytes()
                        .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
            })
        })
    {
        return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_ticket_snapshot"));
    }
    snapshot.labels.sort_unstable();
    snapshot.labels.dedup();
    let outcome = sqlx::query_scalar::<_, Option<Value>>(
        "SELECT craft_queue.observe_standing($1, $2, $3, $4)",
    )
    .bind(actor)
    .bind(rule)
    .bind(sqlx::types::Json(snapshot))
    .bind(work_order)
    .fetch_one(&app.database)
    .await?
    .ok_or(ApiError(
        StatusCode::FORBIDDEN,
        "standing_rule_not_available",
    ))?;
    response("decision", outcome)
}

struct ApiError(StatusCode, &'static str);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"schema_version": 1, "error": self.1}))).into_response()
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> Self {
        if error.as_database_error().is_some_and(|error| {
            matches!(error.code().as_deref(), Some("P0001" | "23514" | "23502"))
        }) {
            Self(StatusCode::CONFLICT, "queue_rejected_request")
        } else {
            Self(StatusCode::SERVICE_UNAVAILABLE, "queue_unavailable")
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum RunnerRequest {
    CheckEnrollment {
        mapped_repositories: Vec<String>,
    },
    Claim {
        request_id: String,
        #[serde(default)]
        context_capable: bool,
        #[serde(default)]
        profile_capable: bool,
        #[serde(default)]
        available_repositories: Option<Vec<String>>,
        agent_adapter: Option<String>,
    },
    RepairContext {
        work_id: String,
        generation: i64,
    },
    TeamSkill {
        work_id: String,
        generation: i64,
    },
    ProjectContext {
        work_id: String,
        generation: i64,
    },
    ProposeKnowledge {
        proposal_id: String,
        work_id: String,
        generation: i64,
        head: String,
        proposal: craft_factory_evidence::KnowledgeProposal,
    },
    Renew {
        work_id: String,
        generation: i64,
    },
    ReportFailedAttemptUsage {
        work_id: String,
        generation: i64,
        tokens: i64,
    },
    FinishFailedAttempt {
        work_id: String,
        generation: i64,
        tokens: Option<i64>,
    },
    HoldObsoleteCi {
        work_id: String,
        generation: i64,
        reason: String,
    },
    #[serde(alias = "start_profiled_attempt")]
    StartAttempt {
        work_id: String,
        generation: i64,
    },
    SubmitCandidate {
        work_id: String,
        generation: i64,
        commit: String,
        evidence_ref: String,
        tokens: Option<i64>,
    },
    SubmitPack {
        work_id: String,
        generation: i64,
        commit: String,
        pack_base64: String,
        tokens: Option<i64>,
    },
}

fn response(field: &'static str, value: impl serde::Serialize) -> Api {
    let mut response = json!({"schema_version": 1});
    response[field] = serde_json::to_value(value)
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "response_error"))?;
    Ok(Json(response))
}

async fn runner(
    State(app): State<App>,
    headers: HeaderMap,
    Json(request): Json<RunnerRequest>,
) -> Api {
    let identity = authenticate(&app, &headers, Role::Runner)?;
    match request {
        RunnerRequest::CheckEnrollment {
            mapped_repositories,
        } => {
            if mapped_repositories
                .iter()
                .any(|name| !operations::review_text(name, 200))
            {
                return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_checkout_scope"));
            }
            let setup = sqlx::query_scalar::<_, Value>("SELECT craft_queue.runner_setup($1, $2)")
                .bind(identity)
                .bind(mapped_repositories)
                .fetch_one(&app.database)
                .await?;
            response("setup", setup)
        }
        RunnerRequest::Claim {
            request_id,
            context_capable,
            profile_capable,
            available_repositories,
            agent_adapter,
        } => {
            if !request_id.split('-').map(str::len).eq([8, 4, 4, 4, 12])
                || !request_id
                    .bytes()
                    .all(|b| b == b'-' || b.is_ascii_hexdigit())
            {
                return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_request_id"));
            }
            if available_repositories.as_ref().is_some_and(|repositories| {
                repositories.iter().any(|repository| {
                    repository.trim().is_empty()
                        || repository.chars().count() > 200
                        || repository.chars().any(char::is_control)
                })
            }) {
                return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_checkout_scope"));
            }
            let agent_adapter = agent_adapter.as_deref().unwrap_or("codex");
            if !matches!(agent_adapter, "codex" | "claude") {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "unsupported_agent_adapter",
                ));
            }
            let claim = sqlx::query_scalar::<_, Value>(
                "SELECT to_jsonb(lease) FROM craft_queue.claim($1, $2::text::uuid, $3, $4, $5, $6) AS lease",
            )
            .bind(identity)
            .bind(request_id)
            .bind(context_capable)
            .bind(profile_capable)
            .bind(available_repositories)
            .bind(agent_adapter)
            .fetch_optional(&app.database)
            .await?;
            response("claim", claim)
        }
        RunnerRequest::RepairContext {
            work_id,
            generation,
        } => {
            let context = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.repair_context($1, $2, $3)",
            )
            .bind(identity)
            .bind(work_id)
            .bind(generation)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(StatusCode::CONFLICT, "repair_lease_not_current"))?;
            response("repair", context)
        }
        RunnerRequest::TeamSkill {
            work_id,
            generation,
        } => {
            let skill = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.team_skill_for_lease($1, $2, $3)",
            )
            .bind(identity)
            .bind(work_id)
            .bind(generation)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(StatusCode::CONFLICT, "team_skill_unavailable"))?;
            response("skill", skill)
        }
        RunnerRequest::ProjectContext {
            work_id,
            generation,
        } => {
            let context = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.project_context_for_lease($1, $2, $3)",
            )
            .bind(identity)
            .bind(work_id)
            .bind(generation)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "project_context_unavailable",
            ))?;
            response("context", context)
        }
        RunnerRequest::ProposeKnowledge {
            proposal_id,
            work_id,
            generation,
            head,
            proposal,
        } => {
            if !identifier(&proposal_id)
                || !identifier(&work_id)
                || generation <= 0
                || !craft_factory_evidence::valid_oid(&head)
                || !proposal.valid()
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_knowledge_proposal",
                ));
            }
            let item = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.propose_knowledge($1, $2, $3, $4, $5, $6)",
            )
            .bind(identity)
            .bind(proposal_id)
            .bind(work_id)
            .bind(generation)
            .bind(head)
            .bind(sqlx::types::Json(&proposal))
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "knowledge_proposal_not_available",
            ))?;
            response("proposal", item)
        }
        RunnerRequest::Renew {
            work_id,
            generation,
        } => {
            let expiry = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT to_jsonb(craft_queue.renew($1, $2, $3))",
            )
            .bind(identity)
            .bind(work_id)
            .bind(generation)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(StatusCode::CONFLICT, "lease_not_current"))?;
            response("expires_at", expiry)
        }
        RunnerRequest::ReportFailedAttemptUsage {
            work_id,
            generation,
            tokens,
        } => {
            let recorded = sqlx::query_scalar::<_, bool>(
                "SELECT craft_queue.report_failed_attempt_usage($1, $2, $3, $4)",
            )
            .bind(identity)
            .bind(work_id)
            .bind(generation)
            .bind(tokens)
            .fetch_one(&app.database)
            .await?;
            if !recorded {
                return Err(ApiError(
                    StatusCode::CONFLICT,
                    "failed_attempt_usage_not_available",
                ));
            }
            response("recorded", true)
        }
        RunnerRequest::FinishFailedAttempt {
            work_id,
            generation,
            tokens,
        } => {
            let recorded = sqlx::query_scalar::<_, bool>(
                "SELECT craft_queue.finish_failed_attempt($1, $2, $3, $4)",
            )
            .bind(identity)
            .bind(work_id)
            .bind(generation)
            .bind(tokens)
            .fetch_one(&app.database)
            .await?;
            if !recorded {
                return Err(ApiError(StatusCode::CONFLICT, "attempt_stop_not_available"));
            }
            response("recorded", true)
        }
        RunnerRequest::HoldObsoleteCi {
            work_id,
            generation,
            reason,
        } => {
            if !identifier(&work_id)
                || generation < 1
                || !matches!(reason.as_str(), "branch_moved" | "workflow_superseded")
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_ci_preflight_hold",
                ));
            }
            let hold = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.hold_obsolete_ci($1, $2, $3, $4)",
            )
            .bind(identity)
            .bind(work_id)
            .bind(generation)
            .bind(reason)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "ci_preflight_hold_not_available",
            ))?;
            response("ci_preflight_hold", hold)
        }
        RunnerRequest::StartAttempt {
            work_id,
            generation,
        } => {
            let started = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT to_jsonb(craft_queue.start_attempt($1, $2, $3))",
            )
            .bind(identity)
            .bind(work_id)
            .bind(generation)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "attempt_start_not_available",
            ))?;
            response("started_at", started)
        }
        RunnerRequest::SubmitCandidate {
            work_id,
            generation,
            commit,
            evidence_ref,
            tokens,
        } => {
            let accepted = sqlx::query_scalar::<_, bool>(
                "SELECT craft_queue.submit_candidate($1, $2, $3, $4, $5, $6)",
            )
            .bind(identity)
            .bind(work_id)
            .bind(generation)
            .bind(commit)
            .bind(evidence_ref)
            .bind(tokens)
            .fetch_one(&app.database)
            .await?;
            if !accepted {
                return Err(ApiError(StatusCode::CONFLICT, "candidate_rejected"));
            }
            response("accepted", true)
        }
        RunnerRequest::SubmitPack {
            work_id,
            generation,
            commit,
            pack_base64,
            tokens,
        } => {
            let pack = base64::engine::general_purpose::STANDARD
                .decode(pack_base64)
                .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_pack"))?;
            if pack.len() > 512 * 1024 {
                return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, "pack_too_large"));
            }
            let evidence = sqlx::query_scalar::<_, Option<String>>(
                "SELECT craft_queue.submit_candidate_pack($1, $2, $3, $4, $5, $6)",
            )
            .bind(identity)
            .bind(work_id)
            .bind(generation)
            .bind(commit)
            .bind(pack)
            .bind(tokens)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(StatusCode::CONFLICT, "candidate_rejected"))?;
            response("candidate_evidence", evidence)
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum OperatorRequest {
    ApproveCiLane {
        lane: String,
        approval: craft_factory_evidence::ci::LaneApproval,
    },
    DisableCiLane {
        lane: String,
        evidence: String,
    },
    Admit {
        repository: String,
        event_key: String,
        work_order: Value,
        token_limit: i64,
    },
    Cancel {
        work_id: String,
        reason: String,
    },
    SetPaused {
        paused: bool,
    },
    Status {
        work_id: String,
    },
    DownloadPack {
        work_id: String,
    },
    RequestRepair {
        work_id: String,
        generation: i64,
        head: String,
        native_url: String,
        evidence: String,
        note: String,
    },
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum VerifierRequest {
    ClaimPendingVerification {
        repository: String,
        after_work_id: Option<String>,
    },
    ReleaseVerificationPickup {
        repository: String,
        pickup_id: String,
    },
    RenewVerificationPickup {
        repository: String,
        pickup_id: String,
    },
    PendingVerifications {
        repository: String,
        after_work_id: Option<String>,
        limit: i32,
    },
    NextGithubReadiness {
        repository: String,
        base_branch: String,
        policy_digest: String,
    },
    RecordGithubReadiness {
        receipt: operations::ShadowReceipt,
    },
    CloseScheduledGithubWork {
        repository: String,
        base_branch: String,
        policy_file_digest: String,
        limit: i32,
    },
    DownloadPack {
        work_id: String,
    },
    Status {
        work_id: String,
    },
    RecordVerification {
        work_id: String,
        generation: i64,
        head: String,
        digest: String,
        previous_sequence: i64,
        passed: bool,
        evidence: String,
        note: String,
        review_summary: Option<String>,
        pickup_id: Option<String>,
    },
    RecordDependencyVerification {
        work_id: String,
        generation: i64,
        head: String,
        digest: String,
        previous_sequence: i64,
        passed: bool,
        approval_id: String,
        proof: DependencyProof,
        pickup_id: Option<String>,
    },
    RecordShadowOutcome {
        receipt_id: String,
        head: String,
        report_digest: String,
        basis: String,
        expected_eligible: bool,
        evidence_digest: String,
        evidence: String,
        note: String,
    },
    ConfirmGithubShadowMerge {
        receipt_id: String,
        head: String,
        report_digest: String,
        observation: Option<craft_factory_evidence::GithubMergeOutcome>,
    },
    ConfirmBitbucketWorkMerge {
        work_id: String,
        head: String,
        publication_key: Option<String>,
        observation: Option<craft_factory_evidence::BitbucketMergeOutcome>,
    },
    PendingMergeChecks {
        repository: String,
        after_work_id: Option<String>,
        limit: i32,
    },
    RecordGithubCiChecks {
        work_id: String,
        generation: i64,
        head: String,
        pack_digest: String,
        verification_sequence: i64,
        proof: Option<Value>,
    },
    RecordBitbucketCiChecks {
        work_id: String,
        generation: i64,
        head: String,
        pack_digest: String,
        verification_sequence: i64,
        proof: Option<Value>,
    },
}

impl VerifierRequest {
    async fn authorize_repository(
        &self,
        database: &sqlx::PgPool,
        repositories: &[String],
    ) -> Result<(), ApiError> {
        let (work_id, receipt_id) = match self {
            Self::RecordGithubReadiness { receipt } => (Some(&receipt.work_id), None),
            Self::DownloadPack { work_id }
            | Self::Status { work_id }
            | Self::RecordVerification { work_id, .. }
            | Self::RecordDependencyVerification { work_id, .. }
            | Self::ConfirmBitbucketWorkMerge { work_id, .. }
            | Self::RecordGithubCiChecks { work_id, .. }
            | Self::RecordBitbucketCiChecks { work_id, .. } => (Some(work_id), None),
            Self::ConfirmGithubShadowMerge { receipt_id, .. }
            | Self::RecordShadowOutcome { receipt_id, .. } => (None, Some(receipt_id)),
            Self::ClaimPendingVerification { repository, .. }
            | Self::ReleaseVerificationPickup { repository, .. }
            | Self::RenewVerificationPickup { repository, .. }
            | Self::PendingVerifications { repository, .. }
            | Self::PendingMergeChecks { repository, .. }
            | Self::NextGithubReadiness { repository, .. }
            | Self::CloseScheduledGithubWork { repository, .. } => {
                if repositories.contains(repository) {
                    return Ok(());
                }
                return Err(ApiError(
                    StatusCode::FORBIDDEN,
                    "verifier_repository_not_allowed",
                ));
            }
        };
        // Work and shadow receipt IDs are append-only and keep their original repository.
        let repository = sqlx::query_scalar::<_, Option<String>>(
            "SELECT craft_queue.verifier_target_repository($1, $2)",
        )
        .bind(work_id)
        .bind(receipt_id)
        .fetch_one(database)
        .await?;
        if repository
            .as_ref()
            .is_some_and(|repo| repositories.contains(repo))
        {
            Ok(())
        } else {
            Err(ApiError(StatusCode::NOT_FOUND, "verifier_target_not_found"))
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DependencyProof {
    intake_digest: String,
    policy_digest: String,
    baseline_run_digest: String,
    candidate_run_digest: String,
    report_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    scanner_fresh_through: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scanner_refresh_digest: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum CutoverRequest {
    Activate { epoch: String, archive: String },
    Release { epoch: String },
    Status { epoch: String },
    Archive { epoch: String },
}

async fn cutover(
    State(app): State<App>,
    headers: HeaderMap,
    Json(request): Json<CutoverRequest>,
) -> Api {
    let actor = authenticate(&app, &headers, Role::Operator)?;
    match request {
        CutoverRequest::Activate { epoch, archive } => {
            let status = sqlx::query_scalar::<_, Value>(
                "SELECT craft_queue.activate_cutover($1::text::uuid, $2, $3)",
            )
            .bind(epoch)
            .bind(archive)
            .bind(actor)
            .fetch_one(&app.database)
            .await?;
            response("cutover", status)
        }
        CutoverRequest::Release { epoch } => {
            let status = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.release_cutover($1::text::uuid, $2)",
            )
            .bind(epoch)
            .bind(actor)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(StatusCode::CONFLICT, "cutover_not_current"))?;
            response("cutover", status)
        }
        CutoverRequest::Status { epoch } => {
            let status = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.cutover_status($1::text::uuid)",
            )
            .bind(epoch)
            .fetch_one(&app.database)
            .await?;
            response("cutover", status)
        }
        CutoverRequest::Archive { epoch } => {
            let archive = sqlx::query_scalar::<_, Option<String>>(
                "SELECT craft_queue.cutover_archive($1::text::uuid)",
            )
            .bind(epoch)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(StatusCode::NOT_FOUND, "cutover_archive_not_found"))?;
            response("archive", archive)
        }
    }
}

async fn download_pack(app: &App, work_id: &str) -> Api {
    let evidence = sqlx::query_scalar::<_, Option<Value>>("SELECT craft_queue.candidate_pack($1)")
        .bind(work_id)
        .fetch_one(&app.database)
        .await?
        .ok_or(ApiError(StatusCode::NOT_FOUND, "candidate_pack_not_found"))?;
    response("candidate", evidence)
}

async fn work_status(app: &App, work_id: &str) -> Api {
    let status = sqlx::query_scalar::<_, Option<Value>>("SELECT craft_queue.status($1)")
        .bind(work_id)
        .fetch_one(&app.database)
        .await?
        .ok_or(ApiError(StatusCode::NOT_FOUND, "unknown_work_order"))?;
    response("work", status)
}

fn valid_verification_pickup_id(value: &str) -> bool {
    craft_factory_evidence::bitbucket_change::normalize_uuid(value).as_deref() == Some(value)
}

async fn verifier(
    State(app): State<App>,
    headers: HeaderMap,
    Json(request): Json<VerifierRequest>,
) -> Api {
    let identity = identities::authenticate_verifier(&app, &headers)?;
    if let Some(repositories) = &identity.repositories {
        request
            .authorize_repository(&app.database, repositories)
            .await?;
    } else if matches!(
        &request,
        VerifierRequest::PendingVerifications { .. }
            | VerifierRequest::ClaimPendingVerification { .. }
            | VerifierRequest::ReleaseVerificationPickup { .. }
            | VerifierRequest::RenewVerificationPickup { .. }
            | VerifierRequest::RecordVerification {
                pickup_id: Some(_),
                ..
            }
            | VerifierRequest::RecordDependencyVerification {
                pickup_id: Some(_),
                ..
            }
            | VerifierRequest::NextGithubReadiness { .. }
            | VerifierRequest::RecordGithubReadiness { .. }
            | VerifierRequest::CloseScheduledGithubWork { .. }
    ) {
        return Err(ApiError(StatusCode::FORBIDDEN, "scoped_verifier_required"));
    }
    let actor = identity.id;
    match request {
        VerifierRequest::ClaimPendingVerification {
            repository,
            after_work_id,
        } => {
            if after_work_id.as_deref().is_some_and(|id| !identifier(id)) {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_pending_verifications_filter",
                ));
            }
            let target = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.claim_pending_verification($1, $2, $3)",
            )
            .bind(actor)
            .bind(repository)
            .bind(after_work_id)
            .fetch_one(&app.database)
            .await?;
            response("verification_pickup", target)
        }
        VerifierRequest::ReleaseVerificationPickup {
            repository,
            pickup_id,
        } => {
            if !valid_verification_pickup_id(&pickup_id) {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_verification_pickup",
                ));
            }
            let released = sqlx::query_scalar::<_, bool>(
                "SELECT craft_queue.release_verification_pickup($1, $2, $3::text::uuid)",
            )
            .bind(actor)
            .bind(repository)
            .bind(pickup_id)
            .fetch_one(&app.database)
            .await?;
            if !released {
                return Err(ApiError(
                    StatusCode::CONFLICT,
                    "verification_pickup_not_owned",
                ));
            }
            response("released", true)
        }
        VerifierRequest::RenewVerificationPickup {
            repository,
            pickup_id,
        } => {
            if !valid_verification_pickup_id(&pickup_id) {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_verification_pickup",
                ));
            }
            let renewed = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.renew_verification_pickup($1, $2, $3::text::uuid)",
            )
            .bind(actor)
            .bind(repository)
            .bind(pickup_id)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "verification_pickup_not_current",
            ))?;
            response("verification_pickup", renewed)
        }
        VerifierRequest::PendingVerifications {
            repository,
            after_work_id,
            limit,
        } => {
            if !(1..=50).contains(&limit)
                || after_work_id.as_deref().is_some_and(|id| !identifier(id))
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_pending_verifications_filter",
                ));
            }
            let inbox = sqlx::query_scalar::<_, Value>(
                "SELECT craft_queue.pending_verifications($1, $2, $3, $4)",
            )
            .bind(actor)
            .bind(repository)
            .bind(after_work_id)
            .bind(limit)
            .fetch_one(&app.database)
            .await?;
            response("verification_inbox", inbox)
        }
        VerifierRequest::NextGithubReadiness {
            repository,
            base_branch,
            policy_digest,
        } => {
            if !operations::review_text(&base_branch, 200)
                || !operations::shadow_digest(&policy_digest)
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_github_readiness_binding",
                ));
            }
            let target = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.next_github_readiness($1, $2, $3, $4)",
            )
            .bind(actor)
            .bind(repository)
            .bind(base_branch)
            .bind(policy_digest)
            .fetch_one(&app.database)
            .await?;
            response("github_readiness_target", target.unwrap_or(Value::Null))
        }
        VerifierRequest::RecordGithubReadiness { receipt } => {
            operations::record_shadow(&app, &actor, receipt, true).await
        }
        VerifierRequest::CloseScheduledGithubWork {
            repository,
            base_branch,
            policy_file_digest,
            limit,
        } => {
            if !operations::review_text(&base_branch, 200)
                || !operations::shadow_digest(&policy_file_digest)
                || !(1..=20).contains(&limit)
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_scheduled_github_closure",
                ));
            }
            let closures = sqlx::query_scalar::<_, Value>(
                "SELECT craft_queue.close_scheduled_github_work($1, $2, $3, $4, $5)",
            )
            .bind(actor)
            .bind(repository)
            .bind(base_branch)
            .bind(policy_file_digest)
            .bind(limit)
            .fetch_one(&app.database)
            .await?;
            response("closures", closures)
        }
        VerifierRequest::DownloadPack { work_id } => download_pack(&app, &work_id).await,
        VerifierRequest::Status { work_id } => work_status(&app, &work_id).await,
        VerifierRequest::ConfirmGithubShadowMerge {
            receipt_id,
            head,
            report_digest,
            observation,
        } => {
            if !identifier(&receipt_id)
                || !craft_factory_evidence::valid_oid(&head)
                || !operations::shadow_digest(&report_digest)
                || observation.as_ref().is_some_and(|merge| {
                    ![&merge.result_commit, &merge.destination_tip]
                        .into_iter()
                        .all(|commit| craft_factory_evidence::valid_oid(commit))
                })
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_github_merge_observation",
                ));
            }
            let merge = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.confirm_github_shadow_merge($1, $2, $3, $4, $5)",
            )
            .bind(actor)
            .bind(receipt_id)
            .bind(head)
            .bind(report_digest)
            .bind(observation.map(sqlx::types::Json))
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "github_merge_observation_not_available",
            ))?;
            response("github_merge", merge)
        }
        VerifierRequest::ConfirmBitbucketWorkMerge {
            work_id,
            head,
            publication_key,
            observation,
        } => {
            if !identifier(&work_id)
                || !craft_factory_evidence::valid_oid(&head)
                || publication_key.is_some() != observation.is_some()
                || publication_key
                    .as_deref()
                    .is_some_and(|key| !operations::review_text(key, 300))
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_bitbucket_merge_observation",
                ));
            }
            let merge = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.confirm_bitbucket_work_merge($1, $2, $3, $4, $5)",
            )
            .bind(actor)
            .bind(work_id)
            .bind(head)
            .bind(publication_key)
            .bind(observation.map(sqlx::types::Json))
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "bitbucket_merge_observation_not_available",
            ))?;
            response("bitbucket_merge", merge)
        }
        request @ (VerifierRequest::RecordGithubCiChecks { .. }
        | VerifierRequest::RecordBitbucketCiChecks { .. }) => {
            let (
                work_id,
                generation,
                head,
                pack_digest,
                verification_sequence,
                proof,
                query,
                key,
                invalid,
                conflict,
            ) = match request {
                VerifierRequest::RecordGithubCiChecks {
                    work_id,
                    generation,
                    head,
                    pack_digest,
                    verification_sequence,
                    proof,
                } => (
                    work_id,
                    generation,
                    head,
                    pack_digest,
                    verification_sequence,
                    proof,
                    "SELECT craft_queue.record_github_ci_checks($1, $2, $3, $4, $5, $6, $7)",
                    "github_ci_checks",
                    "invalid_github_ci_check_binding",
                    "github_ci_check_binding_not_available",
                ),
                VerifierRequest::RecordBitbucketCiChecks {
                    work_id,
                    generation,
                    head,
                    pack_digest,
                    verification_sequence,
                    proof,
                } => (
                    work_id,
                    generation,
                    head,
                    pack_digest,
                    verification_sequence,
                    proof,
                    "SELECT craft_queue.record_bitbucket_ci_checks($1, $2, $3, $4, $5, $6, $7)",
                    "bitbucket_ci_checks",
                    "invalid_bitbucket_ci_check_binding",
                    "bitbucket_ci_check_binding_not_available",
                ),
                _ => unreachable!(),
            };
            if !identifier(&work_id)
                || generation < 1
                || verification_sequence < 1
                || !craft_factory_evidence::valid_oid(&head)
                || !operations::shadow_digest(&pack_digest)
            {
                return Err(ApiError(StatusCode::BAD_REQUEST, invalid));
            }
            let checks = sqlx::query_scalar::<_, Option<Value>>(query)
                .bind(actor)
                .bind(work_id)
                .bind(generation)
                .bind(head)
                .bind(pack_digest)
                .bind(verification_sequence)
                .bind(proof.map(sqlx::types::Json))
                .fetch_one(&app.database)
                .await?
                .ok_or(ApiError(StatusCode::CONFLICT, conflict))?;
            response(key, checks)
        }
        VerifierRequest::PendingMergeChecks {
            repository,
            after_work_id,
            limit,
        } => {
            use craft_factory_evidence::bitbucket_change::valid_slug;
            if !repository
                .split_once('/')
                .is_some_and(|(owner, repo)| valid_slug(owner) && valid_slug(repo))
                || !(1..=50).contains(&limit)
                || after_work_id.as_deref().is_some_and(|id| !identifier(id))
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_merge_check_filter",
                ));
            }
            let checks = sqlx::query_scalar::<_, Value>(
                "SELECT craft_queue.pending_merge_checks($1, $2, $3, $4)",
            )
            .bind(actor)
            .bind(repository)
            .bind(after_work_id)
            .bind(limit)
            .fetch_one(&app.database)
            .await?;
            response("merge_checks", checks)
        }
        VerifierRequest::RecordShadowOutcome {
            receipt_id,
            head,
            report_digest,
            basis,
            expected_eligible,
            evidence_digest,
            evidence,
            note,
        } => {
            if !identifier(&receipt_id)
                || !craft_factory_evidence::valid_oid(&head)
                || ![&report_digest, &evidence_digest]
                    .into_iter()
                    .all(|pin| operations::shadow_digest(pin))
                || !matches!(basis.as_str(), "fixture" | "forge_observed")
                || !operations::review_text(&evidence, 500)
                || !operations::review_text(&note, 1000)
            {
                return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_shadow_outcome"));
            }
            let outcome = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.record_shadow_outcome($1, $2, $3, $4, $5, $6, $7, $8, $9)",
            )
            .bind(actor)
            .bind(receipt_id)
            .bind(head)
            .bind(report_digest)
            .bind(basis)
            .bind(expected_eligible)
            .bind(evidence_digest)
            .bind(evidence)
            .bind(note)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "shadow_outcome_not_available",
            ))?;
            response("outcome", outcome)
        }
        VerifierRequest::RecordVerification {
            work_id,
            generation,
            head,
            digest,
            previous_sequence,
            passed,
            evidence,
            note,
            review_summary,
            pickup_id,
        } => {
            if pickup_id
                .as_deref()
                .is_some_and(|id| !valid_verification_pickup_id(id))
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_verification_pickup",
                ));
            }
            if review_summary
                .as_deref()
                .is_some_and(|summary| !operations::review_text(summary, 240))
            {
                return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_review_summary"));
            }
            let sequence = sqlx::query_scalar::<_, Option<i64>>(
                "SELECT craft_queue.record_picked_verification($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11::text::uuid)",
            )
            .bind(actor)
            .bind(work_id)
            .bind(generation)
            .bind(head)
            .bind(digest)
            .bind(previous_sequence)
            .bind(passed)
            .bind(evidence)
            .bind(note)
            .bind(review_summary)
            .bind(pickup_id)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(StatusCode::CONFLICT, "verification_not_current"))?;
            response("verification_sequence", sequence)
        }
        VerifierRequest::RecordDependencyVerification {
            work_id,
            generation,
            head,
            digest,
            previous_sequence,
            passed,
            approval_id,
            proof,
            pickup_id,
        } => {
            if pickup_id
                .as_deref()
                .is_some_and(|id| !valid_verification_pickup_id(id))
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_verification_pickup",
                ));
            }
            if !identifier(&work_id)
                || !identifier(&approval_id)
                || generation < 1
                || previous_sequence < 0
                || !craft_factory_evidence::valid_oid(&head)
                || !operations::shadow_digest(&digest)
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_dependency_verification",
                ));
            }
            let sequence = sqlx::query_scalar::<_, Option<i64>>(
                "SELECT craft_queue.record_picked_dependency_verification($1, $2, $3, $4, $5, $6, $7, $8, $9, $10::text::uuid)",
            )
            .bind(actor)
            .bind(work_id)
            .bind(generation)
            .bind(head)
            .bind(digest)
            .bind(previous_sequence)
            .bind(passed)
            .bind(approval_id)
            .bind(sqlx::types::Json(proof))
            .bind(pickup_id)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "dependency_verification_not_current",
            ))?;
            response("verification_sequence", sequence)
        }
    }
}

async fn operator(
    State(app): State<App>,
    headers: HeaderMap,
    Json(request): Json<OperatorRequest>,
) -> Api {
    let actor = authenticate(&app, &headers, Role::Operator)?;
    match request {
        OperatorRequest::ApproveCiLane { lane, approval } => {
            ci_lanes::approve(&app, actor, lane, approval).await
        }
        OperatorRequest::DisableCiLane { lane, evidence } => {
            ci_lanes::disable(&app, actor, lane, evidence).await
        }
        OperatorRequest::Admit {
            repository,
            event_key,
            work_order,
            token_limit,
        } => {
            let id = work_order["id"]
                .as_str()
                .filter(|id| identifier(id))
                .ok_or(ApiError(StatusCode::BAD_REQUEST, "invalid_work_order_id"))?;
            let failure = &work_order["ci_failure"];
            if !failure.is_null()
                && (work_order["require_native_checks"] != true
                    || !work_order["ci_policy_digest"]
                        .as_str()
                        .is_some_and(operations::shadow_digest)
                    || failure["repository"] != repository
                    || !(failure["step_uuid"].is_string() ^ failure["run_attempt"].is_u64()))
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_ci_failure_binding",
                ));
            }
            let id =
                sqlx::query_scalar::<_, String>("SELECT craft_queue.admit($1, $2, $3, $4, $5)")
                    .bind(id)
                    .bind(repository)
                    .bind(event_key)
                    .bind(&work_order)
                    .bind(token_limit)
                    .fetch_one(&app.database)
                    .await?;
            response("work_order_id", id)
        }
        OperatorRequest::Cancel { work_id, reason } => {
            let cancelled = sqlx::query_scalar::<_, bool>("SELECT craft_queue.cancel($1, $2)")
                .bind(work_id)
                .bind(reason)
                .fetch_one(&app.database)
                .await?;
            response("cancelled", cancelled)
        }
        OperatorRequest::SetPaused { paused } => {
            let paused = sqlx::query_scalar::<_, bool>("SELECT craft_queue.set_paused($1)")
                .bind(paused)
                .fetch_one(&app.database)
                .await?;
            response("paused", paused)
        }
        OperatorRequest::Status { work_id } => work_status(&app, &work_id).await,
        OperatorRequest::DownloadPack { work_id } => download_pack(&app, &work_id).await,
        OperatorRequest::RequestRepair {
            work_id,
            generation,
            head,
            native_url,
            evidence,
            note,
        } => {
            let repair = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.request_repair($1, $2, $3, $4, $5, $6, $7)",
            )
            .bind(actor)
            .bind(work_id)
            .bind(generation)
            .bind(head)
            .bind(native_url)
            .bind(evidence)
            .bind(note)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(StatusCode::CONFLICT, "repair_target_not_current"))?;
            response("repair", repair)
        }
    }
}

fn tls(settings: &Settings) -> Result<TlsAcceptor, Error> {
    let certs = rustls_pemfile::certs(&mut BufReader::new(File::open(&settings.certificate)?))
        .collect::<Result<Vec<_>, _>>()?;
    let key = rustls_pemfile::private_key(&mut BufReader::new(File::open(&settings.private_key)?))?
        .ok_or("missing PEM private key")?;
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_no_client_auth()
    .with_single_cert(certs, key)?;
    Ok(TlsAcceptor::from(Arc::new(config)))
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let settings = Settings::parse();
    let acceptor = tls(&settings)?;
    let identities = IdentityStore::load(&settings.identities)?;
    let diagnostic_cases = diagnostics::Catalog::load(settings.diagnostic_cases.as_deref())?;
    let rollout_archives = settings
        .rollout_archives
        .as_deref()
        .map(series::ArchiveCatalog::load)
        .transpose()?;
    let database = PgPoolOptions::new()
        .max_connections(12)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&database_url()?)
        .await
        .map_err(|_| "could not connect to the shared PostgreSQL queue")?;
    let status = timeout(Duration::from_secs(5), preflight::check_database(&database))
        .await
        .map_err(|_| "shared queue configuration check timed out")??;
    if settings.check_config {
        let (configured_identities, operator_configured) = identities.settings()?;
        println!(
            "{}",
            json!({
                "schema_version": 1, "ready": true, "configured_identities": configured_identities,
                "operator_configured": operator_configured,
                "database": status,
            })
        );
        return Ok(());
    }
    if let Some(epoch) = &settings.recover_restored_database {
        let recovery = operations::begin_recovery(&database, epoch, "restore-startup")
            .await
            .map_err(|_| "could not fence the restored PostgreSQL queue")?;
        if !recovery["finished_at"].is_null() {
            return Err("this restore recovery epoch already finished".into());
        }
    }
    let endpoints = Router::new()
        .route(
            "/v1/runner",
            post(runner).layer(DefaultBodyLimit::max(704 * 1024)),
        )
        .route(
            "/v1/operator",
            post(operator).layer(DefaultBodyLimit::max(64 * 1024)),
        )
        .route(
            "/v1/verifier",
            post(verifier).layer(DefaultBodyLimit::max(16 * 1024)),
        )
        .route(
            "/v1/cutover",
            post(cutover).layer(DefaultBodyLimit::max(1100 * 1024)),
        )
        .route(
            "/v1/intake",
            post(intake).layer(DefaultBodyLimit::max(64 * 1024)),
        )
        .merge(publication::routes())
        .merge(operations::routes())
        .merge(diagnostics::routes())
        .merge(series::routes())
        .with_state(App {
            identities,
            database,
            diagnostics: diagnostic_cases,
            rollout_verifiers: Arc::new(Semaphore::new(4)),
            rollout_archives,
            metrics_scrape: Arc::new(Semaphore::new(1)),
        });
    let listener = TcpListener::bind(settings.bind).await?;
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    #[cfg(unix)]
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    #[cfg(unix)]
    let shutdown = async {
        tokio::select! {
            _ = terminate.recv() => Ok::<_, std::io::Error>(()),
            _ = interrupt.recv() => Ok(()),
        }
    };
    #[cfg(not(unix))]
    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);

    println!("https://{}", listener.local_addr()?);
    let mut connections = JoinSet::new();
    loop {
        let (stream, _) = tokio::select! {
            biased;
            result = &mut shutdown => {
                result?;
                break;
            }
            _ = connections.join_next(), if !connections.is_empty() => continue,
            result = listener.accept() => result?,
        };
        if connections.len() >= 64 {
            continue;
        }
        let acceptor = acceptor.clone();
        let service = TowerToHyperService::new(endpoints.clone());
        connections.spawn(async move {
            let Ok(Ok(stream)) = timeout(Duration::from_secs(5), acceptor.accept(stream)).await
            else {
                return;
            };
            let mut builder = Builder::new(TokioExecutor::new());
            builder.http1().keep_alive(false);
            let _ = timeout(
                Duration::from_secs(30),
                builder.serve_connection(TokioIo::new(stream), service),
            )
            .await;
        });
    }
    drop(listener);
    while connections.join_next().await.is_some() {}
    Ok(())
}

fn database_url() -> Result<String, Error> {
    const NAME: &str = "CRAFT_DATABASE_URL";
    const FILE_NAME: &str = "CRAFT_DATABASE_URL_FILE";
    match std::env::var(NAME) {
        Ok(value) => return Ok(value),
        Err(std::env::VarError::NotPresent) => (),
        Err(_) => return Err("CRAFT_DATABASE_URL is not valid UTF-8".into()),
    }
    let path = std::env::var_os(FILE_NAME)
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("CREDENTIALS_DIRECTORY")
                .map(|directory| PathBuf::from(directory).join(NAME))
        })
        .ok_or(
            "CRAFT_DATABASE_URL, CRAFT_DATABASE_URL_FILE, or the service credential is required",
        )?;
    let bytes = craft_factory_evidence::release_trace::read_evidence(&path, 16 * 1024)
        .map_err(|_| "could not read the CRAFT_DATABASE_URL credential file")?;
    let value = std::str::from_utf8(&bytes)
        .map_err(|_| "invalid CRAFT_DATABASE_URL credential file")?
        .trim_end_matches(['\r', '\n']);
    if value.trim().is_empty() || value.contains(['\r', '\n', '\0']) {
        return Err("invalid CRAFT_DATABASE_URL credential file".into());
    }
    Ok(value.to_owned())
}
