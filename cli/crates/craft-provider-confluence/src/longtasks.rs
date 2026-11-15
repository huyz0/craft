//! Confluence long-running task status. Polling reads a recorded handle; it never submits work.
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

pub const COMMAND: &str = "confluence.longtask.get";
pub const RESOURCE_TYPE: &str = "longtask";
const ROUTE: &str = "confluence-longtask-get";
const RESPONSE_MAX: u32 = 262_144;

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LongTaskInput {
    #[schemars(regex(pattern = "^[A-Za-z0-9][A-Za-z0-9-]{0,127}$"))]
    pub task_id: String,
}

/// Long-task identifiers are opaque native strings; only path-safe forms are accepted.
pub fn valid_task_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && !id.starts_with('-')
}

fn malformed() -> CraftError {
    CraftError::new(
        ErrorCode::RemoteUnavailable,
        "Confluence task status is malformed; the recorded job state was not changed.",
    )
}

/// `finished` and `successful` decide the outcome. Messages and task names are never returned.
pub fn observe(raw: &Value, task_id: &str) -> Result<JobObservation, CraftError> {
    if !raw.is_object() {
        return Err(malformed());
    }
    if raw.get("id").is_some_and(|id| id.as_str() != Some(task_id)) {
        return Err(malformed());
    }
    let finished = raw["finished"].as_bool().ok_or_else(malformed)?;
    let (state, native_status) = if finished {
        match raw["successful"].as_bool() {
            Some(true) => (JobState::Succeeded, "finished_successful"),
            Some(false) => (JobState::Failed, "finished_unsuccessful"),
            None => return Err(malformed()),
        }
    } else {
        (JobState::Running, "running")
    };
    let progress_percent = match raw.get("percentageComplete") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_u64()
                .filter(|percent| *percent <= 100)
                .and_then(|percent| u8::try_from(percent).ok())
                .ok_or_else(malformed)?,
        ),
    };
    let elapsed_ms = match raw.get("elapsedTime") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.as_u64().ok_or_else(malformed)?),
    };
    Ok(JobObservation {
        state,
        native_status: native_status.into(),
        progress_percent,
        elapsed_ms,
    })
}

pub fn validate(request: &CanonicalRequest) -> Result<(), CraftError> {
    crate::registration::validate_target(&request.target)?;
    if request.command != COMMAND
        || request.preconditions.is_some()
        || request.selection.cursor.is_some()
    {
        return Err(CraftError::field(
            "/request",
            "expected_confluence_task_status_read",
        ));
    }
    let input: LongTaskInput = serde_json::from_value(request.input.clone())
        .map_err(|_| CraftError::field("/input", "invalid_confluence_task_input"))?;
    if !valid_task_id(&input.task_id) {
        return Err(CraftError::field(
            "/task_id",
            "expected_path_safe_confluence_task_id",
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
        native_status: "running".into(),
        progress_percent: Some(40),
        elapsed_ms: Some(1200),
    };
    registry.register_typed_handler::<LongTaskInput>(
        OperationDescriptor {
            id: COMMAND.into(),
            provider: "confluence".into(),
            contract_version: SCHEMA_VERSION.into(),
            api_revision: "confluence-v1-longtask-status-v1".into(),
            description:
                "Read the status of a recorded Confluence long-running task without its messages."
                    .into(),
            effect: EffectClass::Read,
            input_schema: schema_for::<LongTaskInput>()?,
            response_schema: response_schema::<JobObservation>()?,
            example: json!({"task_id":"5e2c4b1a-7d3f-4a2b-9c1d-0e8f7a6b5c4d"}),
            response_example: json!(Envelope::success(COMMAND, "example".into(), json!(example))),
            projection: None,
            scope_alternatives: vec![
                vec!["read:confluence-space.summary".into()],
                vec!["read:content.metadata:confluence".into()],
            ],
            permissions: vec!["Confluence returns task status to permitted users.".into()],
            tenant_conditions: vec![
                "Tasks expire; a missing task is not evidence of failure.".into(),
            ],
            routes: vec![NativeRoute {
                id: ROUTE.into(),
                method: HttpMethod::Get,
                path: vec![
                    RouteSegment::Literal("wiki".into()),
                    RouteSegment::Literal("rest".into()),
                    RouteSegment::Literal("api".into()),
                    RouteSegment::Literal("longtask".into()),
                    RouteSegment::Parameter("id".into()),
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

/// One status request for the prepared handle; the caller interprets status codes.
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
        path_parameters: BTreeMap::from([("id".into(), task_id.into())]),
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
    fn finished_and_successful_decide_the_outcome() -> TestResult {
        let running = observe(
            &json!({"id":"t-1","finished":false,"percentageComplete":30,"elapsedTime":9,"messages":[{"translation":"private-sentinel"}]}),
            "t-1",
        )?;
        assert_eq!(running.state, JobState::Running);
        assert_eq!(running.progress_percent, Some(30));
        assert!(!serde_json::to_string(&running)?.contains("private-sentinel"));
        let done = observe(&json!({"finished":true,"successful":true}), "t-1")?;
        assert_eq!(done.state, JobState::Succeeded);
        let failed = observe(&json!({"finished":true,"successful":false}), "t-1")?;
        assert_eq!(failed.state, JobState::Failed);
        Ok(())
    }

    #[test]
    fn ambiguous_task_evidence_is_malformed() {
        for raw in [
            json!({"finished":true}),
            json!({"successful":true}),
            json!({"id":"t-2","finished":false}),
            json!({"finished":false,"percentageComplete":101}),
            json!([]),
        ] {
            assert_eq!(
                observe(&raw, "t-1").err().map(|error| error.code),
                Some(ErrorCode::RemoteUnavailable)
            );
        }
        assert!(!valid_task_id("../x"));
        assert!(!valid_task_id("-x"));
        assert!(valid_task_id("5e2c4b1a-7d3f"));
    }
}
