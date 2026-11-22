use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use craft_factory_evidence::{
    diagnosis::{ArchiveCase, Case, Report, TelemetrySource, otlp},
    release_trace::{read_evidence, valid_digest},
    sha256,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Mutex, Semaphore};

use crate::{ApiError, App, Error, Role, authenticate, identifier};

const MAX_RESPONSE_BYTES: usize = 512 * 1024;
const NOT_ALLOWED: ApiError = ApiError(StatusCode::FORBIDDEN, "diagnostic_access_denied");
const STORE_UNAVAILABLE: ApiError = ApiError(
    StatusCode::SERVICE_UNAVAILABLE,
    "diagnostic_service_unavailable",
);

#[derive(Clone)]
pub struct Catalog(Arc<CatalogState>);

struct CatalogState {
    path: Option<PathBuf>,
    active: RwLock<Snapshot>,
    reload: Mutex<()>,
    concurrent_reports: Arc<Semaphore>,
}

#[derive(Default)]
struct Snapshot {
    digest: Option<String>,
    cases: HashMap<String, LoadedCase>,
}

impl Snapshot {
    fn pins(&self) -> Value {
        Value::Object(
            self.cases
                .iter()
                .map(|(id, case)| (id.clone(), json!(case.policy_digest())))
                .collect(),
        )
    }
}

#[derive(Clone)]
enum LoadedCase {
    Bundle(Arc<Case>),
    Archive(Arc<ArchiveCase>),
}

impl LoadedCase {
    fn policy_digest(&self) -> &str {
        match self {
            Self::Bundle(case) => case.policy_digest(),
            Self::Archive(case) => case.policy_digest(),
        }
    }

    fn report(&self, from: u64, to: u64) -> craft_factory_evidence::Result<Report> {
        match self {
            Self::Bundle(case) => case.report(from, to),
            Self::Archive(case) => case.report(from, to),
        }
    }

    fn allows_window(&self, from: u64, to: u64) -> bool {
        match self {
            Self::Bundle(case) => case.allows_window(from, to),
            Self::Archive(case) => case.allows_window(from, to),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    id: String,
    policy: PathBuf,
    release: Option<PathBuf>,
    release_manifest: Option<PathBuf>,
    policy_digest: Option<String>,
    telemetry: Option<PathBuf>,
    otlp: Option<otlp::Archive>,
}

impl Catalog {
    pub(crate) fn approves_window(&self, case_id: &str, digest: &str, from: u64, to: u64) -> bool {
        self.case(case_id)
            .ok()
            .flatten()
            .is_some_and(|case| case.policy_digest() == digest && case.allows_window(from, to))
    }

    fn case(&self, case_id: &str) -> Result<Option<LoadedCase>, ApiError> {
        let active = self.0.active.read().map_err(|_| STORE_UNAVAILABLE)?;
        Ok(active.cases.get(case_id).cloned())
    }

    pub fn load(manifest: Option<&Path>) -> Result<Self, Error> {
        let path = manifest.map(Path::canonicalize).transpose()?;
        let active = match &path {
            Some(path) => read_snapshot(path, None)?,
            None => Snapshot::default(),
        };
        Ok(Self(Arc::new(CatalogState {
            path,
            active: RwLock::new(active),
            reload: Mutex::new(()),
            concurrent_reports: Arc::new(Semaphore::new(4)),
        })))
    }

    pub(crate) async fn reload(
        &self,
        app: &App,
        headers: &HeaderMap,
        digest: String,
        evidence: String,
    ) -> Result<Value, ApiError> {
        if !valid_digest(&digest) || !crate::operations::review_text(&evidence, 500) {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "invalid_diagnostic_catalog_approval",
            ));
        }
        let path = self.0.path.clone().ok_or(ApiError(
            StatusCode::CONFLICT,
            "diagnostic_catalog_not_configured",
        ))?;
        let _reload = self.0.reload.try_lock().map_err(|_| {
            ApiError(
                StatusCode::CONFLICT,
                "diagnostic_catalog_reload_in_progress",
            )
        })?;
        let approved_digest = digest.clone();
        let candidate =
            tokio::task::spawn_blocking(move || read_snapshot(&path, Some(&approved_digest)))
                .await
                .map_err(|_| STORE_UNAVAILABLE)?
                .map_err(|_| {
                    ApiError(
                        StatusCode::CONFLICT,
                        "invalid_diagnostic_catalog_replacement",
                    )
                })?;
        let actor = authenticate(app, headers, Role::Operator)?;
        let (previous_pins, changed) = {
            let current = self.0.active.read().map_err(|_| STORE_UNAVAILABLE)?;
            (current.pins(), current.digest != candidate.digest)
        };
        let receipt = sqlx::query_scalar::<_, Option<Value>>(
            "SELECT craft_queue.retire_diagnostic_cases($1,$2,$3,$4,$5)",
        )
        .bind(&actor)
        .bind(&digest)
        .bind(previous_pins)
        .bind(candidate.pins())
        .bind(evidence)
        .fetch_one(&app.database)
        .await?
        .ok_or(ApiError(
            StatusCode::CONFLICT,
            "diagnostic_catalog_activation_blocked",
        ))?;
        let count = candidate.cases.len();
        *self.0.active.write().map_err(|_| STORE_UNAVAILABLE)? = candidate;
        if changed {
            eprintln!(
                "{}",
                json!({"event":"diagnostic_catalog_reloaded", "operator":actor,
                    "digest":digest, "configured_cases":count, "retired_cases":receipt["retired_cases"]})
            );
        }
        Ok(
            json!({"changed":changed, "digest":digest, "configured_cases":count, "retirement":receipt}),
        )
    }
}

fn read_snapshot(path: &Path, expected_digest: Option<&str>) -> Result<Snapshot, Error> {
    let bytes = read_evidence(path, 64 * 1024)?;
    let digest = sha256(&bytes);
    if expected_digest.is_some_and(|expected| expected != digest) {
        return Err("diagnostic catalog differs from the approved digest".into());
    }
    let mut cases = HashMap::new();
    let entries: Vec<Entry> = serde_json::from_slice(&bytes)?;
    if entries.len() > 16 {
        return Err("configure no more than 16 diagnostic cases per service".into());
    }
    for entry in entries {
        if !identifier(&entry.id)
            || cases.contains_key(&entry.id)
            || !entry.policy.is_absolute()
            || (expected_digest.is_some() && entry.policy_digest.is_none())
            || entry
                .policy_digest
                .as_deref()
                .is_some_and(|digest| !valid_digest(digest))
        {
            return Err(
                        "diagnostic cases require unique IDs and absolute evidence paths, with policy pins for reloaded cases".into(),
                    );
        }
        let telemetry = match (&entry.telemetry, &entry.otlp) {
                    (Some(path), None) if path.is_absolute() => TelemetrySource::Normalized(path),
                    (None, Some(archive))
                        if [&archive.policy, &archive.logs, &archive.metrics].iter().all(|path| path.is_absolute()) =>
                    {
                        TelemetrySource::Otlp(archive)
                    }
                    _ => return Err("diagnostic cases require either an absolute telemetry path or absolute OTLP policy, log and metric paths".into()),
                };
        let case = match (entry.release, entry.release_manifest, entry.policy_digest, telemetry) {
                    (Some(release), None, policy_digest, TelemetrySource::Normalized(telemetry)) if release.is_absolute() => {
                        Case::load_with_policy_pin(&entry.policy, &release, telemetry, policy_digest.as_deref())
                            .map(|case| LoadedCase::Bundle(Arc::new(case)))
                    }
                    (None, Some(release_manifest), Some(policy_digest), telemetry) if release_manifest.is_absolute() => {
                        ArchiveCase::load(&entry.policy, &release_manifest, telemetry, &policy_digest)
                            .map(|case| LoadedCase::Archive(Arc::new(case)))
                    }
                    _ => return Err("diagnostic cases require an absolute release path or an absolute release_manifest and policy_digest".into()),
                }
                .map_err(|error| format!("diagnostic case {} did not load: {error}", entry.id))?;
        cases.insert(entry.id, case);
    }
    Ok(Snapshot {
        digest: Some(digest),
        cases,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    case_id: String,
    from: i64,
    to: i64,
}

#[derive(Serialize)]
struct Reply {
    schema_version: u32,
    report: Report,
}

async fn report(
    State(app): State<App>,
    headers: HeaderMap,
    Json(request): Json<Request>,
) -> Result<Response, ApiError> {
    let actor = authenticate(&app, &headers, Role::Diagnostician)?;
    if !identifier(&request.case_id) || request.from <= 0 || request.to <= request.from {
        return Err(NOT_ALLOWED);
    }
    let case = app.diagnostics.case(&request.case_id)?.ok_or(NOT_ALLOWED)?;
    let digest = case.policy_digest();
    let grant_id = sqlx::query_scalar::<_, Option<String>>(
        "SELECT craft_queue.diagnostic_grant_for($1, $2, $3, $4, $5)",
    )
    .bind(&actor)
    .bind(&request.case_id)
    .bind(digest)
    .bind(request.from)
    .bind(request.to)
    .fetch_one(&app.database)
    .await?
    .ok_or(NOT_ALLOWED)?;
    let slot = app
        .diagnostics
        .0
        .concurrent_reports
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(StatusCode::TOO_MANY_REQUESTS, "diagnostic_service_busy"))?;
    let report_case = case.clone();
    let output = tokio::task::spawn_blocking(move || {
        let _slot = slot;
        let report = report_case
            .report(request.from as u64, request.to as u64)
            .map_err(|_| NOT_ALLOWED)?;
        let output = serde_json::to_vec(&Reply {
            schema_version: 1,
            report,
        })
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "response_error"))?;
        if output.len() > MAX_RESPONSE_BYTES {
            return Err(ApiError(
                StatusCode::PAYLOAD_TOO_LARGE,
                "diagnostic_response_too_large",
            ));
        }
        Ok(output)
    })
    .await
    .map_err(|_| {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "diagnostic_service_unavailable",
        )
    })??;
    let consumed = sqlx::query_scalar::<_, bool>(
        "SELECT craft_queue.consume_diagnostic_grant($1, $2, $3, $4, $5, $6)",
    )
    .bind(grant_id)
    .bind(actor)
    .bind(&request.case_id)
    .bind(digest)
    .bind(request.from)
    .bind(request.to)
    .fetch_one(&app.database)
    .await?;
    if !consumed {
        return Err(NOT_ALLOWED);
    }
    Ok((
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::CONTENT_TYPE, "application/json"),
        ],
        output,
    )
        .into_response())
}

pub fn routes() -> Router<App> {
    Router::new().route(
        "/v1/diagnostics",
        post(report).layer(DefaultBodyLimit::max(4 * 1024)),
    )
}
