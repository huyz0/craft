use super::*;
use craft_core::contracts::RequestState;
use craft_core::execution::NativeResponse;
use craft_core::storage::SubmissionState;

pub struct MutationObservation {
    pub state: SubmissionState,
    pub confirmed_resources: Vec<ResourceRef>,
    pub error: Option<CraftError>,
}

/// Interprets this bound operation only. Existing issue identity is not effect evidence.
pub fn interpret(
    command: &str,
    target: &ResourceRef,
    response: &NativeResponse,
) -> MutationObservation {
    let mut observed = MutationObservation {
        state: SubmissionState::Unknown,
        confirmed_resources: vec![],
        error: None,
    };
    let qualified = matches!(
        command,
        EDIT_ISSUE
            | TRANSITION_ISSUE
            | actions::ASSIGN_ISSUE
            | actions::UNASSIGN_ISSUE
            | actions::DELETE_ISSUE
    ) && numeric_id(&target.id)
        && validate_target(target, &target.id).is_ok();
    if !qualified {
        observed.error = Some(failure(
            ErrorCode::PolicyDenied,
            RequestState::SentUnknown,
            "Mutation response lacks a qualified operation and issue binding.",
        ));
        return observed;
    }
    if response.status == 204 {
        observed.state = SubmissionState::Confirmed;
        observed.confirmed_resources.push(target.clone());
        if !response.body.is_null() {
            observed.error = Some(failure(
                ErrorCode::RemoteUnavailable,
                RequestState::Confirmed,
                "Jira acknowledged the operation with unexpected response content.",
            ));
        }
        return observed;
    }
    let refusal = match command {
        EDIT_ISSUE => matches!(response.status, 400 | 401 | 403 | 404 | 409 | 422),
        actions::ASSIGN_ISSUE | actions::UNASSIGN_ISSUE => {
            matches!(response.status, 400 | 403 | 404)
        }
        actions::DELETE_ISSUE => matches!(response.status, 400 | 401 | 403 | 404),
        _ => matches!(response.status, 400 | 401 | 404 | 409 | 413 | 422),
    };
    observed.state = if refusal {
        SubmissionState::Failed
    } else {
        SubmissionState::Unknown
    };
    let code = match response.status {
        400 | 413 | 422 if refusal => ErrorCode::InputInvalid,
        401 => ErrorCode::AuthRequired,
        403 => ErrorCode::PermissionDenied,
        404 if refusal => ErrorCode::PermissionDenied,
        409 if refusal => ErrorCode::VersionConflict,
        429 => ErrorCode::RateLimited,
        _ => ErrorCode::WriteOutcomeUnknown,
    };
    observed.error = Some(failure(
        code,
        if refusal {
            RequestState::Failed
        } else {
            RequestState::SentUnknown
        },
        "Jira did not return qualified mutation success; no retry was performed.",
    ));
    observed
}
fn failure(code: ErrorCode, state: RequestState, message: &str) -> CraftError {
    let mut error = CraftError::new(code, message);
    error.details.write_state = state;
    error.next_action="Retain the operation receipt and inspect its journal; matching later issue values do not authorize replay.".into();
    error
}
