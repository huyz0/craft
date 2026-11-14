use super::*;
use craft_core::storage::JournalStep;
use serde_json::{Value, json};
use std::collections::BTreeMap;
type TestResult = Result<(), Box<dyn std::error::Error>>;
fn fixture() -> Result<(PreparedOperation, OperationBinding), CraftError> {
    let operation = registry()?.prepare(CanonicalRequest {
        command: native::DELETE.into(),
        target: ResourceRef {
            provider: "jira".into(),
            deployment: "cloud".into(),
            tenant: "00000000-1111-2222-3333-444444444444".into(),
            resource_type: "project".into(),
            id: "200".into(),
        },
        input: json!({"issue_id":"100","attachment_id":"301","expected_revision":format!("sha256:{}", "0".repeat(64)),"ownership":"own"}),
        selection: Selection::default(),
        preconditions: None,
    })?;
    let binding = operation.mutation_binding(
        "attachment-removal",
        &GrantRef {
            id: format!("gr_{}", "0".repeat(32)),
        },
        "actor-one",
    )?;
    Ok((operation, binding))
}
fn data(binding: &OperationBinding) -> AttachmentDeleteData {
    empty(binding, "100".into(), "301".into())
}
fn record(
    binding: &OperationBinding,
    state: SubmissionState,
    resources: Vec<ResourceRef>,
) -> JournalRecord {
    JournalRecord {
        binding: binding.clone(),
        revision: 3,
        steps: BTreeMap::from([(
            native::DELETE_ROUTE.into(),
            JournalStep {
                request_digest: Some(format!("sha256:{}", "0".repeat(64))),
                state,
                confirmed_resources: resources,
                job: None,
            },
        )]),
    }
}
fn response(status: u16) -> NativeResponse {
    NativeResponse {
        status,
        body: Value::Null,
        request_id: Some("native-request".into()),
        retry_after: None,
        link_headers: None,
    }
}
fn view<'a>(
    binding: &'a OperationBinding,
    response: Result<&'a NativeResponse, &'a CraftError>,
    pending: Result<&'a JournalRecord, &'a StoreError>,
) -> CompletionView<'a> {
    CompletionView {
        binding,
        command: native::DELETE,
        route: native::DELETE_ROUTE,
        step: native::DELETE_ROUTE,
        response,
        pending,
    }
}
fn echo(
    binding: &OperationBinding,
) -> impl FnOnce(&StepOutcome) -> Result<JournalRecord, StoreError> + '_ {
    move |outcome| {
        Ok(record(
            binding,
            outcome.state,
            outcome.confirmed_resources.clone(),
        ))
    }
}
#[test]
fn confirmed_deletion_records_the_attachment_identity() -> TestResult {
    let (operation, binding) = fixture()?;
    let mut data = data(&binding);
    let pending = record(&binding, SubmissionState::SendStarted, vec![]);
    let response = response(204);
    finish_with(
        &operation,
        &binding,
        view(&binding, Ok(&response), Ok(&pending)),
        &mut data,
        echo(&binding),
    )?;
    assert_eq!(data.receipt.request_state, RequestState::Confirmed);
    assert_eq!(
        data.receipt.confirmed_resources[0].resource_type,
        "attachment"
    );
    assert_eq!(data.receipt.confirmed_resources[0].id, "301");
    assert!(data.acknowledged && !data.reconciliation_required && !data.replay_authorized);
    assert!(matches!(data.persistence, PersistenceState::Recorded));
    assert_eq!(data.http_status, Some(204));
    Ok(())
}
#[test]
fn confirmed_deletion_survives_unknown_local_acknowledgment() -> TestResult {
    let (operation, binding) = fixture()?;
    let mut data = data(&binding);
    let response = response(204);
    let unknown = StoreError::OutcomeUnknown;
    let error = finish_with(
        &operation,
        &binding,
        view(&binding, Ok(&response), Err(&unknown)),
        &mut data,
        |_| Err(StoreError::OutcomeUnknown),
    )
    .err()
    .ok_or("expected uncertain acknowledgment")?;
    assert_eq!(error.code, ErrorCode::Internal);
    assert_eq!(data.receipt.request_state, RequestState::Confirmed);
    assert_eq!(data.receipt.confirmed_resources[0].id, "301");
    assert!(data.reconciliation_required && !data.replay_authorized);
    assert!(matches!(data.persistence, PersistenceState::Uncertain));
    assert_eq!(data.provider_request_id.as_deref(), Some("native-request"));
    Ok(())
}
#[test]
fn lost_response_is_unknown_and_documented_refusal_is_failed() -> TestResult {
    let (operation, binding) = fixture()?;
    let mut data = data(&binding);
    let pending = record(&binding, SubmissionState::Unknown, vec![]);
    let lost = CraftError::new(ErrorCode::RemoteUnavailable, "Lost response");
    assert!(
        finish_with(
            &operation,
            &binding,
            view(&binding, Err(&lost), Ok(&pending)),
            &mut data,
            echo(&binding),
        )
        .is_err()
    );
    assert_eq!(data.receipt.request_state, RequestState::SentUnknown);
    assert!(!data.acknowledged && data.reconciliation_required);
    for status in [403, 404] {
        let mut data = self::data(&binding);
        let refused = response(status);
        let missing = StoreError::Unavailable;
        let error = finish_with(
            &operation,
            &binding,
            view(&binding, Ok(&refused), Err(&missing)),
            &mut data,
            echo(&binding),
        )
        .err()
        .ok_or("refusal accepted")?;
        assert_eq!(data.receipt.request_state, RequestState::Failed);
        assert!(data.receipt.confirmed_resources.is_empty());
        assert_eq!(error.details.write_state, RequestState::Failed);
    }
    Ok(())
}
#[test]
fn foreign_or_wrong_route_evidence_cannot_confirm_deletion() -> TestResult {
    let (operation, binding) = fixture()?;
    let mut foreign = binding.clone();
    foreign.actor = "foreign".into();
    let other = ResourceRef {
        resource_type: "attachment".into(),
        id: "999".into(),
        ..binding.target.clone()
    };
    let foreign_record = record(&foreign, SubmissionState::Confirmed, vec![other.clone()]);
    let response = response(204);
    let mut data = data(&binding);
    assert!(
        finish_with(
            &operation,
            &binding,
            view(&binding, Ok(&response), Ok(&foreign_record)),
            &mut data,
            |_| Ok(foreign_record.clone())
        )
        .is_err()
    );
    assert!(!data.receipt.confirmed_resources.contains(&other));
    let mut data = self::data(&binding);
    let missing = StoreError::Unavailable;
    let mut wrong = view(&binding, Ok(&response), Err(&missing));
    wrong.route = "jira-comment-delete";
    assert!(finish_with(&operation, &binding, wrong, &mut data, echo(&binding)).is_err());
    assert!(data.receipt.confirmed_resources.is_empty() && !data.acknowledged);
    assert_eq!(data.receipt.request_state, RequestState::SentUnknown);
    Ok(())
}
#[test]
fn extra_recorded_effects_require_inspection() -> TestResult {
    let (operation, binding) = fixture()?;
    let extra = ResourceRef {
        resource_type: "attachment".into(),
        id: "999".into(),
        ..binding.target.clone()
    };
    let pending = record(&binding, SubmissionState::Unknown, vec![extra.clone()]);
    let response = response(204);
    let mut data = data(&binding);
    let error = finish_with(
        &operation,
        &binding,
        view(&binding, Ok(&response), Ok(&pending)),
        &mut data,
        echo(&binding),
    )
    .err()
    .ok_or("extra effects accepted")?;
    assert_eq!(error.code, ErrorCode::PartialApply);
    assert!(data.receipt.confirmed_resources.contains(&extra));
    Ok(())
}
