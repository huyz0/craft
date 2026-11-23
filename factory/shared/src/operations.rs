use std::{collections::HashSet, fmt::Write, time::Duration};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use craft_factory_evidence::queue_identity::repository_key;
use serde::Deserialize;
use serde_json::Value;
use sqlx::PgPool;
use tokio::time::timeout;

use crate::{Api, ApiError, App, Role, authenticate, identifier, response};

async fn operation_summary(database: &PgPool) -> Result<Value, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT craft_queue.operation_summary() || craft_queue.review_wait_summary() || craft_queue.verification_wait_summary()",
    )
    .fetch_one(database)
    .await
}

async fn project_context_catalog(
    database: &PgPool,
    repository: &str,
    location: Option<&str>,
    revision: Option<&str>,
) -> Result<Value, sqlx::Error> {
    sqlx::query_scalar("SELECT craft_queue.inspect_project_context($1, $2, $3)")
        .bind(repository)
        .bind(location)
        .bind(revision)
        .fetch_one(database)
        .await
}

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct ComparisonPair {
    case_id: String,
    baseline: String,
    challenger: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewFilters {
    repository: Option<String>,
    owner: Option<String>,
    #[serde(default = "default_review_limit")]
    limit: i32,
    #[serde(default)]
    assigned_only: bool,
    after: Option<String>,
}

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct ShadowAssessment {
    repository: String,
    base: String,
    pull_request: String,
    policy_digest: String,
    observation_digest: String,
    as_of: u64,
    eligible: bool,
    native_verified: bool,
    blockers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    github: Option<craft_factory_evidence::GithubShadowPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    github_queue: Option<craft_factory_evidence::GithubMergeQueueReadiness>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    enqueue_risk: Option<craft_factory_evidence::github_risk::ChangeEvidence>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ShadowReceipt {
    receipt_id: String,
    pub(crate) work_id: String,
    generation: i64,
    head: String,
    pack_digest: String,
    verification_sequence: i64,
    report_digest: String,
    assessment: ShadowAssessment,
}

pub(crate) async fn record_shadow(
    app: &App,
    actor: &str,
    input: ShadowReceipt,
    scheduled: bool,
) -> Api {
    let ShadowReceipt {
        receipt_id,
        work_id,
        generation,
        head,
        pack_digest,
        verification_sequence,
        report_digest,
        assessment,
    } = input;
    let unique: HashSet<_> = assessment.blockers.iter().collect();
    if (scheduled
        && (assessment.github.is_none()
            || assessment.observation_digest != report_digest
            || (assessment.eligible && assessment.github_queue.is_none())))
        || !identifier(&receipt_id)
        || !identifier(&work_id)
        || generation < 1
        || verification_sequence < 1
        || !craft_factory_evidence::valid_oid(&head)
        || !craft_factory_evidence::valid_oid(&assessment.base)
        || ![
            &pack_digest,
            &report_digest,
            &assessment.policy_digest,
            &assessment.observation_digest,
        ]
        .into_iter()
        .all(|digest| shadow_digest(digest))
        || !review_text(&assessment.repository, 200)
        || !review_text(&assessment.pull_request, 500)
        || assessment.as_of > 999_999_999_999
        || assessment.eligible != assessment.blockers.is_empty()
        || assessment.github.as_ref().is_some_and(|pin| {
            pin.digest().as_deref() != Some(assessment.policy_digest.as_str())
                || assessment.eligible != assessment.native_verified
        })
        || assessment.github_queue.as_ref().is_some_and(|queue| {
            assessment.github.is_none() || !assessment.eligible || !queue.valid()
        })
        || assessment
            .enqueue_risk
            .as_ref()
            .is_some_and(|risk| assessment.github_queue.is_none() || !risk.valid())
        || unique.len() != assessment.blockers.len()
        || unique.len() > 24
        || unique.iter().any(|code| {
            code.is_empty()
                || code.len() > 80
                || !code
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
        })
    {
        return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_shadow_receipt"));
    }
    let receipt = sqlx::query_scalar::<_, Option<Value>>(if scheduled {
        "SELECT craft_queue.record_github_readiness($1, $2, $3, $4, $5, $6, $7, $8, $9)"
    } else {
        "SELECT craft_queue.record_shadow_merge($1, $2, $3, $4, $5, $6, $7, $8, $9)"
    })
    .bind(actor)
    .bind(receipt_id)
    .bind(work_id)
    .bind(generation)
    .bind(head)
    .bind(pack_digest)
    .bind(verification_sequence)
    .bind(report_digest)
    .bind(sqlx::types::Json(assessment))
    .fetch_one(&app.database)
    .await?
    .ok_or(ApiError(
        StatusCode::CONFLICT,
        "shadow_receipt_not_available",
    ))?;
    response("receipt", receipt)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DependencyApproval {
    id: String,
    work_id: String,
    generation: i64,
    head: String,
    previous_approval_id: Option<String>,
    evidence: String,
    pins: DependencyPins,
}

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct DependencyPins {
    intake_digest: String,
    policy_digest: String,
    baseline_run_digest: String,
    candidate_run_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    scanner_fresh_through: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scanner_refresh_digest: Option<String>,
}

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct ProjectContextRelease {
    repository: String,
    location: String,
    revision: String,
    kind: String,
    source_reference: String,
    source_revision: String,
    approval_reference: String,
    excerpt: String,
}

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct StandingRule {
    id: String,
    intake_id: String,
    source: String,
    project_key: String,
    repository: String,
    human_account: String,
    human_owner: String,
    required_label: String,
    eligible_status: String,
    allowed_paths: Vec<String>,
    preferred_runner: String,
    fallback_runner: Option<String>,
    fallback_seconds: Option<i32>,
    token_limit: i64,
    approval_reference: String,
    agent_adapter: Option<String>,
    execution_profile: Option<Value>,
}

impl StandingRule {
    fn valid(&self) -> bool {
        let site = self.source.strip_prefix("jira:");
        let valid_site = site
            .and_then(craft_factory_evidence::bitbucket_change::normalize_uuid)
            .is_some_and(|value| site == Some(value.as_str()));
        let github = self
            .source
            .strip_prefix("github:")
            .is_some_and(|repository| {
                let valid_part = |part: &str| {
                    !matches!(part, "" | "." | "..")
                        && part.len() <= 100
                        && part
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                };
                repository
                    .split_once('/')
                    .is_some_and(|(owner, name)| valid_part(owner) && valid_part(name))
                    && self.project_key == "GITHUB"
                    && self.eligible_status == "open"
                    && self
                        .human_account
                        .parse::<u64>()
                        .is_ok_and(|id| id > 0 && id.to_string() == self.human_account)
            });
        let project = self.project_key.as_bytes();
        let unique: HashSet<_> = self.allowed_paths.iter().collect();
        identifier(&self.id)
            && identifier(&self.intake_id)
            && identifier(&self.preferred_runner)
            && (valid_site || github)
            && repository_key(&self.repository)
            && project.first().is_some_and(u8::is_ascii_uppercase)
            && project.len() <= 31
            && project
                .iter()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
            && review_text(&self.human_account, 200)
            && review_text(&self.human_owner, 200)
            && review_text(&self.required_label, 100)
            && review_text(&self.eligible_status, 100)
            && review_text(&self.approval_reference, 500)
            && self.token_limit > 0
            && self.execution_profile.as_ref().is_none_or(Value::is_object)
            && self.agent_adapter.as_deref().is_none_or(|adapter| {
                matches!(adapter, "codex" | "claude") && self.execution_profile.is_some()
            })
            && (1..=50).contains(&unique.len())
            && unique.len() == self.allowed_paths.len()
            && unique.iter().all(|path| {
                review_text(path, 512)
                    && !path.contains(['\\', ':'])
                    && path.split('/').all(|part| {
                        !matches!(part, "" | "." | "..") && !part.eq_ignore_ascii_case(".git")
                    })
            })
            && match (&self.fallback_runner, self.fallback_seconds) {
                (None, None) => true,
                (Some(runner), Some(delay)) => {
                    identifier(runner)
                        && runner != &self.preferred_runner
                        && (1..=900).contains(&delay)
                }
                _ => false,
            }
    }
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Summary,
    DraftPolicy {
        repository: String,
    },
    ConfigureDraftPolicy {
        repository: String,
        base_branch: String,
        publisher_id: String,
        forge: String,
        expected_revision: i64,
        enabled: bool,
        evidence: String,
    },
    RegisterTeamSkill {
        release: TeamSkillRelease,
    },
    TeamSkillRelease {
        location: String,
        revision: String,
    },
    WithdrawTeamSkill {
        location: String,
        revision: String,
        evidence: String,
    },
    #[serde(alias = "approve_jira_standing_rule")]
    ApproveStandingRule {
        policy: StandingRule,
        digest: String,
    },
    #[serde(alias = "disable_jira_standing_rule")]
    DisableStandingRule {
        rule_id: String,
        evidence: String,
    },
    #[serde(alias = "approve_jira_standing_intent")]
    ApproveStandingIntent {
        rule_id: String,
        issue: String,
        source_revision: i64,
        review_token: String,
        evidence: String,
    },
    OperatorBrief {
        repository: Option<String>,
        usage_hours: i32,
        limit: i32,
    },
    ReloadIdentities {
        digest: String,
    },
    ReloadDiagnosticCatalog {
        digest: String,
        evidence: String,
    },
    ApproveDependencyEvidence {
        approval: DependencyApproval,
    },
    RegisterProjectContext {
        release: ProjectContextRelease,
        expected_revision: Option<String>,
    },
    InspectProjectContext {
        repository: String,
        location: Option<String>,
        revision: Option<String>,
    },
    WithdrawProjectContext {
        repository: String,
        location: String,
        revision: String,
        evidence: String,
    },
    ApproveShadowPolicy {
        repository: String,
        policy_digest: String,
        require_native: bool,
        evidence: String,
        valid_seconds: i32,
    },
    RecordShadowMerge {
        receipt_id: String,
        work_id: String,
        generation: i64,
        head: String,
        pack_digest: String,
        verification_sequence: i64,
        report_digest: String,
        assessment: ShadowAssessment,
    },
    ShadowMergeSummary {
        repository: String,
        since: i64,
    },
    ReviewInbox(ReviewFilters),
    GrantReviewInbox {
        grant_id: String,
        reviewer_id: String,
        repository: String,
        evidence: String,
        valid_seconds: i32,
    },
    RevokeReviewInbox {
        grant_id: String,
        evidence: String,
    },
    GrantDiagnosticAccess {
        grant_id: String,
        diagnostician_id: String,
        case_id: String,
        policy_digest: String,
        from: i64,
        to: i64,
        valid_seconds: i32,
        max_reads: i32,
        evidence: String,
    },
    RevokeDiagnosticAccess {
        grant_id: String,
        evidence: String,
    },
    AuditDiagnosticAccess {
        grant_id: String,
    },
    DiagnosticCaseAccess {
        case_id: String,
        policy_digest: Option<String>,
        after: Option<String>,
        limit: i32,
    },
    RetireDiagnosticCase {
        case_id: String,
        policy_digest: String,
        evidence: String,
    },
    AssignReview {
        handoff_id: String,
        work_id: String,
        generation: i64,
        head: String,
        verification_sequence: i64,
        reviewer_id: String,
        evidence: String,
        #[serde(default)]
        queue_repair: bool,
    },
    ReassignReview {
        handoff_id: String,
        replacement_handoff_id: String,
        head: String,
        reviewer_id: String,
        evidence: String,
        #[serde(default)]
        queue_repair: bool,
    },
    RouteReview {
        handoff_id: String,
        work_id: String,
        generation: i64,
        head: String,
        verification_sequence: i64,
        roster: Vec<String>,
        max_pending: i32,
        evidence: String,
        #[serde(default)]
        queue_repair: bool,
    },
    ApplyReviewRepair {
        handoff_id: String,
    },
    PendingKnowledge {
        repository: String,
    },
    InspectKnowledge {
        proposal_id: String,
    },
    ReviewKnowledge {
        proposal_id: String,
        proposal_digest: String,
        promote: bool,
        owner: String,
        evidence: String,
        verification_sequence: Option<i64>,
        repositories: Option<Vec<String>>,
    },
    WithdrawKnowledge {
        proposal_id: String,
        release_revision: String,
        evidence: String,
    },
    RunnerStatus {
        runner_id: String,
    },
    RunnerEnrollment {
        runner_id: String,
        repository: String,
    },
    SetRunnerEnrollment {
        runner_id: String,
        repository: String,
        expected_revision: i64,
        enrolled: bool,
        evidence: String,
    },
    DispatchStatus {
        work_id: String,
    },
    RetryCiPreflight {
        work_id: String,
        generation: i64,
        evidence: String,
    },
    ReviewWait {
        repository: Option<String>,
        limit: i32,
    },
    SetDispatchPriority {
        work_id: String,
        expected_revision: i64,
        priority: String,
        evidence: String,
    },
    SetRunnerMode {
        runner_id: String,
        expected_revision: i64,
        mode: String,
        evidence: String,
    },
    Audit {
        work_id: String,
    },
    CompareProfiles {
        pairs: Vec<ComparisonPair>,
    },
    ProfileUsage {
        work_id: String,
    },
    CloseGithubWork {
        work_id: String,
        merge_receipt_id: String,
        head: String,
        evidence: String,
    },
    CloseBitbucketWork {
        work_id: String,
        publication_key: String,
        head: String,
        evidence: String,
    },
    QueueUsage {
        repository: Option<String>,
        since_hours: i32,
        limit: i32,
    },
    EscalateProfile {
        work_id: String,
        generation: i64,
        replacement_id: String,
        execution_profile: Value,
        evidence: String,
    },
    ApproveProfileFallback {
        work_id: String,
        replacement_id: String,
        execution_profile: Value,
        evidence: String,
        valid_seconds: i32,
    },
    RevokeProfileFallback {
        work_id: String,
    },
    ApproveClaudeRepository {
        repository: String,
        approval_id: String,
        evidence: String,
        valid_seconds: i32,
    },
    RevokeClaudeRepository {
        approval_id: String,
        evidence: String,
    },
    HeldPublications,
    RecoveryStatus,
    BeginRecovery {
        epoch: String,
    },
    FinishRecovery {
        epoch: String,
        evidence: String,
    },
}

fn default_review_limit() -> i32 {
    20
}

pub(crate) fn shadow_digest(digest: &str) -> bool {
    digest.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
    })
}

fn valid_draft_branch(value: &str) -> bool {
    value.len() <= 150
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.'))
        && !value.contains("..")
        && !value.ends_with('.')
        && value
            .split('/')
            .all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock"))
}

pub(crate) fn review_text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.chars().count() <= max && !value.chars().any(char::is_control)
}

fn context_location(value: &str) -> bool {
    catalog_location(value, "project:")
}

fn catalog_location(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|slug| {
        slug.len() <= 80
            && slug
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            && slug
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_'))
    })
}

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct TeamSkillRelease {
    location: String,
    revision: String,
    markdown: String,
    owner_name: String,
    approval_reference: String,
    repositories: Vec<String>,
}

impl TeamSkillRelease {
    fn valid(&self) -> bool {
        catalog_location(&self.location, "team:")
            && self.revision == craft_factory_evidence::sha256(self.markdown.as_bytes())
            && (1..=32768).contains(&self.markdown.len())
            && !self.markdown.contains('\0')
            && review_text(&self.owner_name, 200)
            && review_text(&self.approval_reference, 500)
            && (1..=100).contains(&self.repositories.len())
            && self.repositories.windows(2).all(|pair| pair[0] < pair[1])
            && self.repositories.iter().all(|repo| repository_key(repo))
    }
}

fn context_revision(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && craft_factory_evidence::valid_oid(hex))
}

async fn review_inbox(pool: &PgPool, reviewer: Option<&str>, filters: ReviewFilters) -> Api {
    if !(1..=50).contains(&filters.limit)
        || filters
            .after
            .as_ref()
            .is_some_and(|id| !filters.assigned_only || !identifier(id))
        || [filters.repository.as_deref(), filters.owner.as_deref()]
            .into_iter()
            .flatten()
            .any(|value| !review_text(value, 200))
    {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "invalid_review_inbox_filter",
        ));
    }
    if filters.assigned_only {
        let reviewer = reviewer.ok_or(ApiError(
            StatusCode::BAD_REQUEST,
            "assignments_require_reviewer",
        ))?;
        let inbox = sqlx::query_scalar::<_, Option<Value>>(
            "SELECT craft_queue.review_assignment_page($1, $2, $3, $4, $5)",
        )
        .bind(reviewer)
        .bind(filters.repository)
        .bind(filters.owner)
        .bind(filters.limit)
        .bind(filters.after)
        .fetch_one(pool)
        .await?
        .ok_or(ApiError(
            StatusCode::CONFLICT,
            "review_page_cursor_unavailable",
        ))?;
        return response("inbox", inbox);
    }
    let inbox = sqlx::query_scalar::<_, Value>(
        "SELECT craft_queue.review_inbox($1, $2, $3, $4) || CASE WHEN $4::text IS NULL THEN '{}'::jsonb \
         ELSE jsonb_build_object('assignments', craft_queue.pending_review_handoffs($4, $1, $2, $3)) END",
    )
        .bind(filters.repository)
        .bind(filters.owner)
        .bind(filters.limit)
        .bind(reviewer)
        .fetch_one(pool)
        .await?;
    response("inbox", inbox)
}

pub async fn begin_recovery(pool: &PgPool, epoch: &str, actor: &str) -> Result<Value, sqlx::Error> {
    sqlx::query_scalar::<_, Value>("SELECT craft_queue.begin_recovery($1::text::uuid, $2)")
        .bind(epoch)
        .bind(actor)
        .fetch_one(pool)
        .await
}

async fn operations(
    State(app): State<App>,
    headers: HeaderMap,
    Json(request): Json<Request>,
) -> Api {
    let actor = authenticate(&app, &headers, Role::Operator)?;
    match request {
        Request::Summary => response("summary", operation_summary(&app.database).await?),
        Request::DraftPolicy { repository } => {
            if !repository_key(&repository) {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_draft_repository",
                ));
            }
            let result = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.inspect_draft_policy($1)",
            )
            .bind(repository)
            .fetch_one(&app.database)
            .await?;
            response(
                "policy",
                result.ok_or(ApiError(StatusCode::NOT_FOUND, "draft_policy_not_found"))?,
            )
        }
        Request::ConfigureDraftPolicy {
            repository,
            base_branch,
            publisher_id,
            forge,
            expected_revision,
            enabled,
            evidence,
        } => {
            if !repository_key(&repository)
                || !valid_draft_branch(&base_branch)
                || !identifier(&publisher_id)
                || !matches!(forge.as_str(), "bitbucket" | "github")
                || !(0..i64::MAX).contains(&expected_revision)
                || !review_text(&evidence, 500)
            {
                return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_draft_policy"));
            }
            if enabled && !app.identities.contains(&publisher_id, Role::Publisher)? {
                return Err(ApiError(
                    StatusCode::CONFLICT,
                    "draft_publisher_not_configured",
                ));
            }
            let result = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.configure_draft_policy($1,$2,$3,$4,$5,$6,$7,$8)",
            )
            .bind(actor)
            .bind(repository)
            .bind(base_branch)
            .bind(publisher_id)
            .bind(forge)
            .bind(expected_revision)
            .bind(enabled)
            .bind(evidence)
            .fetch_one(&app.database)
            .await?;
            response(
                "policy",
                result.ok_or(ApiError(StatusCode::CONFLICT, "draft_policy_conflict"))?,
            )
        }
        Request::RegisterTeamSkill { mut release } => {
            release.repositories.sort();
            if !release.valid() {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_team_skill_release",
                ));
            }
            let result = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.register_team_skill($1,$2)",
            )
            .bind(actor)
            .bind(sqlx::types::Json(release))
            .fetch_one(&app.database)
            .await?;
            response(
                "release",
                result.ok_or(ApiError(
                    StatusCode::CONFLICT,
                    "team_skill_release_conflict",
                ))?,
            )
        }
        Request::TeamSkillRelease { location, revision } => {
            if !catalog_location(&location, "team:") || !shadow_digest(&revision) {
                return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_team_skill_pin"));
            }
            let result = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.inspect_team_skill($1,$2)",
            )
            .bind(location)
            .bind(revision)
            .fetch_one(&app.database)
            .await?;
            response(
                "release",
                result.ok_or(ApiError(StatusCode::NOT_FOUND, "team_skill_not_found"))?,
            )
        }
        Request::WithdrawTeamSkill {
            location,
            revision,
            evidence,
        } => {
            if !catalog_location(&location, "team:")
                || !shadow_digest(&revision)
                || !review_text(&evidence, 500)
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_team_skill_withdrawal",
                ));
            }
            let result = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.withdraw_team_skill($1,$2,$3,$4)",
            )
            .bind(actor)
            .bind(location)
            .bind(revision)
            .bind(evidence)
            .fetch_one(&app.database)
            .await?;
            response(
                "release",
                result.ok_or(ApiError(
                    StatusCode::CONFLICT,
                    "team_skill_withdrawal_conflict",
                ))?,
            )
        }
        Request::ApproveStandingRule { policy, digest } => {
            if !policy.valid() || !shadow_digest(&digest) {
                return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_standing_rule"));
            }
            if !app.identities.contains(&policy.intake_id, Role::Intake)?
                || !app
                    .identities
                    .contains(&policy.preferred_runner, Role::Runner)?
                || match &policy.fallback_runner {
                    Some(id) => !app.identities.contains(id, Role::Runner)?,
                    None => false,
                }
            {
                return Err(ApiError(
                    StatusCode::CONFLICT,
                    "standing_identity_not_available",
                ));
            }
            let result = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.register_jira_standing_rule($1,$2,$3)",
            )
            .bind(actor)
            .bind(digest)
            .bind(sqlx::types::Json(policy))
            .fetch_one(&app.database)
            .await?;
            response(
                "standing_rule",
                result.ok_or(ApiError(
                    StatusCode::CONFLICT,
                    "standing_rule_not_available",
                ))?,
            )
        }
        Request::DisableStandingRule { rule_id, evidence } => {
            if !identifier(&rule_id) || !review_text(&evidence, 500) {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_standing_rule_withdrawal",
                ));
            }
            let result = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.disable_managed_standing_rule($1,$2,$3)",
            )
            .bind(actor)
            .bind(rule_id)
            .bind(evidence)
            .fetch_one(&app.database)
            .await?;
            response(
                "standing_rule",
                result.ok_or(ApiError(
                    StatusCode::CONFLICT,
                    "standing_rule_not_available",
                ))?,
            )
        }
        Request::ApproveStandingIntent {
            rule_id,
            issue,
            source_revision,
            review_token,
            evidence,
        } => {
            if !identifier(&rule_id)
                || !review_text(&issue, 50)
                || source_revision < 1
                || !shadow_digest(&review_token)
                || !review_text(&evidence, 500)
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_standing_intent_approval",
                ));
            }
            let result = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.approve_managed_jira_intent($1,$2,$3,$4,$5,$6)",
            )
            .bind(actor)
            .bind(rule_id)
            .bind(issue)
            .bind(source_revision)
            .bind(review_token)
            .bind(evidence)
            .fetch_one(&app.database)
            .await?;
            response(
                "intent_approval",
                result.ok_or(ApiError(
                    StatusCode::CONFLICT,
                    "standing_intent_not_available",
                ))?,
            )
        }
        Request::OperatorBrief {
            repository,
            usage_hours,
            limit,
        } => {
            if !(1..=20).contains(&limit)
                || !(1..=720).contains(&usage_hours)
                || repository
                    .as_deref()
                    .is_some_and(|repo| !repository_key(repo))
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_operator_brief_filter",
                ));
            }
            let mut tx = app.database.begin().await?;
            sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
                .execute(&mut *tx)
                .await?;
            let brief =
                sqlx::query_scalar::<_, Value>("SELECT craft_queue.operator_brief($1, $2, $3)")
                    .bind(repository)
                    .bind(usage_hours)
                    .bind(limit)
                    .fetch_one(&mut *tx)
                    .await?;
            tx.commit().await?;
            response("operator_brief", brief)
        }
        Request::ReloadIdentities { digest } => {
            response("identities", app.identities.reload(&headers, digest).await?)
        }
        Request::ReloadDiagnosticCatalog { digest, evidence } => response(
            "diagnostic_catalog",
            app.diagnostics
                .reload(&app, &headers, digest, evidence)
                .await?,
        ),
        Request::RegisterProjectContext {
            release,
            expected_revision,
        } => {
            if !review_text(&release.repository, 200)
                || !context_location(&release.location)
                || !context_revision(&release.revision)
                || expected_revision
                    .as_deref()
                    .is_some_and(|value| !context_revision(value))
                || !matches!(release.kind.as_str(), "decision" | "runbook")
                || !review_text(&release.source_reference, 500)
                || !review_text(&release.source_revision, 300)
                || !review_text(&release.approval_reference, 500)
                || release.excerpt.trim().is_empty()
                || release.excerpt.len() > 4096
                || release
                    .excerpt
                    .chars()
                    .any(|c| c.is_control() && !matches!(c, '\r' | '\n' | '\t'))
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_project_context_release",
                ));
            }
            let saved = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.register_project_context($1, $2, $3)",
            )
            .bind(actor)
            .bind(sqlx::types::Json(release))
            .bind(expected_revision)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "project_context_release_conflict",
            ))?;
            response("release", saved)
        }
        Request::InspectProjectContext {
            repository,
            location,
            revision,
        } => {
            if !review_text(&repository, 200)
                || location
                    .as_deref()
                    .is_some_and(|value| !context_location(value))
                || revision
                    .as_deref()
                    .is_some_and(|value| location.is_none() || !context_revision(value))
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_project_context_filter",
                ));
            }
            let catalog = project_context_catalog(
                &app.database,
                &repository,
                location.as_deref(),
                revision.as_deref(),
            )
            .await?;
            response("catalog", catalog)
        }
        Request::WithdrawProjectContext {
            repository,
            location,
            revision,
            evidence,
        } => {
            if !review_text(&repository, 200)
                || !context_location(&location)
                || !context_revision(&revision)
                || !review_text(&evidence, 500)
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_project_context_withdrawal",
                ));
            }
            let saved = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.withdraw_project_context($1, $2, $3, $4, $5)",
            )
            .bind(actor)
            .bind(repository)
            .bind(location)
            .bind(revision)
            .bind(evidence)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "project_context_withdrawal_conflict",
            ))?;
            response("release", saved)
        }
        Request::ApproveShadowPolicy {
            repository,
            policy_digest,
            require_native,
            evidence,
            valid_seconds,
        } => {
            if !review_text(&repository, 200)
                || !shadow_digest(&policy_digest)
                || !review_text(&evidence, 500)
                || !(1..=2592000).contains(&valid_seconds)
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_shadow_policy_approval",
                ));
            }
            let pin = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.approve_shadow_policy($1, $2, $3, $4, $5, $6)",
            )
            .bind(actor)
            .bind(repository)
            .bind(policy_digest)
            .bind(require_native)
            .bind(evidence)
            .bind(valid_seconds)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "shadow_policy_approval_not_available",
            ))?;
            response("policy", pin)
        }
        Request::ApproveDependencyEvidence { approval } => {
            if !identifier(&approval.id)
                || !identifier(&approval.work_id)
                || approval.generation < 1
                || !craft_factory_evidence::valid_oid(&approval.head)
                || approval
                    .previous_approval_id
                    .as_deref()
                    .is_some_and(|id| !identifier(id))
                || !review_text(&approval.evidence, 500)
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_dependency_evidence_approval",
                ));
            }
            let approved = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.approve_dependency_evidence($1, $2, $3, $4, $5, $6, $7, $8)",
            )
            .bind(actor)
            .bind(approval.id)
            .bind(approval.work_id)
            .bind(approval.generation)
            .bind(approval.head)
            .bind(approval.previous_approval_id)
            .bind(approval.evidence)
            .bind(sqlx::types::Json(approval.pins))
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "dependency_evidence_approval_not_current",
            ))?;
            response("approval", approved)
        }
        Request::RecordShadowMerge {
            receipt_id,
            work_id,
            generation,
            head,
            pack_digest,
            verification_sequence,
            report_digest,
            assessment,
        } => {
            record_shadow(
                &app,
                &actor,
                ShadowReceipt {
                    receipt_id,
                    work_id,
                    generation,
                    head,
                    pack_digest,
                    verification_sequence,
                    report_digest,
                    assessment,
                },
                false,
            )
            .await
        }
        Request::ShadowMergeSummary { repository, since } => {
            if !review_text(&repository, 200) || !(0..=999_999_999_999).contains(&since) {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_shadow_summary_filter",
                ));
            }
            let summary = sqlx::query_scalar::<_, Value>(
                "SELECT craft_queue.shadow_merge_summary($1, $2) || \
                    jsonb_build_object('qualification', craft_queue.shadow_qualification($1, $2))",
            )
            .bind(repository)
            .bind(since)
            .fetch_one(&app.database)
            .await?;
            response("summary", summary)
        }
        Request::ReviewInbox(filters) => review_inbox(&app.database, None, filters).await,
        Request::GrantReviewInbox {
            grant_id,
            reviewer_id,
            repository,
            evidence,
            valid_seconds,
        } => {
            if !identifier(&grant_id)
                || !identifier(&reviewer_id)
                || !review_text(&repository, 200)
                || !review_text(&evidence, 500)
                || !(1..=2592000).contains(&valid_seconds)
            {
                return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_review_grant"));
            }
            if !app.identities.contains(&reviewer_id, Role::Reviewer)? {
                return Err(ApiError(StatusCode::NOT_FOUND, "unknown_reviewer"));
            }
            let grant = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.grant_review_inbox($1, $2, $3, $4, $5, $6)",
            )
            .bind(actor)
            .bind(grant_id)
            .bind(reviewer_id)
            .bind(repository)
            .bind(evidence)
            .bind(valid_seconds)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(StatusCode::CONFLICT, "review_grant_not_available"))?;
            response("grant", grant)
        }
        Request::RevokeReviewInbox { grant_id, evidence } => {
            if !identifier(&grant_id) || !review_text(&evidence, 500) {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_review_grant_revocation",
                ));
            }
            let grant = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.revoke_review_inbox($1, $2, $3)",
            )
            .bind(actor)
            .bind(grant_id)
            .bind(evidence)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(StatusCode::CONFLICT, "review_grant_not_revocable"))?;
            response("grant", grant)
        }
        Request::GrantDiagnosticAccess {
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
            if !identifier(&grant_id)
                || !identifier(&diagnostician_id)
                || !identifier(&case_id)
                || !shadow_digest(&policy_digest)
                || from <= 0
                || to <= from
                || !(1..=2592000).contains(&valid_seconds)
                || !(1..=1000).contains(&max_reads)
                || !review_text(&evidence, 500)
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_diagnostic_grant",
                ));
            }
            if !app
                .identities
                .contains(&diagnostician_id, Role::Diagnostician)?
            {
                return Err(ApiError(StatusCode::NOT_FOUND, "unknown_diagnostician"));
            }
            if !app
                .diagnostics
                .approves_window(&case_id, &policy_digest, from as u64, to as u64)
            {
                return Err(ApiError(
                    StatusCode::CONFLICT,
                    "diagnostic_case_scope_not_available",
                ));
            }
            let grant = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.issue_diagnostic_grant($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
            )
            .bind(actor)
            .bind(grant_id)
            .bind(diagnostician_id)
            .bind(case_id)
            .bind(policy_digest)
            .bind(from)
            .bind(to)
            .bind(valid_seconds)
            .bind(max_reads)
            .bind(evidence)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "diagnostic_grant_not_available",
            ))?;
            response("grant", grant)
        }
        Request::RevokeDiagnosticAccess { grant_id, evidence } => {
            if !identifier(&grant_id) || !review_text(&evidence, 500) {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_diagnostic_grant_revocation",
                ));
            }
            let grant = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.revoke_diagnostic_grant($1, $2, $3)",
            )
            .bind(actor)
            .bind(grant_id)
            .bind(evidence)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "diagnostic_grant_not_revocable",
            ))?;
            response("grant", grant)
        }
        Request::AuditDiagnosticAccess { grant_id } => {
            if !identifier(&grant_id) {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_diagnostic_grant_id",
                ));
            }
            let audit = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.diagnostic_grant_audit($1)",
            )
            .bind(grant_id)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(StatusCode::NOT_FOUND, "unknown_diagnostic_grant"))?;
            response("audit", audit)
        }
        Request::DiagnosticCaseAccess {
            case_id,
            policy_digest,
            after,
            limit,
        } => {
            if !identifier(&case_id)
                || policy_digest
                    .as_deref()
                    .is_some_and(|value| !shadow_digest(value))
                || after.as_deref().is_some_and(|value| !identifier(value))
                || !(1..=50).contains(&limit)
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_diagnostic_case_access",
                ));
            }
            let page = sqlx::query_scalar::<_, Value>(
                "SELECT craft_queue.inspect_diagnostic_case_access($1,$2,$3,$4)",
            )
            .bind(case_id)
            .bind(policy_digest)
            .bind(after)
            .bind(limit)
            .fetch_one(&app.database)
            .await?;
            response("case_access", page)
        }
        Request::RetireDiagnosticCase {
            case_id,
            policy_digest,
            evidence,
        } => {
            if !identifier(&case_id)
                || !shadow_digest(&policy_digest)
                || !review_text(&evidence, 500)
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_diagnostic_case_retirement",
                ));
            }
            let result = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.retire_diagnostic_case_access($1,$2,$3,$4)",
            )
            .bind(actor)
            .bind(case_id)
            .bind(policy_digest)
            .bind(evidence)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "diagnostic_case_retirement_blocked",
            ))?;
            response("case_retirement", result)
        }
        Request::AssignReview {
            handoff_id,
            work_id,
            generation,
            head,
            verification_sequence,
            reviewer_id,
            evidence,
            queue_repair,
        } => {
            if !identifier(&handoff_id)
                || !identifier(&work_id)
                || !identifier(&reviewer_id)
                || generation < 1
                || verification_sequence < 1
                || !craft_factory_evidence::valid_oid(&head)
                || !review_text(&evidence, 500)
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_review_assignment",
                ));
            }
            if !app.identities.contains(&reviewer_id, Role::Reviewer)? {
                return Err(ApiError(StatusCode::NOT_FOUND, "unknown_reviewer"));
            }
            let handoff = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.assign_review_with_repair($1, $2, $3, $4, $5, $6, $7, $8, $9)",
            )
            .bind(actor)
            .bind(handoff_id)
            .bind(work_id)
            .bind(generation)
            .bind(head)
            .bind(verification_sequence)
            .bind(reviewer_id)
            .bind(evidence)
            .bind(queue_repair)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "review_assignment_not_available",
            ))?;
            response("handoff", handoff)
        }
        Request::ReassignReview {
            handoff_id,
            replacement_handoff_id,
            head,
            reviewer_id,
            evidence,
            queue_repair,
        } => {
            if !identifier(&handoff_id)
                || !identifier(&replacement_handoff_id)
                || handoff_id == replacement_handoff_id
                || !identifier(&reviewer_id)
                || !craft_factory_evidence::valid_oid(&head)
                || !review_text(&evidence, 500)
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_review_reassignment",
                ));
            }
            if !app.identities.contains(&reviewer_id, Role::Reviewer)? {
                return Err(ApiError(StatusCode::NOT_FOUND, "unknown_reviewer"));
            }
            let reassignment = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.reassign_review($1, $2, $3, $4, $5, $6, $7)",
            )
            .bind(actor)
            .bind(handoff_id)
            .bind(replacement_handoff_id)
            .bind(head)
            .bind(reviewer_id)
            .bind(evidence)
            .bind(queue_repair)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "review_reassignment_not_available",
            ))?;
            response("reassignment", reassignment)
        }
        Request::RouteReview {
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
            if !identifier(&handoff_id)
                || !identifier(&work_id)
                || generation < 1
                || verification_sequence < 1
                || !craft_factory_evidence::valid_oid(&head)
                || !review_text(&evidence, 500)
                || !(1..=25).contains(&roster.len())
                || !(1..=50).contains(&max_pending)
                || roster.iter().any(|reviewer| !identifier(reviewer))
                || roster.iter().collect::<HashSet<_>>().len() != roster.len()
            {
                return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_review_routing"));
            }
            for reviewer in &roster {
                if !app.identities.contains(reviewer, Role::Reviewer)? {
                    return Err(ApiError(StatusCode::NOT_FOUND, "unknown_reviewer"));
                }
            }
            let assignment = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.route_review($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
            )
            .bind(actor)
            .bind(handoff_id)
            .bind(work_id)
            .bind(generation)
            .bind(head)
            .bind(verification_sequence)
            .bind(roster)
            .bind(max_pending)
            .bind(evidence)
            .bind(queue_repair)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "review_routing_not_available",
            ))?;
            response("assignment", assignment)
        }
        Request::ApplyReviewRepair { handoff_id } => {
            if !identifier(&handoff_id) {
                return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_review_handoff"));
            }
            let repair = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.apply_review_repair($1, $2)",
            )
            .bind(actor)
            .bind(handoff_id)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "review_repair_not_available",
            ))?;
            response("repair", repair)
        }
        Request::RunnerStatus { runner_id } => {
            if !identifier(&runner_id) {
                return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_runner_id"));
            }
            let status =
                sqlx::query_scalar::<_, Option<Value>>("SELECT craft_queue.runner_status($1)")
                    .bind(runner_id)
                    .fetch_one(&app.database)
                    .await?
                    .ok_or(ApiError(StatusCode::NOT_FOUND, "unknown_runner"))?;
            response("runner", status)
        }
        Request::RunnerEnrollment {
            runner_id,
            repository,
        } => {
            if !identifier(&runner_id) || !repository_key(&repository) {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_runner_enrollment_scope",
                ));
            }
            let configured = app.identities.contains(&runner_id, Role::Runner)?;
            let mut enrollment =
                sqlx::query_scalar::<_, Value>("SELECT craft_queue.runner_enrollment($1, $2)")
                    .bind(runner_id)
                    .bind(repository)
                    .fetch_one(&app.database)
                    .await?;
            enrollment["runner_configured"] = Value::Bool(configured);
            response("enrollment", enrollment)
        }
        Request::SetRunnerEnrollment {
            runner_id,
            repository,
            expected_revision,
            enrolled,
            evidence,
        } => {
            if !identifier(&runner_id)
                || !repository_key(&repository)
                || expected_revision < 0
                || evidence.trim().is_empty()
                || evidence.len() > 500
                || evidence.chars().any(char::is_control)
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_runner_enrollment_change",
                ));
            }
            if enrolled && !app.identities.contains(&runner_id, Role::Runner)? {
                return Err(ApiError(
                    StatusCode::CONFLICT,
                    "runner_identity_not_configured",
                ));
            }
            let decision = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.set_runner_enrollment($1, $2, $3, $4, $5, $6)",
            )
            .bind(actor)
            .bind(runner_id)
            .bind(repository)
            .bind(expected_revision)
            .bind(enrolled)
            .bind(evidence)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "runner_enrollment_change_not_available",
            ))?;
            response("decision", decision)
        }
        Request::RetryCiPreflight {
            work_id,
            generation,
            evidence,
        } => {
            if !identifier(&work_id) || generation < 1 || !review_text(&evidence, 500) {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_ci_preflight_retry",
                ));
            }
            let hold = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.retry_ci_preflight($1, $2, $3, $4)",
            )
            .bind(actor)
            .bind(work_id)
            .bind(generation)
            .bind(evidence)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "ci_preflight_retry_not_available",
            ))?;
            response("ci_preflight_hold", hold)
        }
        Request::DispatchStatus { work_id } => {
            if !identifier(&work_id) {
                return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_work_id"));
            }
            let status =
                sqlx::query_scalar::<_, Option<Value>>("SELECT craft_queue.dispatch_status($1)")
                    .bind(work_id)
                    .fetch_one(&app.database)
                    .await?
                    .ok_or(ApiError(StatusCode::NOT_FOUND, "unknown_work_order"))?;
            response("dispatch", status)
        }
        Request::ReviewWait { repository, limit } => {
            if !(1..=50).contains(&limit)
                || repository
                    .as_deref()
                    .is_some_and(|repo| !repository_key(repo))
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_review_wait_filter",
                ));
            }
            let backlog =
                sqlx::query_scalar::<_, Value>("SELECT craft_queue.review_wait_backlog($1, $2)")
                    .bind(repository)
                    .bind(limit)
                    .fetch_one(&app.database)
                    .await?;
            response("review_wait", backlog)
        }
        Request::SetDispatchPriority {
            work_id,
            expected_revision,
            priority,
            evidence,
        } => {
            if !identifier(&work_id)
                || !(0..i64::MAX).contains(&expected_revision)
                || !matches!(priority.as_str(), "normal" | "expedited")
                || !review_text(&evidence, 500)
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_dispatch_priority",
                ));
            }
            let decision = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.set_dispatch_priority($1, $2, $3, $4, $5)",
            )
            .bind(actor)
            .bind(work_id)
            .bind(expected_revision)
            .bind(priority)
            .bind(evidence)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "dispatch_priority_change_not_available",
            ))?;
            response("decision", decision)
        }
        Request::SetRunnerMode {
            runner_id,
            expected_revision,
            mode,
            evidence,
        } => {
            if !identifier(&runner_id)
                || expected_revision < 0
                || !matches!(mode.as_str(), "active" | "draining" | "stopped")
                || evidence.trim().is_empty()
                || evidence.len() > 500
                || evidence.chars().any(char::is_control)
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_runner_mode_change",
                ));
            }
            let decision = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.set_runner_mode($1, $2, $3, $4, $5)",
            )
            .bind(actor)
            .bind(runner_id)
            .bind(expected_revision)
            .bind(mode)
            .bind(evidence)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "runner_mode_change_not_available",
            ))?;
            response("decision", decision)
        }
        Request::Audit { work_id } => {
            let audit = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.work_audit($1) || jsonb_strip_nulls(\
                jsonb_build_object('profile_escalation', craft_queue.profile_escalation_for($1), \
                'profile_fallback', craft_queue.profile_fallback_for($1), \
                'ci_lane_admission', craft_queue.ci_lane_admission_for($1))) || \
                jsonb_build_object('review_handoffs', craft_queue.review_handoffs_for($1), \
                'shadow_merge_receipts', craft_queue.shadow_merge_receipts_for($1), \
                'shadow_outcomes', craft_queue.shadow_outcomes_for($1), \
                'dependency_verifications', craft_queue.dependency_verifications_for($1), \
                'dependency_approvals', craft_queue.dependency_approvals_for($1), \
                'github_ci_checks', craft_queue.github_ci_checks_for($1), \
                'bitbucket_ci_checks', craft_queue.bitbucket_ci_checks_for($1), \
                'ci_preflight_holds', craft_queue.ci_preflight_holds_for($1), \
                'github_enqueue_intents', craft_queue.github_enqueue_intents_for($1))",
            )
            .bind(work_id)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(StatusCode::NOT_FOUND, "unknown_work_order"))?;
            response("audit", audit)
        }
        Request::ProfileUsage { work_id } => {
            if !identifier(&work_id) {
                return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_profile_work_id"));
            }
            let usage = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.execution_profile_usage($1)",
            )
            .bind(work_id)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::NOT_FOUND,
                "profile_usage_not_available",
            ))?;
            response("usage", usage)
        }
        Request::CloseGithubWork {
            work_id,
            merge_receipt_id,
            head,
            evidence,
        } => {
            if !identifier(&work_id)
                || !identifier(&merge_receipt_id)
                || !craft_factory_evidence::valid_oid(&head)
                || !review_text(&evidence, 500)
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_github_work_closure",
                ));
            }
            let closure = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.close_github_work($1, $2, $3, $4, $5)",
            )
            .bind(actor)
            .bind(work_id)
            .bind(merge_receipt_id)
            .bind(head)
            .bind(evidence)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "github_work_closure_not_available",
            ))?;
            response("closure", closure)
        }
        Request::CloseBitbucketWork {
            work_id,
            publication_key,
            head,
            evidence,
        } => {
            if !identifier(&work_id)
                || !review_text(&publication_key, 300)
                || !craft_factory_evidence::valid_oid(&head)
                || !review_text(&evidence, 500)
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_bitbucket_work_closure",
                ));
            }
            let closure = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.close_bitbucket_work($1, $2, $3, $4, $5)",
            )
            .bind(actor)
            .bind(work_id)
            .bind(publication_key)
            .bind(head)
            .bind(evidence)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "bitbucket_work_closure_not_available",
            ))?;
            response("closure", closure)
        }
        Request::QueueUsage {
            repository,
            since_hours,
            limit,
        } => {
            if !(1..=720).contains(&since_hours)
                || !(1..=100).contains(&limit)
                || repository
                    .as_deref()
                    .is_some_and(|repo| !repository_key(repo))
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_queue_usage_filter",
                ));
            }
            let usage =
                sqlx::query_scalar::<_, Value>("SELECT craft_queue.queue_usage($1, $2, $3)")
                    .bind(repository)
                    .bind(since_hours)
                    .bind(limit)
                    .fetch_one(&app.database)
                    .await?;
            response("usage", usage)
        }
        Request::CompareProfiles { pairs } => {
            let mut cases = HashSet::new();
            let mut orders = HashSet::new();
            if !(1..=20).contains(&pairs.len())
                || pairs.iter().any(|pair| {
                    !identifier(&pair.case_id)
                        || !cases.insert(&pair.case_id)
                        || ![&pair.baseline, &pair.challenger]
                            .into_iter()
                            .all(|id| identifier(id) && orders.insert(id))
                })
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_comparison_pairs",
                ));
            }
            let comparison = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT CASE WHEN EXISTS (\
                    SELECT 1 FROM jsonb_to_recordset($1) AS p(baseline text, challenger text) \
                    WHERE craft_queue.profile_escalation_for(p.baseline) IS NOT NULL \
                       OR craft_queue.profile_escalation_for(p.challenger) IS NOT NULL\
                ) THEN NULL ELSE craft_queue.compare_execution_profiles($1) END",
            )
            .bind(sqlx::types::Json(pairs))
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "comparison_inputs_do_not_match",
            ))?;
            response("comparison", comparison)
        }
        Request::EscalateProfile {
            work_id,
            generation,
            replacement_id,
            execution_profile,
            evidence,
        } => {
            if !identifier(&work_id)
                || !identifier(&replacement_id)
                || generation < 1
                || evidence.trim().is_empty()
                || evidence.len() > 500
                || evidence.chars().any(char::is_control)
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_profile_escalation",
                ));
            }
            let decision = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.escalate_profile($1, $2, $3, $4, $5, $6)",
            )
            .bind(actor)
            .bind(work_id)
            .bind(generation)
            .bind(replacement_id)
            .bind(execution_profile)
            .bind(evidence)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "profile_escalation_not_available",
            ))?;
            response("escalation", decision)
        }
        Request::ApproveProfileFallback {
            work_id,
            replacement_id,
            execution_profile,
            evidence,
            valid_seconds,
        } => {
            if !identifier(&work_id)
                || !identifier(&replacement_id)
                || !(1..=604800).contains(&valid_seconds)
                || evidence.trim().is_empty()
                || evidence.len() > 500
                || evidence.chars().any(char::is_control)
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_profile_fallback",
                ));
            }
            let approval = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.approve_profile_fallback($1, $2, $3, $4, $5, $6)",
            )
            .bind(actor)
            .bind(work_id)
            .bind(replacement_id)
            .bind(execution_profile)
            .bind(evidence)
            .bind(valid_seconds)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "profile_fallback_not_available",
            ))?;
            response("fallback", approval)
        }
        Request::RevokeProfileFallback { work_id } => {
            let revoked = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.revoke_profile_fallback($1, $2)",
            )
            .bind(actor)
            .bind(work_id)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "profile_fallback_not_revocable",
            ))?;
            response("fallback", revoked)
        }
        Request::ApproveClaudeRepository {
            repository,
            approval_id,
            evidence,
            valid_seconds,
        } => {
            if !identifier(&approval_id)
                || !review_text(&repository, 200)
                || !review_text(&evidence, 500)
                || !(1..=2592000).contains(&valid_seconds)
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_claude_repository_approval",
                ));
            }
            let approval = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.approve_claude_repository($1, $2, $3, $4, $5)",
            )
            .bind(actor)
            .bind(repository)
            .bind(approval_id)
            .bind(evidence)
            .bind(valid_seconds)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "claude_repository_approval_not_available",
            ))?;
            response("approval", approval)
        }
        Request::RevokeClaudeRepository {
            approval_id,
            evidence,
        } => {
            if !identifier(&approval_id) || !review_text(&evidence, 500) {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_claude_repository_revocation",
                ));
            }
            let approval = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.revoke_claude_repository($1, $2, $3)",
            )
            .bind(actor)
            .bind(approval_id)
            .bind(evidence)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "claude_repository_approval_not_revocable",
            ))?;
            response("approval", approval)
        }
        Request::PendingKnowledge { repository } => {
            if !review_text(&repository, 200) {
                return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_repository"));
            }
            response(
                "pending",
                sqlx::query_scalar::<_, Value>("SELECT craft_queue.pending_knowledge($1)")
                    .bind(repository)
                    .fetch_one(&app.database)
                    .await?,
            )
        }
        Request::InspectKnowledge { proposal_id } => {
            if !identifier(&proposal_id) {
                return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_proposal_id"));
            }
            let item =
                sqlx::query_scalar::<_, Option<Value>>("SELECT craft_queue.knowledge_proposal($1)")
                    .bind(proposal_id)
                    .fetch_one(&app.database)
                    .await?
                    .ok_or(ApiError(
                        StatusCode::NOT_FOUND,
                        "unknown_knowledge_proposal",
                    ))?;
            response("proposal", item)
        }
        Request::ReviewKnowledge {
            proposal_id,
            proposal_digest,
            promote,
            owner,
            evidence,
            verification_sequence,
            repositories,
        } => {
            if !identifier(&proposal_id)
                || !shadow_digest(&proposal_digest)
                || !review_text(&owner, 200)
                || !review_text(&evidence, 500)
                || (promote && verification_sequence.is_none_or(|sequence| sequence <= 0))
                || (!promote && verification_sequence.is_some())
                || repositories.as_ref().is_some_and(|targets| {
                    !promote
                        || !(1..=16).contains(&targets.len())
                        || !targets
                            .iter()
                            .all(|target| target.len() <= 200 && repository_key(target))
                        || targets.iter().collect::<HashSet<_>>().len() != targets.len()
                })
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_knowledge_decision",
                ));
            }
            let decision = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.review_knowledge($1, $2, $3, $4, $5, $6, $7, $8)",
            )
            .bind(actor)
            .bind(proposal_id)
            .bind(proposal_digest)
            .bind(promote)
            .bind(owner)
            .bind(evidence)
            .bind(verification_sequence)
            .bind(repositories)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "knowledge_decision_not_available",
            ))?;
            response("decision", decision)
        }
        Request::WithdrawKnowledge {
            proposal_id,
            release_revision,
            evidence,
        } => {
            if !identifier(&proposal_id)
                || !shadow_digest(&release_revision)
                || !review_text(&evidence, 500)
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_knowledge_withdrawal",
                ));
            }
            let withdrawal = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.withdraw_knowledge($1,$2,$3,$4)",
            )
            .bind(actor)
            .bind(proposal_id)
            .bind(release_revision)
            .bind(evidence)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "knowledge_withdrawal_conflict",
            ))?;
            response("withdrawal", withdrawal)
        }
        Request::HeldPublications => response(
            "holds",
            sqlx::query_scalar::<_, Value>("SELECT craft_queue.publication_holds()")
                .fetch_one(&app.database)
                .await?,
        ),
        Request::RecoveryStatus => response(
            "recovery",
            sqlx::query_scalar::<_, Option<Value>>("SELECT craft_queue.recovery_status()")
                .fetch_one(&app.database)
                .await?
                .unwrap_or(Value::Null),
        ),
        Request::BeginRecovery { epoch } => response(
            "recovery",
            begin_recovery(&app.database, &epoch, &actor).await?,
        ),
        Request::FinishRecovery { epoch, evidence } => {
            let recovery = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.finish_recovery($1::text::uuid, $2, $3)",
            )
            .bind(epoch)
            .bind(actor)
            .bind(evidence)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(StatusCode::CONFLICT, "recovery_not_ready"))?;
            response("recovery", recovery)
        }
    }
}

async fn health(State(app): State<App>) -> Api {
    let ready = sqlx::query_scalar::<_, bool>(
        "SELECT to_regprocedure('craft_queue.operation_summary()') IS NOT NULL",
    )
    .fetch_one(&app.database)
    .await?;
    if !ready {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "queue_unavailable",
        ));
    }
    response("ready", true)
}

async fn metrics(State(app): State<App>, headers: HeaderMap) -> Result<Response, ApiError> {
    authenticate(&app, &headers, Role::MetricsReader)?;
    let _slot = app
        .metrics_scrape
        .try_acquire()
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "queue_metrics_busy"))?;
    const UNAVAILABLE: ApiError =
        ApiError(StatusCode::SERVICE_UNAVAILABLE, "queue_metrics_unavailable");
    let summary = timeout(Duration::from_secs(2), operation_summary(&app.database))
        .await
        .map_err(|_| UNAVAILABLE)?
        .map_err(|_| UNAVAILABLE)?;
    let mut body = String::new();
    for key in [
        "paused",
        "recovery_active",
        "review_limit",
        "review_reserved",
        "pending_review_assignments",
        "oldest_pending_review_seconds",
        "waiting_verification_candidates",
        "active_verification_pickups",
        "oldest_waiting_verification_seconds",
        "queued",
        "queued_repairs",
        "queued_standing",
        "standing_delegation_blocked",
        "oldest_queued_seconds",
        "live_leases",
        "overdue_leases",
        "oldest_live_lease_seconds",
        "tokens_charged_or_reserved",
        "token_budget",
        "token_overrun_attempts_15m",
        "tokens_above_reservation_15m",
        "held_publications",
    ] {
        let value = match summary.get(key) {
            Some(Value::Number(value)) => value.to_string(),
            Some(Value::Bool(value)) => u8::from(*value).to_string(),
            Some(Value::Null)
                if matches!(
                    key,
                    "oldest_queued_seconds"
                        | "oldest_live_lease_seconds"
                        | "oldest_pending_review_seconds"
                        | "oldest_waiting_verification_seconds"
                ) =>
            {
                continue;
            }
            _ => return Err(UNAVAILABLE),
        };
        writeln!(
            body,
            "# TYPE craft_queue_{key} gauge\ncraft_queue_{key} {value}"
        )
        .expect("writing to a String cannot fail");
    }
    Ok((
        [
            (
                header::CONTENT_TYPE,
                "text/plain; version=0.0.4; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response())
}

pub fn routes() -> Router<App> {
    Router::new()
        .route("/healthz", get(health))
        .route("/metrics", get(metrics))
        .route(
            "/v1/operations",
            post(operations).layer(DefaultBodyLimit::max(256 * 1024)),
        )
        .route(
            "/v1/profile-scheduler",
            post(profile_scheduler).layer(DefaultBodyLimit::max(1024)),
        )
        .route(
            "/v1/context-reader",
            post(context_reader).layer(DefaultBodyLimit::max(1024)),
        )
        .route(
            "/v1/reviewer",
            post(reviewer_inbox).layer(DefaultBodyLimit::max(12 * 1024)),
        )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContextRead {
    repository: String,
    location: String,
}

async fn context_reader(
    State(app): State<App>,
    headers: HeaderMap,
    Json(query): Json<ContextRead>,
) -> Api {
    let identity = crate::identities::authenticate_context_reader(&app, &headers)?;
    if !repository_key(&query.repository) || !context_location(&query.location) {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "invalid_project_context_filter",
        ));
    }
    if !identity
        .repositories
        .as_ref()
        .is_some_and(|scope| scope.contains(&query.repository))
    {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "context_repository_not_allowed",
        ));
    }
    let catalog = project_context_catalog(
        &app.database,
        &query.repository,
        Some(&query.location),
        None,
    )
    .await?;
    response("catalog", catalog)
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum ReviewerRequest {
    ReviewInbox(ReviewFilters),
    ReviewFeedbackTarget {
        handoff_id: String,
    },
    RecordReviewDecision {
        handoff_id: String,
        head: String,
        decision: String,
        evidence: String,
        note: String,
    },
}

async fn reviewer_inbox(
    State(app): State<App>,
    headers: HeaderMap,
    Json(request): Json<ReviewerRequest>,
) -> Api {
    let reviewer = authenticate(&app, &headers, Role::Reviewer)?;
    match request {
        ReviewerRequest::ReviewInbox(filters) => {
            review_inbox(&app.database, Some(&reviewer), filters).await
        }
        ReviewerRequest::ReviewFeedbackTarget { handoff_id } => {
            if !identifier(&handoff_id) {
                return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_handoff_id"));
            }
            let target = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.review_feedback_target($1, $2)",
            )
            .bind(reviewer)
            .bind(handoff_id)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "review_feedback_not_available",
            ))?;
            response("target", target)
        }
        ReviewerRequest::RecordReviewDecision {
            handoff_id,
            head,
            decision,
            evidence,
            note,
        } => {
            if !identifier(&handoff_id)
                || !craft_factory_evidence::valid_oid(&head)
                || !matches!(decision.as_str(), "acknowledged" | "changes_requested")
                || !review_text(&evidence, 500)
                || !review_text(&note, 1000)
            {
                return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_review_decision"));
            }
            let handoff = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.record_review_decision_with_repair($1, $2, $3, $4, $5, $6)",
            )
            .bind(reviewer)
            .bind(handoff_id)
            .bind(head)
            .bind(decision)
            .bind(evidence)
            .bind(note)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "review_decision_not_available",
            ))?;
            response("handoff", handoff)
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum SchedulerRequest {
    ApplyProfileFallbacks,
}

async fn profile_scheduler(
    State(app): State<App>,
    headers: HeaderMap,
    Json(SchedulerRequest::ApplyProfileFallbacks): Json<SchedulerRequest>,
) -> Api {
    let scheduler = authenticate(&app, &headers, Role::ProfileScheduler)?;
    let sweep = sqlx::query_scalar::<_, Value>("SELECT craft_queue.apply_profile_fallbacks($1)")
        .bind(scheduler)
        .fetch_one(&app.database)
        .await?;
    response("sweep", sweep)
}
