use super::*;
use craft_core::contracts::GrantRef;
use craft_core::storage::JournalStep;
type TestResult = Result<(), Box<dyn std::error::Error>>;
fn fixture() -> Result<(OperationBinding, LinkInput), Box<dyn std::error::Error>> {
    let input: LinkInput = serde_json::from_value(
        json!({"inward_issue_id":"10001","outward_issue_id":"10002","inward_project_id":"200","link_type":{"id":"10000","name":"Blocks","inward":"is blocked by","outward":"blocks"}}),
    )?;
    let operation = registry()?.prepare(CanonicalRequest {
        command: CREATE.into(),
        target: ResourceRef {
            provider: "jira".into(),
            deployment: "cloud".into(),
            tenant: "00000000-1111-2222-3333-444444444444".into(),
            resource_type: "project".into(),
            id: "100".into(),
        },
        input: json!(input),
        selection: Selection::default(),
        preconditions: None,
    })?;
    Ok((
        operation.mutation_binding(
            "dependency",
            &GrantRef {
                id: format!("gr_{}", "0".repeat(32)),
            },
            "actor-one",
        )?,
        input,
    ))
}
fn record(
    binding: &OperationBinding,
    state: SubmissionState,
    resources: Vec<ResourceRef>,
) -> JournalRecord {
    JournalRecord {
        binding: binding.clone(),
        revision: 3,
        steps: std::collections::BTreeMap::from([(
            CREATE_ROUTE.into(),
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
        body: json!(null),
        request_id: None,
        retry_after: None,
        link_headers: None,
    }
}
#[test]
fn unexpected_same_step_resources_require_reconciliation_for_success_refusal_and_loss() -> TestResult
{
    for status in [Some(201), Some(400), None] {
        let (binding, input) = fixture()?;
        let extra = ResourceRef {
            resource_type: "issue".into(),
            id: "99999".into(),
            ..binding.target.clone()
        };
        let prior = record(&binding, SubmissionState::Unknown, vec![extra.clone()]);
        let response = status.map(response);
        let lost = CraftError::new(ErrorCode::RemoteUnavailable, "Lost response.");
        let mut data = empty(&binding, input.clone());
        let outcome = finish_with(
            &binding,
            &input,
            None,
            CompletionView {
                binding: &binding,
                command: CREATE,
                route: CREATE_ROUTE,
                step: CREATE_ROUTE,
                response: response.as_ref().ok_or(&lost),
                pending: Ok(&prior),
            },
            &mut data,
            |outcome| {
                Ok(record(
                    &binding,
                    outcome.state,
                    outcome.confirmed_resources.clone(),
                ))
            },
        );
        assert_eq!(outcome.err().map(|e| e.code), Some(ErrorCode::PartialApply));
        assert!(data.reconciliation_required);
        assert!(data.receipt.confirmed_resources.contains(&extra));
        assert_eq!(data.pending_record, Some(prior));
        assert!(data.latest_record.is_some());
        assert_eq!(data.relation_confirmed, status == Some(201));
    }
    Ok(())
}
#[test]
fn foreign_saved_records_are_retained_without_promoting_their_resources() -> TestResult {
    for foreign_pending in [true, false] {
        let (binding, input) = fixture()?;
        let mut foreign_binding = binding.clone();
        foreign_binding.actor = "foreign-actor".into();
        let foreign_resource = ResourceRef {
            resource_type: "issue".into(),
            id: "99999".into(),
            ..binding.target.clone()
        };
        let foreign = record(
            &foreign_binding,
            SubmissionState::Confirmed,
            vec![foreign_resource.clone()],
        );
        let ordinary = record(&binding, SubmissionState::Unknown, vec![]);
        let response = response(201);
        let mut data = empty(&binding, input.clone());
        let outcome = finish_with(
            &binding,
            &input,
            None,
            CompletionView {
                binding: &binding,
                command: CREATE,
                route: CREATE_ROUTE,
                step: CREATE_ROUTE,
                response: Ok(&response),
                pending: Ok(if foreign_pending { &foreign } else { &ordinary }),
            },
            &mut data,
            |outcome| {
                Ok(if foreign_pending {
                    record(&binding, outcome.state, outcome.confirmed_resources.clone())
                } else {
                    foreign.clone()
                })
            },
        );
        assert_eq!(outcome.err().map(|e| e.code), Some(ErrorCode::PartialApply));
        assert!(data.reconciliation_required);
        assert!(!data.receipt.confirmed_resources.contains(&foreign_resource));
        assert!(if foreign_pending {
            data.pending_record.as_ref() == Some(&foreign)
        } else {
            data.latest_record.as_ref() == Some(&foreign)
        });
    }
    Ok(())
}
#[test]
fn confirmed_relation_survives_unknown_local_receipt_acknowledgment() -> TestResult {
    let (binding, input) = fixture()?;
    let response = response(201);
    let mut data = empty(&binding, input.clone());
    let unknown = StoreError::OutcomeUnknown;
    assert!(
        finish_with(
            &binding,
            &input,
            None,
            CompletionView {
                binding: &binding,
                command: CREATE,
                route: CREATE_ROUTE,
                step: CREATE_ROUTE,
                response: Ok(&response),
                pending: Err(&unknown)
            },
            &mut data,
            |_| Err(StoreError::OutcomeUnknown)
        )
        .is_err()
    );
    assert_eq!(data.receipt.request_state, RequestState::Confirmed);
    assert_eq!(data.receipt.confirmed_resources.len(), 2);
    assert!(data.relation_confirmed);
    assert!(matches!(data.persistence, PersistenceState::Uncertain));
    assert!(matches!(
        data.pending_record_state,
        PersistenceState::Uncertain
    ));
    assert!(data.link_id.is_none());
    assert!(data.new_link_created.is_none());
    Ok(())
}
