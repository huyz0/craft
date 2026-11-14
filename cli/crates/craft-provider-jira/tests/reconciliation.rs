use craft_core::contracts::{CanonicalRequest, ResourceRef, Selection};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{NativeCall, NativeResponse, OperationContext};
use craft_core::registry::Registry;
use craft_core::storage::Clock;
use craft_provider_jira::reconciliation::{self, JiraReconciliation, Visibility};
use serde_json::{Value, json};
use std::collections::VecDeque;
type TestResult = Result<(), Box<dyn std::error::Error>>;
fn target() -> ResourceRef {
    ResourceRef {
        provider: "jira".into(),
        deployment: "cloud".into(),
        tenant: "00000000-1111-2222-3333-444444444444".into(),
        resource_type: "project".into(),
        id: "100".into(),
    }
}
fn request() -> CanonicalRequest {
    CanonicalRequest {
        command: reconciliation::COMMAND.into(),
        target: target(),
        input: json!({"issue_ids":["10000","10001"]}),
        selection: Selection::default(),
        preconditions: None,
    }
}
fn registry() -> Result<Registry, CraftError> {
    let mut registry = Registry::default();
    registry.register_provider(&JiraReconciliation)?;
    Ok(registry)
}
fn state(id: &str) -> Value {
    json!({"id":id,"key":"EX-1","fields":{"project":{"id":"100"},"issuetype":{"id":"101"},"status":{"id":"102"},"updated":"2026-11-10T01:00:00.000+0000","assignee":null}})
}
struct FixedClock;
impl Clock for FixedClock {
    fn unix_millis(&self) -> u64 {
        1_000
    }
    fn monotonic_millis(&self) -> u64 {
        1_000
    }
}
struct Context {
    responses: VecDeque<NativeResponse>,
    calls: Vec<NativeCall>,
}
impl Context {
    fn new(values: Vec<(u16, Value)>) -> Self {
        Self {
            responses: values
                .into_iter()
                .map(|(status, body)| NativeResponse {
                    status,
                    body,
                    request_id: None,
                    retry_after: None,
                    link_headers: None,
                })
                .collect(),
            calls: vec![],
        }
    }
}
impl OperationContext for Context {
    fn checkpoint(&self) -> Result<(), CraftError> {
        Ok(())
    }
    fn invoke(&mut self, call: &NativeCall) -> Result<NativeResponse, CraftError> {
        self.calls.push(call.clone());
        self.responses
            .pop_front()
            .ok_or_else(|| CraftError::new(ErrorCode::Internal, "Unexpected inspection request."))
    }
}
#[test]
fn inspection_uses_narrow_exact_reads_and_keeps_404_ambiguous() -> TestResult {
    let registry = registry()?;
    let operation = registry.prepare(request())?;
    let mut context = Context::new(vec![
        (200, state("10000")),
        (404, json!({"message":"private text"})),
    ]);
    let batch = reconciliation::inspect(&operation, &mut context, &FixedClock);
    assert!(batch.error.is_none());
    assert_eq!(batch.observations.len(), 2);
    assert_eq!(batch.observations[0].visibility, Visibility::Visible);
    assert_eq!(
        batch.observations[1].visibility,
        Visibility::AbsentOrInvisible
    );
    assert!(batch.observations[1].state.is_none());
    for call in &context.calls {
        assert_eq!(
            call.query["fields"],
            "project,issuetype,status,updated,assignee"
        );
        assert_eq!(call.query["updateHistory"], "false");
        assert!(call.body.is_none());
    }
    assert_eq!(context.calls[1].path_parameters["issueIdOrKey"], "10001");
    Ok(())
}
#[test]
fn malformed_foreign_and_wrong_identity_reads_preserve_prior_qualified_observations() -> TestResult
{
    let registry = registry()?;
    let operation = registry.prepare(request())?;
    for change in ["project", "id", "assignee", "warnings", "oversized"] {
        let mut bad = state("10001");
        match change {
            "project" => bad["fields"]["project"]["id"] = json!("999"),
            "id" => bad["id"] = json!("10002"),
            "assignee" => {
                bad["fields"]
                    .as_object_mut()
                    .ok_or("fields")?
                    .remove("assignee");
            }
            "warnings" => bad["warningMessages"] = json!(["private text"]),
            _ => bad["ignored"] = json!("x".repeat(131_072)),
        }
        let mut context = Context::new(vec![(200, state("10000")), (200, bad)]);
        let batch = reconciliation::inspect(&operation, &mut context, &FixedClock);
        assert_eq!(batch.observations.len(), 1);
        assert!(batch.error.is_some());
        assert_eq!(
            batch.error.as_ref().ok_or("error")?.code,
            if change == "project" {
                ErrorCode::PolicyDenied
            } else {
                ErrorCode::RemoteUnavailable
            }
        );
    }
    Ok(())
}
#[test]
fn inspection_inputs_are_bounded_numeric_unique_and_nonpaginated() -> TestResult {
    let registry = registry()?;
    for ids in [
        json!([]),
        json!(["10000", "10000"]),
        json!(["EX-1"]),
        json!((10000..10021).map(|i| i.to_string()).collect::<Vec<_>>()),
    ] {
        let mut bad = request();
        bad.input["issue_ids"] = ids;
        assert!(registry.prepare(bad).is_err());
    }
    for field in ["query", "jql", "url", "original"] {
        let mut bad = request();
        bad.input[field] = json!("inert");
        assert!(registry.prepare(bad).is_err());
    }
    let mut bad = request();
    bad.selection.cursor = Some("cursor".into());
    assert!(registry.prepare(bad).is_err());
    let mut bad = request();
    bad.target.id = "200".into();
    bad.input["project_id"] = json!("100");
    assert!(registry.prepare(bad).is_err());
    Ok(())
}
#[test]
fn partial_rate_failure_is_not_retried_or_interpreted_as_absence() -> TestResult {
    let registry = registry()?;
    let operation = registry.prepare(request())?;
    let mut context = Context::new(vec![
        (200, state("10000")),
        (429, json!({"message":"private text"})),
    ]);
    let batch = reconciliation::inspect(&operation, &mut context, &FixedClock);
    assert_eq!(context.calls.len(), 2);
    assert_eq!(batch.observations.len(), 1);
    assert_eq!(batch.http_status, Some(429));
    assert_eq!(
        batch.error.as_ref().ok_or("error")?.code,
        ErrorCode::RateLimited
    );
    assert!(!serde_json::to_string(batch.error.as_ref().ok_or("error")?)?.contains("private text"));
    Ok(())
}
