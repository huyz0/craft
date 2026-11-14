use super::*;
use crate::dispatch::CommandResult;
use crate::output::OutputPolicy;
use craft_core::contracts::{EffectClass, SCHEMA_VERSION};
use craft_core::registry::{
    OperationDescriptor, ReconciliationClass, Registry, RetryClass, response_schema, schema_for,
};
use craft_runtime::artifacts::NativeArtifacts;
use craft_runtime::auth::ServiceOAuthClient;
use craft_runtime::control::SystemClock;
use craft_runtime::profiles::resolve_state_root;
use craft_runtime::secrets::NativeCredentials;
use craft_runtime::transport::TlsPolicy;
use std::sync::Arc;

pub fn execute(
    command: &str,
    args: ApplyArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> CommandResult {
    let fallback = empty(&args);
    let result = (|| {
        if ![CREATE, RESUME].contains(&command) {
            return Err(invalid("expected_ticket_application_command"));
        }
        args.validate(fields, options)?;
        let root = resolve_state_root(options.state_root, |key| std::env::var_os(key))?;
        let store = ProfileStore::open(&root, control)?;
        let profile = options
            .profile
            .ok_or_else(|| invalid("explicit_ticket_profile_required"))?;
        let artifacts = NativeArtifacts::open(
            &store.path().join("reviews"),
            SNAPSHOT_BYTES as u64,
            control,
        )
        .map_err(local_error)?;
        let journal = NativeJournal::open(&store, control).map_err(local_error)?;
        let credentials = NativeCredentials::open(&store)?;
        let client = ServiceOAuthClient::new(
            TlsPolicy::Platform,
            control.clone(),
            Arc::new(SystemClock::default()),
        )?;
        Ok(apply_native(
            args,
            command == RESUME,
            profile,
            options.request_id,
            BindingEnvironment {
                store: &store,
                credentials: &credentials,
                client: &client,
                tls: TlsPolicy::Platform,
                control: control.clone(),
            },
            &artifacts,
            &journal,
        ))
    })();
    let (data, error) = result.unwrap_or_else(|error| (fallback, Some(error)));
    CommandResult {
        envelope: match error {
            Some(error) => {
                Envelope::failure_with_data(command, options.request_id.into(), error, json!(data))
            }
            None => Envelope::success(command, options.request_id.into(), json!(data)),
        },
        output_policy: OutputPolicy::PreserveEvidence,
    }
}
pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    for command in [CREATE, RESUME] {
        let example = json!({"run_id":"approved-delivery","preview_artifact":format!("tickets-{}.json","0".repeat(64)),"result_title":"Delivery ticket results","consistency":"revalidate_each_batch","apply":true});
        let args: ApplyArgs = serde_json::from_value(example.clone())
            .map_err(|_| invalid("invalid_ticket_application_example"))?;
        registry.register_typed_handler::<ApplyArgs>(OperationDescriptor {id:command.into(),provider:"craft".into(),contract_version:SCHEMA_VERSION.into(),api_revision:"reviewed-ticket-execution-v1".into(),description:if command==CREATE {"Apply an immutable reviewed ticket preview with durable issue/link mappings and independent result publication."} else {"Resume the same reviewed ticket run without replaying confirmed, refused or unknown native claims."}.into(),effect:EffectClass::Write,input_schema:schema_for::<ApplyArgs>()?,response_schema:response_schema::<ExecutionData>()?,example,response_example:json!(Envelope::success(command,"example".into(),json!(empty(&args)))),projection:None,scope_alternatives:vec![],permissions:vec!["Explicit review, issue creation, endpoint linking and result-page creation policies; native provider permissions remain required.".into()],tenant_conditions:vec!["Original preview approvals are revalidated per unfinished batch; unknown native steps require reconciliation and never replay.".into()],routes:vec![],retry_class:RetryClass::ReconcileBeforeReplay,reconciliation:ReconciliationClass::KnownResource},Arc::new(crate::public_commands::VerifiedHostRequired))?;
    }
    super::recovery::register(registry)
}
