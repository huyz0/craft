use craft_core::contracts::validate_identity;
use reqwest::header::AUTHORIZATION;
use serde::Deserialize;

use super::*;
use crate::transport::{bearer_header, product_identity_url};

/// A successful current-user probe, not a claim of project/space or mutation permission.
#[derive(Debug, Clone, Serialize)]
pub struct ProductIdentity {
    tenant: TenantRef,
    actor: String,
    verified_at_unix_ms: u64,
}
impl ProductIdentity {
    pub fn tenant(&self) -> &TenantRef {
        &self.tenant
    }
    pub fn actor(&self) -> &str {
        &self.actor
    }
    pub fn verified_at_unix_ms(&self) -> u64 {
        self.verified_at_unix_ms
    }
}

#[derive(Deserialize)]
struct CurrentUser {
    #[serde(rename = "accountId")]
    account_id: String,
    active: Option<bool>,
    #[serde(rename = "type")]
    kind: Option<String>,
}

fn permits_probe(token: &ServiceToken, provider: &str) -> bool {
    let Some(scopes) = &token.metadata.scopes else {
        return false;
    };
    let has = |scope: &str| scopes.iter().any(|actual| actual == scope);
    match provider {
        "jira" => {
            has("read:jira-user")
                || [
                    "read:application-role:jira",
                    "read:group:jira",
                    "read:user:jira",
                    "read:avatar:jira",
                ]
                .iter()
                .all(|scope| has(scope))
        }
        "confluence" => has("read:confluence-user") || has("read:content-details:confluence"),
        _ => false,
    }
}

impl ServiceOAuthClient {
    /// Probes one configured product/cloud ID, requiring reported scopes and a fresh token.
    /// An expected or already verified actor cannot silently change between products/sites.
    pub fn verify_product(
        &self,
        token: &mut ServiceToken,
        tenant: &TenantRef,
        expected_actor: Option<&str>,
    ) -> Result<ProductIdentity, AuthFailure> {
        self.control.check()?;
        let url = product_identity_url(tenant)?;
        #[cfg(feature = "synthetic-acceptance")]
        let url = {
            if self.synthetic != token.synthetic {
                return Err(crate::synthetic_acceptance::refusal().into());
            }
            if let Some(route) = &self.synthetic {
                if token.access_token.expose()
                    != crate::synthetic_acceptance::ACCESS_TOKEN.as_bytes()
                {
                    return Err(crate::synthetic_acceptance::refusal().into());
                }
                route.product_url(url)?
            } else {
                url
            }
        };
        if let Some(actor) = expected_actor {
            validate_identity("/actor", actor)?;
        }
        if token.needs_reacquisition() {
            return Err(CraftError::new(
                ErrorCode::AuthRequired,
                "Service token requires reacquisition.",
            )
            .into());
        }
        if !permits_probe(token, &tenant.provider) {
            return Err(CraftError::new(
                ErrorCode::PermissionDenied,
                "Reported token scopes do not establish current-user probe access.",
            )
            .into());
        }
        // Revalidation replaces access evidence. A failed read must not leave this target usable.
        token.verified.retain(|proof| proof.tenant() != tenant);
        if token.verified.len() >= 32 {
            return Err(CraftError::field("/targets", "too_many_verified_products").into());
        }
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
            .header(AUTHORIZATION, bearer_header(&token.access_token)?)
            .timeout(self.control.remaining()?)
            .build()
            .map_err(|_| invalid_client())?;
        let user: CurrentUser = self
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
                // A failed identity read does not imply a product-resource mutation.
                failure.error.details.write_state = RequestState::Failed;
                failure.error.message = "Product identity read failed or returned an invalid response.".into();
                failure.error.next_action = "Inspect product access, token scopes and provider rate advice; no retry was performed.".into();
                failure
            })?;
        validate_identity("/actor", &user.account_id).map_err(|_| identity_refusal())?;
        if user.account_id == "unknown"
            || user.active == Some(false)
            || (tenant.provider == "jira" && user.active != Some(true))
            || user
                .kind
                .as_deref()
                .is_some_and(|kind| matches!(kind, "anonymous" | "unknown"))
            || user
                .account_id
                .as_bytes()
                .windows(token.access_token.expose().len())
                .any(|part| part == token.access_token.expose())
            || expected_actor.is_some_and(|actor| actor != user.account_id)
            || token
                .actor
                .as_ref()
                .is_some_and(|actor| actor != &user.account_id)
        {
            return Err(identity_refusal());
        }
        if token.needs_reacquisition() {
            return Err(CraftError::new(
                ErrorCode::AuthRequired,
                "Service token expired during identity verification.",
            )
            .into());
        }
        let identity = ProductIdentity {
            tenant: tenant.clone(),
            actor: user.account_id,
            verified_at_unix_ms: self.clock.unix_millis(),
        };
        token.actor = Some(identity.actor.clone());
        token.verified.push(identity.clone());
        Ok(identity)
    }
}

fn identity_refusal() -> AuthFailure {
    let mut error = CraftError::new(
        ErrorCode::PolicyDenied,
        "Product identity did not match the expected active actor.",
    );
    error.details.write_state = RequestState::Failed;
    error.into()
}
