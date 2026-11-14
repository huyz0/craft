use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::contracts::{CanonicalRequest, EffectClass, validate_identity};
use crate::error::{CraftError, ErrorCode};
use crate::input::canonical_digest;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum RouteSegment {
    Literal(String),
    Parameter(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeRoute {
    pub id: String,
    pub method: HttpMethod,
    pub path: Vec<RouteSegment>,
    pub query_parameters: Vec<String>,
    #[serde(default)]
    pub continuation_parameters: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repeated_query_parameters: Vec<String>,
    pub response_max_bytes: u32,
}

impl NativeRoute {
    pub fn validate(&self) -> Result<(), CraftError> {
        validate_identity("/routes/id", &self.id)?;
        if self.path.is_empty()
            || self.path.len() > 32
            || !(512..=16_777_216).contains(&self.response_max_bytes)
        {
            return Err(CraftError::new(
                ErrorCode::Internal,
                "Invalid native route bounds.",
            ));
        }
        let mut parameters = BTreeSet::new();
        for segment in &self.path {
            match segment {
                RouteSegment::Literal(value) => {
                    if value.is_empty()
                        || value == "."
                        || value == ".."
                        || value.len() > 128
                        || !value
                            .chars()
                            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
                    {
                        return Err(CraftError::new(
                            ErrorCode::Internal,
                            "Invalid literal route segment.",
                        ));
                    }
                }
                RouteSegment::Parameter(name) => {
                    if !valid_name(name) || !parameters.insert(name) {
                        return Err(CraftError::new(
                            ErrorCode::Internal,
                            "Invalid route parameter declaration.",
                        ));
                    }
                }
            }
        }
        let mut query = BTreeSet::new();
        for name in &self.query_parameters {
            if !valid_name(name)
                || !query.insert(name)
                || matches!(
                    name.to_ascii_lowercase().replace('-', "_").as_str(),
                    "access_token" | "refresh_token" | "client_secret" | "authorization"
                )
            {
                return Err(CraftError::new(
                    ErrorCode::Internal,
                    "Invalid native query declaration.",
                ));
            }
        }
        let mut continuation = BTreeSet::new();
        for name in &self.continuation_parameters {
            if !query.contains(name) || !continuation.insert(name) {
                return Err(CraftError::new(
                    ErrorCode::Internal,
                    "Invalid continuation parameter declaration.",
                ));
            }
        }
        let mut repeated = BTreeSet::new();
        for name in &self.repeated_query_parameters {
            if !matches!(self.method, HttpMethod::Get)
                || !query.contains(name)
                || continuation.contains(name)
                || !repeated.insert(name)
            {
                return Err(CraftError::new(
                    ErrorCode::Internal,
                    "Invalid repeated GET query declaration.",
                ));
            }
        }
        Ok(())
    }

    pub fn validate_call(&self, call: &NativeCall) -> Result<(), CraftError> {
        let parameters: BTreeSet<_> = self
            .path
            .iter()
            .filter_map(|segment| match segment {
                RouteSegment::Parameter(name) => Some(name.as_str()),
                _ => None,
            })
            .collect();
        if call.route != self.id
            || parameters != call.path_parameters.keys().map(String::as_str).collect()
        {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Native call does not match its declared route.",
            ));
        }
        for value in call.path_parameters.values() {
            validate_identity("/path_parameters", value)?;
        }
        if call.query.len().saturating_add(call.repeated_query.len()) > 32
            || call.query.iter().any(|(name, value)| {
                !self.query_parameters.contains(name)
                    || self.repeated_query_parameters.contains(name)
                    || value.len() > 16_384
            })
            || call.repeated_query.iter().any(|(name, values)| {
                !matches!(self.method, HttpMethod::Get)
                    || !self.repeated_query_parameters.contains(name)
                    || !self.query_parameters.contains(name)
                    || self.continuation_parameters.contains(name)
                    || call.query.contains_key(name)
                    || values.is_empty()
                    || values.len() > 100
                    || values.iter().any(|value| value.len() > 16_384)
            })
            || call
                .repeated_query
                .values()
                .try_fold(call.query.len(), |count, values| {
                    count.checked_add(values.len())
                })
                .is_none_or(|count| count > 256)
        {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Native query does not match its declared route.",
            ));
        }
        if let Some(body) = &call.body {
            let size = serde_json::to_vec(body)
                .map_err(|_| CraftError::new(ErrorCode::Internal, "Cannot encode native request."))?
                .len();
            if size > crate::input::MAX_INPUT_BYTES {
                return Err(CraftError::field("/body", "native_body_limit_exceeded"));
            }
        }
        Ok(())
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
}

/// Identifies a registered route without carrying a host, credential, header or raw URL.
#[derive(Clone)]
pub struct NativeCall {
    pub route: String,
    pub path_parameters: BTreeMap<String, String>,
    pub query: BTreeMap<String, String>,
    /// Declared GET arrays only. Order and duplicates are preserved; adapters validate semantics.
    pub repeated_query: BTreeMap<String, Vec<String>>,
    pub body: Option<Value>,
}

pub struct NativeResponse {
    pub status: u16,
    pub body: Value,
    pub request_id: Option<String>,
    pub retry_after: Option<RetryAfter>,
    /// Bounded untrusted Link values. None means unavailable; an empty vector proves observation.
    pub link_headers: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum RetryAfter {
    DelaySeconds(u64),
    AtUnixMillis(u64),
}

pub trait OperationContext {
    /// Confirms that an execution port belongs to this sealed operation without changing it.
    fn verify_operation(&self, _operation: &PreparedOperation) -> Result<(), CraftError> {
        Ok(())
    }
    /// Checks cancellation/deadline without sending a request or maintaining credentials.
    fn checkpoint(&self) -> Result<(), CraftError>;
    fn invoke(&mut self, call: &NativeCall) -> Result<NativeResponse, CraftError>;
    /// Validates a remote continuation as typed data; it never requests the supplied raw URL.
    fn normalize_next(&self, _call: &NativeCall, _link: &str) -> Result<NativeCall, CraftError> {
        Err(CraftError::new(
            ErrorCode::UnsupportedCapability,
            "Continuation normalization is unavailable in this context.",
        ))
    }
}

pub trait OperationHandler: Send + Sync {
    /// Pure provider-specific checks run after core validation and before host access.
    fn validate(&self, request: &CanonicalRequest) -> Result<(), CraftError>;
    fn execute(
        &self,
        operation: &PreparedOperation,
        context: &mut dyn OperationContext,
    ) -> Result<Value, CraftError>;
    fn reconcile(
        &self,
        _operation: &PreparedOperation,
        _context: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        Err(CraftError::new(
            ErrorCode::UnsupportedCapability,
            "This operation does not implement reconciliation.",
        ))
    }
}

/// Seals preparation so execution cannot silently replace payload, target, effect or descriptor.
pub struct PreparedOperation {
    request: CanonicalRequest,
    effect: EffectClass,
    digest: String,
    descriptor_digest: String,
}

impl PreparedOperation {
    pub(crate) fn new(
        request: CanonicalRequest,
        effect: EffectClass,
        descriptor_digest: String,
    ) -> Result<Self, CraftError> {
        let digest = canonical_digest(&serde_json::to_value(&request).map_err(|_| {
            CraftError::new(ErrorCode::Internal, "Cannot encode prepared request.")
        })?)?;
        Ok(Self {
            request,
            effect,
            digest,
            descriptor_digest,
        })
    }

    pub fn request(&self) -> &CanonicalRequest {
        &self.request
    }
    pub fn effect(&self) -> EffectClass {
        self.effect
    }
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// Binds mutation identity without output-only selection/budgets. The host supplies
    /// its authenticated actor/grant; these fields do not themselves prove authorization.
    pub fn mutation_binding(
        &self,
        operation_id: &str,
        grant: &crate::contracts::GrantRef,
        actor: &str,
    ) -> Result<crate::storage::OperationBinding, CraftError> {
        if matches!(self.effect, EffectClass::Read) {
            return Err(CraftError::new(
                ErrorCode::UnsupportedCapability,
                "Read operations cannot claim mutation authority.",
            ));
        }
        validate_identity("/operation_id", operation_id)?;
        validate_identity("/actor", actor)?;
        grant.validate()?;
        Ok(crate::storage::OperationBinding {
            operation_id: operation_id.into(),
            request_digest: canonical_digest(&serde_json::json!({
                "command": self.request.command,
                "target": self.request.target,
                "input": self.request.input,
                "preconditions": self.request.preconditions,
            }))?,
            descriptor_digest: self.descriptor_digest.clone(),
            target: self.request.target.clone(),
            grant: grant.clone(),
            actor: actor.into(),
        })
    }
    pub(crate) fn descriptor_digest(&self) -> &str {
        &self.descriptor_digest
    }
}

pub(crate) struct CheckedContext<'a> {
    pub routes: &'a [NativeRoute],
    pub effect: EffectClass,
    pub host: &'a mut dyn OperationContext,
}

impl OperationContext for CheckedContext<'_> {
    fn verify_operation(&self, operation: &PreparedOperation) -> Result<(), CraftError> {
        self.host.verify_operation(operation)
    }
    fn checkpoint(&self) -> Result<(), CraftError> {
        self.host.checkpoint()
    }

    fn invoke(&mut self, call: &NativeCall) -> Result<NativeResponse, CraftError> {
        self.checkpoint()?;
        let route = self
            .routes
            .iter()
            .find(|route| route.id == call.route)
            .ok_or_else(|| {
                CraftError::new(
                    ErrorCode::PolicyDenied,
                    "Operation did not declare this native route.",
                )
            })?;
        route.validate_call(call)?;
        let response = self.host.invoke(call)?;
        let size = serde_json::to_vec(&response.body)
            .map_err(|_| CraftError::new(ErrorCode::Internal, "Cannot encode native response."))?
            .len();
        if size > route.response_max_bytes as usize {
            // Native searches can use POST, so the declared effect owns write-state semantics.
            let read = matches!(self.effect, EffectClass::Read);
            let mut error = CraftError::new(
                if read {
                    ErrorCode::RemoteUnavailable
                } else {
                    ErrorCode::WriteOutcomeUnknown
                },
                "Native response exceeded its declared byte budget.",
            );
            error.details.write_state = if read {
                crate::contracts::RequestState::Failed
            } else {
                crate::contracts::RequestState::SentUnknown
            };
            return Err(error);
        }
        Ok(response)
    }

    fn normalize_next(&self, call: &NativeCall, link: &str) -> Result<NativeCall, CraftError> {
        self.checkpoint()?;
        let route = self
            .routes
            .iter()
            .find(|route| route.id == call.route)
            .ok_or_else(|| {
                CraftError::new(
                    ErrorCode::PolicyDenied,
                    "Operation did not declare this native route.",
                )
            })?;
        route.validate_call(call)?;
        let next = self.host.normalize_next(call, link)?;
        route.validate_call(&next)?;
        Ok(next)
    }
}
