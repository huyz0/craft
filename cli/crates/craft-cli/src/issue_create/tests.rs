use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use craft_core::contracts::{AuthMode, CanonicalRequest, GrantRef, Selection};
use craft_core::control::{CancellationToken, InvocationControl};
use craft_core::execution::{
    HttpMethod, NativeCall, NativeRoute, OperationContext, OperationHandler, PreparedOperation,
    RouteSegment,
};
use craft_core::input::content_digest;
use craft_core::profiles::ProfileInput;
use craft_core::registry::{OperationDescriptor, ReconciliationClass, Registry, RetryClass};
use craft_core::storage::OperationJournal;
use craft_runtime::control::SystemClock;
use craft_runtime::journal::{MutationAuthority, NativeJournal, SendClaim};
use craft_runtime::profiles::ProfileStore;

use super::*;

#[test]
fn already_recorded_effects_and_jobs_survive_failed_persistence_or_conflicting_refusal()
-> Result<(), Box<dyn std::error::Error>> {
    let binding = binding();
    let resource = ResourceRef {
        resource_type: "issue".into(),
        id: "10002".into(),
        ..target()
    };
    let mut prior = receipt_record(
        &binding,
        &StepOutcome {
            state: SubmissionState::Unknown,
            confirmed_resources: vec![resource.clone()],
            job: None,
        },
    )?;
    prior.steps.get_mut(ROUTE).ok_or("Missing step")?.job =
        Some(craft_core::contracts::RemoteJob {
            resource: ResourceRef {
                resource_type: "job".into(),
                id: "job-one".into(),
                ..target()
            },
            state: craft_core::contracts::JobState::Running,
        });
    for status in [400, 502] {
        let response = NativeResponse {
            status,
            body: Value::Null,
            request_id: None,
            retry_after: None,
            link_headers: None,
        };
        let mut view = view(&binding, Ok(&response));
        view.prior = Some(&prior);
        let result = CreateCompletion::prepare(binding.clone(), "request", &[], 16_384)?
            .finish_with(view, |_| Err(StoreError::Unavailable));
        let data = result
            .envelope
            .data
            .as_ref()
            .ok_or("Missing prior evidence")?;
        assert_eq!(data["receipt"]["confirmed_resources"][0]["id"], "10002");
        assert_eq!(data["receipt"]["job"]["state"], "running");
        assert_eq!(data["receipt"]["request_state"], "sent_unknown");
        assert_eq!(data["reconciliation_required"], true);
    }
    let response = response(json!({"id":"10001"}));
    let result = CreateCompletion::prepare(binding.clone(), "request", &[], 16_384)?.finish_with(
        view(&binding, Ok(&response)),
        |outcome| {
            let mut saved = receipt_record(&binding, outcome)?;
            let step = saved.steps.get_mut(ROUTE).ok_or(StoreError::Corrupt)?;
            step.confirmed_resources.push(resource);
            step.job = prior.steps[ROUTE].job.clone();
            Ok(saved)
        },
    );
    assert_eq!(
        result.envelope.error.as_ref().map(|error| error.code),
        Some(ErrorCode::PartialApply)
    );
    let data = result
        .envelope
        .data
        .as_ref()
        .ok_or("Missing additional evidence")?;
    assert_eq!(
        data["receipt"]["confirmed_resources"]
            .as_array()
            .ok_or("Missing resources")?
            .len(),
        2
    );
    assert_eq!(data["receipt"]["job"]["resource"]["id"], "job-one");
    Ok(())
}

struct Fixture {
    parent: PathBuf,
    root: PathBuf,
}
impl Fixture {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        fs::create_dir_all(&parent)?;
        let parent = parent.canonicalize()?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let root = parent.join(format!("create-completion-{}-{stamp}", std::process::id()));
        Ok(Self { parent, root })
    }
    fn claim(
        &self,
    ) -> Result<(NativeJournal, SendClaim, CancellationToken), Box<dyn std::error::Error>> {
        let token = CancellationToken::default();
        let control =
            InvocationControl::new(Arc::new(SystemClock::default()), token.clone(), 10_000)?;
        let profiles = ProfileStore::open(&self.root, &control)?;
        let profile = profiles.create(
            &ProfileInput {
                name: "team".into(),
                provider: "jira".into(),
                deployment: "cloud".into(),
                auth_mode: AuthMode::Delegated,
                grant: None,
                tenant: Some(target().tenant),
                defaults: BTreeMap::new(),
            },
            &control,
            |_| Ok(()),
        )?;
        let journal = NativeJournal::open(&profiles, &control)?;
        let mut registry = Registry::default();
        registry.register_typed_handler::<Value>(
            OperationDescriptor {
                id: COMMAND.into(),
                provider: "jira".into(),
                contract_version: "1.0".into(),
                api_revision: "fixture".into(),
                description: "Owned create completion fixture.".into(),
                effect: craft_core::contracts::EffectClass::Write,
                input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
                response_schema: response_schema::<Value>()?,
                example: json!({}),
                response_example: json!(Envelope::success(COMMAND, "example".into(), json!({}))),
                projection: None,
                scope_alternatives: vec![vec!["write:jira-work".into()]],
                permissions: vec!["create".into()],
                tenant_conditions: vec![],
                routes: vec![NativeRoute {
                    repeated_query_parameters: Vec::new(),
                    id: ROUTE.into(),
                    method: HttpMethod::Post,
                    path: vec![
                        RouteSegment::Literal("rest".into()),
                        RouteSegment::Literal("api".into()),
                        RouteSegment::Literal("3".into()),
                        RouteSegment::Literal("issue".into()),
                    ],
                    query_parameters: vec![],
                    continuation_parameters: vec![],
                    response_max_bytes: 16_384,
                }],
                retry_class: RetryClass::ReconcileBeforeReplay,
                reconciliation: ReconciliationClass::Correlation,
            },
            Arc::new(Handler),
        )?;
        let operation = registry.prepare(CanonicalRequest {
            command: COMMAND.into(),
            target: target(),
            input: json!({}),
            selection: Selection::default(),
            preconditions: None,
        })?;
        let claim = journal.claim_send(
            &registry,
            &operation,
            &MutationAuthority {
                apply: true,
                operation_id: "publish-summary",
                grant: &profile.grant,
                actor: "account-one",
            },
            ROUTE,
            &NativeCall {
                repeated_query: std::collections::BTreeMap::new(),
                route: ROUTE.into(),
                path_parameters: BTreeMap::new(),
                query: BTreeMap::new(),
                body: Some(json!({"fields":{"summary":"synthetic"}})),
            },
        )?;
        Ok((journal, claim, token))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(root) = self.root.canonicalize()
            && root.starts_with(&self.parent)
            && root != self.parent
        {
            let _ = fs::remove_dir_all(root);
        }
    }
}
struct Handler;
impl OperationHandler for Handler {
    fn validate(&self, _: &CanonicalRequest) -> Result<(), CraftError> {
        Ok(())
    }
    fn execute(
        &self,
        _: &PreparedOperation,
        _: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        Err(CraftError::new(
            ErrorCode::UnsupportedCapability,
            "Fixture does not send.",
        ))
    }
}
fn target() -> ResourceRef {
    ResourceRef {
        provider: "jira".into(),
        deployment: "cloud".into(),
        tenant: "00000000-1111-2222-3333-444444444444".into(),
        resource_type: "project".into(),
        id: "10000".into(),
    }
}
fn binding() -> OperationBinding {
    OperationBinding {
        operation_id: "publish-summary".into(),
        request_digest: content_digest(b"logical"),
        descriptor_digest: content_digest(b"descriptor"),
        target: target(),
        grant: GrantRef {
            id: "gr_00000000000000000000000000000000".into(),
        },
        actor: "account-one".into(),
    }
}
fn response(body: Value) -> NativeResponse {
    NativeResponse {
        status: 201,
        body,
        request_id: Some("trace-one".into()),
        retry_after: None,
        link_headers: None,
    }
}
fn view<'a>(
    binding: &'a OperationBinding,
    response: Result<&'a NativeResponse, &'a CraftError>,
) -> AttemptView<'a> {
    AttemptView {
        binding,
        command: COMMAND,
        route: ROUTE,
        step: ROUTE,
        response,
        pending_failed: false,
        prior: None,
    }
}
fn receipt_record(
    binding: &OperationBinding,
    outcome: &StepOutcome,
) -> Result<JournalRecord, StoreError> {
    let mut record = JournalRecord::initial(binding.clone())?;
    record.revision = 1;
    record.steps.insert(
        ROUTE.into(),
        craft_core::storage::JournalStep {
            request_digest: Some(content_digest(b"native")),
            state: outcome.state,
            confirmed_resources: outcome.confirmed_resources.clone(),
            job: None,
        },
    );
    Ok(record)
}

#[test]
fn native_journal_keeps_created_issue_through_partial_response_and_cancelled_completion()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let (journal, claim, token) = fixture.claim()?;
    let binding = journal
        .get("publish-summary")?
        .ok_or("Missing binding")?
        .binding;
    let completion = CreateCompletion::prepare(binding.clone(), "original-request", &[], 16_384)?;
    let response = response(
        json!({"id":"10001","key":"ED-24","transition":{"status":400,"errorCollection":{"errorMessages":["raw-secret-sentinel"]}}}),
    );
    token.cancel();
    let result = completion.finish_with(view(&binding, Ok(&response)), |outcome| {
        claim.record_outcome(outcome)
    });
    assert!(!result.envelope.ok);
    assert_eq!(
        result.envelope.error.as_ref().map(|e| e.code),
        Some(ErrorCode::PartialApply)
    );
    let encoded = crate::output::encode(result.envelope, 16_384, result.output_policy)?;
    let body: Value = serde_json::from_slice(&encoded.bytes)?;
    assert_eq!(body["request_id"], "original-request");
    assert_eq!(
        body["data"]["receipt"]["confirmed_resources"][0]["id"],
        "10001"
    );
    assert_eq!(body["data"]["persistence"], "recorded");
    assert_eq!(body["data"]["replay_authorized"], false);
    assert!(!String::from_utf8_lossy(&encoded.bytes).contains("raw-secret-sentinel"));
    assert!(jsonschema::validator_for(&schema()?)?.is_valid(&body));
    Ok(())
}

#[test]
fn persistence_failures_and_lost_acknowledgment_keep_remote_evidence()
-> Result<(), Box<dyn std::error::Error>> {
    for storage in [StoreError::Unavailable, StoreError::OutcomeUnknown] {
        let binding = binding();
        let response = response(json!({"id":"10001","key":42}));
        let result = CreateCompletion::prepare(binding.clone(), "request", &[], 16_384)?
            .finish_with(view(&binding, Ok(&response)), |_| Err(storage));
        let data = result
            .envelope
            .data
            .as_ref()
            .ok_or("Missing recovery data")?;
        assert_eq!(data["receipt"]["confirmed_resources"][0]["id"], "10001");
        assert_eq!(data["receipt"]["request_state"], "confirmed");
        assert_eq!(
            data["persistence"],
            if storage == StoreError::OutcomeUnknown {
                "uncertain"
            } else {
                "unrecorded"
            }
        );
        assert_eq!(data["reconciliation_required"], true);
        assert!(data["journal_revision"].is_null());
        assert_eq!(
            result
                .envelope
                .error
                .as_ref()
                .map(|e| e.details.write_state),
            Some(RequestState::Confirmed)
        );
    }
    Ok(())
}

#[test]
fn projection_preserves_receipt_and_budget_failure_cannot_discard_observed_ids()
-> Result<(), Box<dyn std::error::Error>> {
    let binding = binding();
    let fields = vec!["issue_key".into()];
    let response = response(json!({"id":"10001","key":"ED-24"}));
    assert_eq!(
        CreateCompletion::prepare(binding.clone(), "request", &fields, 512)
            .err()
            .ok_or("Expected preflight refusal")?
            .code,
        ErrorCode::OutputBudgetTooSmall
    );
    assert_eq!(
        CreateCompletion::prepare(binding.clone(), "request", &["invented".into()], 16_384)
            .err()
            .ok_or("Expected field refusal")?
            .code,
        ErrorCode::FieldUnknown
    );
    let result = CreateCompletion::prepare(binding.clone(), "request", &fields, 16_384)?
        .finish_with(view(&binding, Ok(&response)), |outcome| {
            receipt_record(&binding, outcome)
        });
    let full = json!(result.envelope);
    assert!(
        jsonschema::validator_for(
            &projection()
                .compile(&schema()?, &fields)?
                .response_schema(&schema()?)?
        )?
        .is_valid(&full)
    );
    assert_eq!(
        full["data"]["receipt"]["confirmed_resources"][0]["id"],
        "10001"
    );
    assert!(full["data"].get("http_status").is_none());
    let encoded = crate::output::encode(result.envelope, 512, result.output_policy)?;
    let retained: Value = serde_json::from_slice(&encoded.bytes)?;
    assert_eq!(
        retained["data"]["receipt"]["confirmed_resources"][0]["id"],
        "10001"
    );
    assert!(
        !retained["warnings"]
            .as_array()
            .ok_or("Missing warnings")?
            .is_empty()
    );
    Ok(())
}

#[test]
fn completion_mismatch_and_invalid_record_never_claim_the_wrong_receipt_was_saved()
-> Result<(), Box<dyn std::error::Error>> {
    let binding = binding();
    let response = response(json!({"id":"10001"}));
    let mut other = binding.clone();
    other.operation_id = "other-operation".into();
    let result = CreateCompletion::prepare(other.clone(), "request", &[], 16_384)?
        .finish_with(view(&binding, Ok(&response)), |outcome| {
            receipt_record(&binding, outcome)
        });
    assert_eq!(
        result.envelope.error.as_ref().map(|e| e.code),
        Some(ErrorCode::PolicyDenied)
    );
    assert_eq!(
        result.envelope.data.as_ref().ok_or("Missing evidence")?["receipt"]["operation_id"],
        binding.operation_id
    );
    let result = CreateCompletion::prepare(binding.clone(), "request", &[], 16_384)?
        .finish_with(view(&binding, Ok(&response)), |outcome| {
            receipt_record(&other, outcome)
        });
    let data = result.envelope.data.as_ref().ok_or("Missing evidence")?;
    assert_eq!(data["persistence"], "unrecorded");
    assert_eq!(data["receipt"]["confirmed_resources"][0]["id"], "10001");
    assert!(!result.envelope.ok);
    Ok(())
}

#[test]
fn unknown_exchange_and_rate_advice_retain_recovery_without_raw_bodies()
-> Result<(), Box<dyn std::error::Error>> {
    let binding = binding();
    let error = CraftError::new(ErrorCode::Cancelled, "transport-sentinel");
    let result = CreateCompletion::prepare(binding.clone(), "request", &[], 16_384)?
        .finish_with(view(&binding, Err(&error)), |outcome| {
            receipt_record(&binding, outcome)
        });
    assert_eq!(
        result
            .envelope
            .error
            .as_ref()
            .map(|e| e.details.write_state),
        Some(RequestState::SentUnknown)
    );
    assert!(!serde_json::to_string(&result.envelope)?.contains("transport-sentinel"));
    let response = NativeResponse {
        status: 429,
        body: json!({"private":"body-sentinel"}),
        request_id: Some("trace-one".into()),
        retry_after: Some(RetryAfter::DelaySeconds(7)),
        link_headers: None,
    };
    let result = CreateCompletion::prepare(binding.clone(), "request", &[], 16_384)?
        .finish_with(view(&binding, Ok(&response)), |outcome| {
            receipt_record(&binding, outcome)
        });
    assert_eq!(
        result.envelope.data.as_ref().ok_or("Missing recovery")?["retry_after"]["value"],
        7
    );
    assert!(!serde_json::to_string(&result.envelope)?.contains("body-sentinel"));
    Ok(())
}
