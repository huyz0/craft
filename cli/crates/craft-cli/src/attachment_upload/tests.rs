use super::*;
fn args() -> command::UploadArgs {
    command::UploadArgs {
        operation_id: "upload-design".into(),
        project_id: "200".into(),
        issue_id: "100".into(),
        artifact_name: "snapshot.bin".into(),
        file_root: None,
        file: None,
        from_artifact: true,
        expected_digest: Some(format!("sha256:{}", "a".repeat(64))),
        filename: "design.bin".into(),
        media_type: "application/octet-stream".into(),
        byte_limit: 100,
        cloud_id: None,
        apply: true,
    }
}
fn target() -> ResourceRef {
    ResourceRef {
        provider: "jira".into(),
        deployment: "cloud".into(),
        tenant: "00000000-1111-2222-3333-444444444444".into(),
        resource_type: "project".into(),
        id: "200".into(),
    }
}
fn operation() -> Result<PreparedOperation, CraftError> {
    let mut registry = Registry::default();
    native::register(&mut registry)?;
    registry.prepare(CanonicalRequest {
        command: native::UPLOAD.into(),
        target: target(),
        input: json!(UploadInput {
            issue_id: "100".into(),
            artifact_name: "snapshot.bin".into(),
            digest: format!("sha256:{}", "a".repeat(64)),
            source_selection_digest: args().selection_digest()?,
            size_bytes: 3,
            filename: "design.bin".into(),
            media_type: "application/octet-stream".into()
        }),
        selection: Selection::default(),
        preconditions: None,
    })
}
fn binding(op: &PreparedOperation) -> Result<OperationBinding, CraftError> {
    op.mutation_binding(
        "upload-design",
        &GrantRef {
            id: format!("gr_{}", "0".repeat(32)),
        },
        "actor-one",
    )
}
fn resource(id: &str) -> ResourceRef {
    ResourceRef {
        resource_type: "attachment".into(),
        id: id.into(),
        ..target()
    }
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
            native::UPLOAD_ROUTE.into(),
            craft_core::storage::JournalStep {
                request_digest: Some(format!("sha256:{}", "0".repeat(64))),
                state,
                confirmed_resources: resources,
                job: None,
            },
        )]),
    }
}
fn response() -> NativeResponse {
    NativeResponse {
        status: 200,
        body: json!([{"id":"10001","filename":"design.bin","size":3}]),
        request_id: Some("trace-one".into()),
        retry_after: None,
        link_headers: None,
    }
}
fn view<'a>(
    binding: &'a OperationBinding,
    response: Result<&'a NativeResponse, &'a CraftError>,
    pending: &'a JournalRecord,
) -> Completion<'a> {
    Completion {
        binding,
        command: native::UPLOAD,
        route: native::UPLOAD_ROUTE,
        step: native::UPLOAD_ROUTE,
        response,
        incomplete: None,
        pending: Ok(pending),
    }
}
type TestResult = Result<(), Box<dyn std::error::Error>>;
#[test]
fn upload_source_schema_and_validation_enforce_exclusive_modes_without_state() -> TestResult {
    let options = InvocationOptions {
        state_root: None,
        profile: Some("factory"),
        request_id: "validate-upload",
        max_bytes: MINIMUM_OUTPUT,
    };
    let good = args();
    good.validate(&[], &options)?;
    let mut registry = Registry::default();
    command::register(&mut registry)?;
    registry.validate(native::UPLOAD, &json!(good))?;
    let mut workspace = good.clone();
    workspace.from_artifact = false;
    workspace.file_root = Some(if cfg!(windows) {
        "D:/input".into()
    } else {
        "/tmp/input".into()
    });
    workspace.file = Some("design.bin".into());
    workspace.expected_digest = None;
    workspace.validate(&[], &options)?;
    registry.validate(native::UPLOAD, &json!(workspace))?;
    for mode in [
        "missing_digest",
        "mixed",
        "empty_workspace",
        "escape",
        "name",
        "apply",
        "budget",
        "profile",
        "projection",
    ] {
        let mut bad = good.clone();
        let mut options = InvocationOptions { ..options };
        let mut fields = vec![];
        match mode {
            "missing_digest" => bad.expected_digest = None,
            "mixed" => bad.file = Some("source.bin".into()),
            "empty_workspace" => bad.from_artifact = false,
            "escape" => {
                bad = workspace.clone();
                bad.file = Some("../escape".into());
            }
            "name" => bad.artifact_name = "..".into(),
            "apply" => bad.apply = false,
            "budget" => options.max_bytes = 1024,
            "profile" => options.profile = None,
            _ => fields.push("attachment".into()),
        };
        assert!(bad.validate(&fields, &options).is_err(), "{mode}");
        if matches!(mode, "missing_digest" | "mixed" | "empty_workspace") {
            assert!(
                registry.validate(native::UPLOAD, &json!(bad)).is_err(),
                "schema {mode}"
            );
        }
    }
    Ok(())
}
#[test]
fn upload_acknowledged_effect_survives_unknown_or_failed_local_persistence() -> TestResult {
    let op = operation()?;
    let binding = binding(&op)?;
    let pending = record(&binding, SubmissionState::Unknown, vec![]);
    let native = response();
    for fault in [StoreError::Unavailable, StoreError::OutcomeUnknown] {
        let mut data = empty(&args(), target());
        let result = finish_with(
            &op,
            &binding,
            view(&binding, Ok(&native), &pending),
            &mut data,
            |_| Err(fault),
        );
        assert!(result.is_err());
        assert_eq!(data.receipt.request_state, RequestState::Confirmed);
        assert_eq!(data.receipt.confirmed_resources, vec![resource("10001")]);
        assert!(data.acknowledged);
        assert!(data.reconciliation_required);
        assert_eq!(data.http_status, Some(200));
        assert!(!data.remote_digest_verified);
    }
    Ok(())
}
#[test]
fn incomplete_upload_acknowledgment_remains_candidate_evidence_without_confirmed_resources()
-> TestResult {
    let op = operation()?;
    let binding = binding(&op)?;
    let pending = record(&binding, SubmissionState::Unknown, vec![]);
    let native = response();
    let mut error = CraftError::new(ErrorCode::WriteOutcomeUnknown, "Source incomplete");
    error.details.write_state = RequestState::SentUnknown;
    let mut view = view(&binding, Err(&error), &pending);
    view.incomplete = Some(&native);
    let mut data = empty(&args(), target());
    assert!(
        finish_with(&op, &binding, view, &mut data, |outcome| Ok(record(
            &binding,
            outcome.state,
            outcome.confirmed_resources.clone()
        )))
        .is_err()
    );
    assert_eq!(data.receipt.request_state, RequestState::SentUnknown);
    assert!(data.receipt.confirmed_resources.is_empty());
    assert_eq!(data.unqualified_acknowledgments, vec![resource("10001")]);
    assert!(!data.desired_fields_verified);
    assert!(!data.acknowledged);
    Ok(())
}
#[test]
fn unexpected_persisted_effects_are_retained_and_foreign_completion_never_records() -> TestResult {
    let op = operation()?;
    let binding = binding(&op)?;
    let pending = record(&binding, SubmissionState::Unknown, vec![]);
    let native = response();
    let mut data = empty(&args(), target());
    let result = finish_with(
        &op,
        &binding,
        view(&binding, Ok(&native), &pending),
        &mut data,
        |_| {
            Ok(record(
                &binding,
                SubmissionState::Confirmed,
                vec![resource("10001"), resource("10002")],
            ))
        },
    );
    assert_eq!(result.err().ok_or("error")?.code, ErrorCode::PartialApply);
    assert_eq!(data.receipt.confirmed_resources.len(), 2);
    assert!(data.reconciliation_required);
    let mut foreign = binding.clone();
    foreign.actor = "other".into();
    let mut data = empty(&args(), target());
    let called = std::cell::Cell::new(false);
    let result = finish_with(
        &op,
        &binding,
        view(&foreign, Ok(&native), &pending),
        &mut data,
        |_| {
            called.set(true);
            Ok(pending.clone())
        },
    );
    assert!(result.is_err());
    assert!(!called.get());
    assert_eq!(data.receipt.request_state, RequestState::SentUnknown);
    Ok(())
}
