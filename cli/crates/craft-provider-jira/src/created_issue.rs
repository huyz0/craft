use craft_core::contracts::{RequestState, ResourceRef, validate_identity};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::NativeResponse;
use craft_core::storage::SubmissionState;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const COMMAND: &str = "jira.issue.create";
pub const ROUTE: &str = "create-issue";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SecondaryState {
    Confirmed,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SecondaryResults {
    pub transition: Option<SecondaryState>,
    pub watchers: Option<SecondaryState>,
}

pub struct CreateObservation {
    pub state: SubmissionState,
    pub resources: Vec<ResourceRef>,
    pub key: Option<String>,
    pub secondary: SecondaryResults,
    pub error: Option<CraftError>,
}

pub fn validate_target(target: &ResourceRef) -> Result<(), CraftError> {
    target.validate()?;
    let uuid = target.tenant.len() == 36
        && target.tenant.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        });
    if target.provider != "jira"
        || target.deployment != "cloud"
        || target.resource_type != "project"
        || !uuid
        || !numeric_id(&target.id)
    {
        return Err(CraftError::field(
            "/target",
            "expected_jira_cloud_project_id",
        ));
    }
    Ok(())
}

fn numeric_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 512 && id.bytes().all(|byte| byte.is_ascii_digit())
}

fn error(code: ErrorCode, state: RequestState, message: &str) -> CraftError {
    let mut error = CraftError::new(code, message);
    error.details.write_state = state;
    error.next_action = "Retain the operation receipt and inspect known issue IDs; reconcile before another create.".into();
    error
}

fn secondary(value: &Value) -> SecondaryState {
    let Some(object) = value.as_object() else {
        return SecondaryState::Unknown;
    };
    if object.keys().any(|name| {
        !matches!(
            name.as_str(),
            "status" | "errorCollection" | "warningCollection"
        )
    }) {
        return SecondaryState::Unknown;
    }
    let Some(status) = object
        .get("status")
        .and_then(Value::as_u64)
        .filter(|status| (100..600).contains(status))
    else {
        return SecondaryState::Unknown;
    };
    if let Some(warnings) = object.get("warningCollection") {
        let valid = warnings.as_object().is_some_and(|object| {
            object.keys().all(|name| name == "warnings")
                && object.get("warnings").is_none_or(|value| {
                    value
                        .as_array()
                        .is_some_and(|values| values.iter().all(Value::is_string))
                })
        });
        if !valid {
            return SecondaryState::Unknown;
        }
    }
    let errors = object.get("errorCollection").map(|collection| {
        let object = collection.as_object()?;
        if object
            .keys()
            .any(|name| !matches!(name.as_str(), "status" | "errorMessages" | "errors"))
        {
            return None;
        }
        let messages = match object.get("errorMessages") {
            None => false,
            Some(value) => {
                let values = value.as_array()?;
                if values.iter().any(|v| !v.is_string()) {
                    return None;
                }
                !values.is_empty()
            }
        };
        let fields = match object.get("errors") {
            None => false,
            Some(value) => {
                let values = value.as_object()?;
                if values.values().any(|v| !v.is_string()) {
                    return None;
                }
                !values.is_empty()
            }
        };
        if object
            .get("status")
            .is_some_and(|value| value.as_i64().is_none())
        {
            return None;
        }
        Some(messages || fields)
    });
    match errors {
        Some(None) => SecondaryState::Unknown,
        Some(Some(true)) => SecondaryState::Failed,
        _ if status >= 400 => SecondaryState::Failed,
        _ if (200..300).contains(&status) && status != 202 => SecondaryState::Confirmed,
        _ => SecondaryState::Unknown,
    }
}

/// Extracts a qualified creation before interpreting optional and secondary result metadata.
/// Native response URLs and free-text provider errors are never returned as authority or diagnostics.
pub fn interpret(target: &ResourceRef, response: &NativeResponse) -> CreateObservation {
    let mut observation = CreateObservation {
        state: SubmissionState::Unknown,
        resources: Vec::new(),
        key: None,
        secondary: SecondaryResults {
            transition: None,
            watchers: None,
        },
        error: None,
    };
    if validate_target(target).is_err() {
        observation.error = Some(error(
            ErrorCode::PolicyDenied,
            RequestState::SentUnknown,
            "Create response does not have a qualified Jira project binding.",
        ));
        return observation;
    }
    if response.status != 201 {
        let (code, known_refusal) = match response.status {
            400 | 422 => (ErrorCode::InputInvalid, true),
            401 => (ErrorCode::AuthRequired, true),
            403 => (ErrorCode::PermissionDenied, true),
            429 => (ErrorCode::RateLimited, false),
            _ => (ErrorCode::WriteOutcomeUnknown, false),
        };
        observation.state = if known_refusal {
            SubmissionState::Failed
        } else {
            SubmissionState::Unknown
        };
        observation.error = Some(error(
            code,
            if known_refusal {
                RequestState::Failed
            } else {
                RequestState::SentUnknown
            },
            "Jira issue creation did not return qualified success evidence.",
        ));
        return observation;
    }
    let Some(object) = response.body.as_object() else {
        observation.error = Some(error(
            ErrorCode::WriteOutcomeUnknown,
            RequestState::SentUnknown,
            "Jira create response has no qualified issue identity.",
        ));
        return observation;
    };
    if let Some(id) = object
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| numeric_id(id))
    {
        observation.resources.push(ResourceRef {
            resource_type: "issue".into(),
            id: id.into(),
            ..target.clone()
        });
        observation.state = SubmissionState::Confirmed;
    }
    if observation.resources.is_empty() {
        observation.error = Some(error(
            ErrorCode::WriteOutcomeUnknown,
            RequestState::SentUnknown,
            "Jira create response has no qualified issue identity.",
        ));
        return observation;
    }
    let mut malformed = object.keys().any(|name| {
        !matches!(
            name.as_str(),
            "id" | "key" | "self" | "transition" | "watchers"
        )
    });
    if let Some(key) = object.get("key") {
        if let Some(key) = key.as_str().filter(|key| {
            validate_identity("/key", key).is_ok()
                && key
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
        }) {
            observation.key = Some(key.into());
        } else {
            malformed = true;
        }
    }
    if object.get("self").is_some_and(|value| !value.is_string()) {
        malformed = true;
    }
    observation.secondary.transition = object.get("transition").map(secondary);
    observation.secondary.watchers = object.get("watchers").map(secondary);
    let states = [
        observation.secondary.transition,
        observation.secondary.watchers,
    ];
    if malformed || states.contains(&Some(SecondaryState::Unknown)) {
        observation.error = Some(error(
            ErrorCode::RemoteUnavailable,
            RequestState::Confirmed,
            "The issue was created but response metadata or secondary results require inspection.",
        ));
    } else if states.contains(&Some(SecondaryState::Failed)) {
        observation.error = Some(error(
            ErrorCode::PartialApply,
            RequestState::Confirmed,
            "The issue was created but a requested secondary action failed.",
        ));
    }
    observation
}
