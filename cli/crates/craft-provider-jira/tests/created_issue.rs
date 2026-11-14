use craft_core::contracts::{RequestState, ResourceRef};
use craft_core::error::ErrorCode;
use craft_core::execution::NativeResponse;
use craft_core::storage::SubmissionState;
use craft_provider_jira::created_issue::{SecondaryState, interpret};
use serde_json::{Value, json};

fn target() -> ResourceRef {
    ResourceRef {
        provider: "jira".into(),
        deployment: "cloud".into(),
        tenant: "00000000-1111-2222-3333-444444444444".into(),
        resource_type: "project".into(),
        id: "10000".into(),
    }
}
fn response(status: u16, body: Value) -> NativeResponse {
    NativeResponse {
        status,
        body,
        request_id: None,
        retry_after: None,
        link_headers: None,
    }
}

#[test]
fn native_example_confirms_creation_without_trusting_returned_urls() {
    let observed = interpret(
        &target(),
        &response(
            201,
            json!({"id":"10001","key":"ED-24","self":"https://untrusted.invalid/read-my-token","transition":{"status":200,"errorCollection":{"errorMessages":[],"errors":{}}},"watchers":{"status":204,"warningCollection":{"warnings":["raw-warning-sentinel"]}}}),
        ),
    );
    assert_eq!(observed.state, SubmissionState::Confirmed);
    assert_eq!(observed.resources[0].id, "10001");
    assert_eq!(observed.resources[0].tenant, target().tenant);
    assert_eq!(observed.resources[0].resource_type, "issue");
    assert_eq!(observed.key.as_deref(), Some("ED-24"));
    assert_eq!(
        observed.secondary.transition,
        Some(SecondaryState::Confirmed)
    );
    assert!(observed.error.is_none());
}

#[test]
fn malformed_optional_metadata_never_erases_a_qualified_creation() {
    for body in [
        json!({"id":"10001","key":42}),
        json!({"id":"10001","self":{}}),
        json!({"id":"10001","invented":"raw-secret-sentinel"}),
        json!({"id":"10001","key":"bad\nkey"}),
    ] {
        let observed = interpret(&target(), &response(201, body));
        assert_eq!(observed.state, SubmissionState::Confirmed);
        assert_eq!(observed.resources[0].id, "10001");
        let error = observed
            .error
            .as_ref()
            .unwrap_or_else(|| panic!("Missing metadata error"));
        assert_eq!(error.details.write_state, RequestState::Confirmed);
        assert!(!error.message.contains("raw-secret-sentinel"));
        assert!(!error.retryable);
    }
}

#[test]
fn invalid_or_missing_identity_and_unqualified_status_never_confirm_creation() {
    for id in [
        Value::Null,
        json!(10001),
        json!(""),
        json!("../10001"),
        json!("id-secret-sentinel"),
        json!("1".repeat(513)),
    ] {
        let observed = interpret(&target(), &response(201, json!({"id":id,"key":"ED-24"})));
        assert_eq!(observed.state, SubmissionState::Unknown);
        assert!(observed.resources.is_empty());
        assert_eq!(
            observed.error.map(|e| e.code),
            Some(ErrorCode::WriteOutcomeUnknown)
        );
    }
    for body in [json!({}), json!([]), Value::Null] {
        assert!(
            interpret(&target(), &response(201, body))
                .resources
                .is_empty()
        );
    }
    for status in [200, 202, 204, 302, 500, 503] {
        let observed = interpret(&target(), &response(status, json!({"id":"10001"})));
        assert_eq!(observed.state, SubmissionState::Unknown);
        assert!(observed.resources.is_empty());
    }
}

#[test]
fn secondary_failures_and_unknown_results_preserve_the_main_issue() {
    for (value, state, code) in [
        (
            json!({"status":400,"errorCollection":{"errors":{"transition":"private-error-sentinel"}}}),
            SecondaryState::Failed,
            ErrorCode::PartialApply,
        ),
        (
            json!({"status":200,"errorCollection":{"errorMessages":["private-error-sentinel"]}}),
            SecondaryState::Failed,
            ErrorCode::PartialApply,
        ),
        (
            json!({"status":202}),
            SecondaryState::Unknown,
            ErrorCode::RemoteUnavailable,
        ),
        (
            json!({"status":200,"warningCollection":null}),
            SecondaryState::Unknown,
            ErrorCode::RemoteUnavailable,
        ),
        (
            json!({"status":200,"errorCollection":{"errors":[]}}),
            SecondaryState::Unknown,
            ErrorCode::RemoteUnavailable,
        ),
        (
            json!({}),
            SecondaryState::Unknown,
            ErrorCode::RemoteUnavailable,
        ),
    ] {
        for field in ["transition", "watchers"] {
            let mut body = json!({"id":"10001"});
            body[field] = value.clone();
            let observed = interpret(&target(), &response(201, body));
            assert_eq!(observed.resources[0].id, "10001");
            assert_eq!(
                if field == "transition" {
                    observed.secondary.transition
                } else {
                    observed.secondary.watchers
                },
                Some(state)
            );
            let error = observed
                .error
                .unwrap_or_else(|| panic!("Missing secondary error"));
            assert_eq!(error.code, code);
            assert_eq!(error.details.write_state, RequestState::Confirmed);
            assert!(!error.message.contains("private-error-sentinel"));
        }
    }
}

#[test]
fn documented_refusals_and_unqualified_rate_status_remain_distinct() {
    for (status, code, state) in [
        (400, ErrorCode::InputInvalid, SubmissionState::Failed),
        (401, ErrorCode::AuthRequired, SubmissionState::Failed),
        (403, ErrorCode::PermissionDenied, SubmissionState::Failed),
        (422, ErrorCode::InputInvalid, SubmissionState::Failed),
        (429, ErrorCode::RateLimited, SubmissionState::Unknown),
    ] {
        let observed = interpret(
            &target(),
            &response(
                status,
                json!({"id":"10001","errorMessages":["private-refusal-sentinel"]}),
            ),
        );
        assert_eq!(observed.state, state);
        assert!(observed.resources.is_empty());
        let error = observed.error.unwrap_or_else(|| panic!("Missing refusal"));
        assert_eq!(error.code, code);
        assert!(!error.retryable);
        assert!(!error.message.contains("private-refusal-sentinel"));
    }
}

#[test]
fn wrong_product_or_project_authority_never_qualifies_response_identity() {
    for field in ["provider", "deployment", "tenant", "resource_type", "id"] {
        let mut target = target();
        match field {
            "provider" => target.provider = "confluence".into(),
            "deployment" => target.deployment = "server".into(),
            "tenant" => target.tenant = "wrong-site".into(),
            "resource_type" => target.resource_type = "issue".into(),
            _ => target.id = "ED".into(),
        }
        let observed = interpret(&target, &response(201, json!({"id":"10001"})));
        assert!(observed.resources.is_empty());
        assert_eq!(
            observed.error.map(|e| e.code),
            Some(ErrorCode::PolicyDenied)
        );
    }
}
