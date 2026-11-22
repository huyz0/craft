use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderMap, StatusCode},
    routing::post,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use craft_factory_evidence::{
    release_trace::{Target as ReleaseTarget, read_evidence, valid_digest},
    remediation, sha256,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Mutex, RwLock};

use crate::{Api, ApiError, App, Error, Role, authenticate, identifier, response};

const MAX_POLICY_BYTES: usize = 64 * 1024;
const MAX_RELEASE_BYTES: usize = 512 * 1024;
const MAX_PROOF_REQUEST: usize = 800 * 1024;
const PROOF_REJECTED: ApiError = ApiError(StatusCode::CONFLICT, "rollout_proof_not_current");

#[derive(Clone)]
pub struct ArchiveCatalog(Arc<CatalogState>);

struct CatalogState {
    path: PathBuf,
    active: RwLock<CatalogSnapshot>,
    reload: Mutex<()>,
}

struct CatalogSnapshot {
    digest: String,
    entries: HashMap<String, remediation::ArchiveEntry>,
}

impl ArchiveCatalog {
    pub fn load(path: &Path) -> Result<Self, Error> {
        let path = path.canonicalize()?;
        let active = CatalogSnapshot::read(&path, None)?;
        Ok(Self(Arc::new(CatalogState {
            path,
            active: RwLock::new(active),
            reload: Mutex::new(()),
        })))
    }

    async fn reload(&self, digest: String) -> Result<Value, ApiError> {
        if !valid_digest(&digest) {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "invalid_rollout_catalog_digest",
            ));
        }
        let _reload =
            self.0.reload.try_lock().map_err(|_| {
                ApiError(StatusCode::CONFLICT, "rollout_catalog_reload_in_progress")
            })?;
        let path = self.0.path.clone();
        let candidate =
            tokio::task::spawn_blocking(move || CatalogSnapshot::read(&path, Some(&digest)))
                .await
                .map_err(|_| {
                    ApiError(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "rollout_catalog_reload_failed",
                    )
                })?
                .map_err(|_| {
                    ApiError(StatusCode::CONFLICT, "invalid_rollout_catalog_replacement")
                })?;
        let mut active = self.0.active.write().await;
        let changed = active.digest != candidate.digest;
        *active = candidate;
        Ok(json!({"changed": changed, "digest": active.digest, "entries": active.entries.len()}))
    }
}

impl CatalogSnapshot {
    fn read(path: &Path, expected_digest: Option<&str>) -> Result<Self, Error> {
        if path.canonicalize()? != path {
            return Err("rollout catalog must stay at its configured path".into());
        }
        let root = path
            .parent()
            .ok_or("rollout archive catalog has no parent")?;
        let bytes = read_evidence(path, 128 * 1024)?;
        let digest = sha256(&bytes);
        if expected_digest.is_some_and(|expected| digest != expected) {
            return Err("rollout archive catalog digest mismatch".into());
        }
        let entries: Vec<remediation::ArchiveEntry> = serde_json::from_slice(&bytes)?;
        if entries.len() > 256 {
            return Err("too many rollout archives".into());
        }
        let mut cases = HashMap::new();
        for mut entry in entries {
            if !identifier(&entry.work_id)
                || !valid_digest(&entry.policy_digest)
                || !valid_digest(&entry.sources_digest)
            {
                return Err("invalid rollout archive entry".into());
            }
            entry.policy = archive_file(root, &entry.policy)?;
            entry.sources = archive_file(root, &entry.sources)?;
            if expected_digest.is_some()
                && (sha256(&read_evidence(&entry.policy, MAX_POLICY_BYTES as u64)?)
                    != entry.policy_digest
                    || sha256(&read_evidence(&entry.sources, 32 * 1024)?) != entry.sources_digest)
            {
                return Err("rollout archive entry does not match its file digests".into());
            }
            if cases.insert(entry.work_id.clone(), entry).is_some() {
                return Err("duplicate rollout archive work ID".into());
            }
        }
        Ok(Self {
            digest,
            entries: cases,
        })
    }
}

fn archive_file(root: &Path, path: &Path) -> Result<PathBuf, Error> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("rollout archive paths must be relative to the catalog".into());
    }
    let path = root.join(path).canonicalize()?;
    if !path.starts_with(root) || !path.is_file() {
        return Err("rollout archive file must resolve inside the catalog directory".into());
    }
    Ok(path)
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Target {
    service: String,
    environment: String,
    region: String,
    resource: String,
}

impl Target {
    fn valid(&self) -> bool {
        [
            &self.service,
            &self.environment,
            &self.region,
            &self.resource,
        ]
        .iter()
        .all(|field| {
            !field.trim().is_empty() && field.len() <= 200 && !field.chars().any(char::is_control)
        })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Member {
    repository: String,
    observer: String,
    target: Target,
    token_limit: i64,
    work_order: Value,
    #[serde(default, skip_serializing_if = "is_false")]
    requires_rollout_proof: bool,
}

impl Member {
    fn dependency_update(&self) -> bool {
        self.work_order
            .as_object()
            .is_some_and(|order| order.contains_key("dependency_intake_digest"))
    }
}

fn is_false(value: &bool) -> bool {
    !value
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum OperatorRequest {
    Register {
        id: String,
        owner: String,
        approval: String,
        members: Vec<Member>,
    },
    Status {
        id: String,
    },
    TransferObserver {
        transfer_id: String,
        work_id: String,
        expected_revision: i64,
        observer_id: String,
        evidence: String,
    },
    ReloadRolloutCatalog {
        digest: String,
    },
    ApproveRollout {
        work_id: String,
        generation: i64,
        head: String,
        verification_sequence: i64,
        policy_digest: String,
        archive_index_digest: Option<String>,
        approval: String,
    },
    ApproveRolloutRefresh {
        work_id: String,
        refresh_id: String,
        observation_sequence: i64,
        policy_digest: String,
        archive_index_digest: String,
        approval: String,
    },
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum ObserverRequest {
    Status {
        work_id: String,
    },
    Inbox {
        repository: Option<String>,
        after: Option<String>,
        limit: i32,
        #[serde(default)]
        include_closed: bool,
    },
    Observe {
        work_id: String,
        generation: i64,
        head: String,
        verification_sequence: i64,
        target: Target,
        artifact_digest: String,
        evidence: String,
    },
    Withdraw {
        work_id: String,
        sequence: i64,
        evidence: String,
    },
}

async fn operator(
    State(app): State<App>,
    headers: HeaderMap,
    Json(request): Json<OperatorRequest>,
) -> Api {
    let actor = authenticate(&app, &headers, Role::Operator)?;
    let series = match request {
        OperatorRequest::Register {
            id,
            owner,
            approval,
            members,
        } => {
            if !identifier(&id)
                || !(1..=12).contains(&members.len())
                || (members.len() == 1 && !members[0].dependency_update())
                || members.iter().any(|member| {
                    !identifier(&member.observer)
                        || !member.target.valid()
                        || member.repository.trim().is_empty()
                        || member.repository.len() > 200
                        || member.token_limit <= 0
                        || (member.dependency_update() && !member.requires_rollout_proof)
                        || !member.work_order["id"].as_str().is_some_and(identifier)
                })
            {
                return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_release_series"));
            }
            sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.register_series($1, $2, $3, $4, $5)",
            )
            .bind(actor)
            .bind(id)
            .bind(owner)
            .bind(approval)
            .bind(sqlx::types::Json(members))
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::CONFLICT,
                "series_registration_conflict",
            ))?
        }
        OperatorRequest::Status { id } => {
            sqlx::query_scalar::<_, Option<Value>>("SELECT craft_queue.series_status($1)")
                .bind(id)
                .fetch_one(&app.database)
                .await?
                .ok_or(ApiError(StatusCode::NOT_FOUND, "unknown_release_series"))?
        }
        OperatorRequest::TransferObserver {
            transfer_id,
            work_id,
            expected_revision,
            observer_id,
            evidence,
        } => {
            if !identifier(&transfer_id)
                || !identifier(&work_id)
                || !identifier(&observer_id)
                || !(0..i64::MAX).contains(&expected_revision)
                || !crate::operations::review_text(&evidence, 500)
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_observer_transfer",
                ));
            }
            if !app
                .identities
                .contains(&observer_id, Role::ReleaseObserver)?
            {
                return Err(ApiError(StatusCode::NOT_FOUND, "unknown_release_observer"));
            }
            let receipt = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.transfer_release_observer($1,$2,$3,$4,$5,$6)",
            )
            .bind(actor)
            .bind(transfer_id)
            .bind(work_id)
            .bind(expected_revision)
            .bind(observer_id)
            .bind(evidence)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(StatusCode::CONFLICT, "observer_transfer_conflict"))?;
            return response("observer_transfer", receipt);
        }
        OperatorRequest::ReloadRolloutCatalog { digest } => {
            let catalog = app.rollout_archives.as_ref().ok_or(ApiError(
                StatusCode::CONFLICT,
                "rollout_catalog_not_configured",
            ))?;
            return response("rollout_catalog", catalog.reload(digest).await?);
        }
        OperatorRequest::ApproveRollout {
            work_id,
            generation,
            head,
            verification_sequence,
            policy_digest,
            archive_index_digest,
            approval,
        } => {
            let catalog = match &app.rollout_archives {
                Some(catalog) => Some(catalog.0.active.read().await),
                None => None,
            };
            match (&catalog, archive_index_digest.as_deref()) {
                (None, None) => {}
                (Some(catalog), Some(digest))
                    if catalog.entries.get(&work_id).is_some_and(|case| {
                        case.policy_digest == policy_digest && case.sources_digest == digest
                    }) => {}
                _ => {
                    return Err(ApiError(
                        StatusCode::CONFLICT,
                        "rollout_approval_archive_mismatch",
                    ));
                }
            }
            let approved = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.approve_series_rollout($1, $2, $3, $4, $5, $6, $7, $8)",
            )
            .bind(actor)
            .bind(work_id)
            .bind(generation)
            .bind(head)
            .bind(verification_sequence)
            .bind(policy_digest)
            .bind(approval)
            .bind(archive_index_digest)
            .fetch_one(&app.database)
            .await?
            .ok_or(PROOF_REJECTED)?;
            return response("rollout_approval", approved);
        }
        OperatorRequest::ApproveRolloutRefresh {
            work_id,
            refresh_id,
            observation_sequence,
            policy_digest,
            archive_index_digest,
            approval,
        } => {
            let catalog = app
                .rollout_archives
                .as_ref()
                .ok_or(ApiError(
                    StatusCode::CONFLICT,
                    "rollout_approval_archive_mismatch",
                ))?
                .0
                .active
                .read()
                .await;
            let case = catalog.entries.get(&work_id);
            if case.is_none_or(|case| {
                case.policy_digest != policy_digest || case.sources_digest != archive_index_digest
            }) {
                return Err(ApiError(
                    StatusCode::CONFLICT,
                    "rollout_approval_archive_mismatch",
                ));
            }
            let approved = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.approve_rollout_refresh($1, $2, $3, $4, $5, $6, $7)",
            )
            .bind(actor)
            .bind(work_id)
            .bind(refresh_id)
            .bind(observation_sequence)
            .bind(policy_digest)
            .bind(archive_index_digest)
            .bind(approval)
            .fetch_one(&app.database)
            .await?
            .ok_or(PROOF_REJECTED)?;
            return response("rollout_refresh", approved);
        }
    };
    response("series", series)
}

async fn observer(
    State(app): State<App>,
    headers: HeaderMap,
    Json(request): Json<ObserverRequest>,
) -> Api {
    let actor = authenticate(&app, &headers, Role::ReleaseObserver)?;
    let release = match request {
        ObserverRequest::Inbox {
            repository,
            after,
            limit,
            include_closed,
        } => {
            if repository
                .as_deref()
                .is_some_and(|value| !crate::operations::review_text(value, 200))
                || after.as_deref().is_some_and(|value| !identifier(value))
                || !(1..=50).contains(&limit)
            {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_release_observer_inbox",
                ));
            }
            let page = sqlx::query_scalar::<_, Value>(
                "SELECT craft_queue.release_observer_inbox($1,$2,$3,$4,$5)",
            )
            .bind(actor)
            .bind(repository)
            .bind(after)
            .bind(limit)
            .bind(include_closed)
            .fetch_one(&app.database)
            .await?;
            return response("observer_inbox", page);
        }
        ObserverRequest::Status { work_id } => {
            if !identifier(&work_id) {
                return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_release_work_id"));
            }
            let status = sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.release_observer_status($1,$2)",
            )
            .bind(actor)
            .bind(work_id)
            .fetch_one(&app.database)
            .await?
            .ok_or(ApiError(
                StatusCode::NOT_FOUND,
                "release_work_not_available",
            ))?;
            return response("observer_status", status);
        }
        ObserverRequest::Observe {
            work_id,
            generation,
            head,
            verification_sequence,
            target,
            artifact_digest,
            evidence,
        } => {
            if !identifier(&work_id) || !target.valid() {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_release_observation",
                ));
            }
            sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.record_series_release($1, $2, $3, $4, $5, $6, $7, $8)",
            )
            .bind(actor)
            .bind(work_id)
            .bind(generation)
            .bind(head)
            .bind(verification_sequence)
            .bind(sqlx::types::Json(target))
            .bind(artifact_digest)
            .bind(evidence)
            .fetch_one(&app.database)
            .await?
        }
        ObserverRequest::Withdraw {
            work_id,
            sequence,
            evidence,
        } => {
            sqlx::query_scalar::<_, Option<Value>>(
                "SELECT craft_queue.withdraw_series_release($1, $2, $3, $4)",
            )
            .bind(actor)
            .bind(work_id)
            .bind(sequence)
            .bind(evidence)
            .fetch_one(&app.database)
            .await?
        }
    }
    .ok_or(ApiError(
        StatusCode::CONFLICT,
        "release_observation_not_current",
    ))?;
    response("release", release)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RolloutRequest {
    work_id: String,
    generation: i64,
    head: String,
    verification_sequence: i64,
    #[serde(default)]
    archived: bool,
    #[serde(default)]
    check_only: bool,
    refresh_id: Option<String>,
    policy_base64: Option<String>,
    evidence_base64: Option<String>,
}

enum RolloutEvidence {
    Normalized { policy: String, evidence: String },
    Archive(Option<remediation::ArchiveEntry>),
}

#[derive(Deserialize)]
struct RolloutBinding {
    repository: String,
    work_item: String,
    target: ReleaseTarget,
    policy_digest: String,
    archive_index_digest: Option<String>,
    artifact_digest: Option<String>,
    github_merge: Option<GithubRolloutMerge>,
    as_of: u64,
}

#[derive(Deserialize, serde::Serialize)]
struct GithubRolloutMerge {
    pull_request: String,
    result_commit: String,
}

async fn verified_rollout(State(app): State<App>, request: Request) -> Api {
    let actor = authenticate(&app, request.headers(), Role::ReleaseObserver)?;
    let body = axum::body::to_bytes(request.into_body(), MAX_PROOF_REQUEST)
        .await
        .map_err(|_| ApiError(StatusCode::PAYLOAD_TOO_LARGE, "rollout_request_limit"))?;
    let input: RolloutRequest = serde_json::from_slice(&body)
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_rollout_request"))?;
    if !identifier(&input.work_id)
        || !craft_factory_evidence::valid_oid(&input.head)
        || (input.check_only && !input.archived)
        || input
            .refresh_id
            .as_deref()
            .is_some_and(|id| !input.archived || !identifier(id))
    {
        return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_rollout_request"));
    }
    let preflight = |mut details: Value| {
        details["work_id"] = json!(input.work_id);
        details["generation"] = json!(input.generation);
        details["head"] = json!(input.head);
        details["verification_sequence"] = json!(input.verification_sequence);
        details["refresh_id"] = json!(input.refresh_id);
        response("preflight", details)
    };
    let blocked = |code, targets| {
        if input.check_only {
            preflight(json!({"evidence_ready": false, "blocker": code, "pending_targets": targets}))
        } else {
            Err(PROOF_REJECTED)
        }
    };
    // Keep the active snapshot through the database write. Reload cannot overtake this decision.
    let catalog = match &app.rollout_archives {
        Some(catalog) => Some(catalog.0.active.read().await),
        None => None,
    };
    let evidence = match (
        &catalog,
        input.archived,
        input.policy_base64,
        input.evidence_base64,
    ) {
        (None, false, Some(policy), Some(evidence)) => {
            RolloutEvidence::Normalized { policy, evidence }
        }
        (Some(catalog), true, None, None) => {
            RolloutEvidence::Archive(catalog.entries.get(&input.work_id).cloned())
        }
        (Some(_), false, Some(_), Some(_)) | (None, true, None, None) => {
            return blocked("rollout_mode_unavailable", Vec::<Value>::new());
        }
        _ => return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_rollout_request")),
    };
    let query = sqlx::query_scalar::<_, Option<Value>>(if input.refresh_id.is_some() {
        "SELECT craft_queue.series_rollout_refresh_binding($1, $2, $3, $4, $5, $6)"
    } else {
        "SELECT craft_queue.series_rollout_binding($1, $2, $3, $4, $5)"
    })
    .bind(&actor)
    .bind(&input.work_id)
    .bind(input.generation)
    .bind(&input.head)
    .bind(input.verification_sequence);
    let query = if let Some(id) = &input.refresh_id {
        query.bind(id)
    } else {
        query
    };
    let Some(binding) = query.fetch_one(&app.database).await? else {
        if input.check_only
            && input.refresh_id.is_none()
            && sqlx::query_scalar::<_, bool>(
                "SELECT craft_queue.series_dependency_scan_expired($1, $2, $3, $4, $5)",
            )
            .bind(&actor)
            .bind(&input.work_id)
            .bind(input.generation)
            .bind(&input.head)
            .bind(input.verification_sequence)
            .fetch_one(&app.database)
            .await?
        {
            return blocked("dependency_scan_expired", Vec::<Value>::new());
        }
        return blocked("rollout_binding_unavailable", Vec::<Value>::new());
    };
    let binding: RolloutBinding = serde_json::from_value(binding)
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "invalid_rollout_binding"))?;
    let evaluated_at = binding.as_of;
    let slot = app
        .rollout_verifiers
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(StatusCode::TOO_MANY_REQUESTS, "rollout_verifier_busy"))?;
    let expected_head = input.head.clone();
    let checked = tokio::task::spawn_blocking(move || {
        let _slot = slot;
        let fail = |code| (code, Vec::<Value>::new());
        let archived = matches!(&evidence, RolloutEvidence::Archive(_));
        let report = match evidence {
            RolloutEvidence::Normalized { policy, evidence } => {
                let policy = STANDARD
                    .decode(policy)
                    .map_err(|_| fail("invalid_evidence"))?;
                let evidence = STANDARD
                    .decode(evidence)
                    .map_err(|_| fail("invalid_evidence"))?;
                if policy.len() > MAX_POLICY_BYTES
                    || evidence.len() > MAX_RELEASE_BYTES
                    || sha256(&policy) != binding.policy_digest
                    || binding.archive_index_digest.is_some()
                {
                    return Err(fail("approval_pins_mismatch"));
                }
                remediation::evaluate_bytes(&policy, &evidence, binding.as_of)
            }
            RolloutEvidence::Archive(case) => {
                let case = case.ok_or_else(|| fail("archive_not_registered"))?;
                if case.policy_digest != binding.policy_digest
                    || binding.archive_index_digest.as_deref() != Some(case.sources_digest.as_str())
                {
                    return Err(fail("approval_pins_mismatch"));
                }
                let policy = read_evidence(&case.policy, MAX_POLICY_BYTES as u64)
                    .map_err(|_| fail("invalid_evidence"))?;
                remediation::evaluate_sources(
                    &policy,
                    &case.policy_digest,
                    &case.sources,
                    &case.sources_digest,
                    binding.as_of,
                )
            }
        }
        .map_err(|_| fail("invalid_evidence"))?;
        let proof = report.proof_for(&binding.target).ok_or_else(|| {
            if report.rollout_verified {
                fail("required_target_missing")
            } else {
                ("targets_not_verified", report.pending_targets())
            }
        })?;
        let binds_candidate = if archived {
            let matches_forge = match &binding.github_merge {
                Some(merge) => {
                    proof.repository.strip_prefix("github:") == Some(binding.repository.as_str())
                        && proof.pull_request == merge.pull_request
                        && proof.source_commit == merge.result_commit
                }
                None => {
                    proof.repository.strip_prefix("bitbucket:") == Some(binding.repository.as_str())
                }
            };
            matches_forge && report.binds_reviewed_head(&expected_head)
        } else {
            proof.repository == binding.repository && proof.source_commit == expected_head
        };
        if !binds_candidate
            || proof.work_item != binding.work_item
            || binding
                .artifact_digest
                .as_deref()
                .is_some_and(|artifact| artifact != proof.artifact_digest)
        {
            return Err(fail("candidate_or_artifact_mismatch"));
        }
        let fresh_through =
            i64::try_from(proof.fresh_through).map_err(|_| fail("invalid_evidence"))?;
        Ok((
            proof.policy_digest.to_owned(),
            proof.evidence_digest.to_owned(),
            proof.artifact_digest.to_owned(),
            fresh_through,
            binding.archive_index_digest,
            binding.github_merge,
        ))
    })
    .await
    .map_err(|_| {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "rollout_verifier_unavailable",
        )
    })?;
    let (
        policy_digest,
        evidence_digest,
        artifact_digest,
        fresh_through,
        archive_index_digest,
        github_merge,
    ) = match checked {
        Ok(proof) => proof,
        Err((code, targets)) => return blocked(code, targets),
    };
    if input.check_only {
        return preflight(json!({
            "evidence_ready": true, "artifact_digest": artifact_digest, "policy_digest": policy_digest,
            "archive_index_digest": archive_index_digest, "github_merge": github_merge,
            "evaluated_at": evaluated_at, "fresh_through": fresh_through,
        }));
    }
    if let Some(id) = input.refresh_id {
        let refresh = sqlx::query_scalar::<_, Option<Value>>(
            "SELECT craft_queue.record_rollout_refresh($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        )
        .bind(actor)
        .bind(input.work_id)
        .bind(input.generation)
        .bind(input.head)
        .bind(input.verification_sequence)
        .bind(id)
        .bind(policy_digest)
        .bind(archive_index_digest)
        .bind(artifact_digest)
        .bind(fresh_through)
        .bind(github_merge.map(sqlx::types::Json))
        .fetch_one(&app.database)
        .await?
        .ok_or(PROOF_REJECTED)?;
        return response("rollout_refresh", refresh);
    }
    let release = sqlx::query_scalar::<_, Option<Value>>(
        "SELECT craft_queue.record_series_verified_rollout($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(actor)
    .bind(input.work_id)
    .bind(input.generation)
    .bind(input.head)
    .bind(input.verification_sequence)
    .bind(policy_digest)
    .bind(evidence_digest)
    .bind(artifact_digest)
    .bind(fresh_through)
    .bind(archive_index_digest)
    .bind(github_merge.map(sqlx::types::Json))
    .fetch_one(&app.database)
    .await?
    .ok_or(PROOF_REJECTED)?;
    response("release", release)
}

pub fn routes() -> Router<App> {
    Router::new()
        .route(
            "/v1/series",
            post(operator).layer(DefaultBodyLimit::max(256 * 1024)),
        )
        .route(
            "/v1/release-observer",
            post(observer).layer(DefaultBodyLimit::max(16 * 1024)),
        )
        .route("/v1/series-rollout", post(verified_rollout))
}
