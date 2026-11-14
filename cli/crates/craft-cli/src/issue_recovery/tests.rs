use super::*;
use craft_core::input::content_digest;
use craft_core::storage::JournalStep;
type TestResult = Result<(), Box<dyn std::error::Error>>;
pub(super) fn project() -> ResourceRef {
    ResourceRef {
        provider: "jira".into(),
        deployment: "cloud".into(),
        tenant: "00000000-1111-2222-3333-444444444444".into(),
        resource_type: "project".into(),
        id: "100".into(),
    }
}
fn policy() -> RecoveryPolicy {
    RecoveryPolicy {
        enabled: true,
        project: project(),
        grant: GrantRef {
            id: "gr_00000000000000000000000000000000".into(),
        },
        expected_actor: "actor-one".into(),
        allowed_commands: vec![created_issue::COMMAND.into()],
    }
}
pub(super) fn original() -> CanonicalRequest {
    CanonicalRequest {
        command: created_issue::COMMAND.into(),
        target: project(),
        input: json!({"issue_type_id":"101","summary":"Reviewed"}),
        selection: Selection::default(),
        preconditions: None,
    }
}
fn prepared(ids: Vec<String>) -> Result<PreparedRecovery, CraftError> {
    PreparedRecovery::prepare(
        RecoveryInput {
            operation_id: "create-work".into(),
            original: original(),
            candidate_issue_ids: ids,
        },
        &[],
        16_777_216,
        "original-request",
        policy(),
    )
}
fn record(prepared: &PreparedRecovery, state: SubmissionState) -> JournalRecord {
    JournalRecord {
        binding: prepared.binding.clone(),
        revision: 3,
        steps: std::collections::BTreeMap::from([(
            created_issue::ROUTE.into(),
            JournalStep {
                request_digest: Some(content_digest(b"native")),
                state,
                confirmed_resources: vec![],
                job: None,
            },
        )]),
    }
}
#[test]
fn recovery_plan_refuses_binding_changes_without_rewriting_or_inheriting_ids() -> TestResult {
    let prepared = prepared(vec!["10001".into()])?;
    let saved = record(&prepared, SubmissionState::Unknown);
    assert_eq!(
        prepared.plan(&prepared.data(Some(saved.clone())))?,
        vec!["10001"]
    );
    for field in ["actor", "grant", "target", "request", "descriptor"] {
        let mut changed = saved.clone();
        match field {
            "actor" => changed.binding.actor = "actor-two".into(),
            "grant" => changed.binding.grant.id = "gr_11111111111111111111111111111111".into(),
            "target" => changed.binding.target.id = "200".into(),
            "request" => changed.binding.request_digest = content_digest(b"different"),
            _ => changed.binding.descriptor_digest = content_digest(b"different"),
        }
        assert_eq!(
            prepared
                .plan(&prepared.data(Some(changed)))
                .err()
                .ok_or("expected mismatch")?
                .code,
            ErrorCode::OperationIdReused
        );
    }
    let mut unrelated = saved.clone();
    unrelated.steps.insert(
        "other-step".into(),
        JournalStep {
            request_digest: Some(content_digest(b"other")),
            state: SubmissionState::Confirmed,
            confirmed_resources: vec![ResourceRef {
                resource_type: "issue".into(),
                id: "10002".into(),
                ..project()
            }],
            job: None,
        },
    );
    assert_eq!(
        prepared.plan(&prepared.data(Some(unrelated)))?,
        vec!["10001"]
    );
    assert_eq!(saved, record(&prepared, SubmissionState::Unknown));
    Ok(())
}
#[test]
fn recorded_submission_assessment_and_job_evidence_survive_projection() -> TestResult {
    let mut prepared = prepared(vec![])?;
    prepared.projection = projection().compile(&schema()?, &["assessment".into()])?;
    for (state, expected) in [
        (SubmissionState::Prepared, Assessment::NotLocallyStarted),
        (SubmissionState::SendStarted, Assessment::Unresolved),
        (SubmissionState::Unknown, Assessment::Unresolved),
        (SubmissionState::Confirmed, Assessment::RecordedConfirmation),
        (SubmissionState::Failed, Assessment::RecordedFailure),
    ] {
        let saved = record(&prepared, state);
        let data = prepared.data(Some(saved.clone()));
        assert_eq!(data.assessment, expected);
        let envelope = prepared.envelope(&data, None)?;
        assert!(jsonschema::validator_for(&schema()?)?.is_valid(&serde_json::to_value(envelope)?));
        assert_eq!(data.initial_record, Some(saved));
        assert!(!data.replay_authorized);
        assert!(!data.journal_updated);
        assert!(!data.attribution_upgraded);
    }
    for state in [
        JobState::Accepted,
        JobState::Running,
        JobState::Succeeded,
        JobState::Failed,
        JobState::Unknown,
    ] {
        let mut saved = record(&prepared, SubmissionState::Confirmed);
        let job = craft_core::contracts::RemoteJob {
            resource: ResourceRef {
                resource_type: "job".into(),
                id: "job-one".into(),
                ..project()
            },
            state,
        };
        saved.steps.get_mut(created_issue::ROUTE).ok_or("step")?.job = Some(job.clone());
        saved.validate()?;
        let data = prepared.data(Some(saved));
        let envelope = prepared.envelope(&data, None)?;
        let output = envelope.data.ok_or("projected evidence")?;
        assert_eq!(
            output["initial_record"]["steps"][created_issue::ROUTE]["job"],
            json!(job)
        );
        assert_eq!(
            output["reconciliation_required"],
            state == JobState::Unknown
        );
        assert!(output.get("observations").is_some());
        assert_eq!(output["attribution_upgraded"], false);
    }
    let data = prepared.data(None);
    assert_eq!(data.assessment, Assessment::LocalEvidenceMissing);
    assert_eq!(data.reconciliation_required, None);
    assert!(prepared.plan(&data)?.is_empty());
    Ok(())
}
#[test]
fn recovery_input_policy_candidates_and_output_are_purely_strict() -> TestResult {
    for (error, code) in [
        (StoreError::Cancelled, ErrorCode::Cancelled),
        (StoreError::DeadlineExceeded, ErrorCode::DeadlineExceeded),
        (StoreError::Unavailable, ErrorCode::Internal),
    ] {
        assert_eq!(journal_read_error(error).code, code);
    }
    for mode in [
        "disabled",
        "actor",
        "project",
        "operation",
        "duplicate",
        "unknown",
    ] {
        let mut policy = policy();
        match mode {
            "disabled" => policy.enabled = false,
            "actor" => policy.expected_actor = "anonymous".into(),
            "project" => policy.project.id = "200".into(),
            "operation" => policy.allowed_commands = vec!["jira.issue.delete".into()],
            "duplicate" => policy.allowed_commands.push(created_issue::COMMAND.into()),
            _ => policy.allowed_commands.push("unknown".into()),
        }
        assert!(
            PreparedRecovery::prepare(
                RecoveryInput {
                    operation_id: "create-work".into(),
                    original: original(),
                    candidate_issue_ids: vec![]
                },
                &[],
                16_384,
                "request",
                policy
            )
            .is_err()
        );
    }
    for ids in [
        vec!["EX-1".into()],
        vec!["10000".into(), "10000".into()],
        (10000..10021).map(|id| id.to_string()).collect(),
    ] {
        assert!(prepared(ids).is_err());
    }
    assert!(
        PreparedRecovery::prepare(
            RecoveryInput {
                operation_id: "create-work".into(),
                original: original(),
                candidate_issue_ids: vec![]
            },
            &["unknown".into()],
            16_384,
            "request",
            policy()
        )
        .is_err()
    );
    Ok(())
}
#[test]
fn preflight_covers_escaped_native_metadata_and_complete_rate_failure() -> TestResult {
    let ids: Vec<_> = (10000..10020).map(|id| id.to_string()).collect();
    let mut prepared = prepared(ids.clone())?;
    let mut data = prepared.data(Some(record(&prepared, SubmissionState::Unknown)));
    let (mut low, mut high) = (512, 16_777_216);
    while low < high {
        let middle = (low + high) / 2;
        prepared.maximum = middle;
        if prepared.preflight(&data, &ids).is_ok() {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    prepared.maximum = low;
    for id in ids {
        data.observations.push(IssueInspection {
            issue_id: id.clone(),
            visibility: Visibility::Visible,
            state: Some(IssueState {
                id,
                key: format!("{}-1", "A".repeat(509)),
                project_id: "100".into(),
                issue_type_id: "9".repeat(512),
                status_id: "9".repeat(512),
                updated: "\"".repeat(128),
            }),
            assignee_account_id: craft_core::presence::InputField::Value("\"".repeat(128)),
            observed_at: u64::MAX,
        });
    }
    let mut error = failure(ErrorCode::WriteOutcomeUnknown, &"x".repeat(500));
    error.next_action = "x".repeat(512);
    data.inspection_error = Some(error.clone());
    data.retry_after = Some(RetryAfter::AtUnixMillis(u64::MAX));
    data.credential_cache_update = Some(CredentialCacheUpdate {
        grant: prepared.binding.grant.clone(),
        write_state: RequestState::SentUnknown,
        cleanup_pending: Some(false),
    });
    let envelope = prepared.envelope(&data, Some(error))?;
    assert!(
        serde_json::to_vec(&envelope)?.len() < low,
        "preflight underreserved escaped evidence"
    );
    Ok(())
}
