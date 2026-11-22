use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use axum::http::{HeaderMap, StatusCode, header::AUTHORIZATION};
use craft_factory_evidence::{
    queue_identity::{IdentityConfig, Role, identifier},
    release_trace::{read_evidence, valid_digest},
    sha256,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;

use crate::{ApiError, App, Error};

#[derive(Clone)]
pub struct IdentityStore(Arc<IdentityState>);

struct IdentityState {
    path: PathBuf,
    active: RwLock<Snapshot>,
    reload: Mutex<()>,
}

struct Snapshot {
    digest: String,
    identities: Vec<Identity>,
}

struct Identity {
    id: String,
    role: Role,
    digest: [u8; 32],
    repositories: Option<Arc<[String]>>,
}

pub struct AuthenticatedIdentity {
    pub id: String,
    pub repositories: Option<Arc<[String]>>,
}

impl IdentityStore {
    pub fn contains(&self, id: &str, role: Role) -> Result<bool, ApiError> {
        let active = self.0.active.read().map_err(|_| STORE_UNAVAILABLE)?;
        Ok(active
            .identities
            .iter()
            .any(|entry| entry.id == id && entry.role == role))
    }

    pub fn load(path: &Path) -> Result<Self, Error> {
        let path = path.canonicalize()?;
        let active = read_snapshot(&path, None)?;
        Ok(Self(Arc::new(IdentityState {
            path,
            active: RwLock::new(active),
            reload: Mutex::new(()),
        })))
    }

    pub fn settings(&self) -> Result<(usize, bool), Error> {
        let active = self
            .0
            .active
            .read()
            .map_err(|_| "identity store unavailable")?;
        Ok((active.identities.len(), active.has_operator()))
    }

    pub async fn reload(&self, headers: &HeaderMap, digest: String) -> Result<Value, ApiError> {
        if !valid_digest(&digest) {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "invalid_identity_file_digest",
            ));
        }
        let _reload = self
            .0
            .reload
            .try_lock()
            .map_err(|_| ApiError(StatusCode::CONFLICT, "identity_reload_in_progress"))?;
        // A previous reload may have revoked this request before it obtained the reload lock.
        let actor = authorized_identity(self, headers, Role::Operator)?.id;
        let path = self.0.path.clone();
        let candidate = tokio::task::spawn_blocking(move || read_snapshot(&path, Some(&digest)))
            .await
            .map_err(|_| STORE_UNAVAILABLE)?
            .map_err(|_| ApiError(StatusCode::CONFLICT, "invalid_identity_file_replacement"))?;
        let mut active = self.0.active.write().map_err(|_| STORE_UNAVAILABLE)?;
        let changed = active.digest != candidate.digest;
        if changed {
            *active = candidate;
        }
        let digest = active.digest.clone();
        let count = active.identities.len();
        drop(active);
        if changed {
            eprintln!(
                "{}",
                json!({"event":"queue_identities_reloaded", "operator":actor,
                "digest":digest, "configured_identities":count})
            );
        }
        Ok(json!({"changed":changed, "digest":digest, "configured_identities":count}))
    }
}

impl Snapshot {
    fn has_operator(&self) -> bool {
        self.identities
            .iter()
            .any(|identity| identity.role == Role::Operator)
    }
}

fn read_snapshot(path: &Path, expected: Option<&str>) -> Result<Snapshot, Error> {
    if path.canonicalize()? != path {
        return Err("the identity file must stay at its configured path".into());
    }
    let bytes = read_evidence(path, 128 * 1024)?;
    let digest = sha256(&bytes);
    if expected.is_some_and(|expected| expected != digest) {
        return Err("identity file digest mismatch".into());
    }
    let config: Vec<IdentityConfig> = serde_json::from_slice(&bytes)?;
    if config.is_empty() || config.len() > 256 {
        return Err("configure between 1 and 256 service identities".into());
    }
    let mut ids = HashSet::new();
    let mut digests = HashSet::new();
    let mut identities = Vec::new();
    for entry in config {
        if !identifier(&entry.id) || !ids.insert(entry.id.clone()) {
            return Err("identity IDs must be valid and unique".into());
        }
        let repositories = entry.repository_scope()?.map(Arc::from);
        if entry.token_sha256.len() != 64
            || !entry
                .token_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("each identity needs a full SHA-256 token digest".into());
        }
        let mut digest = [0; 32];
        for (index, byte) in digest.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&entry.token_sha256[index * 2..index * 2 + 2], 16)
                .map_err(|_| "each identity needs a hexadecimal token digest")?;
        }
        if !digests.insert(digest) {
            return Err("token digests must be unique".into());
        }
        identities.push(Identity {
            id: entry.id,
            role: entry.role,
            digest,
            repositories,
        });
    }
    let snapshot = Snapshot { digest, identities };
    if expected.is_some() && !snapshot.has_operator() {
        return Err("a replacement identity file must retain an operator".into());
    }
    Ok(snapshot)
}

const STORE_UNAVAILABLE: ApiError = ApiError(
    StatusCode::SERVICE_UNAVAILABLE,
    "identity_store_unavailable",
);

pub fn authenticate(app: &App, headers: &HeaderMap, role: Role) -> Result<String, ApiError> {
    Ok(authorized_identity(&app.identities, headers, role)?.id)
}

pub fn authenticate_verifier(
    app: &App,
    headers: &HeaderMap,
) -> Result<AuthenticatedIdentity, ApiError> {
    authorized_identity(&app.identities, headers, Role::Verifier)
}

pub fn authenticate_context_reader(
    app: &App,
    headers: &HeaderMap,
) -> Result<AuthenticatedIdentity, ApiError> {
    authorized_identity(&app.identities, headers, Role::ContextReader)
}

fn authorized_identity(
    store: &IdentityStore,
    headers: &HeaderMap,
    role: Role,
) -> Result<AuthenticatedIdentity, ApiError> {
    let token = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| (32..=256).contains(&value.len()))
        .ok_or(ApiError(
            StatusCode::UNAUTHORIZED,
            "authentication_required",
        ))?;
    let digest = Sha256::digest(token.as_bytes());
    let active = store.0.active.read().map_err(|_| STORE_UNAVAILABLE)?;
    let mut identity = None;
    for entry in &active.identities {
        if bool::from(entry.digest.as_slice().ct_eq(digest.as_slice())) {
            identity = Some(entry);
        }
    }
    let identity = identity.ok_or(ApiError(
        StatusCode::UNAUTHORIZED,
        "authentication_required",
    ))?;
    if identity.role != role {
        return Err(ApiError(StatusCode::FORBIDDEN, "role_not_allowed"));
    }
    Ok(AuthenticatedIdentity {
        id: identity.id.clone(),
        repositories: identity.repositories.clone(),
    })
}
