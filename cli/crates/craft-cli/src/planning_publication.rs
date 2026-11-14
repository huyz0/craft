//! Validated visible documents use the same page-write authority and journal as ordinary creation.
use crate::args::DocumentFileArgs;
use crate::confluence_pages::command::{PageCreateArgs, PageWriteArgs};
use crate::dispatch::CommandResult;
use crate::profiles::InvocationOptions;
use clap::Args;
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::registry::{Registry, schema_for};
use craft_provider_confluence::pages::{PAGE_CREATE, PageBody, PagePlacement, PageRepresentation};
use craft_workflows::plans::{DocumentTarget, ReviewState};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

pub const PLAN_CREATE: &str = "plan.page-create";
pub const DESIGN_CREATE: &str = "design.page-create";
// Only the envelope command changes after execution; native receipt bytes stay untouched.
const _: () = assert!(PLAN_CREATE.len() <= PAGE_CREATE.len());
const _: () = assert!(DESIGN_CREATE.len() <= PAGE_CREATE.len());

#[derive(Args, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentPublicationArgs {
    #[command(flatten)]
    #[serde(flatten)]
    pub source: DocumentFileArgs,
    #[arg(long, default_value = "")]
    pub operation_id: String,
    #[arg(long)]
    #[serde(default)]
    pub apply: bool,
}

pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    for (command, file, description) in [
        (
            PLAN_CREATE,
            "plan.json",
            "Publish the complete proposed plan as a shared page; no approval or Jira creation.",
        ),
        (
            DESIGN_CREATE,
            "design.json",
            "Publish the complete validated design as a shared page; no approval or Jira creation.",
        ),
    ] {
        let mut descriptor = registry.descriptor(PAGE_CREATE)?.clone();
        descriptor.id = command.into();
        descriptor.description = description.into();
        descriptor.input_schema = schema_for::<DocumentPublicationArgs>()?;
        descriptor.example = json!({"file":file,"operation_id":"publish-plan","apply":true});
        descriptor.response_example["command"] = json!(command);
        registry.register_typed_handler::<DocumentPublicationArgs>(
            descriptor,
            Arc::new(crate::public_commands::VerifiedHostRequired),
        )?;
    }
    Ok(())
}

fn page_args(
    args: DocumentPublicationArgs,
    target: DocumentTarget,
    title: String,
    storage: String,
) -> PageWriteArgs {
    PageWriteArgs::Create(PageCreateArgs {
        operation_id: args.operation_id,
        apply: args.apply,
        cloud_id: Some(target.confluence_cloud_id),
        space_id: Some(target.space_id),
        title,
        placement: match target.parent_page_id {
            Some(id) => PagePlacement::Parent { id },
            None => PagePlacement::Root,
        },
        body: PageBody {
            representation: PageRepresentation::Storage,
            value: storage,
        },
    })
}

pub fn prepare(
    command: &str,
    args: DocumentPublicationArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> Result<PageWriteArgs, CraftError> {
    if !args.apply {
        return Err(CraftError::new(
            ErrorCode::ApplyRequired,
            "Document publication requires explicit apply.",
        ));
    }
    craft_core::contracts::validate_identity("/operation_id", &args.operation_id)?;
    let (target, title, document) = match command {
        PLAN_CREATE => {
            let plan = crate::planning::load_plan(&args.source, control)?;
            if !matches!(plan.review_state, ReviewState::Proposed) {
                return Err(CraftError::field(
                    "/review_state",
                    "publication_requires_proposed_plan",
                ));
            }
            (
                plan.target.confluence.clone(),
                plan.title.clone(),
                plan.render()?,
            )
        }
        DESIGN_CREATE => {
            let design = crate::planning::load_design(&args.source, control)?;
            (
                design.target.clone(),
                design.title.clone(),
                design.render()?,
            )
        }
        _ => {
            return Err(CraftError::field(
                "/command",
                "expected_document_publication",
            ));
        }
    };
    let storage = document
        .to_storage()
        .map_err(|error| CraftError::field("/document", error.reason))?;
    control.check()?;
    let native = page_args(args, target, title, storage);
    native.validate(fields, options)?;
    Ok(native)
}

pub fn execute(
    command: &str,
    args: DocumentPublicationArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> Result<CommandResult, CraftError> {
    let native = prepare(command, args, fields, options, control)?;
    let result = crate::confluence_pages::command::execute(native, fields, options, control);
    Ok(with_workflow_command(command, result))
}

fn with_workflow_command(command: &str, mut result: CommandResult) -> CommandResult {
    result.envelope.command = command.into();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_core::contracts::{Envelope, RequestState};

    #[test]
    fn publication_contracts_support_profile_state_and_native_receipts()
    -> Result<(), Box<dyn std::error::Error>> {
        let snapshot = crate::contracts::snapshot()?;
        for command in [PLAN_CREATE, DESIGN_CREATE] {
            let operation = &snapshot["operations"][command];
            assert_eq!(operation["controls"]["profile"], "supported");
            assert_eq!(operation["controls"]["state_root"], "supported");
            assert_eq!(operation["descriptor"]["effect"], "write");
            assert_eq!(
                operation["descriptor"]["response_schema"],
                snapshot["operations"][PAGE_CREATE]["descriptor"]["response_schema"]
            );
        }
        Ok(())
    }

    #[test]
    fn workflow_command_preserves_uncertain_receipt_and_output_capacity()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut error = CraftError::new(ErrorCode::WriteOutcomeUnknown, "Keep the known evidence.");
        error.details.write_state = RequestState::SentUnknown;
        let mut envelope = Envelope::failure_with_data(
            PAGE_CREATE,
            "request".into(),
            error,
            json!({"receipt":{"operation_id":"publish-plan","request_state":"sent_unknown","confirmed_resources":[]},"journal":{"binding":{"descriptor_digest":"sha256:native-descriptor"}},"credential_cache_update":{"cleanup_pending":true},"replay_authorized":false}),
        );
        envelope.warnings.push("Retain this receipt.".into());
        let original = serde_json::to_value(&envelope)?;
        let original_size = serde_json::to_vec(&envelope)?.len();
        for command in [PLAN_CREATE, DESIGN_CREATE] {
            let result = with_workflow_command(
                command,
                CommandResult {
                    envelope: serde_json::from_value(original.clone())?,
                    output_policy: crate::output::OutputPolicy::PreserveEvidence,
                },
            );
            assert!(serde_json::to_vec(&result.envelope)?.len() <= original_size);
            let mut changed = serde_json::to_value(result.envelope)?;
            changed["command"] = json!(PAGE_CREATE);
            assert_eq!(changed, original);
        }
        Ok(())
    }
}
