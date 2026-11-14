//! Immutable canonical source data. Stored requests never supply current authority or replay rights.
use crate::dispatch::CommandResult;
use craft_core::contracts::{CanonicalRequest, RequestState};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::input::{content_digest, parse_document_json};
use craft_core::projection::ProjectionContract;
use craft_core::registry::response_schema;
use craft_core::storage::{
    ArtifactSpec, ArtifactStore, JournalRecord, OperationBinding, OperationJournal, StoreError,
    SubmissionState,
};
use craft_runtime::artifacts::NativeArtifacts;
use craft_runtime::journal::NativeJournal;
use craft_runtime::profiles::ProfileStore;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::Write;

const LIMIT: u64 = 2_097_152;
const RECEIPT_RESERVE: usize = 512;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SavedReceipt {
    #[schemars(
        length(min = 77, max = 77),
        regex(pattern = "^request-[0-9a-f]{64}\\.json$")
    )]
    pub name: String,
    #[schemars(length(min = 71, max = 71), regex(pattern = "^sha256:[0-9a-f]{64}$"))]
    pub digest: String,
    pub write_state: RequestState,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PublicData<T> {
    #[serde(flatten)]
    pub native: T,
    pub saved_request: Option<SavedReceipt>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedRequest {
    pub format: u8,
    pub binding: OperationBinding,
    pub original: CanonicalRequest,
}
pub(crate) struct CaptureFailure {
    pub error: CraftError,
    pub receipt: Option<SavedReceipt>,
    pub prior: Option<JournalRecord>,
    pub evidence_unavailable: bool,
}
fn invalid(message: &str) -> CraftError {
    let mut error = CraftError::new(ErrorCode::Internal, message);
    error.next_action = "Inspect the same operation and saved request under the same state root. Source data grants no replay authority.".into();
    error
}
fn name(operation_id: &str) -> String {
    format!(
        "request-{}.json",
        &content_digest(operation_id.as_bytes())[7..]
    )
}
fn artifacts(
    store: &ProfileStore,
    control: &InvocationControl,
) -> Result<NativeArtifacts, CraftError> {
    NativeArtifacts::open(&store.path().join("requests"), LIMIT, control).map_err(|_| {
        invalid("Saved request storage could not be opened; no new issue request was sent.")
    })
}
impl SavedRequest {
    fn validate(&self, operation_id: &str) -> Result<(), CraftError> {
        if self.format != 1 || self.binding.operation_id != operation_id {
            return Err(invalid(
                "Saved request format or operation identity is invalid.",
            ));
        }
        let prepared = if self.original.command == craft_provider_jira::created_issue::COMMAND {
            crate::issue_create::host::prepare_original(self.original.clone())?
        } else if self.original.command == craft_provider_jira::attachment_upload::UPLOAD {
            let mut registry = craft_core::registry::Registry::default();
            craft_provider_jira::attachment_upload::register(&mut registry)?;
            registry.prepare(self.original.clone())?
        } else {
            crate::issue_mutation::host::prepare_original(self.original.clone())?
        };
        if prepared.mutation_binding(operation_id, &self.binding.grant, &self.binding.actor)?
            != self.binding
        {
            return Err(invalid(
                "Saved canonical request differs from its native binding.",
            ));
        }
        Ok(())
    }
}
fn read_from(
    artifacts: &dyn ArtifactStore,
    operation_id: &str,
) -> Result<Option<(SavedRequest, SavedReceipt)>, CraftError> {
    let blob = artifacts.read(&name(operation_id), LIMIT).map_err(|_| {
        invalid("Saved request is unavailable or corrupt; absence was not established.")
    })?;
    let Some(blob) = blob else {
        return Ok(None);
    };
    let value = parse_document_json(&blob.bytes, LIMIT as usize)
        .map_err(|_| invalid("Saved request JSON is invalid."))?;
    let saved: SavedRequest =
        serde_json::from_value(value).map_err(|_| invalid("Saved request shape is invalid."))?;
    saved.validate(operation_id)?;
    if blob.receipt.name != name(operation_id)
        || blob.receipt.media_type != "application/json"
        || blob.receipt.source.as_ref() != Some(&saved.binding.target)
    {
        return Err(invalid(
            "Saved request receipt does not identify its canonical source.",
        ));
    }
    Ok(Some((
        saved,
        SavedReceipt {
            name: blob.receipt.name,
            digest: blob.receipt.digest,
            write_state: RequestState::Confirmed,
        },
    )))
}
pub(crate) fn inspect(
    store: &ProfileStore,
    operation_id: &str,
    control: &InvocationControl,
) -> Result<Option<(SavedRequest, SavedReceipt)>, CraftError> {
    craft_core::contracts::validate_identity("/operation_id", operation_id)?;
    read_from(&artifacts(store, control)?, operation_id)
}
pub(crate) fn load(
    store: &ProfileStore,
    operation_id: &str,
    control: &InvocationControl,
) -> Result<SavedRequest, CraftError> {
    craft_core::contracts::validate_identity("/operation_id", operation_id)?;
    read_from(&artifacts(store, control)?, operation_id)?.map(|(saved, _)| saved)
        .ok_or_else(|| invalid("No saved source exists for this operation; retain or supply its exact original request."))
}
pub(crate) fn capture(
    store: &ProfileStore,
    journal: &NativeJournal,
    original: &CanonicalRequest,
    binding: &OperationBinding,
    step: &str,
    control: &InvocationControl,
) -> Result<Option<SavedReceipt>, Box<CaptureFailure>> {
    let journal = journal.for_invocation(control);
    capture_using(&journal, original, binding, step, control, || {
        Ok(Box::new(artifacts(store, control)?) as Box<dyn ArtifactStore>)
    })
}
fn capture_using(
    journal: &dyn OperationJournal,
    original: &CanonicalRequest,
    binding: &OperationBinding,
    step: &str,
    control: &InvocationControl,
    storage: impl FnOnce() -> Result<Box<dyn ArtifactStore>, CraftError>,
) -> Result<Option<SavedReceipt>, Box<CaptureFailure>> {
    let mut receipt = None;
    let mut prior = None;
    let result = (|| {
        control.check()?;
        prior = journal
            .get(&binding.operation_id)
            .map_err(|_| invalid("Cannot inspect the original journal before source capture."))?;
        if prior.as_ref().is_some_and(|record| {
            record.binding != *binding
                || record
                    .steps
                    .iter()
                    .any(|(id, value)| id != step || value.state != SubmissionState::Prepared)
        }) {
            return Ok(None);
        }
        let store = storage()?;
        if let Some((saved, observed)) = read_from(store.as_ref(), &binding.operation_id)? {
            if saved.binding != *binding {
                return Err(CraftError::new(
                    ErrorCode::OperationIdReused,
                    "Operation identity already has a different saved source.",
                ));
            }
            return Ok(Some(observed));
        }
        let saved = SavedRequest {
            format: 1,
            binding: binding.clone(),
            original: original.clone(),
        };
        saved.validate(&binding.operation_id)?;
        let bytes =
            serde_json::to_vec(&saved).map_err(|_| invalid("Cannot encode saved request."))?;
        if bytes.len() as u64 > LIMIT {
            return Err(CraftError::field("/input", "saved_request_exceeds_bound"));
        }
        let proposed = SavedReceipt {
            name: name(&binding.operation_id),
            digest: content_digest(&bytes),
            write_state: RequestState::SentUnknown,
        };
        let mut writer = match store.begin(ArtifactSpec {
            name: proposed.name.clone(),
            media_type: "application/json".into(),
            max_bytes: LIMIT,
            source: Some(binding.target.clone()),
        }) {
            Ok(writer) => writer,
            Err(StoreError::Conflict) => {
                let (winner, observed) = read_from(store.as_ref(), &binding.operation_id)?
                    .ok_or_else(|| invalid("Competing source publication is unresolved."))?;
                if winner.binding != *binding {
                    return Err(CraftError::new(
                        ErrorCode::OperationIdReused,
                        "Competing source uses another binding.",
                    ));
                }
                return Ok(Some(observed));
            }
            Err(StoreError::OutcomeUnknown) => {
                receipt = Some(proposed);
                return Err(invalid(
                    "Saved request staging is uncertain; no new issue request was sent.",
                ));
            }
            Err(_) => {
                return Err(invalid(
                    "Saved request staging failed; no new issue request was sent.",
                ));
            }
        };
        receipt = Some(proposed.clone());
        writer.write_all(&bytes).map_err(|_| {
            invalid("Saved request staging was interrupted; no new issue request was sent.")
        })?;
        let committed = writer.finish().map_err(|_| invalid("Saved request publication was not confirmed; inspect it before another attempt. No new issue request was sent."))?;
        if committed.digest != proposed.digest || committed.bytes != bytes.len() as u64 {
            return Err(invalid(
                "Saved request receipt differs from the intended source.",
            ));
        }
        Ok(Some(SavedReceipt {
            write_state: RequestState::Confirmed,
            ..proposed
        }))
    })();
    result.map_err(|error| {
        let evidence_unavailable = match journal.get(&binding.operation_id) {
            Ok(Some(latest)) => {
                let foreign = latest.binding != *binding;
                prior = Some(latest);
                foreign
            }
            Ok(None) => prior.is_some(),
            Err(_) => true,
        };
        Box::new(CaptureFailure {
            error,
            receipt,
            prior,
            evidence_unavailable,
        })
    })
}
pub(crate) fn public_schema<T: JsonSchema>() -> Result<Value, CraftError> {
    response_schema::<PublicData<T>>()
}
pub(crate) fn projection(mut native: ProjectionContract) -> ProjectionContract {
    native.always_returned.push("saved_request".into());
    native.default_selected.push("saved_request".into());
    native
}
pub(crate) fn native_fields<T: JsonSchema>(
    native: ProjectionContract,
    fields: &[String],
) -> Result<Vec<String>, CraftError> {
    if !fields
        .iter()
        .any(|field| field == "saved_request" || field.starts_with("saved_request."))
    {
        return Ok(fields.to_vec());
    }
    let always = native.always_returned.clone();
    projection(native).compile(&public_schema::<T>()?, fields)?;
    let selected: Vec<_> = fields
        .iter()
        .filter(|field| field.as_str() != "saved_request" && !field.starts_with("saved_request."))
        .cloned()
        .collect();
    Ok(if selected.is_empty() {
        always
    } else {
        selected
    })
}
pub(crate) fn native_budget(maximum: usize) -> Result<usize, CraftError> {
    maximum
        .checked_sub(RECEIPT_RESERVE)
        .filter(|value| *value >= 512)
        .ok_or_else(|| {
            CraftError::new(
                ErrorCode::OutputBudgetTooSmall,
                "Increase max-bytes to retain native and saved-source evidence.",
            )
        })
}
pub(crate) fn failure_prior(
    journal: &NativeJournal,
    binding: &OperationBinding,
    control: &InvocationControl,
) -> (Option<JournalRecord>, bool) {
    match journal.for_invocation(control).get(&binding.operation_id) {
        Ok(Some(record)) => {
            let foreign = record.binding != *binding;
            (Some(record), foreign)
        }
        Ok(None) => (None, false),
        Err(_) => (None, true),
    }
}
pub(crate) fn attach(mut result: CommandResult, receipt: Option<SavedReceipt>) -> CommandResult {
    if let Some(Value::Object(data)) = result.envelope.data.as_mut() {
        data.insert("saved_request".into(), json!(receipt));
    }
    result
}
pub(crate) fn attach_failure(
    mut result: CommandResult,
    receipt: Option<SavedReceipt>,
    evidence_unavailable: bool,
) -> CommandResult {
    if evidence_unavailable && let Some(Value::Object(data)) = result.envelope.data.as_mut() {
        data.insert("reconciliation_required".into(), json!(true));
        data.insert("persistence".into(), json!("uncertain"));
    }
    attach(result, receipt)
}

#[cfg(test)]
mod tests;
