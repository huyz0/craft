use axum::http::StatusCode;
use craft_factory_evidence::{
    bitbucket_change::normalize_uuid,
    ci::{CiFailure, LaneApproval},
    queue_identity::{Role, identifier, repository_key},
    release_trace::valid_digest,
    valid_oid,
};
use serde_json::Value;
use sqlx::types::Json;

use crate::{Api, ApiError, App, response};

const INVALID: ApiError = ApiError(StatusCode::BAD_REQUEST, "invalid_ci_lane_binding");
const UNAVAILABLE: ApiError = ApiError(StatusCode::FORBIDDEN, "ci_lane_not_available");

fn valid_text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

pub async fn approve(app: &App, actor: String, lane: String, approval: LaneApproval) -> Api {
    let template = &approval.template;
    if !identifier(&lane)
        || !identifier(&approval.observer)
        || !repository_key(&approval.repository)
        || !valid_text(&approval.branch, 200)
        || !approval.selector.valid()
        || !valid_digest(&approval.manifest_digest)
        || !valid_digest(&approval.policy_digest)
        || !valid_text(&approval.evidence, 500)
        || !(1..=10).contains(&approval.daily_limit)
        || approval.token_limit < 1
        || template["schema_version"] != 1
        || !template["id"].as_str().is_some_and(identifier)
        || !template["base"].as_str().is_some_and(valid_oid)
        || template["repository"] != approval.repository
        || [
            "ci_failure",
            "ci_anchor",
            "ci_policy_digest",
            "dependency_intake_digest",
        ]
        .iter()
        .any(|key| !template[key].is_null())
    {
        return Err(INVALID);
    }
    if !app.identities.contains(&approval.observer, Role::Intake)? {
        return Err(UNAVAILABLE);
    }
    let result =
        sqlx::query_scalar::<_, Option<Value>>("SELECT craft_queue.approve_ci_lane($1,$2,$3)")
            .bind(actor)
            .bind(lane)
            .bind(Json(approval))
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(StatusCode::CONFLICT, "ci_lane_approval_conflict"))?;
    response("ci_lane", result)
}

pub async fn disable(app: &App, actor: String, lane: String, evidence: String) -> Api {
    if !identifier(&lane) || !valid_text(&evidence, 500) {
        return Err(INVALID);
    }
    let result =
        sqlx::query_scalar::<_, Option<Value>>("SELECT craft_queue.disable_ci_lane($1,$2,$3)")
            .bind(actor)
            .bind(lane)
            .bind(evidence)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(StatusCode::CONFLICT, "ci_lane_disable_conflict"))?;
    response("ci_lane", result)
}

fn valid_failure(failure: &CiFailure) -> bool {
    let repo = failure.repository();
    if !repository_key(repo) || !valid_oid(failure.commit()) {
        return false;
    }
    match failure {
        CiFailure::Bitbucket(step) => {
            step.build > 0
                && normalize_uuid(&step.pipeline_uuid).as_deref() == Some(&step.pipeline_uuid)
                && normalize_uuid(&step.step_uuid).as_deref() == Some(&step.step_uuid)
                && valid_text(&step.completed_on, 80)
                && step.evidence_url
                    == format!(
                        "https://bitbucket.org/{repo}/pipelines/results/{}/steps/%7B{}%7D",
                        step.build, step.step_uuid
                    )
        }
        CiFailure::Github(job) => {
            job.run_id > 0
                && job.run_attempt > 0
                && job.job_id > 0
                && job.check_app_id > 0
                && valid_text(&job.completed_at, 80)
                && job.evidence_url
                    == format!(
                        "https://github.com/{repo}/actions/runs/{}/job/{}",
                        job.run_id, job.job_id
                    )
        }
    }
}

pub async fn observe(
    app: &App,
    actor: String,
    lane: String,
    manifest_digest: String,
    failure: Option<CiFailure>,
    admit: bool,
) -> Api {
    if !identifier(&lane)
        || !valid_digest(&manifest_digest)
        || failure.as_ref().is_some_and(|value| !valid_failure(value))
    {
        return Err(INVALID);
    }
    let event = failure
        .as_ref()
        .map(CiFailure::poll_digest)
        .transpose()
        .map_err(|_| INVALID)?;
    let outcome = sqlx::query_scalar::<_, Option<Value>>(
        "SELECT craft_queue.observe_ci_lane($1,$2,$3,$4,$5,$6)",
    )
    .bind(actor)
    .bind(lane)
    .bind(manifest_digest)
    .bind(failure.map(Json))
    .bind(event)
    .bind(admit)
    .fetch_one(&app.database)
    .await?
    .ok_or(UNAVAILABLE)?;
    response("decision", outcome)
}
