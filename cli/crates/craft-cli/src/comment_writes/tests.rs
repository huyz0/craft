use super::*;
use craft_core::storage::JournalStep;
use serde_json::{Value, json};
use std::collections::BTreeMap;
type TestResult = Result<(), Box<dyn std::error::Error>>;
#[test]
fn unavailable_first_or_later_journal_observation_preserves_uncertainty() -> TestResult {
    let (_, binding) = fixture()?;
    for earlier in [false, true] {
        let mut data = empty(&binding, "100".into());
        if earlier {
            data.prior_record = Some(record(&binding, SubmissionState::Prepared, vec![]));
            data.persistence = PersistenceState::Recorded;
        }
        assert!(journal_observation(Err(StoreError::Unavailable), &mut data).is_err());
        assert!(matches!(data.persistence, PersistenceState::Uncertain));
        assert!(data.reconciliation_required && !data.replay_authorized);
        assert_eq!(data.prior_record.is_some(), earlier);
    }
    Ok(())
}
fn fixture() -> Result<(PreparedOperation, OperationBinding), CraftError> {
    let operation=registry()?.prepare(CanonicalRequest {command:native::CREATE.into(),target:ResourceRef {provider:"jira".into(),deployment:"cloud".into(),tenant:"00000000-1111-2222-3333-444444444444".into(),resource_type:"project".into(),id:"200".into()},input:json!({"issue_id":"100","body":{"type":"doc","version":1,"content":[{"type":"paragraph","content":[{"type":"text","text":"Reviewed"}]}]},"visibility":{"mode":"project"}}),selection:Selection::default(),preconditions:None})?;
    let binding = operation.mutation_binding(
        "comment-change",
        &GrantRef {
            id: format!("gr_{}", "0".repeat(32)),
        },
        "actor-one",
    )?;
    Ok((operation, binding))
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
            native::CREATE_ROUTE.into(),
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
        body: json!({"id":"101","body":{"type":"doc","version":1,"content":[{"type":"paragraph","content":[{"type":"text","text":"Reviewed"}]}]}}),
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
        command: native::CREATE,
        route: native::CREATE_ROUTE,
        step: native::CREATE_ROUTE,
        response,
        pending,
    }
}
#[test]
fn confirmed_comment_identity_survives_unknown_local_acknowledgment() -> TestResult {
    let (operation, binding) = fixture()?;
    let mut data = empty(&binding, "100".into());
    let response = response(201);
    let unknown = StoreError::OutcomeUnknown;
    assert!(
        finish_with(
            &operation,
            &binding,
            view(&binding, Ok(&response), Err(&unknown)),
            &mut data,
            |_| Err(StoreError::OutcomeUnknown)
        )
        .is_err()
    );
    assert_eq!(data.receipt.request_state, RequestState::Confirmed);
    assert_eq!(data.receipt.confirmed_resources[0].id, "101");
    assert!(
        data.acknowledged
            && data.desired_fields_verified
            && data.reconciliation_required
            && !data.replay_authorized
    );
    assert!(matches!(data.persistence, PersistenceState::Uncertain));
    assert!(matches!(
        data.pending_record_state,
        PersistenceState::Uncertain
    ));
    assert_eq!(data.provider_request_id.as_deref(), Some("native-request"));
    Ok(())
}
#[test]
fn lost_create_response_is_unknown_and_preserves_pending_and_completion_evidence() -> TestResult {
    let (operation, binding) = fixture()?;
    let mut data = empty(&binding, "100".into());
    let pending = record(&binding, SubmissionState::Unknown, vec![]);
    let lost = CraftError::new(ErrorCode::RemoteUnavailable, "Lost response");
    assert!(
        finish_with(
            &operation,
            &binding,
            view(&binding, Err(&lost), Ok(&pending)),
            &mut data,
            |outcome| {
                assert!(
                    outcome.state == SubmissionState::Unknown
                        && outcome.confirmed_resources.is_empty()
                );
                Ok(record(&binding, outcome.state, vec![]))
            }
        )
        .is_err()
    );
    assert_eq!(data.receipt.request_state, RequestState::SentUnknown);
    assert!(!data.acknowledged && !data.replay_authorized);
    assert_eq!(data.pending_record, Some(pending));
    assert!(data.latest_record.is_some() && data.reconciliation_required);
    Ok(())
}
#[test]
fn extra_recorded_effects_survive_success_refusal_and_response_loss() -> TestResult {
    for status in [Some(201), Some(400), None] {
        let (operation, binding) = fixture()?;
        let extra = ResourceRef {
            resource_type: "comment".into(),
            id: "999".into(),
            ..binding.target.clone()
        };
        let pending = record(&binding, SubmissionState::Unknown, vec![extra.clone()]);
        let response = status.map(response);
        let lost = CraftError::new(ErrorCode::RemoteUnavailable, "Lost");
        let mut data = empty(&binding, "100".into());
        let error = finish_with(
            &operation,
            &binding,
            view(&binding, response.as_ref().ok_or(&lost), Ok(&pending)),
            &mut data,
            |outcome| {
                Ok(record(
                    &binding,
                    outcome.state,
                    outcome.confirmed_resources.clone(),
                ))
            },
        )
        .err()
        .ok_or("expected error")?;
        assert_eq!(error.code, ErrorCode::PartialApply);
        assert!(data.receipt.confirmed_resources.contains(&extra));
        assert!(data.reconciliation_required);
        assert_eq!(data.acknowledged, status == Some(201));
    }
    Ok(())
}
#[test]
fn foreign_or_wrong_route_evidence_cannot_promote_returned_identity() -> TestResult {
    let (operation, binding) = fixture()?;
    let mut foreign = binding.clone();
    foreign.actor = "foreign".into();
    let other = ResourceRef {
        resource_type: "comment".into(),
        id: "999".into(),
        ..binding.target.clone()
    };
    let foreign_record = record(&foreign, SubmissionState::Confirmed, vec![other.clone()]);
    let response = response(201);
    let mut data = empty(&binding, "100".into());
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
    assert_eq!(data.pending_record, Some(foreign_record.clone()));
    assert_eq!(data.latest_record, Some(foreign_record));
    data = empty(&binding, "100".into());
    let missing = StoreError::Unavailable;
    let mut wrong = view(&binding, Ok(&response), Err(&missing));
    wrong.route = native::UPDATE_ROUTE;
    assert!(
        finish_with(&operation, &binding, wrong, &mut data, |outcome| Ok(
            record(&binding, outcome.state, outcome.confirmed_resources.clone())
        ))
        .is_err()
    );
    assert!(data.receipt.confirmed_resources.is_empty() && !data.acknowledged);
    let mut incomplete = response;
    incomplete.body = Value::Null;
    data = empty(&binding, "100".into());
    assert!(
        finish_with(
            &operation,
            &binding,
            view(&binding, Ok(&incomplete), Err(&missing)),
            &mut data,
            |outcome| Ok(record(
                &binding,
                outcome.state,
                outcome.confirmed_resources.clone()
            ))
        )
        .is_err()
    );
    assert!(
        data.acknowledged
            && data.receipt.confirmed_resources.is_empty()
            && !data.desired_fields_verified
    );
    Ok(())
}
