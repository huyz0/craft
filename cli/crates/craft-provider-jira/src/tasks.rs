//! Jira asynchronous task status. Polling reads a recorded handle; it never submits work.
use std::collections::BTreeMap;
use std::sync::Arc;

use craft_core::contracts::{
    CanonicalRequest, EffectClass, Envelope, JobObservation, JobState, SCHEMA_VERSION,
};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{
    HttpMethod, NativeCall, NativeResponse, NativeRoute, OperationContext, OperationHandler,
    PreparedOperation, RouteSegment,
};
use craft_core::registry::{
    OperationDescriptor, ReconciliationClass, Registry, RetryClass, response_schema, schema_for,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const COMMAND: &str = "jira.task.get";
pub const RESOURCE_TYPE: &str = "task";
const ROUTE: &str = "jira-task-get";
const RESPONSE_MAX: u32 = 65_536;

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskInput {
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub task_id: String,
}

pub fn valid_task_id(id: &str) -> bool {
    crate::comments::numeric(id)
}

fn malformed() -> CraftError {
    CraftError::new(
        ErrorCode::RemoteUnavailable,
        "Jira task status is malformed; the recorded job state was not changed.",
    )
}

/// Maps a documented task status. Cancellation is a failed outcome with its native status kept.
pub fn observe(raw: &Value, task_id: &str) -> Result<JobObservation, CraftError> {
    if !raw.is_object() || raw.get("errorMessages").is_some() || raw.get("errors").is_some() {
        return Err(malformed());
    }
    let id = match &raw["id"] {
        Value::String(id) => id.clone(),
        Value::Number(id) => id.to_string(),
        _ => return Err(malformed()),
    };
    if id != task_id {
        return Err(malformed());
    }
    let status = raw["status"].as_str().ok_or_else(malformed)?;
    let state = match status {
        "ENQUEUED" => JobState::Accepted,
        "RUNNING" | "CANCEL_REQUESTED" => JobState::Running,
        "COMPLETE" => JobState::Succeeded,
        "FAILED" | "CANCELLED" | "DEAD" => JobState::Failed,
        _ => return Err(malformed()),
    };
    let progress_percent = match raw.get("progress") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_u64()
                .filter(|percent| *percent <= 100)
                .and_then(|percent| u8::try_from(percent).ok())
                .ok_or_else(malformed)?,
        ),
    };
    let elapsed_ms = match raw.get("elapsedRuntime") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.as_u64().ok_or_else(malformed)?),
    };
    Ok(JobObservation {
        state,
        native_status: status.into(),
        progress_percent,
        elapsed_ms,
    })
}

pub fn validate(request: &CanonicalRequest) -> Result<(), CraftError> {
    crate::projects::validate_target(&request.target)?;
    if request.command != COMMAND
        || request.preconditions.is_some()
        || request.selection.cursor.is_some()
    {
        return Err(CraftError::field(
            "/request",
            "expected_jira_task_status_read",
        ));
    }
    let input: TaskInput = serde_json::from_value(request.input.clone())
        .map_err(|_| CraftError::field("/input", "invalid_jira_task_input"))?;
    if !valid_task_id(&input.task_id) {
        return Err(CraftError::field(
            "/task_id",
            "expected_numeric_jira_task_id",
        ));
    }
    Ok(())
}

struct Handler;
impl OperationHandler for Handler {
    fn validate(&self, request: &CanonicalRequest) -> Result<(), CraftError> {
        validate(request)
    }
    fn execute(
        &self,
        _: &PreparedOperation,
        _: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "Task status reads require the verified job host.",
        ))
    }
}

pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    let example = JobObservation {
        state: JobState::Running,
        native_status: "RUNNING".into(),
        progress_percent: Some(40),
        elapsed_ms: Some(1200),
    };
    registry.register_typed_handler::<TaskInput>(
        OperationDescriptor {
            id: COMMAND.into(),
            provider: "jira".into(),
            contract_version: SCHEMA_VERSION.into(),
            api_revision: "jira-v3-task-status-v1".into(),
            description:
                "Read the status of a recorded Jira asynchronous task without its result text."
                    .into(),
            effect: EffectClass::Read,
            input_schema: schema_for::<TaskInput>()?,
            response_schema: response_schema::<JobObservation>()?,
            example: json!({"task_id":"10641"}),
            response_example: json!(Envelope::success(COMMAND, "example".into(), json!(example))),
            projection: None,
            scope_alternatives: vec![vec!["read:jira-work".into()]],
            permissions: vec![
                "Jira returns task status to the submitting user or an administrator.".into(),
            ],
            tenant_conditions: vec![
                "Tasks expire; a missing task is not evidence of failure.".into(),
            ],
            routes: vec![NativeRoute {
                id: ROUTE.into(),
                method: HttpMethod::Get,
                path: vec![
                    RouteSegment::Literal("rest".into()),
                    RouteSegment::Literal("api".into()),
                    RouteSegment::Literal("3".into()),
                    RouteSegment::Literal("task".into()),
                    RouteSegment::Parameter("taskId".into()),
                ],
                query_parameters: vec![],
                continuation_parameters: vec![],
                repeated_query_parameters: vec![],
                response_max_bytes: RESPONSE_MAX,
            }],
            retry_class: RetryClass::SafeRead,
            reconciliation: ReconciliationClass::NotApplicable,
        },
        Arc::new(Handler),
    )
}

/// One status request for the prepared handle. Interpretation is left to the caller so that
/// 404 and rate advice stay distinct from malformed evidence.
pub fn read_status(
    operation: &PreparedOperation,
    context: &mut dyn OperationContext,
) -> Result<NativeResponse, CraftError> {
    validate(operation.request())?;
    context.verify_operation(operation)?;
    let task_id = operation.request().input["task_id"]
        .as_str()
        .ok_or_else(malformed)?;
    let call = NativeCall {
        route: ROUTE.into(),
        path_parameters: BTreeMap::from([("taskId".into(), task_id.into())]),
        query: BTreeMap::new(),
        repeated_query: BTreeMap::new(),
        body: None,
    };
    context.checkpoint()?;
    let response = context.invoke(&call)?;
    context.checkpoint()?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn documented_statuses_map_without_claiming_completion_early() -> TestResult {
        for (status, state) in [
            ("ENQUEUED", JobState::Accepted),
            ("RUNNING", JobState::Running),
            ("CANCEL_REQUESTED", JobState::Running),
            ("COMPLETE", JobState::Succeeded),
            ("FAILED", JobState::Failed),
            ("CANCELLED", JobState::Failed),
            ("DEAD", JobState::Failed),
        ] {
            let observed = observe(
                &json!({"id":"7","status":status,"progress":10,"elapsedRuntime":5,"result":"private-sentinel","message":"private-sentinel"}),
                "7",
            )?;
            assert_eq!(observed.state, state);
            assert_eq!(observed.native_status, status);
            assert!(!serde_json::to_string(&observed)?.contains("private-sentinel"));
        }
        Ok(())
    }

    #[test]
    fn unknown_or_mismatched_task_evidence_is_malformed() {
        for raw in [
            json!({"id":"7","status":"DONE"}),
            json!({"id":"8","status":"COMPLETE"}),
            json!({"id":"7"}),
            json!({"id":"7","status":"RUNNING","progress":101}),
            json!({"id":"7","status":"RUNNING","elapsedRuntime":-1}),
            json!({"errorMessages":["x"]}),
            json!([]),
        ] {
            assert_eq!(
                observe(&raw, "7").err().map(|error| error.code),
                Some(ErrorCode::RemoteUnavailable)
            );
        }
    }
}
