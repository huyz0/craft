//! Explicit debug-only fixtures for cross-crate acceptance. No production selector exists.

use std::sync::Arc;

use craft_core::contracts::GrantRef;
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::storage::{Clock, SecretBytes};

use crate::auth::{ServiceCredentials, ServiceOAuthClient};
use crate::profiles::ProfileStore;
use crate::secrets::{LockedCredentials, synthetic::SyntheticCredentials};
use crate::transport::TlsPolicy;

pub const CLIENT_ID: &str = "craft-synthetic-client";
pub const CLIENT_SECRET: &str = "craft-synthetic-client-secret";
pub const ACCESS_TOKEN: &str = "craft-synthetic-access-token";

pub(crate) fn refusal() -> CraftError {
    CraftError::new(
        ErrorCode::PolicyDenied,
        "Synthetic acceptance accepts only its fixed fixture material and loopback ports.",
    )
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Loopback {
    pub(crate) issuer: u16,
    product: u16,
}

impl Loopback {
    pub(crate) fn new(issuer: u16, product: u16) -> Result<Self, CraftError> {
        if issuer == 0 || product == 0 {
            return Err(refusal());
        }
        Ok(Self { issuer, product })
    }
    pub(crate) fn product_url(&self, mut url: reqwest::Url) -> Result<reqwest::Url, CraftError> {
        if url.scheme() != "https" {
            return Err(refusal());
        }
        url.set_host(Some("localhost")).map_err(|_| refusal())?;
        url.set_port(Some(self.product)).map_err(|_| refusal())?;
        Ok(url)
    }
}

/// Owns an isolated memory vault and native grant locks for actual service/cache/host calls.
/// Issuer/resource ports are loopback-only; the client rejects all other credential/token values.
pub struct ServiceFixture {
    client: ServiceOAuthClient,
    credentials: SyntheticCredentials,
}

impl ServiceFixture {
    pub fn new(
        profiles: &ProfileStore,
        issuer_port: u16,
        product_port: u16,
        roots: Vec<u8>,
        control: InvocationControl,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, CraftError> {
        let client = ServiceOAuthClient::synthetic_fixture(
            Loopback::new(issuer_port, product_port)?,
            TlsPolicy::AdditionalCaBundle(roots),
            control,
            clock,
        )?;
        let credentials = SyntheticCredentials::new(profiles)?;
        Ok(Self {
            client,
            credentials,
        })
    }
    pub fn client(&self) -> &ServiceOAuthClient {
        &self.client
    }
    pub fn lock(
        &self,
        grant: &GrantRef,
        control: &InvocationControl,
    ) -> Result<LockedCredentials<'_>, CraftError> {
        self.credentials.lock(grant, control)
    }
    pub fn service_credentials(&self) -> Result<ServiceCredentials, CraftError> {
        let bytes = serde_json::to_vec(
            &serde_json::json!({"client_id":CLIENT_ID,"client_secret":CLIENT_SECRET}),
        )
        .map_err(|_| refusal())?;
        ServiceCredentials::from_provisioning(SecretBytes::new(bytes).map_err(|_| refusal())?)
    }
}
