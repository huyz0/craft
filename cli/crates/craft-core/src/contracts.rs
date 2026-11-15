use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::CraftError;

pub const SCHEMA_VERSION: &str = "1.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RequestState {
    NotSent,
    SentUnknown,
    Confirmed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Accepted,
    Running,
    Succeeded,
    Failed,
    Unknown,
}

impl JobState {
    pub fn allows(self, next: Self) -> bool {
        self == next
            || matches!(
                (self, next),
                (
                    Self::Accepted,
                    Self::Running | Self::Succeeded | Self::Failed | Self::Unknown
                ) | (
                    Self::Running,
                    Self::Succeeded | Self::Failed | Self::Unknown
                ) | (
                    Self::Unknown,
                    Self::Accepted | Self::Running | Self::Succeeded | Self::Failed
                )
            )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EffectClass {
    Read,
    Write,
    Trash,
    Permanent,
    AccessChange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    Service,
    Delegated,
}

impl std::str::FromStr for AuthMode {
    type Err = CraftError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "service" => Ok(Self::Service),
            "delegated" => Ok(Self::Delegated),
            _ => Err(CraftError::field(
                "/auth_mode",
                "expected_service_or_delegated",
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderRef {
    pub provider: String,
    pub deployment: String,
}

impl ProviderRef {
    pub fn validate(&self) -> Result<(), CraftError> {
        validate_identity("/provider", &self.provider)?;
        validate_identity("/deployment", &self.deployment)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TenantRef {
    pub provider: String,
    pub deployment: String,
    pub tenant: String,
}

impl TenantRef {
    pub fn validate(&self) -> Result<(), CraftError> {
        self.provider_ref().validate()?;
        validate_identity("/tenant", &self.tenant)
    }

    pub fn provider_ref(&self) -> ProviderRef {
        ProviderRef {
            provider: self.provider.clone(),
            deployment: self.deployment.clone(),
        }
    }
}

/// An opaque local grant identity, never an access or refresh token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GrantRef {
    pub id: String,
}

/// Credential-cache effects are independent from any product-resource mutation.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CredentialCacheUpdate {
    pub grant: GrantRef,
    pub write_state: RequestState,
    pub cleanup_pending: Option<bool>,
}

impl GrantRef {
    pub fn validate(&self) -> Result<(), CraftError> {
        validate_identity("/grant/id", &self.id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResourceRef {
    pub provider: String,
    pub deployment: String,
    pub tenant: String,
    pub resource_type: String,
    pub id: String,
}

impl ResourceRef {
    pub fn tenant_ref(&self) -> TenantRef {
        TenantRef {
            provider: self.provider.clone(),
            deployment: self.deployment.clone(),
            tenant: self.tenant.clone(),
        }
    }

    pub fn validate(&self) -> Result<(), CraftError> {
        for (name, value) in [
            ("provider", &self.provider),
            ("deployment", &self.deployment),
            ("tenant", &self.tenant),
            ("resource_type", &self.resource_type),
            ("id", &self.id),
        ] {
            validate_identity(&format!("/{name}"), value)?;
        }
        Ok(())
    }
}

/// Checks an opaque identity without normalizing it; providers add resource-specific rules.
pub fn validate_identity(path: &str, value: &str) -> Result<(), CraftError> {
    if value.is_empty() || value.len() > 512 || value.trim() != value {
        return Err(CraftError::field(path, "expected_nonempty_raw_identity"));
    }
    if value.chars().any(|character| character.is_control()
        || matches!(character, '?' | '#' | '%' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')) {
        return Err(CraftError::field(path, "invalid_identity_characters"));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Selection {
    #[schemars(length(max = 32))]
    pub fields: Vec<String>,
    #[schemars(range(min = 1, max = 100))]
    pub limit: u16,
    #[schemars(range(min = 512, max = 16777216))]
    pub max_bytes: u32,
    pub cursor: Option<String>,
}

impl Default for Selection {
    fn default() -> Self {
        Self {
            fields: Vec::new(),
            limit: 20,
            max_bytes: 16_384,
            cursor: None,
        }
    }
}

impl Selection {
    pub fn validate(&self) -> Result<(), CraftError> {
        if !(1..=100).contains(&self.limit) {
            return Err(CraftError::field(
                "/limit",
                "expected_integer_between_1_and_100",
            ));
        }
        if !(512..=16_777_216).contains(&self.max_bytes) {
            return Err(CraftError::field(
                "/max_bytes",
                "expected_byte_budget_between_512_and_16777216",
            ));
        }
        if self.fields.len() > 32 {
            return Err(CraftError::field("/fields", "too_many_fields"));
        }
        for field in &self.fields {
            let parts: Vec<_> = field.split('.').collect();
            if parts.len() > 8
                || parts.iter().any(|part| {
                    part.is_empty()
                        || !part
                            .chars()
                            .all(|character| character.is_ascii_alphanumeric() || character == '_')
                })
            {
                return Err(CraftError::field("/fields", "expected_declared_dot_path"));
            }
        }
        if let Some(cursor) = &self.cursor {
            validate_identity("/cursor", cursor)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Preconditions {
    pub expected_version: Option<u64>,
    pub expected_digest: Option<String>,
    pub expected_parent: Option<ResourceRef>,
}

impl Preconditions {
    pub fn validate(&self, target: &ResourceRef) -> Result<(), CraftError> {
        if let Some(digest) = &self.expected_digest {
            let hexadecimal = digest.strip_prefix("sha256:").ok_or_else(|| {
                CraftError::field("/preconditions/expected_digest", "expected_sha256_digest")
            })?;
            if hexadecimal.len() != 64
                || !hexadecimal
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(CraftError::field(
                    "/preconditions/expected_digest",
                    "expected_sha256_digest",
                ));
            }
        }
        if let Some(parent) = &self.expected_parent {
            parent.validate()?;
            if parent.tenant_ref() != target.tenant_ref() {
                return Err(CraftError::field(
                    "/preconditions/expected_parent",
                    "parent_must_share_target_tenant",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CanonicalRequest {
    pub command: String,
    pub target: ResourceRef,
    pub input: Value,
    #[serde(default)]
    pub selection: Selection,
    pub preconditions: Option<Preconditions>,
}

impl CanonicalRequest {
    pub fn validate(&self) -> Result<(), CraftError> {
        validate_identity("/command", &self.command)?;
        self.target.validate()?;
        self.selection.validate()?;
        if !self.input.is_object() {
            return Err(CraftError::field("/input", "expected_object"));
        }
        if let Some(preconditions) = &self.preconditions {
            preconditions.validate(&self.target)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoteJob {
    pub resource: ResourceRef,
    pub state: JobState,
}

/// Bounded provider task status. Native result and message text never enter this record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JobObservation {
    pub state: JobState,
    #[schemars(length(min = 1, max = 64))]
    pub native_status: String,
    #[schemars(range(max = 100))]
    pub progress_percent: Option<u8>,
    pub elapsed_ms: Option<u64>,
}

/// Confirmed effects survive a later failed or uncertain request in a composite operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationReceipt {
    pub operation_id: String,
    pub request_state: RequestState,
    pub confirmed_resources: Vec<ResourceRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job: Option<RemoteJob>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Available,
    Unavailable,
    Unknown,
    NotApplicable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Capability {
    pub implemented: bool,
    pub policy_enabled: Availability,
    pub credential_scoped: Availability,
    pub tenant_supported: Availability,
    pub verified: Availability,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Page {
    pub returned: usize,
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub truncated: bool,
    pub stop_reason: String,
    pub consistency: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub schema_version: String,
    pub ok: bool,
    pub command: String,
    pub request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<CraftError>,
    pub warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<Page>,
}

impl Envelope {
    pub fn success(command: &str, request_id: String, data: Value) -> Self {
        Self {
            schema_version: SCHEMA_VERSION.into(),
            ok: true,
            command: command.into(),
            request_id,
            data: Some(data),
            error: None,
            warnings: Vec::new(),
            page: None,
        }
    }

    pub fn failure(command: &str, request_id: String, error: CraftError) -> Self {
        Self {
            schema_version: SCHEMA_VERSION.into(),
            ok: false,
            command: command.into(),
            request_id,
            data: None,
            error: Some(error),
            warnings: Vec::new(),
            page: None,
        }
    }

    /// Keeps bounded confirmed-effect and recovery evidence on partial or unknown failure.
    pub fn failure_with_data(
        command: &str,
        request_id: String,
        error: CraftError,
        data: Value,
    ) -> Self {
        Self {
            data: Some(data),
            ..Self::failure(command, request_id, error)
        }
    }
}
