//! `operation wait` arguments and production wiring.
use std::sync::Arc;
use std::time::Duration;

use clap::Args;
use craft_core::contracts::{AuthMode, EffectClass, Envelope, SCHEMA_VERSION, validate_identity};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::profiles::validate_profile_name;
use craft_core::registry::{
    OperationDescriptor, ReconciliationClass, Registry, RetryClass, response_schema, schema_for,
};
use craft_core::storage::{OperationJournal, StoreError};
use craft_runtime::control::SystemClock;
use craft_runtime::journal::NativeJournal;
use craft_runtime::profiles::{ProfileStore, resolve_state_root};
use craft_runtime::secrets::NativeCredentials;
use craft_runtime::transport::TlsPolicy;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::*;
use crate::profiles::{InvocationOptions, selected_profile_context};

/// Upper bound for one wait; the invocation timeout still applies.
pub const MAX_WAIT_MS: u64 = 3_600_000;
/// The response carries no projected collections, so a fixed minimum always fits.
const MIN_OUTPUT_BYTES: usize = 4_096;

#[derive(Args, Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationWaitArgs {
    #[arg(
        long,
        default_value = "",
        help = "Operation whose recorded job handle to poll"
    )]
    #[schemars(length(min = 1, max = 512))]
    pub operation_id: String,
    #[arg(
        long,
        help = "Step holding the job; needed only when several steps have jobs"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 512))]
    pub step: Option<String>,
    #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u64).range(0..=MAX_WAIT_MS), help = "How long to keep polling; 0 reads the status once")]
    #[serde(default)]
    #[schemars(range(max = 3_600_000))]
    pub wait_ms: u64,
}

impl OperationWaitArgs {
    pub fn validate(
        &self,
        fields: &[String],
        options: &InvocationOptions<'_>,
    ) -> Result<(), CraftError> {
        validate_identity("/operation_id", &self.operation_id)?;
        if let Some(step) = &self.step {
            validate_identity("/step", step)?;
        }
        if self.wait_ms > MAX_WAIT_MS {
            return Err(CraftError::field("/wait_ms", "wait_out_of_range"));
        }
        if !fields.is_empty() {
            return Err(CraftError::field("/fields", "projection_not_supported"));
        }
        if let Some(profile) = options.profile {
            validate_profile_name(profile)?;
        }
        if options.max_bytes < MIN_OUTPUT_BYTES {
            let mut error = CraftError::new(
                ErrorCode::OutputBudgetTooSmall,
                "Job wait output needs room for its complete recovery evidence.",
            );
            error.next_action = format!("Use --max-bytes {MIN_OUTPUT_BYTES} or more.");
            return Err(error);
        }
        Ok(())
    }
}

pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    let example = json!({"operation_id":"delete-space","found":true,"step":"submit","submission_state":"confirmed","job":{"resource":{"provider":"confluence","deployment":"cloud","tenant":"00000000-1111-2222-3333-444444444444","resource_type":"longtask","id":"5e2c4b1a"},"state":"running"},"observation":{"state":"running","native_status":"running","progress_percent":40,"elapsed_ms":1200},"polls":1,"stop_reason":"wait_elapsed","retry_after":null,"journal_current":true,"reconciliation_required":false,"credential_cache_update":null,"replay_authorized":false});
    registry.register_typed_handler::<OperationWaitArgs>(
        OperationDescriptor {
            id: COMMAND.into(),
            provider: "craft".into(),
            contract_version: SCHEMA_VERSION.into(),
            api_revision: "recorded-job-status-v1".into(),
            description: "Poll a recorded provider job handle within a bounded wait and record each state change; never submits or resubmits work.".into(),
            effect: EffectClass::Read,
            input_schema: schema_for::<OperationWaitArgs>()?,
            response_schema: response_schema::<WaitData>()?,
            example: json!({"operation_id":"delete-space","wait_ms":30000}),
            response_example: json!(Envelope::success(COMMAND, "example".into(), example)),
            projection: None,
            scope_alternatives: vec![
                vec!["read:jira-work".into()],
                vec!["read:confluence-space.summary".into()],
                vec!["read:content.metadata:confluence".into()],
            ],
            permissions: vec!["The selected service profile must hold the grant and actor that recorded the job.".into()],
            tenant_conditions: vec!["Only the job's own site is read, through the provider's task-status GET.".into()],
            routes: vec![],
            retry_class: RetryClass::SafeRead,
            reconciliation: ReconciliationClass::NotApplicable,
        },
        Arc::new(crate::public_commands::VerifiedHostRequired),
    )
}

fn read_error(error: StoreError) -> CraftError {
    let code = match error {
        StoreError::Cancelled => ErrorCode::Cancelled,
        StoreError::DeadlineExceeded => ErrorCode::DeadlineExceeded,
        _ => ErrorCode::Internal,
    };
    let mut error = CraftError::new(code, "Local operation evidence could not be read.");
    error.next_action =
        "Inspect the selected state root; an unreadable record does not authorize replay.".into();
    error
}

/// Loads local evidence first; credentials are only opened for a pollable recorded handle.
pub fn execute(
    args: OperationWaitArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> CommandResult {
    let result = (|| -> Result<CommandResult, CraftError> {
        args.validate(fields, options)?;
        let root = resolve_state_root(options.state_root, |key| std::env::var_os(key))?;
        let store = ProfileStore::open(&root, control)?;
        let journal = NativeJournal::open(&store, control).map_err(read_error)?;
        let record = journal.get(&args.operation_id).map_err(read_error)?;
        let PollTarget {
            binding,
            step,
            job,
            submission,
        } = match select(&args.operation_id, args.step.as_deref(), record)? {
            Selected::Local(data) => return Ok(local(*data, options.request_id)),
            Selected::Poll(target) => *target,
        };
        let (name, _, _) = selected_profile_context(&store, None, options, control)?;
        let profile = store.get(&name, control)?;
        if profile.provider != job.resource.provider
            || profile.deployment != job.resource.deployment
            || profile.auth_mode != AuthMode::Service
            || profile.tenant.as_deref() != Some(job.resource.tenant.as_str())
        {
            return Err(denied(
                "Job polling needs a service profile for the job's own provider site.",
            ));
        }
        let credentials = NativeCredentials::open(&store)?;
        let locked = credentials.lock(&profile.grant, control)?;
        let client = ServiceOAuthClient::new(
            TlsPolicy::Platform,
            control.clone(),
            Arc::new(SystemClock::default()),
        )?;
        Ok(poll(
            PollRequest {
                binding,
                step,
                job,
                submission,
                grant: profile.grant.clone(),
                wait: Duration::from_millis(args.wait_ms),
                request_id: options.request_id.into(),
            },
            WaitEnvironment {
                journal: &journal,
                client: &client,
                locked: &locked,
                tls: TlsPolicy::Platform,
                control: control.clone(),
                pause: &pause,
            },
        ))
    })();
    result.unwrap_or_else(|error| CommandResult {
        envelope: Envelope::failure(COMMAND, options.request_id.into(), error),
        output_policy: OutputPolicy::PreserveEvidence,
    })
}
