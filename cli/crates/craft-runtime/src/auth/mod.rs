//! Host-owned service authentication. Secret material never becomes discovery output.

mod input;
mod prompt;
mod service;
mod wire;

pub use input::{read_service_credentials_from_stdin, read_static_credentials_from_stdin};
pub use prompt::read_service_credentials_interactively;

/// Validates a configured service product/cloud target without constructing a session or request.
pub fn validate_service_target(
    tenant: &craft_core::contracts::TenantRef,
) -> Result<(), craft_core::error::CraftError> {
    crate::transport::product_identity_url(tenant).map(|_| ())
}

pub use service::{
    AuthFailure, ProductIdentity, ServiceCredentials, ServiceGrantReceipt, ServiceOAuthClient,
    ServiceSession, ServiceToken, StaticCredentials, StaticGrantReceipt, TokenMetadata,
    VerifiedServiceAccess,
};
