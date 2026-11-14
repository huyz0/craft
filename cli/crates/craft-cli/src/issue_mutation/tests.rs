use super::*;
use craft_core::contracts::{CanonicalRequest, GrantRef, Selection};
use craft_core::input::content_digest;
use craft_core::storage::JournalStep;
use host::{IssueMutationPolicy, MutationPermission, PreparedIssueMutation};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn action_authority_is_separate_for_assignment_unassignment_deletion_and_cascade() -> TestResult {
    for (command, cascade, required) in [
        (ASSIGN_ISSUE, false, MutationPermission::Assign),
        (UNASSIGN_ISSUE, false, MutationPermission::Unassign),
        (DELETE_ISSUE, false, MutationPermission::Delete),
        (DELETE_ISSUE, true, MutationPermission::DeleteAllSubtasks),
    ] {
        let mut raw = request(EDIT_ISSUE);
        raw.command = command.into();
        raw.input.as_object_mut().ok_or("input")?.remove("changes");
        if command == DELETE_ISSUE {
            raw.input["subtasks"] = if cascade {
                json!({"mode":"delete_all","expected_visible_ids":["10001"]})
            } else {
                json!({"mode":"refuse"})
            };
        } else {
            raw.input["expected_assignee"] = json!({"kind":"unassigned"});
            if command == ASSIGN_ISSUE {
                raw.input["account_id"] = json!("account-one");
            }
        }
        for allowed in [
            MutationPermission::Edit,
            MutationPermission::Transition,
            MutationPermission::Assign,
            MutationPermission::Unassign,
            MutationPermission::Delete,
            MutationPermission::DeleteAllSubtasks,
        ] {
            let mut policy = policy();
            policy.permissions = vec![allowed];
            let prepared = PreparedIssueMutation::prepare(
                raw.clone(),
                "issue-action",
                true,
                "original-request",
                policy,
            );
            assert_eq!(
                prepared.is_ok(),
                allowed == required,
                "{command} cascade={cascade}"
            );
        }
        let mut policy = policy();
        policy.permissions = vec![required];
        policy.project.id = "999".into();
        assert_eq!(
            PreparedIssueMutation::prepare(raw, "issue-action", true, "original-request", policy)
                .err()
                .ok_or("expected project denial")?
                .code,
            ErrorCode::PolicyDenied
        );
    }
    Ok(())
}
fn target() -> ResourceRef {
    ResourceRef {
        provider: "jira".into(),
        deployment: "cloud".into(),
        tenant: "00000000-1111-2222-3333-444444444444".into(),
        resource_type: "issue".into(),
        id: "10000".into(),
    }
}
fn binding() -> OperationBinding {
    OperationBinding {
        operation_id: "issue-change".into(),
        request_digest: content_digest(b"input"),
        target: target(),
        grant: GrantRef {
            id: "gr_00000000000000000000000000000000".into(),
        },
        actor: "actor-one".into(),
        descriptor_digest: content_digest(b"descriptor"),
    }
}
fn policy() -> IssueMutationPolicy {
    IssueMutationPolicy {
        enabled: true,
        project: ResourceRef {
            resource_type: "project".into(),
            id: "100".into(),
            ..target()
        },
        grant: binding().grant,
        expected_actor: "actor-one".into(),
        permissions: vec![MutationPermission::Edit, MutationPermission::Transition],
    }
}
fn request(command: &str) -> CanonicalRequest {
    let mut input = json!({"issue":"10000","expected":{"id":"10000","key":"ED-7","project_id":"100","issue_type_id":"101","status_id":"102","updated":"2026-11-10T01:00:00.000+0000"},"changes":{"summary":"Reviewed"}});
    if command == TRANSITION_ISSUE {
        input["transition_id"] = json!("31");
        input["expected_status_id"] = json!("103");
    }
    CanonicalRequest {
        command: command.into(),
        target: target(),
        input,
        selection: Selection::default(),
        preconditions: None,
    }
}
fn pending(binding: OperationBinding, command: &str) -> Result<JournalRecord, CraftError> {
    let mut record = JournalRecord::initial(binding)
        .map_err(|_| CraftError::new(ErrorCode::Internal, "Fixture binding failed."))?;
    record.revision = 2;
    record.steps.insert(
        route(command)?.into(),
        JournalStep {
            state: SubmissionState::Unknown,
            request_digest: Some(content_digest(b"native")),
            confirmed_resources: vec![],
            job: None,
        },
    );
    Ok(record)
}
fn completion(command: &str) -> Result<MutationCompletion, CraftError> {
    MutationCompletion::prepare(
        binding(),
        command,
        "original-request",
        &["http_status".into()],
        16_384,
    )
}
fn finish(
    command: &str,
    status: u16,
    persist: Option<StoreError>,
) -> Result<CommandResult, CraftError> {
    let binding = binding();
    let prior = pending(binding.clone(), command)?;
    let response = NativeResponse {
        status,
        body: Value::Null,
        request_id: Some("request-native".into()),
        retry_after: Some(RetryAfter::DelaySeconds(4)),
        link_headers: None,
    };
    let mut completion = completion(command)?;
    completion.set_cache_update(Some(CredentialCacheUpdate {
        grant: binding.grant.clone(),
        write_state: RequestState::Confirmed,
        cleanup_pending: Some(true),
    }));
    Ok(completion.finish_with(
        AttemptView {
            binding: &binding,
            command,
            route: route(command)?,
            step: route(command)?,
            response: Ok(&response),
            pending: Ok(&prior),
        },
        |outcome| {
            if let Some(error) = persist {
                return Err(error);
            }
            let mut record = prior.clone();
            record.revision += 1;
            if let Some(step) = record
                .steps
                .get_mut(route(command).map_err(|_| StoreError::Invalid)?)
            {
                step.state = outcome.state;
                step.confirmed_resources = outcome.confirmed_resources.clone();
                step.job = outcome.job.clone();
            }
            Ok(record)
        },
    ))
}
#[test]
fn pure_host_preparation_rejects_invalid_policy_apply_input_cursor_and_recovery_budget()
-> TestResult {
    for command in [EDIT_ISSUE, TRANSITION_ISSUE] {
        PreparedIssueMutation::prepare(
            request(command),
            "issue-change",
            true,
            "request",
            policy(),
        )?;
        assert!(
            PreparedIssueMutation::prepare(
                request(command),
                "issue-change",
                false,
                "request",
                policy()
            )
            .is_err()
        );
        for change in ["disabled", "project", "actor", "permission"] {
            let mut policy = policy();
            match change {
                "disabled" => policy.enabled = false,
                "project" => policy.project.id = "999".into(),
                "actor" => policy.expected_actor = "anonymous".into(),
                _ => policy.permissions.clear(),
            };
            assert!(
                PreparedIssueMutation::prepare(
                    request(command),
                    "issue-change",
                    true,
                    "request",
                    policy
                )
                .is_err()
            );
        }
        let mut cursor = request(command);
        cursor.selection.cursor = Some("page".into());
        assert!(
            PreparedIssueMutation::prepare(cursor, "issue-change", true, "request", policy())
                .is_err()
        );
        let mut small = request(command);
        small.selection.max_bytes = 512;
        assert!(
            PreparedIssueMutation::prepare(small, "issue-change", true, "request", policy())
                .is_err()
        );
        let mut raw = request(command);
        raw.input["update"] = json!({});
        assert!(
            PreparedIssueMutation::prepare(raw, "issue-change", true, "request", policy()).is_err()
        );
    }
    Ok(())
}
#[test]
fn completion_preserves_confirmed_effect_and_cache_receipt_when_local_recording_fails() -> TestResult
{
    for command in [
        EDIT_ISSUE,
        TRANSITION_ISSUE,
        "jira.issue.assign",
        "jira.issue.unassign",
        "jira.issue.delete",
    ] {
        for persistence in [
            None,
            Some(StoreError::Unavailable),
            Some(StoreError::OutcomeUnknown),
        ] {
            let result = finish(command, 204, persistence)?;
            assert_eq!(result.envelope.ok, persistence.is_none());
            let data = result.envelope.data.as_ref().ok_or("data")?;
            assert_eq!(data["receipt"]["request_state"], "confirmed");
            assert_eq!(data["receipt"]["confirmed_resources"], json!([target()]));
            assert_eq!(data["credential_cache_update"]["cleanup_pending"], true);
            assert_eq!(data["replay_authorized"], false);
            assert_eq!(
                data["persistence"],
                match persistence {
                    None => "recorded",
                    Some(StoreError::OutcomeUnknown) => "uncertain",
                    _ => "unrecorded",
                }
            );
            let schema = projection()
                .compile(&schema()?, &["http_status".into()])?
                .response_schema(&schema()?)?;
            assert!(
                jsonschema::validator_for(&schema)?
                    .is_valid(&serde_json::to_value(&result.envelope)?)
            );
        }
    }
    Ok(())
}
#[test]
fn refusal_unknown_and_lost_responses_never_claim_an_existing_issue_was_changed() -> TestResult {
    for command in [EDIT_ISSUE, TRANSITION_ISSUE] {
        for (status, state) in [
            (409, "failed"),
            (200, "sent_unknown"),
            (429, "sent_unknown"),
            (500, "sent_unknown"),
        ] {
            let result = finish(command, status, None)?;
            let data = result.envelope.data.ok_or("data")?;
            assert_eq!(data["receipt"]["request_state"], state);
            assert_eq!(data["receipt"]["confirmed_resources"], json!([]));
            assert_eq!(data["replay_authorized"], false);
        }
    }
    Ok(())
}
#[test]
fn foreign_prior_receipt_is_retained_without_treating_it_as_a_new_send() -> TestResult {
    let mut foreign = binding();
    foreign.target.id = "20000".into();
    let mut prior = pending(foreign, EDIT_ISSUE)?;
    let step = prior.steps.get_mut(UPDATE_ROUTE).ok_or("step")?;
    step.state = SubmissionState::Confirmed;
    step.confirmed_resources = vec![prior.binding.target.clone()];
    let result = completion(EDIT_ISSUE)?.before_send_failure(
        CraftError::new(ErrorCode::OperationIdReused, "Different intent."),
        Some(&prior),
    );
    let data = result.envelope.data.ok_or("data")?;
    assert_eq!(data["receipt"]["request_state"], "not_sent");
    assert_eq!(data["prior_record"], json!(prior));
    assert_eq!(data["replay_authorized"], false);
    Ok(())
}

#[test]
fn uncertain_claim_cannot_be_downgraded_by_an_older_prepared_record() -> TestResult {
    let mut prior = pending(binding(), EDIT_ISSUE)?;
    prior.steps.get_mut(UPDATE_ROUTE).ok_or("step")?.state = SubmissionState::Prepared;
    for prior in [None, Some(&prior)] {
        let mut error =
            CraftError::new(ErrorCode::WriteOutcomeUnknown, "Claim acknowledgment lost.");
        error.details.write_state = RequestState::SentUnknown;
        let result = completion(EDIT_ISSUE)?.before_send_failure(error, prior);
        assert_eq!(
            result.envelope.data.ok_or("data")?["receipt"]["request_state"],
            "sent_unknown"
        );
    }
    Ok(())
}

#[test]
fn completion_retains_actual_foreign_attempt_confirmation_and_lost_transport_uncertainty()
-> TestResult {
    for foreign in [false, true] {
        let mut actual = binding();
        if foreign {
            actual.target.id = "20000".into();
        }
        let prior = pending(actual.clone(), EDIT_ISSUE)?;
        let response = NativeResponse {
            status: 204,
            body: Value::Null,
            request_id: None,
            retry_after: None,
            link_headers: None,
        };
        let error = failure(
            ErrorCode::Cancelled,
            RequestState::SentUnknown,
            "Exchange cancelled.",
        );
        let result = completion(EDIT_ISSUE)?.finish_with(
            AttemptView {
                binding: &actual,
                command: EDIT_ISSUE,
                route: UPDATE_ROUTE,
                step: UPDATE_ROUTE,
                response: if foreign { Ok(&response) } else { Err(&error) },
                pending: Ok(&prior),
            },
            |outcome| {
                let mut record = prior.clone();
                record.revision += 1;
                let step = record
                    .steps
                    .get_mut(UPDATE_ROUTE)
                    .ok_or(StoreError::Invalid)?;
                step.state = outcome.state;
                step.confirmed_resources = outcome.confirmed_resources.clone();
                Ok(record)
            },
        );
        assert!(!result.envelope.ok);
        let data = result.envelope.data.ok_or("data")?;
        assert_eq!(
            data["receipt"]["request_state"],
            if foreign { "confirmed" } else { "sent_unknown" }
        );
        assert_eq!(
            data["receipt"]["confirmed_resources"],
            if foreign {
                json!([actual.target])
            } else {
                json!([])
            }
        );
        assert_eq!(data["replay_authorized"], false);
    }
    Ok(())
}

#[test]
fn output_preflight_reserves_the_complete_receipt_and_selected_metadata() -> TestResult {
    let mut minimum = None;
    for maximum in 512..=16_384 {
        if MutationCompletion::prepare(
            binding(),
            EDIT_ISSUE,
            "request",
            &["http_status".into()],
            maximum,
        )
        .is_ok()
        {
            minimum = Some(maximum);
            break;
        }
    }
    let minimum = minimum.ok_or("minimum")?;
    assert!(
        MutationCompletion::prepare(
            binding(),
            EDIT_ISSUE,
            "request",
            &["http_status".into()],
            minimum - 1
        )
        .is_err()
    );
    let result = finish(EDIT_ISSUE, 204, None)?;
    let encoded = crate::output::encode(result.envelope, minimum, result.output_policy)?;
    assert!(encoded.bytes.len() <= minimum);
    assert_eq!(encoded.exit, 0);
    Ok(())
}

#[test]
fn additional_recorded_steps_cannot_disappear_from_completion_evidence() -> TestResult {
    let binding = binding();
    let mut prior = pending(binding.clone(), EDIT_ISSUE)?;
    prior.steps.insert(
        "other-effect".into(),
        JournalStep {
            state: SubmissionState::Confirmed,
            request_digest: Some(content_digest(b"other")),
            confirmed_resources: vec![ResourceRef {
                id: "20000".into(),
                ..target()
            }],
            job: None,
        },
    );
    let response = NativeResponse {
        status: 204,
        body: Value::Null,
        request_id: None,
        retry_after: None,
        link_headers: None,
    };
    let result = completion(EDIT_ISSUE)?.finish_with(
        AttemptView {
            binding: &binding,
            command: EDIT_ISSUE,
            route: UPDATE_ROUTE,
            step: UPDATE_ROUTE,
            response: Ok(&response),
            pending: Ok(&prior),
        },
        |outcome| {
            let mut record = prior.clone();
            record.revision += 1;
            let step = record
                .steps
                .get_mut(UPDATE_ROUTE)
                .ok_or(StoreError::Invalid)?;
            step.state = outcome.state;
            step.confirmed_resources = outcome.confirmed_resources.clone();
            Ok(record)
        },
    );
    assert!(!result.envelope.ok);
    let data = result.envelope.data.ok_or("data")?;
    assert_eq!(
        data["prior_record"]["steps"]["other-effect"]["confirmed_resources"][0]["id"],
        "20000"
    );
    assert_eq!(data["receipt"]["request_state"], "confirmed");
    Ok(())
}
