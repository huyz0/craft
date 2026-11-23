use axum::http::StatusCode;
use craft_factory_evidence::{
    github_risk::EnqueuePolicy,
    queue_identity::{Role, identifier},
    valid_github_node_id, valid_oid,
};
use serde::Deserialize;
use serde_json::Value;

use crate::{Api, ApiError, App, operations::shadow_digest, response};

const INVALID: ApiError = ApiError(StatusCode::BAD_REQUEST, "invalid_github_enqueue_binding");
const CONFLICT: ApiError = ApiError(StatusCode::CONFLICT, "github_enqueue_not_eligible");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Approval {
    intent_id: String,
    receipt_id: String,
    head: String,
    report_digest: String,
    publisher_id: String,
    evidence: String,
    valid_seconds: i32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resolution {
    outcome: String,
    queue_entry_id: Option<String>,
    evidence: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Clearance {
    pull_request_node_id: String,
    observed_head: String,
    evidence: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyAuthorization {
    policy_id: String,
    intent_id: String,
    receipt_id: String,
    head: String,
    report_digest: String,
}

fn valid_evidence(text: &str) -> bool {
    !text.trim().is_empty() && text.len() <= 500 && !text.chars().any(char::is_control)
}

pub async fn claim_scheduled(app: &App, actor: &str, policy: String, digest: String) -> Api {
    if !identifier(&policy) || !shadow_digest(&digest) {
        return Err(INVALID);
    }
    let result = sqlx::query_scalar::<_, Option<Value>>(
        "SELECT craft_queue.claim_scheduled_github_enqueue($1,$2,$3)",
    )
    .bind(actor)
    .bind(policy)
    .bind(digest)
    .fetch_one(&app.database)
    .await?
    .ok_or(CONFLICT)?;
    response("github_enqueue_dispatch", result)
}

pub async fn approve_policy(app: &App, actor: &str, policy: EnqueuePolicy) -> Api {
    if !policy.valid() {
        return Err(INVALID);
    }
    if !app
        .identities
        .contains(&policy.publisher, Role::Publisher)?
    {
        return Err(CONFLICT);
    }
    let result = sqlx::query_scalar::<_, Option<Value>>(
        "SELECT craft_queue.approve_github_enqueue_policy($1,$2)",
    )
    .bind(actor)
    .bind(sqlx::types::Json(policy))
    .fetch_one(&app.database)
    .await?
    .ok_or(CONFLICT)?;
    response("github_enqueue_policy", result)
}

pub async fn revoke_policy(app: &App, actor: &str, policy: String, evidence: String) -> Api {
    if !identifier(&policy) || !valid_evidence(&evidence) {
        return Err(INVALID);
    }
    let result = sqlx::query_scalar::<_, Option<Value>>(
        "SELECT craft_queue.revoke_github_enqueue_policy($1,$2,$3)",
    )
    .bind(actor)
    .bind(policy)
    .bind(evidence)
    .fetch_one(&app.database)
    .await?
    .ok_or(CONFLICT)?;
    response("github_enqueue_policy", result)
}

pub async fn authorize_policy(app: &App, actor: &str, target: PolicyAuthorization) -> Api {
    if ![&target.policy_id, &target.intent_id, &target.receipt_id]
        .into_iter()
        .all(|id| identifier(id))
        || !valid_oid(&target.head)
        || !shadow_digest(&target.report_digest)
    {
        return Err(INVALID);
    }
    let result = sqlx::query_scalar::<_, Option<Value>>(
        "SELECT craft_queue.authorize_github_enqueue_policy($1,$2,$3,$4,$5,$6)",
    )
    .bind(actor)
    .bind(target.policy_id)
    .bind(target.intent_id)
    .bind(target.receipt_id)
    .bind(target.head)
    .bind(target.report_digest)
    .fetch_one(&app.database)
    .await?
    .ok_or(CONFLICT)?;
    response("github_enqueue", result)
}

pub async fn approve(app: &App, actor: &str, approval: Approval) -> Api {
    if ![
        &approval.intent_id,
        &approval.receipt_id,
        &approval.publisher_id,
    ]
    .into_iter()
    .all(|id| identifier(id))
        || !valid_oid(&approval.head)
        || !shadow_digest(&approval.report_digest)
        || !valid_evidence(&approval.evidence)
        || !(1..=600).contains(&approval.valid_seconds)
    {
        return Err(INVALID);
    }
    if !app
        .identities
        .contains(&approval.publisher_id, Role::Publisher)?
    {
        return Err(CONFLICT);
    }
    let result = sqlx::query_scalar::<_, Option<Value>>(
        "SELECT craft_queue.approve_github_enqueue($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(actor)
    .bind(approval.intent_id)
    .bind(approval.receipt_id)
    .bind(approval.head)
    .bind(approval.report_digest)
    .bind(approval.publisher_id)
    .bind(approval.evidence)
    .bind(approval.valid_seconds)
    .fetch_one(&app.database)
    .await?
    .ok_or(CONFLICT)?;
    response("github_enqueue", result)
}

pub async fn status(app: &App, intent: String) -> Api {
    if !identifier(&intent) {
        return Err(INVALID);
    }
    let result =
        sqlx::query_scalar::<_, Option<Value>>("SELECT craft_queue.github_enqueue_status($1)")
            .bind(intent)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(StatusCode::NOT_FOUND, "unknown_github_enqueue"))?;
    response("github_enqueue", result)
}

pub async fn holds(app: &App, after: Option<String>, include_queued: bool) -> Api {
    if after.as_deref().is_some_and(|id| !identifier(id)) {
        return Err(INVALID);
    }
    let result = sqlx::query_scalar::<_, Value>("SELECT craft_queue.github_enqueue_waits($1,$2)")
        .bind(after)
        .bind(include_queued)
        .fetch_one(&app.database)
        .await?;
    response("github_enqueue_holds", result)
}

pub async fn clear(app: &App, actor: &str, intent: String, observed: Clearance) -> Api {
    if !identifier(&intent)
        || !valid_github_node_id(&observed.pull_request_node_id)
        || !valid_oid(&observed.observed_head)
        || !valid_evidence(&observed.evidence)
    {
        return Err(INVALID);
    }
    let result = sqlx::query_scalar::<_, Option<Value>>(
        "SELECT craft_queue.clear_github_enqueue($1,$2,$3,$4,$5)",
    )
    .bind(actor)
    .bind(intent)
    .bind(observed.pull_request_node_id)
    .bind(observed.observed_head)
    .bind(observed.evidence)
    .fetch_one(&app.database)
    .await?
    .ok_or(CONFLICT)?;
    response("github_enqueue", result)
}

pub async fn prepare(app: &App, actor: &str, intent: String, start_send: bool) -> Api {
    if !identifier(&intent) {
        return Err(INVALID);
    }
    let result = sqlx::query_scalar::<_, Option<Value>>(
        "SELECT craft_queue.prepare_github_enqueue($1,$2,$3)",
    )
    .bind(actor)
    .bind(intent)
    .bind(start_send)
    .fetch_one(&app.database)
    .await?
    .ok_or(CONFLICT)?;
    response("github_enqueue", result)
}

pub async fn resolve(
    app: &App,
    actor: &str,
    intent: String,
    observed: Resolution,
    operator: bool,
) -> Api {
    if !identifier(&intent)
        || !valid_evidence(&observed.evidence)
        || !matches!(
            observed.outcome.as_str(),
            "queued" | "abandoned" | "uncertain"
        )
        || (observed.outcome == "queued") != observed.queue_entry_id.is_some()
        || observed
            .queue_entry_id
            .as_deref()
            .is_some_and(|id| !valid_github_node_id(id))
    {
        return Err(INVALID);
    }
    let result = sqlx::query_scalar::<_, Option<Value>>(
        "SELECT craft_queue.resolve_github_enqueue($1,$2,$3,$4,$5,$6)",
    )
    .bind(actor)
    .bind(intent)
    .bind(operator)
    .bind(observed.outcome)
    .bind(observed.queue_entry_id)
    .bind(observed.evidence)
    .fetch_one(&app.database)
    .await?
    .ok_or(CONFLICT)?;
    response("github_enqueue", result)
}
