//! Provider-neutral page data. Continuation never supplies routing or authorization authority.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::contracts::{GrantRef, Page, ResourceRef, validate_identity};
use crate::control::InvocationControl;
use crate::error::{CraftError, ErrorCode};
use crate::input::{canonical_digest, parse_object};

/// Resolved semantics, excluding output budgets, local cursor and credential generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListBinding {
    pub operation: String,
    pub descriptor_digest: String,
    pub schema_digest: String,
    pub projection_digest: String,
    pub api_revision: String,
    pub target: ResourceRef,
    pub grant: GrantRef,
    pub actor: String,
    pub query: BTreeMap<String, Value>,
}

impl ListBinding {
    pub fn digest(&self) -> Result<String, CraftError> {
        validate_identity("/operation", &self.operation)?;
        validate_identity("/api_revision", &self.api_revision)?;
        self.target.validate()?;
        self.grant.validate()?;
        validate_identity("/actor", &self.actor)?;
        for (path, digest) in [
            ("/descriptor_digest", &self.descriptor_digest),
            ("/schema_digest", &self.schema_digest),
            ("/projection_digest", &self.projection_digest),
        ] {
            if !valid_digest(digest) {
                return Err(CraftError::field(path, "expected_sha256_digest"));
            }
        }
        let encoded = serde_json::to_vec(self)
            .map_err(|_| CraftError::new(ErrorCode::Internal, "Cannot encode list binding."))?;
        if encoded.len() > 65_536 {
            return Err(CraftError::field("/query", "list_binding_too_large"));
        }
        canonical_digest(&parse_object(&encoded)?)
    }
}

pub fn valid_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hash| {
        hash.len() == 64
            && hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

/// Native values only. An adapter must enforce declared names and provider progression rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NativePosition(pub BTreeMap<String, String>);

impl NativePosition {
    pub fn validate(&self) -> Result<(), CraftError> {
        if self.0.is_empty() || self.0.len() > 16 {
            return Err(CraftError::field(
                "/continuation",
                "invalid_position_bounds",
            ));
        }
        for (name, value) in &self.0 {
            if name.is_empty()
                || name.len() > 128
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                || matches!(
                    name.to_ascii_lowercase().replace('-', "_").as_str(),
                    "access_token" | "refresh_token" | "client_secret" | "authorization"
                )
                || value.is_empty()
                || value.len() > 16_384
                || value.chars().any(char::is_control)
            {
                return Err(CraftError::field(
                    "/continuation",
                    "invalid_native_parameter",
                ));
            }
        }
        if serde_json::to_vec(self)
            .map_err(|_| CraftError::new(ErrorCode::Internal, "Cannot encode continuation."))?
            .len()
            > 32_768
        {
            return Err(CraftError::field("/continuation", "position_too_large"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativePage {
    /// Inert page evidence, never routing or authorization. Omitted for legacy pages.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_page_context"
    )]
    pub context: Option<Value>,
    pub items: Vec<Value>,
    pub next: Option<NativePosition>,
}

/// Preserve explicit null for page validators instead of treating it as absent legacy evidence.
pub fn deserialize_page_context<'de, D>(deserializer: D) -> Result<Option<Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Value::deserialize(deserializer).map(Some)
}

/// Implemented by trusted composition, never by user-supplied routing or cursor contents.
pub trait PageSource {
    /// Rechecks exact target/grant/actor and current scope/access. Cache is not authorization.
    fn check_access(
        &mut self,
        binding: &ListBinding,
        control: &InvocationControl,
    ) -> Result<(), CraftError>;
    /// One normalized native page through declared sealed operation routes, without counting.
    fn fetch(
        &mut self,
        binding: &ListBinding,
        position: Option<&NativePosition>,
        control: &InvocationControl,
    ) -> Result<NativePage, CraftError>;
}

pub struct ListRequest<'a> {
    pub binding: &'a ListBinding,
    pub cursor: Option<&'a str>,
    pub limit: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageObservation {
    pub observed_at: u64,
    /// Digest of normalized native page, not a full raw-response hash or snapshot identifier.
    pub page_digest: String,
}

/// The host must preserve `page` when promoting an oversized-item error to its envelope.
#[derive(Debug, Clone, Serialize)]
pub struct ListPage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<Value>,
    pub items: Vec<Value>,
    pub page: Page,
    pub observation: PageObservation,
    pub cached: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<CraftError>,
}
