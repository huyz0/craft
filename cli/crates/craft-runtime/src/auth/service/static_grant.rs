//! Grants for providers that issue a long-lived key or token instead of an OAuth exchange.
//!
//! Provisioning proves the secret against the provider's fixed identity route and stores it in
//! the grant's native credential generation. Opening a session reads that generation only: it
//! never logs in, refreshes or contacts the provider.
use craft_core::contracts::validate_identity;
use craft_core::storage::MAX_SECRET_BYTES;
use reqwest::header::AUTHORIZATION;
use serde::Deserialize;

use super::*;
use crate::secrets::{GenerationReceipt, IncompletePolicy, LockedCredentials};
use crate::transport::{CredentialScheme, product_identity_url};

const STATIC_FORMAT: u8 = 2;

/// Maps a provider to its credential scheme; providers without one use the OAuth path.
fn scheme_for(provider: &str) -> Result<CredentialScheme, CraftError> {
    match provider {
        "trello" => Ok(CredentialScheme::TrelloKeyToken),
        "bitbucket" => Ok(CredentialScheme::Basic),
        _ => Err(CraftError::new(
            ErrorCode::UnsupportedCapability,
            "No static credential scheme exists for this provider.",
        )),
    }
}

/// Secret material in `<left>:<right>` form: Trello `<api key>:<member token>` or Bitbucket
/// `<account email>:<api token>`. The shape is checked when a session header is built.
pub struct StaticCredentials(SecretBytes);
impl fmt::Debug for StaticCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("StaticCredentials([REDACTED])")
    }
}
impl StaticCredentials {
    pub fn new(secret: SecretBytes) -> Self {
        Self(secret)
    }

    /// Parses the provisioning object piped to the host: Trello `{"key","token"}` or Bitbucket
    /// `{"email","api_token"}`. Unknown fields are refused so a mistaken object cannot be stored.
    pub fn from_provisioning(provider: &str, input: SecretBytes) -> Result<Self, CraftError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Trello {
            key: SecretText,
            token: SecretText,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Bitbucket {
            email: SecretText,
            api_token: SecretText,
        }
        let invalid = || CraftError::field("/credentials", "invalid_static_credentials");
        if input.expose().len() > super::PROVISIONING_LIMIT {
            return Err(invalid());
        }
        let joined = |left: &SecretText, right: &SecretText| {
            let mut text = Zeroizing::new(String::new());
            text.push_str(&left.0);
            text.push(':');
            text.push_str(&right.0);
            text
        };
        let text = match provider {
            "trello" => {
                let parsed: Trello =
                    serde_json::from_slice(input.expose()).map_err(|_| invalid())?;
                joined(&parsed.key, &parsed.token)
            }
            "bitbucket" => {
                let parsed: Bitbucket =
                    serde_json::from_slice(input.expose()).map_err(|_| invalid())?;
                joined(&parsed.email, &parsed.api_token)
            }
            _ => return Err(invalid()),
        };
        Ok(Self(
            SecretBytes::new(text.as_bytes().to_vec()).map_err(|_| invalid())?,
        ))
    }
}

#[derive(Debug)]
pub struct StaticGrantReceipt {
    pub actor: String,
    pub generation: GenerationReceipt,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredStatic {
    format: u8,
    grant: GrantRef,
    tenant: TenantRef,
    actor: String,
    secret: SecretText,
}

#[derive(Serialize)]
struct StaticView<'a> {
    format: u8,
    grant: &'a GrantRef,
    tenant: &'a TenantRef,
    actor: &'a str,
    secret: &'a str,
}

/// Fields of the identity probe that bind the secret to the configured tenant.
#[derive(Deserialize)]
struct Probe {
    /// Trello member id.
    id: Option<String>,
    /// Bitbucket workspace slug and UUID.
    slug: Option<String>,
    uuid: Option<String>,
}

fn invalid_stored() -> CraftError {
    CraftError::new(
        ErrorCode::KeyringUnavailable,
        "Static grant data is invalid or unavailable.",
    )
}

fn identity_mismatch() -> AuthFailure {
    let mut error = CraftError::new(
        ErrorCode::PolicyDenied,
        "Credential does not belong to the configured provider tenant.",
    );
    error.details.write_state = RequestState::Failed;
    error.into()
}

impl ServiceOAuthClient {
    /// Verifies the secret reaches exactly the configured tenant, then replaces the locked grant.
    pub fn provision_static_grant(
        &self,
        locked: &LockedCredentials<'_>,
        tenant: &TenantRef,
        credentials: StaticCredentials,
    ) -> Result<StaticGrantReceipt, AuthFailure> {
        self.control.check()?;
        let scheme = scheme_for(&tenant.provider)?;
        let grant = locked.grant();
        let previous = locked.read()?;
        let session =
            HostSession::with_scheme(tenant.clone(), grant.clone(), credentials.0, scheme)?;
        let actor = self.probe_static(tenant, &session)?;
        validate_identity("/actor", &actor).map_err(|_| identity_mismatch())?;
        let secret =
            std::str::from_utf8(session.secret().expose()).map_err(|_| invalid_stored())?;
        let view = StaticView {
            format: STATIC_FORMAT,
            grant: &grant,
            tenant,
            actor: &actor,
            secret,
        };
        let mut bytes = Zeroizing::new(Vec::new());
        serde_json::to_writer(&mut *bytes, &view).map_err(|_| invalid_stored())?;
        let bytes = SecretBytes::new(std::mem::take(&mut *bytes)).map_err(|_| invalid_stored())?;
        self.control.check()?;
        let generation = locked
            .replace(&previous.version, &bytes, IncompletePolicy::KeepPrevious)
            .map_err(|error| {
                let state = error.details.write_state;
                let mut failure = AuthFailure::from(error);
                failure.credential_write_state = Some(state);
                failure
            })?;
        Ok(StaticGrantReceipt { actor, generation })
    }

    /// Reads the committed generation only. An inactive grant requires explicit provisioning.
    pub fn open_static_session(
        &self,
        locked: &LockedCredentials<'_>,
        tenant: &TenantRef,
        expected_actor: &str,
    ) -> Result<HostSession, AuthFailure> {
        self.control.check()?;
        let scheme = scheme_for(&tenant.provider)?;
        product_identity_url(tenant)?;
        let snapshot = locked.read()?;
        let bytes = snapshot.secret.ok_or_else(|| {
            CraftError::new(
                ErrorCode::AuthRequired,
                "Static grant requires explicit provisioning.",
            )
        })?;
        if bytes.expose().len() > MAX_SECRET_BYTES {
            return Err(invalid_stored().into());
        }
        let stored: StoredStatic =
            serde_json::from_slice(bytes.expose()).map_err(|_| invalid_stored())?;
        let grant = locked.grant();
        if stored.format != STATIC_FORMAT || stored.grant != grant {
            return Err(invalid_stored().into());
        }
        validate_identity("/actor", &stored.actor).map_err(|_| invalid_stored())?;
        if stored.actor != expected_actor {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Stored credential actor differs from the configured policy actor.",
            )
            .into());
        }
        if &stored.tenant != tenant {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Provider tenant is outside this provisioned static grant.",
            )
            .into());
        }
        let secret =
            SecretBytes::new(stored.secret.0.as_bytes().to_vec()).map_err(|_| invalid_stored())?;
        let session = HostSession::with_scheme(tenant.clone(), grant, secret, scheme)?;
        #[cfg(feature = "synthetic-acceptance")]
        let session = session.with_synthetic(self.synthetic.clone())?;
        Ok(session)
    }

    pub(super) fn probe_static(
        &self,
        tenant: &TenantRef,
        session: &HostSession,
    ) -> Result<String, AuthFailure> {
        let url = product_identity_url(tenant)?;
        #[cfg(feature = "synthetic-acceptance")]
        let url = if let Some(route) = &self.synthetic {
            route.product_url(url)?
        } else {
            url
        };
        #[cfg(test)]
        let url = if let Some(origin) = &self.api_origin {
            let mut value = url;
            value
                .set_host(origin.host_str())
                .map_err(|_| invalid_client())?;
            value
                .set_port(origin.port())
                .map_err(|_| invalid_client())?;
            value
        } else {
            url
        };
        let request = self
            .client
            .get(url)
            .header(ACCEPT, "application/json")
            .header(ACCEPT_ENCODING, "identity")
            .header(AUTHORIZATION, session.header()?)
            .timeout(self.control.remaining()?)
            .build()
            .map_err(|_| invalid_client())?;
        let probe: Probe = self
            .run(async {
                let response = self
                    .client
                    .execute(request)
                    .await
                    .map_err(network_failure)?;
                let bytes = bounded_body(response).await?;
                serde_json::from_slice(&bytes).map_err(|_| response_failure())
            })
            .map_err(|mut failure| {
                failure.error.details.write_state = RequestState::Failed;
                failure.error.message =
                    "Provider identity read failed or returned an invalid response.".into();
                failure.error.next_action = "Inspect the credential, its provider access and rate advice; no retry was performed.".into();
                failure
            })?;
        match tenant.provider.as_str() {
            "trello" if probe.id.as_deref() == Some(tenant.tenant.as_str()) => {
                Ok(tenant.tenant.clone())
            }
            "bitbucket" if probe.slug.as_deref() == Some(tenant.tenant.as_str()) => probe
                .uuid
                .map(|uuid| uuid.trim_matches(['{', '}']).to_owned())
                .filter(|uuid| !uuid.is_empty())
                .ok_or_else(identity_mismatch),
            _ => Err(identity_mismatch()),
        }
    }
}
