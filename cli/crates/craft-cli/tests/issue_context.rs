use craft_cli::issue_context::{IssueContextPolicy, PreparedIssueContext};
use craft_core::contracts::{CanonicalRequest, GrantRef, ResourceRef, Selection};
use craft_core::error::ErrorCode;
use craft_provider_jira::issue_context::{EDIT, TRANSITIONS};
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;
fn request(command: &str) -> CanonicalRequest {
    CanonicalRequest {
        command: command.into(),
        target: ResourceRef {
            provider: "jira".into(),
            deployment: "cloud".into(),
            tenant: "00000000-1111-2222-3333-444444444444".into(),
            resource_type: "issue".into(),
            id: "ED-1".into(),
        },
        input: json!({"issue":"ED-1"}),
        selection: Selection {
            max_bytes: 16_384,
            ..Selection::default()
        },
        preconditions: None,
    }
}
fn policy(request: &CanonicalRequest) -> IssueContextPolicy {
    IssueContextPolicy {
        target: request.target.clone(),
        grant: GrantRef {
            id: "gr_00000000000000000000000000000000".into(),
        },
        expected_actor: "account-one".into(),
    }
}

#[test]
fn issue_metadata_preparation_accepts_only_exact_configured_target_and_issued_principal()
-> TestResult {
    for command in [EDIT, TRANSITIONS] {
        let request = request(command);
        assert!(
            PreparedIssueContext::prepare(request.clone(), "original-request", policy(&request))
                .is_ok()
        );
        for change in ["target", "tenant", "grant", "actor"] {
            let mut policy = policy(&request);
            match change {
                "target" => policy.target.id = "ED-2".into(),
                "tenant" => policy.target.tenant = "11111111-1111-2222-3333-444444444444".into(),
                "grant" => policy.grant.id = "other-grant".into(),
                _ => policy.expected_actor = "unknown".into(),
            }
            assert!(
                PreparedIssueContext::prepare(request.clone(), "original-request", policy).is_err(),
                "{command}/{change}"
            );
        }
    }
    Ok(())
}

#[test]
fn issue_metadata_preparation_refuses_unknown_input_and_bypass_flags() -> TestResult {
    for command in [EDIT, TRANSITIONS] {
        for name in [
            "overrideScreenSecurity",
            "overrideEditableFlag",
            "skipRemoteOnlyCondition",
            "status",
        ] {
            let mut request = request(command);
            request.input[name] = json!(true);
            assert!(
                PreparedIssueContext::prepare(
                    request.clone(),
                    "original-request",
                    policy(&request)
                )
                .is_err(),
                "{command}/{name}"
            );
        }
        let mut request = request(command);
        request.input["issue"] = json!("ED-2");
        assert!(
            PreparedIssueContext::prepare(request.clone(), "original-request", policy(&request))
                .is_err()
        );
    }
    Ok(())
}

#[test]
fn issue_metadata_preparation_reserves_complete_context_and_recovery_before_authentication()
-> TestResult {
    for command in [EDIT, TRANSITIONS] {
        for max_bytes in [512, 1024, 2048, 4096] {
            let mut request = request(command);
            request.selection.max_bytes = max_bytes;
            let error = PreparedIssueContext::prepare(
                request.clone(),
                "original-request",
                policy(&request),
            )
            .err()
            .ok_or("Expected budget refusal")?;
            assert!(matches!(
                error.code,
                ErrorCode::InputInvalid | ErrorCode::OutputBudgetTooSmall
            ));
        }
    }
    Ok(())
}

#[test]
fn unsupported_metadata_projection_is_refused_during_pure_preparation() -> TestResult {
    for (command, field) in [
        (EDIT, "autoCompleteUrl"),
        (EDIT, "to.id"),
        (TRANSITIONS, "authorization"),
        (TRANSITIONS, "fields.autoCompleteUrl"),
    ] {
        let mut request = request(command);
        request.selection.fields = vec![field.into()];
        assert!(
            PreparedIssueContext::prepare(request.clone(), "original-request", policy(&request))
                .is_err(),
            "{command}/{field}"
        );
    }
    Ok(())
}
