use craft_core::contracts::{JobState, RemoteJob, ResourceRef, validate_identity};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::projection::CompiledProjection;
use craft_core::storage::{
    JournalRecord, OperationBinding, OperationJournal, StoreError, SubmissionState,
};
use craft_runtime::journal::NativeJournal;
use craft_runtime::profiles::{ProfileStore, resolve_state_root};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::args::{OperationAction, OperationGetArgs};
use crate::dispatch::DispatchOutput;
use crate::profiles::InvocationOptions;

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationStepData {
    pub name: String,
    pub request_digest: Option<String>,
    pub state: SubmissionState,
    pub confirmed_resources: Vec<ResourceRef>,
    pub job: Option<RemoteJob>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationData {
    pub operation_id: String,
    pub found: bool,
    pub binding: Option<OperationBinding>,
    pub revision: Option<u64>,
    pub steps: Vec<OperationStepData>,
    /// Null means this installation has no record from which to infer reconciliation needs.
    pub reconciliation_required: Option<bool>,
    #[schemars(extend("const" = "local_installation"))]
    pub evidence_scope: String,
    #[schemars(extend("const" = false))]
    pub live_verified: bool,
    #[schemars(extend("const" = false))]
    pub replay_authorized: bool,
}

impl OperationData {
    fn from_record(operation_id: String, record: Option<JournalRecord>) -> Self {
        let reconciliation_required = record.as_ref().map(|record| {
            record.steps.values().any(|step| {
                step.state.requires_reconciliation()
                    || step
                        .job
                        .as_ref()
                        .is_some_and(|job| job.state == JobState::Unknown)
            })
        });
        let (binding, revision, steps) = match record {
            Some(record) => (
                Some(record.binding),
                Some(record.revision),
                record
                    .steps
                    .into_iter()
                    .map(|(name, step)| OperationStepData {
                        name,
                        request_digest: step.request_digest,
                        state: step.state,
                        confirmed_resources: step.confirmed_resources,
                        job: step.job,
                    })
                    .collect(),
            ),
            None => (None, None, Vec::new()),
        };
        Self {
            operation_id,
            found: binding.is_some(),
            binding,
            revision,
            steps,
            reconciliation_required,
            evidence_scope: "local_installation".into(),
            live_verified: false,
            replay_authorized: false,
        }
    }
}

pub fn validate_input(command: &str, input: &Value) -> Result<(), CraftError> {
    if command == "operation.get" {
        let arguments: OperationGetArgs = serde_json::from_value(input.clone())
            .map_err(|_| CraftError::field("/operation_id", "invalid_operation_input"))?;
        validate_identity("/operation_id", &arguments.operation_id)?;
    }
    Ok(())
}

fn read_error(error: StoreError) -> CraftError {
    let code = match error {
        StoreError::Cancelled => ErrorCode::Cancelled,
        StoreError::DeadlineExceeded => ErrorCode::DeadlineExceeded,
        _ => ErrorCode::Internal,
    };
    let mut error = CraftError::new(code, "Local operation evidence could not be read.");
    error.next_action = "Inspect the selected state root and retain existing recovery evidence; an unreadable record does not authorize replay.".into();
    error
}

/// Reads validated local evidence without loading a grant, verifying remote state or issuing sends.
pub fn execute(
    action: OperationAction,
    projection: Option<CompiledProjection>,
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> Result<DispatchOutput, CraftError> {
    let OperationAction::Get(arguments) = action else {
        return Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "Recovery candidates require complete command-host dispatch.",
        ));
    };
    validate_identity("/operation_id", &arguments.operation_id)?;
    if options.profile.is_some() {
        return Err(CraftError::field(
            "/profile",
            "profile_selection_not_applicable",
        ));
    }
    let path = resolve_state_root(options.state_root, |key| std::env::var_os(key))?;
    let store = ProfileStore::open(&path, control)?;
    let journal = NativeJournal::open(&store, control).map_err(read_error)?;
    let record = journal.get(&arguments.operation_id).map_err(read_error)?;
    let data = serde_json::to_value(OperationData::from_record(arguments.operation_id, record))
        .map_err(|_| {
            CraftError::new(
                ErrorCode::Internal,
                "Cannot encode local operation evidence.",
            )
        })?;
    let data = match projection {
        Some(projection) => projection.data(&data)?,
        None => data,
    };
    Ok(DispatchOutput { data, page: None })
}
