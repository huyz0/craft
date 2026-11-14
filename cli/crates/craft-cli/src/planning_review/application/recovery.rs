//! Restore retained native completion evidence without obtaining execution or approval authority.
use super::*;
use crate::dispatch::CommandResult;
use crate::output::OutputPolicy;
use craft_core::contracts::{EffectClass, SCHEMA_VERSION};
use craft_core::registry::{
    OperationDescriptor, ReconciliationClass, Registry, RetryClass, response_schema, schema_for,
};
use craft_runtime::artifacts::NativeArtifacts;
use craft_runtime::profiles::resolve_state_root;
use std::sync::Arc;

pub const COMMAND: &str = "plan.tickets-record-completion";

#[derive(clap::Args, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecordArgs {
    #[command(flatten)]
    #[serde(flatten)]
    pub run: ApplyArgs,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 64), regex(pattern = "^[A-Za-z0-9._-]+$"))]
    pub task_id: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 4096))]
    pub completion_file: String,
}
impl RecordArgs {
    pub fn validate(
        &self,
        fields: &[String],
        options: &InvocationOptions<'_>,
    ) -> Result<(), CraftError> {
        self.run.validate(fields, options)?;
        if !identifier(&self.task_id)
            || self.completion_file.is_empty()
            || self.completion_file.len() > 4096
            || self.completion_file.chars().any(char::is_control)
        {
            return Err(invalid("invalid_completion_task_or_file"));
        }
        Ok(())
    }
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecordData {
    pub operation_id: Option<String>,
    pub outcome_artifact_name: Option<String>,
    pub outcome_artifact: Option<ArtifactReceipt>,
    pub artifact_write_state: RequestState,
    pub journal: Option<JournalRecord>,
    pub latest_journal: Option<JournalRecord>,
    pub journal_updated: bool,
    pub replay_authorized: bool,
}
fn empty_record() -> RecordData {
    RecordData {
        operation_id: None,
        outcome_artifact_name: None,
        outcome_artifact: None,
        artifact_write_state: RequestState::NotSent,
        journal: None,
        latest_journal: None,
        journal_updated: false,
        replay_authorized: false,
    }
}
fn run_value(args: &ApplyArgs, profile: &str, preview: &Preview) -> Value {
    json!({"format":"craft.ticket-run/v1","profile":profile,"run_id":args.run_id,"review_id":preview.review_id,"preview":preview.receipt,"manifest_digest":preview.manifest_digest,"result_title":args.result_title,"consistency":args.consistency})
}

pub fn record_native(
    args: &RecordArgs,
    profile: &str,
    envelope: &Envelope,
    artifacts: &dyn ArtifactStore,
    journal: &dyn OperationJournal,
) -> (RecordData, Option<CraftError>) {
    let mut data = empty_record();
    let result = (|| {
        args.validate(
            &[],
            &InvocationOptions {
                state_root: None,
                profile: Some(profile),
                request_id: "local-completion-import",
                max_bytes: OUTPUT,
            },
        )?;
        let preview = Preview::load(&args.run.preview_artifact, artifacts)?;
        let value = run_value(&args.run, profile, &preview);
        let run = digest(&value)?;
        let run_name = format!(
            "ticket-run-{}.json",
            &digest(&json!({"profile":profile,"run_id":args.run.run_id}))?[7..]
        );
        let blob = artifacts
            .read(&run_name, SNAPSHOT_BYTES as u64)
            .map_err(local_error)?
            .ok_or_else(|| invalid("existing_ticket_run_required"))?;
        if blob.receipt.media_type != "application/vnd.craft.ticket-run+json"
            || blob.receipt.source.as_ref() != Some(&preview.source)
            || craft_core::input::parse_document_json(&blob.bytes, SNAPSHOT_BYTES)? != value
        {
            return Err(invalid("completion_run_intent_mismatch"));
        }
        let spec = Run::issue_spec(preview.task(&args.task_id)?)?;
        let operation = ledger::operation_id(&run, &spec.key)?;
        data.operation_id = Some(operation.clone());
        data.outcome_artifact_name = Some(format!("{operation}-outcome.json"));
        let observed = ledger::inspect(&run, &spec, artifacts, journal, &preview.source)?;
        data.journal = observed.record;
        let record = data
            .journal
            .as_ref()
            .ok_or_else(|| invalid("confirmed_native_journal_required"))?;
        ledger::validate_import(&spec, envelope, record)?;
        let receipt = ledger::publish_outcome(&run, &spec, envelope, artifacts, &preview.source)?
            .ok_or_else(|| invalid("published_completion_receipt_missing"))?;
        data.outcome_artifact = Some(receipt);
        data.artifact_write_state = RequestState::Confirmed;
        data.latest_journal = journal.get(&operation).map_err(local_error)?;
        if data.latest_journal.as_ref() != Some(record) {
            return Err(CraftError::new(
                ErrorCode::VersionConflict,
                "Journal changed during completion import; retain the published artifact and inspect before resume.",
            ));
        }
        Ok(())
    })();
    if let Err(error) = &result
        && data.outcome_artifact.is_none()
    {
        data.artifact_write_state = error.details.write_state;
    }
    (data, result.err())
}

pub fn execute(
    args: RecordArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &craft_core::control::InvocationControl,
) -> CommandResult {
    let result = (|| {
        args.validate(fields, options)?;
        control.check()?;
        let root = std::env::current_dir().map_err(|_| invalid("cannot_resolve_project_root"))?;
        let bytes = craft_runtime::inputs::read_project_file(
            &root,
            std::path::Path::new(&args.completion_file),
            OUTPUT,
            control,
        )?;
        let value = craft_core::input::parse_document_json(&bytes, OUTPUT)?;
        let envelope: Envelope =
            serde_json::from_value(value).map_err(|_| invalid("invalid_completion_envelope"))?;
        control.check()?;
        let root = resolve_state_root(options.state_root, |key| std::env::var_os(key))?;
        let store = ProfileStore::open(&root, control)?;
        let artifacts = NativeArtifacts::open(
            &store.path().join("reviews"),
            SNAPSHOT_BYTES as u64,
            control,
        )
        .map_err(local_error)?;
        let journal = NativeJournal::open(&store, control).map_err(local_error)?;
        Ok(record_native(
            &args,
            options
                .profile
                .ok_or_else(|| invalid("explicit_ticket_profile_required"))?,
            &envelope,
            &artifacts,
            &journal,
        ))
    })();
    let (data, error) = result.unwrap_or_else(|e| (empty_record(), Some(e)));
    CommandResult {
        envelope: match error {
            Some(error) => {
                Envelope::failure_with_data(COMMAND, options.request_id.into(), error, json!(data))
            }
            None => Envelope::success(COMMAND, options.request_id.into(), json!(data)),
        },
        output_policy: OutputPolicy::PreserveEvidence,
    }
}

pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    registry.register_typed_handler::<RecordArgs>(OperationDescriptor {
        id: COMMAND.into(), provider: "craft".into(), contract_version: SCHEMA_VERSION.into(),
        api_revision: "reviewed-ticket-completion-import-v1".into(),
        description: "Restore a retained confirmed issue completion into the same local ticket run without provider access or replay.".into(),
        effect: EffectClass::Write, input_schema: schema_for::<RecordArgs>()?, response_schema: response_schema::<RecordData>()?,
        example: json!({"run_id":"approved-delivery","preview_artifact":format!("tickets-{}.json","0".repeat(64)),"result_title":"Delivery ticket results","consistency":"revalidate_each_batch","apply":true,"task_id":"implement-payment","completion_file":"retained-completion.json"}),
        response_example: json!(Envelope::success(COMMAND, "example".into(), json!(empty_record()))),
        projection: None, scope_alternatives: vec![], permissions: vec!["Explicit local apply and exact existing run/native confirmed evidence.".into()],
        tenant_conditions: vec!["Imports supplied evidence, not provider authenticity, approval or unknown-write resolution.".into()],
        routes: vec![], retry_class: RetryClass::ReconcileBeforeReplay, reconciliation: ReconciliationClass::KnownResource,
    }, Arc::new(crate::public_commands::VerifiedHostRequired))?;
    Ok(())
}
