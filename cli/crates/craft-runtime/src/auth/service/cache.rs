use craft_core::contracts::validate_identity;
use craft_core::storage::MAX_SECRET_BYTES;
use serde::{Deserialize, Serialize};

use super::*;
use crate::secrets::{GenerationReceipt, IncompletePolicy, LockedCredentials};
use crate::transport::product_identity_url;

#[derive(Debug)]
pub struct ServiceGrantReceipt {
    pub token: TokenMetadata,
    pub products: Vec<ProductIdentity>,
    pub generation: GenerationReceipt,
}

#[derive(Debug)]
pub struct ServiceSession {
    pub session: HostSession,
    pub token: TokenMetadata,
    pub identity: ProductIdentity,
    pub cache_update: Option<GenerationReceipt>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredToken {
    access_token: SecretText,
    issued_at_unix_ms: u64,
    expires_at_unix_ms: u64,
    reacquire_at_unix_ms: u64,
    // The field is mandatory even when the issuer did not report scopes.
    #[serde(deserialize_with = "required_scopes")]
    scopes: Option<Vec<String>>,
}

fn required_scopes<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<String>>, D::Error> {
    Option::deserialize(deserializer)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredGrant {
    format: u8,
    grant: GrantRef,
    targets: Vec<TenantRef>,
    actor: String,
    credentials: Provisioning,
    token: StoredToken,
}

#[derive(Serialize)]
struct GrantView<'a> {
    format: u8,
    grant: &'a GrantRef,
    targets: &'a [TenantRef],
    actor: &'a str,
    credentials: CredentialsView<'a>,
    token: TokenView<'a>,
}

#[derive(Serialize)]
struct CredentialsView<'a> {
    client_id: &'a str,
    client_secret: &'a str,
}

#[derive(Serialize)]
struct TokenView<'a> {
    access_token: &'a str,
    #[serde(flatten)]
    metadata: &'a TokenMetadata,
}

fn invalid_cache() -> CraftError {
    CraftError::new(
        ErrorCode::KeyringUnavailable,
        "Service grant data is invalid or unavailable.",
    )
}

fn validate_targets(targets: &[TenantRef]) -> Result<(), CraftError> {
    if targets.is_empty() || targets.len() > 32 {
        return Err(CraftError::field(
            "/targets",
            "expected_bounded_product_targets",
        ));
    }
    for (index, target) in targets.iter().enumerate() {
        product_identity_url(target)?;
        if targets[..index].contains(target) {
            return Err(CraftError::field("/targets", "duplicate_product_target"));
        }
    }
    Ok(())
}

impl StoredGrant {
    fn load(bytes: &SecretBytes, grant: &GrantRef) -> Result<Self, CraftError> {
        if bytes.expose().len() > MAX_SECRET_BYTES {
            return Err(invalid_cache());
        }
        let stored: Self = serde_json::from_slice(bytes.expose()).map_err(|_| invalid_cache())?;
        if stored.format != 1
            || &stored.grant != grant
            || !valid_text(&stored.credentials.client_id.0, 1_024)
            || !valid_text(&stored.credentials.client_secret.0, 65_536)
        {
            return Err(invalid_cache());
        }
        validate_targets(&stored.targets).map_err(|_| invalid_cache())?;
        validate_identity("/actor", &stored.actor).map_err(|_| invalid_cache())?;
        if matches!(stored.actor.as_str(), "unknown" | "anonymous") {
            return Err(invalid_cache());
        }
        Ok(stored)
    }
}

impl StoredToken {
    fn restore(
        &self,
        client: &ServiceOAuthClient,
        actor: &str,
    ) -> Result<Option<ServiceToken>, CraftError> {
        let clock = client.clock.clone();
        let ttl = self
            .expires_at_unix_ms
            .checked_sub(self.issued_at_unix_ms)
            .filter(|ttl| (1_000..=86_400_000).contains(ttl) && ttl % 1_000 == 0)
            .ok_or_else(invalid_cache)?;
        if self.reacquire_at_unix_ms != self.expires_at_unix_ms - 60_000.min(ttl / 10)
            || !bearer_is_valid(self.access_token.0.as_bytes())
        {
            return Err(invalid_cache());
        }
        if let Some(scopes) = &self.scopes {
            let text = SecretText(Zeroizing::new(scopes.join(" ")));
            let parsed = parse_scopes(&text).map_err(|_| invalid_cache())?;
            if &parsed != scopes {
                return Err(invalid_cache());
            }
        }
        let wall = clock.unix_millis();
        let elapsed = clock
            .monotonic_millis()
            .checked_sub(client.cache_origin_monotonic_ms)
            .ok_or_else(invalid_cache)?;
        // Reopening the bundle cannot restart its remaining lifetime in this process.
        let now = wall.max(
            client
                .cache_origin_unix_ms
                .checked_add(elapsed)
                .ok_or_else(invalid_cache)?,
        );
        if wall < self.issued_at_unix_ms || now >= self.reacquire_at_unix_ms {
            return Ok(None);
        }
        let monotonic = clock
            .monotonic_millis()
            .checked_add(self.reacquire_at_unix_ms - now)
            .ok_or_else(invalid_cache)?;
        Ok(Some(ServiceToken {
            #[cfg(feature = "synthetic-acceptance")]
            synthetic: client.synthetic.clone(),
            access_token: SecretBytes::new(self.access_token.0.as_bytes().to_vec())
                .map_err(|_| invalid_cache())?,
            metadata: TokenMetadata {
                issued_at_unix_ms: self.issued_at_unix_ms,
                expires_at_unix_ms: self.expires_at_unix_ms,
                reacquire_at_unix_ms: self.reacquire_at_unix_ms,
                scopes: self.scopes.clone(),
            },
            reacquire_at_monotonic_ms: monotonic,
            clock,
            verified: Vec::new(),
            actor: Some(actor.into()),
        }))
    }
}

fn encode(
    grant: &GrantRef,
    targets: &[TenantRef],
    credentials: &ServiceCredentials,
    token: &ServiceToken,
) -> Result<SecretBytes, CraftError> {
    let actor = token.actor.as_deref().ok_or_else(invalid_cache)?;
    let access_token =
        std::str::from_utf8(token.access_token.expose()).map_err(|_| invalid_cache())?;
    let view = GrantView {
        format: 1,
        grant,
        targets,
        actor,
        credentials: CredentialsView {
            client_id: &credentials.client_id.0,
            client_secret: &credentials.client_secret.0,
        },
        token: TokenView {
            access_token,
            metadata: &token.metadata,
        },
    };
    let mut bytes = Zeroizing::new(Vec::new());
    serde_json::to_writer(&mut *bytes, &view).map_err(|_| invalid_cache())?;
    SecretBytes::new(std::mem::take(&mut *bytes)).map_err(|_| invalid_cache())
}

impl ServiceOAuthClient {
    /// Explicit provisioning replaces a locked grant only after all configured products verify.
    pub fn provision_grant(
        &self,
        locked: &LockedCredentials<'_>,
        credentials: ServiceCredentials,
        targets: &[TenantRef],
        expected_actor: Option<&str>,
    ) -> Result<ServiceGrantReceipt, AuthFailure> {
        self.provision_grant_with_preflight(locked, credentials, targets, expected_actor, |_, _| {
            Ok(())
        })
    }

    /// Runs the host's complete output/policy preflight after proof and before credential writes.
    pub fn provision_grant_with_preflight(
        &self,
        locked: &LockedCredentials<'_>,
        credentials: ServiceCredentials,
        targets: &[TenantRef],
        expected_actor: Option<&str>,
        preflight: impl FnOnce(&TokenMetadata, &[ProductIdentity]) -> Result<(), CraftError>,
    ) -> Result<ServiceGrantReceipt, AuthFailure> {
        self.control.check()?;
        validate_targets(targets)?;
        if let Some(actor) = expected_actor {
            validate_identity("/actor", actor)?;
        }
        let previous = locked.read()?;
        let mut token = self.acquire(&credentials)?;
        for target in targets {
            self.verify_product(&mut token, target, expected_actor)?;
        }
        preflight(&token.metadata, &token.verified)?;
        self.control.check()?;
        if token.needs_reacquisition() {
            return Err(CraftError::new(
                ErrorCode::AuthRequired,
                "Service token expired during provisioning preflight.",
            )
            .into());
        }
        let bytes = encode(&locked.grant(), targets, &credentials, &token)?;
        let generation = locked
            .replace(&previous.version, &bytes, IncompletePolicy::KeepPrevious)
            .map_err(activation_failure)?;
        Ok(ServiceGrantReceipt {
            token: token.metadata.clone(),
            products: token.verified.clone(),
            generation,
        })
    }

    /// Uses a committed keyring bundle; an inactive grant never initiates login or acquisition.
    fn open_verified_token(
        &self,
        locked: &LockedCredentials<'_>,
        tenant: &TenantRef,
    ) -> Result<(ServiceToken, Option<GenerationReceipt>), AuthFailure> {
        self.control.check()?;
        product_identity_url(tenant)?;
        let snapshot = locked.read()?;
        let bytes = snapshot.secret.ok_or_else(|| {
            CraftError::new(
                ErrorCode::AuthRequired,
                "Service grant requires explicit provisioning.",
            )
        })?;
        let grant = locked.grant();
        let stored = StoredGrant::load(&bytes, &grant)?;
        #[cfg(feature = "synthetic-acceptance")]
        if self.synthetic.is_some() {
            use crate::synthetic_acceptance::{ACCESS_TOKEN, CLIENT_ID, CLIENT_SECRET, refusal};
            if stored.credentials.client_id.0.as_str() != CLIENT_ID
                || stored.credentials.client_secret.0.as_str() != CLIENT_SECRET
                || stored.token.access_token.0.as_str() != ACCESS_TOKEN
            {
                return Err(refusal().into());
            }
        }
        if !stored.targets.contains(tenant) {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Product target is outside this provisioned service grant.",
            )
            .into());
        }
        let restored = stored.token.restore(self, &stored.actor)?;
        let credentials = ServiceCredentials {
            client_id: stored.credentials.client_id,
            client_secret: stored.credentials.client_secret,
        };
        let (token, cache_update) = if let Some(mut token) = restored {
            self.verify_product(&mut token, tenant, Some(&stored.actor))?;
            (token, None)
        } else {
            let mut token = self.acquire(&credentials)?;
            for target in &stored.targets {
                self.verify_product(&mut token, target, Some(&stored.actor))?;
            }
            let bytes = encode(&grant, &stored.targets, &credentials, &token)?;
            let receipt = locked
                .replace(&snapshot.version, &bytes, IncompletePolicy::KeepPrevious)
                .map_err(activation_failure)?;
            (token, Some(receipt))
        };
        Ok((token, cache_update))
    }

    pub fn open_service_session(
        &self,
        locked: &LockedCredentials<'_>,
        tenant: &TenantRef,
    ) -> Result<ServiceSession, AuthFailure> {
        let grant = locked.grant();
        let (token, cache_update) = self.open_verified_token(locked, tenant)?;
        let metadata = token.metadata.clone();
        let result = (|| {
            let identity = token
                .verified
                .iter()
                .find(|proof| proof.tenant() == tenant)
                .cloned()
                .ok_or_else(invalid_cache)?;
            let session = token.into_host_session(tenant.clone(), grant)?;
            Ok::<_, CraftError>((session, identity))
        })();
        match result {
            Ok((session, identity)) => Ok(ServiceSession {
                session,
                token: metadata,
                identity,
                cache_update,
            }),
            Err(error) => {
                let mut failure = AuthFailure::from(error);
                failure.credential_update = cache_update.map(Box::new);
                if failure.credential_update.is_some() {
                    failure.credential_write_state = Some(RequestState::Confirmed);
                }
                Err(failure)
            }
        }
    }

    /// Retains sealed token lifetime and scope evidence without exporting bearer material.
    pub fn open_service_access(
        &self,
        locked: &LockedCredentials<'_>,
        tenant: &TenantRef,
    ) -> Result<VerifiedServiceAccess, AuthFailure> {
        let (token, cache_update) = self.open_verified_token(locked, tenant)?;
        match token.into_verified_access(tenant.clone(), locked.grant()) {
            Ok(mut access) => {
                access.cache_update = cache_update;
                Ok(access)
            }
            Err(error) => {
                let mut failure = AuthFailure::from(error);
                failure.credential_update = cache_update.map(Box::new);
                if failure.credential_update.is_some() {
                    failure.credential_write_state = Some(RequestState::Confirmed);
                }
                Err(failure)
            }
        }
    }
}

fn activation_failure(error: CraftError) -> AuthFailure {
    let state = error.details.write_state;
    let mut failure = AuthFailure::from(error);
    failure.credential_write_state = Some(state);
    failure
}
