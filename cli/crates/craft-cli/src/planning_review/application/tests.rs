use super::*;
type TestResult = Result<(), Box<dyn std::error::Error>>;
fn args() -> ApplyArgs {
    ApplyArgs {
        run_id: "delivery".into(),
        preview_artifact: "tickets-preview.json".into(),
        result_title: "Delivery results".into(),
        consistency: Some(Consistency::RevalidateEachBatch),
        apply: true,
    }
}
#[test]
fn application_input_requires_explicit_intent_and_complete_evidence_capacity() -> TestResult {
    let options = InvocationOptions {
        state_root: None,
        profile: Some("factory"),
        request_id: "request",
        max_bytes: OUTPUT,
    };
    args().validate(&[], &options)?;
    for mode in ["apply", "consistency", "path", "title", "run"] {
        let mut input = args();
        match mode {
            "apply" => input.apply = false,
            "consistency" => input.consistency = None,
            "path" => input.preview_artifact = "../preview.json".into(),
            "title" => input.result_title = " ".into(),
            _ => input.run_id = "run/other".into(),
        };
        assert!(input.validate(&[], &options).is_err(), "{mode}");
    }
    assert!(args().validate(&["complete".into()], &options).is_err());
    assert!(
        args()
            .validate(
                &[],
                &InvocationOptions {
                    max_bytes: 65536,
                    ..options
                }
            )
            .is_err()
    );
    assert!(serde_json::from_value::<ApplyArgs>(json!({"run_id":"delivery","preview_artifact":"x.json","result_title":"Results","consistency":null,"apply":true})).is_err());
    Ok(())
}

#[test]
fn completion_import_requires_full_matching_confirmed_native_evidence() -> TestResult {
    let fixture = Fixture::new(Kind::Issue)?;
    fixture.intent()?;
    fixture.finish(SubmissionState::Confirmed, None)?;
    let record = fixture
        .journal
        .get(&fixture.binding.operation_id)?
        .ok_or("record")?;
    let mut envelope = fixture.outcome();
    let data = envelope.data.as_mut().ok_or("completion")?;
    data["target"] = json!(fixture.binding.target);
    data["step"] = json!(fixture.spec.route);
    data["issue_key"] = json!("PAY-11001");
    data["secondary"] = json!({"transition":null,"watchers":null});
    data["persistence"] = json!("recorded");
    data["journal_revision"] = json!(record.revision);
    data["provider_request_id"] = Value::Null;
    data["retry_after"] = Value::Null;
    data["replay_authorized"] = json!(false);
    data["credential_cache_update"] = Value::Null;
    ledger::validate_import(&fixture.spec, &envelope, &record)?;
    for mode in [
        "command",
        "operation",
        "target",
        "resource",
        "revision",
        "secondary",
        "http",
        "replay",
        "persistence",
        "missing",
        "unknown",
    ] {
        let mut changed = envelope.clone();
        if mode == "command" {
            changed.command = "jira.issue.update".into();
        }
        let data = changed.data.as_mut().ok_or("completion")?;
        match mode {
            "operation" => data["receipt"]["operation_id"] = json!("foreign-operation"),
            "target" => data["target"]["id"] = json!("999"),
            "resource" => data["receipt"]["confirmed_resources"][0]["id"] = json!("999"),
            "revision" => data["journal_revision"] = json!(record.revision + 1),
            "secondary" => data["secondary"]["watchers"] = json!("unknown"),
            "http" => data["http_status"] = json!(500),
            "replay" => data["replay_authorized"] = json!(true),
            "persistence" => data["persistence"] = json!("uncertain"),
            "missing" => {
                data.as_object_mut().ok_or("object")?.remove("secondary");
            }
            "unknown" => data["unsupported"] = json!(true),
            _ => {}
        }
        assert!(
            ledger::validate_import(&fixture.spec, &changed, &record).is_err(),
            "{mode}"
        );
    }
    let mut uncertain = record;
    uncertain
        .steps
        .get_mut(&fixture.spec.route)
        .ok_or("step")?
        .state = SubmissionState::SendStarted;
    assert!(ledger::validate_import(&fixture.spec, &envelope, &uncertain).is_err());
    Ok(())
}

#[test]
fn reviewed_reuse_records_reject_foreign_origins_and_preserve_unknown_attribution() -> TestResult {
    for mode in [
        "valid",
        "origin",
        "descriptor",
        "project",
        "failed",
        "prepared",
        "confirmed_without_resource",
        "resource",
        "binding",
        "job",
    ] {
        let mut fixture = Fixture::new(Kind::Reuse)?;
        let issue = fixture.resource();
        fixture.spec.route = "reviewed-existing-issue".into();
        fixture.spec.expected_resources = vec![issue.clone()];
        fixture.spec.origin_operation_id = Some("earlier-create".into());
        let mut binding = fixture.binding.clone();
        binding.operation_id = "earlier-create".into();
        let origin = JournalRecord {
            binding,
            revision: 3,
            steps: BTreeMap::from([(
                "create-issue".into(),
                JournalStep {
                    request_digest: Some(fixture.binding.request_digest.clone()),
                    state: SubmissionState::Unknown,
                    confirmed_resources: vec![],
                    job: None,
                },
            )]),
        };
        let mut value =
            json!({"run_digest":fixture.run,"spec":fixture.spec,"issue":issue,"origin":origin});
        match mode {
            "origin" => value["origin"]["binding"]["operation_id"] = json!("different-origin"),
            "descriptor" => {
                value["origin"]["binding"]["descriptor_digest"] =
                    json!(format!("sha256:{}", "3".repeat(64)))
            }
            "project" => value["origin"]["binding"]["target"]["id"] = json!("999"),
            "failed" => value["origin"]["steps"]["create-issue"]["state"] = json!("failed"),
            "prepared" => value["origin"]["steps"]["create-issue"]["state"] = json!("prepared"),
            "confirmed_without_resource" => {
                value["origin"]["steps"]["create-issue"]["state"] = json!("confirmed")
            }
            "resource" => {
                value["origin"]["steps"]["create-issue"]["confirmed_resources"] =
                    json!([ResourceRef {
                        id: "999".into(),
                        ..issue
                    }])
            }
            "binding" => {
                value["spec"]["logical_digest"] = json!(format!("sha256:{}", "3".repeat(64)))
            }
            "job" => {
                value["origin"]["steps"]["create-issue"]["job"] = json!(RemoteJob {
                    resource: ResourceRef {
                        resource_type: "job".into(),
                        id: "job-one".into(),
                        ..fixture.spec.target.clone()
                    },
                    state: JobState::Unknown,
                })
            }
            _ => {}
        }
        ledger::ensure(
            &fixture.artifacts,
            &format!(
                "{}-reuse.json",
                ledger::operation_id(&fixture.run, &fixture.spec.key)?
            ),
            &value,
            "application/vnd.craft.ticket-reuse+json",
            &fixture.source,
        )?;
        let observed = reuse::inspect(
            &fixture.run,
            &fixture.spec,
            &fixture.artifacts,
            &fixture.source,
            &fixture.binding.descriptor_digest,
        );
        if mode == "valid" {
            let observed = observed?;
            assert!(observed.report.complete);
            assert_eq!(observed.report.kind, Kind::Reuse);
            assert!(observed.reuse.ok_or("reuse")?.origin_attribution_unresolved);
            assert!(
                fixture
                    .journal
                    .get(&fixture.binding.operation_id)?
                    .is_none()
            );
        } else {
            assert!(observed.is_err(), "{mode}");
        }
    }
    Ok(())
}

use craft_core::contracts::{GrantRef, JobState, RemoteJob};
use craft_core::storage::{JournalStep, OperationBinding, OperationJournal, SubmissionState};
use craft_test_support::state::{FaultPoint, Faults, MemoryArtifacts, MemoryJournal};
struct Fixture {
    run: String,
    spec: StepSpec,
    binding: OperationBinding,
    source: ResourceRef,
    artifacts: MemoryArtifacts,
    journal: MemoryJournal,
}
impl Fixture {
    fn new(kind: Kind) -> Result<Self, CraftError> {
        let run = format!("sha256:{}", "0".repeat(64));
        let cloud = "00000000-1111-2222-3333-444444444444";
        let source = ResourceRef {
            provider: "confluence".into(),
            deployment: "cloud".into(),
            tenant: cloud.into(),
            resource_type: "page".into(),
            id: "101".into(),
        };
        let target = if kind == Kind::ResultsPage {
            ResourceRef {
                resource_type: "site".into(),
                id: cloud.into(),
                ..source.clone()
            }
        } else {
            ResourceRef {
                provider: "jira".into(),
                resource_type: "project".into(),
                id: "10000".into(),
                ..source.clone()
            }
        };
        let spec = StepSpec {
            origin_operation_id: None,
            key: "issue:api".into(),
            kind,
            target: target.clone(),
            logical_digest: run.clone(),
            route: "native-step".into(),
            expected_resources: vec![],
        };
        let binding = OperationBinding {
            operation_id: ledger::operation_id(&run, &spec.key)?,
            request_digest: format!("sha256:{}", "1".repeat(64)),
            descriptor_digest: format!("sha256:{}", "2".repeat(64)),
            target,
            grant: GrantRef {
                id: format!("gr_{}", "0".repeat(32)),
            },
            actor: "actor-one".into(),
        };
        Ok(Self {
            run,
            spec,
            binding,
            source,
            artifacts: MemoryArtifacts::new(Faults::default(), 64 * 1024 * 1024),
            journal: MemoryJournal::new(Faults::default()),
        })
    }
    fn resource(&self) -> ResourceRef {
        ResourceRef {
            resource_type: if self.spec.kind == Kind::ResultsPage {
                "page"
            } else {
                "issue"
            }
            .into(),
            id: "11001".into(),
            ..self.spec.target.clone()
        }
    }
    fn intent(&self) -> Result<(), CraftError> {
        ledger::prepare_intent(
            &self.run,
            &self.spec,
            &self.binding,
            &self.artifacts,
            &self.source,
        )
    }
    fn finish(&self, state: SubmissionState, job: Option<RemoteJob>) -> TestResult {
        let mut record = self.journal.begin(&self.binding)?;
        for next in [
            SubmissionState::Prepared,
            SubmissionState::SendStarted,
            state,
        ] {
            let previous = record.revision;
            record.revision += 1;
            record.steps.insert(
                self.spec.route.clone(),
                JournalStep {
                    request_digest: if next == SubmissionState::Prepared {
                        None
                    } else {
                        Some(self.binding.request_digest.clone())
                    },
                    state: next,
                    confirmed_resources: if next == SubmissionState::Confirmed {
                        vec![self.resource()]
                    } else {
                        vec![]
                    },
                    job: if next == state { job.clone() } else { None },
                },
            );
            self.journal.compare_and_swap(previous, &record)?;
        }
        Ok(())
    }
    fn outcome(&self) -> Envelope {
        Envelope::success(
            if self.spec.kind == Kind::ResultsPage {
                craft_provider_confluence::pages::PAGE_CREATE
            } else {
                craft_provider_jira::created_issue::COMMAND
            },
            "native".into(),
            json!({"receipt":{"operation_id":self.binding.operation_id,"request_state":"confirmed","confirmed_resources":[self.resource()]},"http_status":201,"reconciliation_required":false}),
        )
    }
    fn inspect(&self) -> Result<Observed, CraftError> {
        ledger::inspect(
            &self.run,
            &self.spec,
            &self.artifacts,
            &self.journal,
            &self.source,
        )
    }
}
#[test]
fn immutable_ticket_intents_refuse_changed_actor_binding_and_source() -> TestResult {
    let fixture = Fixture::new(Kind::Issue)?;
    fixture.intent()?;
    fixture.intent()?;
    let mut changed = fixture.binding.clone();
    changed.actor = "another-actor".into();
    assert!(
        ledger::prepare_intent(
            &fixture.run,
            &fixture.spec,
            &changed,
            &fixture.artifacts,
            &fixture.source
        )
        .is_err()
    );
    let foreign = ResourceRef {
        id: "102".into(),
        ..fixture.source.clone()
    };
    assert!(
        ledger::inspect(
            &fixture.run,
            &fixture.spec,
            &fixture.artifacts,
            &fixture.journal,
            &foreign
        )
        .is_err()
    );
    assert!(
        fixture
            .journal
            .get(&fixture.binding.operation_id)?
            .is_none()
    );
    Ok(())
}
#[test]
fn claimed_unknown_and_refused_ticket_steps_never_become_ready_again() -> TestResult {
    for state in [SubmissionState::Unknown, SubmissionState::Failed] {
        let fixture = Fixture::new(Kind::Issue)?;
        fixture.intent()?;
        fixture.finish(state, None)?;
        let before = fixture.journal.get(&fixture.binding.operation_id)?;
        let observed = fixture.inspect()?;
        assert!(!observed.ready());
        assert!(!observed.report.complete);
        assert_eq!(fixture.journal.get(&fixture.binding.operation_id)?, before);
    }
    Ok(())
}
#[test]
fn confirmed_issue_without_full_completion_preserves_identity_for_reconciliation() -> TestResult {
    let fixture = Fixture::new(Kind::Issue)?;
    fixture.intent()?;
    fixture.finish(SubmissionState::Confirmed, None)?;
    let observed = fixture.inspect()?;
    assert_eq!(observed.report.state, RequestState::Confirmed);
    assert_eq!(observed.report.resources, vec![fixture.resource()]);
    assert!(!observed.report.complete);
    assert!(!observed.ready());
    Ok(())
}
#[test]
fn exact_result_page_confirmation_recovers_without_an_outcome_but_extra_job_blocks() -> TestResult {
    for extra_job in [false, true] {
        let fixture = Fixture::new(Kind::ResultsPage)?;
        fixture.intent()?;
        let job = extra_job.then(|| RemoteJob {
            resource: ResourceRef {
                resource_type: "job".into(),
                id: "remote-job".into(),
                ..fixture.spec.target.clone()
            },
            state: JobState::Running,
        });
        fixture.finish(SubmissionState::Confirmed, job.clone())?;
        let observed = fixture.inspect()?;
        assert_eq!(observed.report.complete, !extra_job);
        assert_eq!(observed.report.resources, vec![fixture.resource()]);
        assert_eq!(
            observed
                .record
                .as_ref()
                .and_then(|r| r.steps.get(&fixture.spec.route))
                .and_then(|s| s.job.clone()),
            job
        );
        assert!(!observed.ready());
    }
    Ok(())
}
#[test]
fn stored_outcome_cannot_restore_a_missing_native_send_claim() -> TestResult {
    let fixture = Fixture::new(Kind::Issue)?;
    fixture.intent()?;
    ledger::save_outcome(
        &fixture.run,
        &fixture.spec,
        &fixture.outcome(),
        &fixture.artifacts,
        &fixture.source,
    )?;
    assert!(fixture.inspect().is_err());
    Ok(())
}
#[test]
fn native_claim_loser_does_not_poison_the_exchange_owners_completion() -> TestResult {
    let fixture = Fixture::new(Kind::Issue)?;
    fixture.intent()?;
    fixture.finish(SubmissionState::Confirmed, None)?;
    let loser = Envelope::failure_with_data(
        craft_provider_jira::created_issue::COMMAND,
        "loser".into(),
        CraftError::new(
            ErrorCode::OperationIdReused,
            "Another invocation owns this step.",
        ),
        json!({"http_status":null}),
    );
    ledger::save_outcome(
        &fixture.run,
        &fixture.spec,
        &loser,
        &fixture.artifacts,
        &fixture.source,
    )?;
    assert!(!fixture.inspect()?.report.complete);
    ledger::save_outcome(
        &fixture.run,
        &fixture.spec,
        &fixture.outcome(),
        &fixture.artifacts,
        &fixture.source,
    )?;
    assert!(fixture.inspect()?.report.complete);
    Ok(())
}
#[test]
fn unknown_artifact_acknowledgment_is_recovered_by_read_without_republication() -> TestResult {
    let fixture = Fixture::new(Kind::Issue)?;
    let faults = Faults::default();
    let artifacts = MemoryArtifacts::new(faults.clone(), 64 * 1024 * 1024);
    faults.fail_on(FaultPoint::ArtifactAfterCommit, 1)?;
    let value = json!({"run":"delivery"});
    let failure = ledger::ensure(
        &artifacts,
        "run.json",
        &value,
        "application/json",
        &fixture.source,
    )
    .err()
    .ok_or("unknown acknowledgment")?;
    assert_eq!(failure.details.write_state, RequestState::SentUnknown);
    ledger::ensure(
        &artifacts,
        "run.json",
        &value,
        "application/json",
        &fixture.source,
    )?;
    assert_eq!(faults.calls(FaultPoint::ArtifactBegin)?, 1);
    Ok(())
}
#[test]
fn failed_post_attempt_inspection_cannot_clear_native_or_record_uncertainty() -> TestResult {
    let mut data = empty(&args());
    data.current_operation_id = Some("active-write".into());
    data.current_attempt = Some(Envelope::failure_with_data(
        "jira.issue.create",
        "request".into(),
        CraftError::new(ErrorCode::RemoteUnavailable, "Lost response."),
        json!({"receipt":{"operation_id":"active-write","request_state":"sent_unknown","confirmed_resources":[]}}),
    ));
    let failure = CraftError::new(
        ErrorCode::Internal,
        "Post-attempt ledger inspection failed.",
    );
    assert!(has_uncertainty(&data, &BTreeMap::new(), &failure));
    let fixture = Fixture::new(Kind::Issue)?;
    fixture.intent()?;
    fixture.finish(SubmissionState::Unknown, None)?;
    data.current_attempt = None;
    data.current_operation_id = Some(fixture.binding.operation_id.clone());
    data.unresolved_record = fixture.journal.get(&fixture.binding.operation_id)?;
    assert!(has_uncertainty(&data, &BTreeMap::new(), &failure));
    Ok(())
}
#[test]
fn current_native_receipt_is_qualified_by_command_and_operation_identity() -> TestResult {
    let mut data = empty(&args());
    data.current_operation_id = Some("owned-write".into());
    for (command, operation, expected) in [
        (
            "jira.issue.create",
            "owned-write",
            Some(RequestState::Failed),
        ),
        ("jira.issue.create", "other-write", None),
        ("local.read", "owned-write", None),
    ] {
        data.current_attempt = Some(Envelope::failure_with_data(
            command,
            "request".into(),
            CraftError::new(ErrorCode::InputInvalid, "Refused."),
            json!({"receipt":{"operation_id":operation,"request_state":"failed","confirmed_resources":[]}}),
        ));
        assert_eq!(current_receipt_state(&data), expected);
    }
    Ok(())
}
