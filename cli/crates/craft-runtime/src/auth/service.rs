use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use craft_core::contracts::{GrantRef, RequestState, TenantRef};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::RetryAfter;
use craft_core::storage::{Clock, SecretBytes};
use reqwest::header::{ACCEPT, ACCEPT_ENCODING, CONTENT_TYPE, RETRY_AFTER};
use serde::Serialize;
use tokio::runtime::Runtime;
use zeroize::Zeroizing;

use super::wire::{Provisioning, SecretText, TokenResponse};
use crate::transport::{HostSession, TlsPolicy, bearer_is_valid, client_builder, retry_after};

mod identity;
pub use identity::ProductIdentity;
mod cache;
pub use cache::{ServiceGrantReceipt, ServiceSession};
mod access;
pub use access::VerifiedServiceAccess;
mod static_grant;
pub use static_grant::{StaticCredentials, StaticGrantReceipt};

#[cfg(test)]
mod tests;

const TOKEN_ENDPOINT: &str = "https://auth.atlassian.com/oauth/token";
const RESPONSE_LIMIT: usize = 65_536;
pub(super) const PROVISIONING_LIMIT: usize = 262_144;

pub struct ServiceCredentials {
    client_id: SecretText,
    client_secret: SecretText,
}
impl fmt::Debug for ServiceCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ServiceCredentials([REDACTED])")
    }
}
impl ServiceCredentials {
    /// Consumes secure pipe/prompt bytes. This interface accepts no argv, file or environment fallback.
    pub fn from_provisioning(input: SecretBytes) -> Result<Self, CraftError> {
        if input.expose().len() > PROVISIONING_LIMIT {
            return Err(invalid_credentials());
        }
        let parsed: Provisioning =
            serde_json::from_slice(input.expose()).map_err(|_| invalid_credentials())?;
        Self::from_secret_parts(parsed.client_id.0, parsed.client_secret.0)
    }
    pub(super) fn from_secret_parts(
        client_id: Zeroizing<String>,
        client_secret: Zeroizing<String>,
    ) -> Result<Self, CraftError> {
        if !valid_text(&client_id, 1_024) || !valid_text(&client_secret, 65_536) {
            return Err(invalid_credentials());
        }
        Ok(Self {
            client_id: SecretText(client_id),
            client_secret: SecretText(client_secret),
        })
    }
    fn form(&self) -> Zeroizing<String> {
        let mut encoded = url::form_urlencoded::Serializer::new(String::new());
        encoded.append_pair("grant_type", "client_credentials");
        encoded.append_pair("client_id", &self.client_id.0);
        encoded.append_pair("client_secret", &self.client_secret.0);
        Zeroizing::new(encoded.finish())
    }
}
fn valid_text(text: &str, maximum: usize) -> bool {
    !text.is_empty() && text.len() <= maximum && !text.chars().any(char::is_control)
}
fn invalid_credentials() -> CraftError {
    CraftError::field("/credentials", "invalid_service_credentials")
}
fn invalid_response() -> CraftError {
    CraftError::new(
        ErrorCode::RemoteUnavailable,
        "Service token response is invalid or exceeds its bound.",
    )
}

#[derive(Debug, Clone, Serialize)]
pub struct TokenMetadata {
    pub issued_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    pub reacquire_at_unix_ms: u64,
    pub scopes: Option<Vec<String>>,
}

pub struct ServiceToken {
    #[cfg(feature = "synthetic-acceptance")]
    synthetic: Option<crate::synthetic_acceptance::Loopback>,
    access_token: SecretBytes,
    metadata: TokenMetadata,
    reacquire_at_monotonic_ms: u64,
    clock: Arc<dyn Clock>,
    verified: Vec<ProductIdentity>,
    actor: Option<String>,
}
impl fmt::Debug for ServiceToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ServiceToken([REDACTED])")
    }
}
impl ServiceToken {
    pub fn metadata(&self) -> &TokenMetadata {
        &self.metadata
    }
    pub fn verified_products(&self) -> &[ProductIdentity] {
        &self.verified
    }
    /// Uses both clock domains; a backward wall clock or elapsed monotonic budget prevents reuse.
    pub fn needs_reacquisition(&self) -> bool {
        let now = self.clock.unix_millis();
        now < self.metadata.issued_at_unix_ms
            || now >= self.metadata.reacquire_at_unix_ms
            || self.clock.monotonic_millis() >= self.reacquire_at_monotonic_ms
    }
    /// Consumes a fresh token with proof for this exact product/site without exporting bytes.
    /// The host must separately enforce eligibility, operation scopes and resource permissions.
    pub fn into_host_session(
        self,
        tenant: TenantRef,
        grant: GrantRef,
    ) -> Result<HostSession, CraftError> {
        if self.needs_reacquisition()
            || !self.verified.iter().any(|proof| proof.tenant() == &tenant)
        {
            return Err(CraftError::new(
                ErrorCode::AuthRequired,
                "Service session requires a fresh token and verified product identity.",
            ));
        }
        let session = HostSession::new(tenant, grant, self.access_token)?;
        #[cfg(feature = "synthetic-acceptance")]
        let session = session.with_synthetic(self.synthetic)?;
        Ok(session)
    }
}

#[derive(Debug)]
pub struct AuthFailure {
    pub error: CraftError,
    pub retry_after: Option<RetryAfter>,
    pub credential_update: Option<Box<crate::secrets::GenerationReceipt>>,
    pub credential_write_state: Option<RequestState>,
}
impl fmt::Display for AuthFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(formatter)
    }
}
impl std::error::Error for AuthFailure {}
impl From<CraftError> for AuthFailure {
    fn from(error: CraftError) -> Self {
        Self {
            error,
            retry_after: None,
            credential_update: None,
            credential_write_state: None,
        }
    }
}

pub struct ServiceOAuthClient {
    #[cfg(feature = "synthetic-acceptance")]
    synthetic: Option<crate::synthetic_acceptance::Loopback>,
    client: reqwest::Client,
    runtime: Runtime,
    endpoint: reqwest::Url,
    control: InvocationControl,
    clock: Arc<dyn Clock>,
    cache_origin_unix_ms: u64,
    cache_origin_monotonic_ms: u64,
    #[cfg(test)]
    api_origin: Option<reqwest::Url>,
}
impl ServiceOAuthClient {
    /// Selects only the compiled Atlassian service endpoint and the shared verified TLS policy.
    pub fn new(
        tls: TlsPolicy,
        control: InvocationControl,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, CraftError> {
        control.check()?;
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(invalid_client());
        }
        Ok(Self {
            #[cfg(feature = "synthetic-acceptance")]
            synthetic: None,
            client: client_builder(tls)?.build().map_err(|_| invalid_client())?,
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| invalid_client())?,
            endpoint: reqwest::Url::parse(TOKEN_ENDPOINT).map_err(|_| invalid_client())?,
            control,
            cache_origin_unix_ms: clock.unix_millis(),
            cache_origin_monotonic_ms: clock.monotonic_millis(),
            clock,
            #[cfg(test)]
            api_origin: None,
        })
    }
    /// Performs one service exchange. It never refreshes a delegated grant or retries a request.
    pub fn acquire(&self, credentials: &ServiceCredentials) -> Result<ServiceToken, AuthFailure> {
        self.control.check()?;
        #[cfg(feature = "synthetic-acceptance")]
        if self.synthetic.is_some() {
            use crate::synthetic_acceptance::{CLIENT_ID, CLIENT_SECRET, refusal};
            if credentials.client_id.0.as_str() != CLIENT_ID
                || credentials.client_secret.0.as_str() != CLIENT_SECRET
            {
                return Err(refusal().into());
            }
        }
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(invalid_client().into());
        }
        let body = credentials.form();
        let request = self
            .client
            .post(self.endpoint.clone())
            .header(ACCEPT, "application/json")
            .header(ACCEPT_ENCODING, "identity")
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .timeout(self.control.remaining()?)
            .body(body.as_bytes().to_vec())
            .build()
            .map_err(|_| invalid_client())?;
        self.control.check()?;
        let wall = self.clock.unix_millis();
        let monotonic = self.clock.monotonic_millis();
        self.run(self.exchange(request, wall, monotonic, credentials))
    }

    fn run<T>(
        &self,
        exchange: impl std::future::Future<Output = Result<T, AuthFailure>>,
    ) -> Result<T, AuthFailure> {
        self.control.check()?;
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(invalid_client().into());
        }
        self.runtime.block_on(async {
            tokio::pin!(exchange);
            loop {
                tokio::select! {
                    biased;
                    result = &mut exchange => return result,
                    _ = tokio::time::sleep(Duration::from_millis(10)) => {
                        if let Err(mut error) = self.control.check() {
                            error.details.write_state = RequestState::SentUnknown;
                            return Err(error.into());
                        }
                    }
                }
            }
        })
    }

    async fn exchange(
        &self,
        request: reqwest::Request,
        wall: u64,
        monotonic: u64,
        credentials: &ServiceCredentials,
    ) -> Result<ServiceToken, AuthFailure> {
        let response = self
            .client
            .execute(request)
            .await
            .map_err(network_failure)?;
        let bytes = bounded_body(response).await?;
        self.parse_token(&bytes, wall, monotonic, credentials)
    }

    fn parse_token(
        &self,
        bytes: &[u8],
        wall: u64,
        monotonic: u64,
        credentials: &ServiceCredentials,
    ) -> Result<ServiceToken, AuthFailure> {
        let response: TokenResponse =
            serde_json::from_slice(bytes).map_err(|_| response_failure())?;
        if !response.token_type.0.eq_ignore_ascii_case("Bearer")
            || !(1..=86_400).contains(&response.expires_in)
        {
            return Err(response_failure());
        }
        let ttl = response
            .expires_in
            .checked_mul(1_000)
            .ok_or_else(response_failure)?;
        let expires = wall.checked_add(ttl).ok_or_else(response_failure)?;
        let margin = 60_000.min(ttl / 10);
        let reacquire = expires - margin;
        let reacquire_monotonic = monotonic
            .checked_add(ttl - margin)
            .ok_or_else(response_failure)?;
        if response.scope.as_ref().is_some_and(|scope| {
            [
                &*credentials.client_id.0,
                &*credentials.client_secret.0,
                &*response.access_token.0,
            ]
            .iter()
            .any(|secret| !secret.is_empty() && scope.0.contains(secret.as_str()))
        }) {
            return Err(response_failure());
        }
        let scopes = response
            .scope
            .as_ref()
            .map(parse_scopes)
            .transpose()
            .map_err(|_| response_failure())?;
        let access = response.access_token.0.as_bytes();
        #[cfg(feature = "synthetic-acceptance")]
        if self.synthetic.is_some()
            && access != crate::synthetic_acceptance::ACCESS_TOKEN.as_bytes()
        {
            return Err(crate::synthetic_acceptance::refusal().into());
        }
        if !bearer_is_valid(access) {
            return Err(response_failure());
        }
        let access_token = SecretBytes::new(access.to_vec()).map_err(|_| response_failure())?;
        let token = ServiceToken {
            #[cfg(feature = "synthetic-acceptance")]
            synthetic: self.synthetic.clone(),
            access_token,
            metadata: TokenMetadata {
                issued_at_unix_ms: wall,
                expires_at_unix_ms: expires,
                reacquire_at_unix_ms: reacquire,
                scopes,
            },
            reacquire_at_monotonic_ms: reacquire_monotonic,
            clock: self.clock.clone(),
            verified: Vec::new(),
            actor: None,
        };
        if token.needs_reacquisition() {
            return Err(response_failure());
        }
        Ok(token)
    }
}

#[cfg(feature = "synthetic-acceptance")]
impl ServiceOAuthClient {
    pub(crate) fn synthetic_fixture(
        route: crate::synthetic_acceptance::Loopback,
        tls: TlsPolicy,
        control: InvocationControl,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, CraftError> {
        let mut client = Self::new(tls, control, clock)?;
        client.endpoint =
            reqwest::Url::parse(&format!("https://localhost:{}/oauth/token", route.issuer))
                .map_err(|_| invalid_client())?;
        client.synthetic = Some(route);
        Ok(client)
    }
}

async fn bounded_body(mut response: reqwest::Response) -> Result<Zeroizing<Vec<u8>>, AuthFailure> {
    let status = response.status().as_u16();
    if status != 200 {
        let code = match status {
            300..=399 => ErrorCode::PolicyDenied,
            400 | 401 => ErrorCode::AuthRequired,
            403 => ErrorCode::PermissionDenied,
            429 => ErrorCode::RateLimited,
            _ => ErrorCode::RemoteUnavailable,
        };
        let mut error =
            CraftError::new(code, "Service authentication endpoint refused the request.");
        error.details.write_state = RequestState::Failed;
        error.next_action = "Inspect service-account provisioning, eligibility and provider rate advice; no retry was performed.".into();
        return Err(AuthFailure {
            error,
            retry_after: retry_after(response.headers().get(RETRY_AFTER)),
            credential_update: None,
            credential_write_state: None,
        });
    }
    let media = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if !(media == "application/json"
        || (media.starts_with("application/") && media.ends_with("+json")))
        || response
            .content_length()
            .is_some_and(|length| length > RESPONSE_LIMIT as u64)
    {
        return Err(response_failure());
    }
    let mut bytes = Zeroizing::new(Vec::new());
    while let Some(chunk) = response.chunk().await.map_err(network_failure)? {
        if chunk.len() > RESPONSE_LIMIT.saturating_sub(bytes.len()) {
            return Err(response_failure());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
fn parse_scopes(scope: &SecretText) -> Result<Vec<String>, ()> {
    if scope.0.is_empty() || scope.0.len() > 16_384 {
        return Err(());
    }
    let mut scopes = BTreeSet::new();
    for token in scope.0.split(' ') {
        if token.is_empty()
            || token.len() > 256
            || !token.bytes().all(|byte| {
                byte == 0x21 || (0x23..=0x5b).contains(&byte) || (0x5d..=0x7e).contains(&byte)
            })
        {
            return Err(());
        }
        scopes.insert(token.to_owned());
        if scopes.len() > 256 {
            return Err(());
        }
    }
    Ok(scopes.into_iter().collect())
}
fn invalid_client() -> CraftError {
    CraftError::new(
        ErrorCode::Internal,
        "Cannot initialize service token exchange.",
    )
}
fn response_failure() -> AuthFailure {
    let mut error = invalid_response();
    error.details.write_state = RequestState::SentUnknown;
    error.next_action = "Inspect service authentication state; token issuance may have occurred, and no automatic retry was performed.".into();
    error.into()
}
fn network_failure(error: reqwest::Error) -> AuthFailure {
    let mut failure = response_failure();
    if error.is_timeout() {
        failure.error.code = ErrorCode::DeadlineExceeded;
    }
    failure
}
