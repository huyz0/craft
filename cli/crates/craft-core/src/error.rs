use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::contracts::RequestState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    InputInvalid,
    CommandUnknown,
    FieldUnknown,
    TargetAmbiguous,
    UnsupportedCapability,
    ApplyRequired,
    OutputBudgetTooSmall,
    CursorInvalid,
    CursorUnavailable,
    CursorExpired,
    CursorMismatch,
    CursorCorrupt,
    CursorCapacity,
    ItemTooLarge,
    PaginationStalled,
    AuthRequired,
    KeyringUnavailable,
    PermissionDenied,
    PolicyDenied,
    VersionConflict,
    OperationIdReused,
    RateLimited,
    WriteOutcomeUnknown,
    PartialApply,
    RemoteUnavailable,
    DeadlineExceeded,
    Cancelled,
    Internal,
}

impl ErrorCode {
    pub fn exit_code(self) -> u8 {
        match self {
            Self::InputInvalid
            | Self::CommandUnknown
            | Self::FieldUnknown
            | Self::TargetAmbiguous
            | Self::UnsupportedCapability
            | Self::ApplyRequired
            | Self::OutputBudgetTooSmall
            | Self::CursorInvalid
            | Self::CursorUnavailable
            | Self::CursorExpired
            | Self::CursorMismatch
            | Self::CursorCorrupt
            | Self::CursorCapacity
            | Self::ItemTooLarge
            | Self::PaginationStalled => 10,
            Self::AuthRequired => 20,
            Self::KeyringUnavailable => 21,
            Self::PermissionDenied | Self::PolicyDenied => 30,
            Self::VersionConflict | Self::OperationIdReused => 40,
            Self::RateLimited => 50,
            Self::WriteOutcomeUnknown => 51,
            Self::PartialApply => 52,
            Self::RemoteUnavailable | Self::DeadlineExceeded | Self::Internal => 60,
            Self::Cancelled => 130,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FieldIssue {
    pub path: String,
    pub rule: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub path_truncated: bool,
}

fn is_false(value: &bool) -> bool {
    !value
}

impl FieldIssue {
    pub fn new(path: impl Into<String>, rule: impl Into<String>) -> Self {
        let path = path.into();
        Self {
            path_truncated: path.chars().take(257).count() > 256,
            path: path.chars().take(256).collect(),
            rule: rule.into().chars().take(256).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ErrorDetails {
    pub write_state: RequestState,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<FieldIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, thiserror::Error)]
#[error("{message}")]
#[serde(deny_unknown_fields)]
pub struct CraftError {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
    pub details: ErrorDetails,
    pub next_action: String,
}

impl CraftError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into().chars().take(500).collect(),
            retryable: false,
            details: ErrorDetails {
                write_state: RequestState::NotSent,
                issues: Vec::new(),
            },
            next_action: "Inspect the selected command schema and correct the request.".into(),
        }
    }

    pub fn field(path: impl Into<String>, rule: impl Into<String>) -> Self {
        let mut error = Self::new(ErrorCode::InputInvalid, "Request validation failed.");
        error.details.issues.push(FieldIssue::new(path, rule));
        error
    }
}
