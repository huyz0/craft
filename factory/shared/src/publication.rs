use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    routing::post,
};
use serde::Deserialize;
use serde_json::Value;

use crate::{Api, ApiError, App, Role, authenticate, response};

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Outcome {
    Confirmed,
    Uncertain,
    Absent,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Observation {
    outcome: Outcome,
    head: String,
    native_url: Option<String>,
    source_branch: Option<String>,
    base_branch: Option<String>,
    evidence: String,
}

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct GithubObservation {
    native_url: String,
    head: String,
    base: String,
    source_branch: String,
    base_branch: String,
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum OperatorRequest {
    ApproveGithubEnqueuePolicy {
        policy: craft_factory_evidence::github_risk::EnqueuePolicy,
    },
    RevokeGithubEnqueuePolicy {
        policy_id: String,
        evidence: String,
    },
    ApproveGithubEnqueue {
        approval: crate::github_enqueue::Approval,
    },
    GithubEnqueueStatus {
        intent_id: String,
    },
    GithubEnqueueHolds {
        after_intent: Option<String>,
        #[serde(default)]
        include_queued: bool,
    },
    ClearGithubEnqueue {
        intent_id: String,
        observation: crate::github_enqueue::Clearance,
    },
    ResolveGithubEnqueue {
        intent_id: String,
        resolution: crate::github_enqueue::Resolution,
    },
    Reserve {
        work_id: String,
        generation: i64,
        head: String,
        verification_sequence: i64,
        #[serde(default)]
        update_draft: bool,
    },
    Status {
        action_key: String,
    },
    Reconcile {
        action_key: String,
        observation: Observation,
    },
    BindGithubReview {
        work_id: String,
        generation: i64,
        head: String,
        verification_sequence: i64,
        observation: Option<GithubObservation>,
    },
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum PublisherRequest {
    ClaimScheduledGithubEnqueue {
        policy_id: String,
        policy_digest: String,
    },
    AuthorizeGithubEnqueuePolicy {
        target: crate::github_enqueue::PolicyAuthorization,
    },
    PrepareGithubEnqueue {
        intent_id: String,
    },
    BeginGithubEnqueue {
        intent_id: String,
    },
    RecordGithubEnqueue {
        intent_id: String,
        resolution: crate::github_enqueue::Resolution,
    },
    Prepare {
        action_key: String,
        #[serde(default)]
        update_draft: bool,
    },
    Begin {
        action_key: String,
        #[serde(default)]
        update_draft: bool,
    },
    Record {
        action_key: String,
        observation: Observation,
    },
}

fn current(value: Option<Value>) -> Api {
    let value = value.ok_or(ApiError(StatusCode::CONFLICT, "publication_not_eligible"))?;
    response("publication", value)
}

async fn record(app: &App, actor: &str, key: String, observed: Observation, operator: bool) -> Api {
    let outcome = match observed.outcome {
        Outcome::Confirmed => "confirmed",
        Outcome::Uncertain => "uncertain",
        Outcome::Absent => "absent",
    };
    current(
        sqlx::query_scalar::<_, Option<Value>>(
            "SELECT craft_queue.record_draft_outcome($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(actor)
        .bind(key)
        .bind(operator)
        .bind(outcome)
        .bind(observed.head)
        .bind(observed.native_url)
        .bind(observed.source_branch)
        .bind(observed.base_branch)
        .bind(observed.evidence)
        .fetch_one(&app.database)
        .await?,
    )
}

async fn operator(
    State(app): State<App>,
    headers: HeaderMap,
    Json(request): Json<OperatorRequest>,
) -> Api {
    let actor = authenticate(&app, &headers, Role::Operator)?;
    match request {
        OperatorRequest::ApproveGithubEnqueuePolicy { policy } => {
            crate::github_enqueue::approve_policy(&app, &actor, policy).await
        }
        OperatorRequest::RevokeGithubEnqueuePolicy {
            policy_id,
            evidence,
        } => crate::github_enqueue::revoke_policy(&app, &actor, policy_id, evidence).await,
        OperatorRequest::ApproveGithubEnqueue { approval } => {
            crate::github_enqueue::approve(&app, &actor, approval).await
        }
        OperatorRequest::GithubEnqueueStatus { intent_id } => {
            crate::github_enqueue::status(&app, intent_id).await
        }
        OperatorRequest::GithubEnqueueHolds {
            after_intent,
            include_queued,
        } => crate::github_enqueue::holds(&app, after_intent, include_queued).await,
        OperatorRequest::ClearGithubEnqueue {
            intent_id,
            observation,
        } => crate::github_enqueue::clear(&app, &actor, intent_id, observation).await,
        OperatorRequest::ResolveGithubEnqueue {
            intent_id,
            resolution,
        } => crate::github_enqueue::resolve(&app, &actor, intent_id, resolution, true).await,
        OperatorRequest::Reserve {
            work_id,
            generation,
            head,
            verification_sequence,
            update_draft,
        } => current(
            sqlx::query_scalar::<_, Option<Value>>(if update_draft {
                "SELECT craft_queue.reserve_draft_update($1, $2, $3, $4, $5)"
            } else {
                "SELECT craft_queue.reserve_draft($1, $2, $3, $4, $5)"
            })
            .bind(actor)
            .bind(work_id)
            .bind(generation)
            .bind(head)
            .bind(verification_sequence)
            .fetch_one(&app.database)
            .await?,
        ),
        OperatorRequest::Status { action_key } => {
            let publication =
                sqlx::query_scalar::<_, Option<Value>>("SELECT craft_queue.publication_status($1)")
                    .bind(action_key)
                    .fetch_one(&app.database)
                    .await?
                    .ok_or(ApiError(StatusCode::NOT_FOUND, "unknown_publication"))?;
            response("publication", publication)
        }
        OperatorRequest::Reconcile {
            action_key,
            observation,
        } => record(&app, &actor, action_key, observation, true).await,
        OperatorRequest::BindGithubReview {
            work_id,
            generation,
            head,
            verification_sequence,
            observation,
        } => {
            if generation < 1
                || verification_sequence < 1
                || !craft_factory_evidence::valid_oid(&head)
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_github_review_binding",
                ));
            }
            let binding = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.bind_github_review($1, $2, $3, $4, $5, $6)",
            )
            .bind(actor)
            .bind(work_id)
            .bind(generation)
            .bind(head)
            .bind(verification_sequence)
            .bind(observation.map(sqlx::types::Json))
            .fetch_one(&app.database)
            .await?;
            current(binding)
        }
    }
}

async fn publisher(
    State(app): State<App>,
    headers: HeaderMap,
    Json(request): Json<PublisherRequest>,
) -> Api {
    let actor = authenticate(&app, &headers, Role::Publisher)?;
    match request {
        PublisherRequest::ClaimScheduledGithubEnqueue {
            policy_id,
            policy_digest,
        } => crate::github_enqueue::claim_scheduled(&app, &actor, policy_id, policy_digest).await,
        PublisherRequest::AuthorizeGithubEnqueuePolicy { target } => {
            crate::github_enqueue::authorize_policy(&app, &actor, target).await
        }
        PublisherRequest::PrepareGithubEnqueue { intent_id } => {
            crate::github_enqueue::prepare(&app, &actor, intent_id, false).await
        }
        PublisherRequest::BeginGithubEnqueue { intent_id } => {
            crate::github_enqueue::prepare(&app, &actor, intent_id, true).await
        }
        PublisherRequest::RecordGithubEnqueue {
            intent_id,
            resolution,
        } => crate::github_enqueue::resolve(&app, &actor, intent_id, resolution, false).await,
        PublisherRequest::Prepare {
            action_key,
            update_draft,
        } => current(
            sqlx::query_scalar::<_, Option<Value>>("SELECT craft_queue.prepare_draft($1, $2, $3)")
                .bind(actor)
                .bind(action_key)
                .bind(update_draft)
                .fetch_one(&app.database)
                .await?,
        ),
        PublisherRequest::Begin {
            action_key,
            update_draft,
        } => current(
            sqlx::query_scalar::<_, Option<Value>>("SELECT craft_queue.begin_draft($1, $2, $3)")
                .bind(actor)
                .bind(action_key)
                .bind(update_draft)
                .fetch_one(&app.database)
                .await?,
        ),
        PublisherRequest::Record {
            action_key,
            observation,
        } => record(&app, &actor, action_key, observation, false).await,
    }
}

pub fn routes() -> Router<App> {
    Router::new()
        .route("/v1/publication", post(operator))
        .route("/v1/publisher", post(publisher))
        .layer(DefaultBodyLimit::max(16 * 1024))
}
