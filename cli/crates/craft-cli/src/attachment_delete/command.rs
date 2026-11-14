//! Strict deletion inputs reserve full recovery capacity before opening private state.
use super::*;
use crate::dispatch::CommandResult;
use crate::output::OutputPolicy;
use crate::profiles::InvocationOptions;
use craft_core::contracts::{Envelope, validate_identity};
use craft_core::registry::{response_schema, schema_for};
use craft_runtime::control::SystemClock;
use craft_runtime::profiles::resolve_state_root;
use craft_runtime::secrets::NativeCredentials;
use native::DeleteInput;
use serde_json::json;
use std::sync::Arc;

fn parse_ownership(text: &str) -> Result<Ownership, String> {
    match text {
        "own" => Ok(Ownership::Own),
        "any" => Ok(Ownership::Any),
        _ => Err("expected_own_or_any".into()),
    }
}
#[derive(clap::Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttachmentDeleteArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 512))]
    pub operation_id: String,
    #[arg(long)]
    #[serde(default)]
    pub apply: bool,
    #[arg(long, default_value = "")]
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub project_id: String,
    #[arg(long, default_value = "")]
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub issue_id: String,
    #[arg(long, default_value = "")]
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub attachment_id: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 71, max = 71), regex(pattern = "^sha256:[0-9a-f]{64}$"))]
    pub expected_revision: String,
    #[arg(long,value_parser=parse_ownership,default_value="own")]
    #[serde(default)]
    pub ownership: Ownership,
}
impl AttachmentDeleteArgs {
    fn request(&self, cloud: &str, max: usize) -> Result<CanonicalRequest, CraftError> {
        Ok(CanonicalRequest {
            command: native::DELETE.into(),
            target: ResourceRef {
                provider: "jira".into(),
                deployment: "cloud".into(),
                tenant: cloud.into(),
                resource_type: "project".into(),
                id: self.project_id.clone(),
            },
            input: json!(DeleteInput {
                issue_id: self.issue_id.clone(),
                attachment_id: self.attachment_id.clone(),
                expected_revision: self.expected_revision.clone(),
                ownership: self.ownership,
            }),
            selection: Selection {
                max_bytes: u32::try_from(max).map_err(|_| {
                    CraftError::field("/max_bytes", "attachment_budget_out_of_range")
                })?,
                ..Selection::default()
            },
            preconditions: None,
        })
    }
    pub fn validate(
        &self,
        fields: &[String],
        options: &InvocationOptions<'_>,
    ) -> Result<(), CraftError> {
        validate_identity("/operation_id", &self.operation_id)?;
        validate_identity("/request_id", options.request_id)?;
        craft_core::profiles::validate_profile_name(options.profile.ok_or_else(|| {
            CraftError::field("/profile", "explicit_attachment_profile_required")
        })?)?;
        if !fields.is_empty() || options.request_id.len() > 80 {
            return Err(CraftError::field(
                "/output",
                "complete_attachment_receipts_required",
            ));
        }
        if !(MINIMUM_OUTPUT..=16_777_216).contains(&options.max_bytes) {
            return Err(CraftError::new(
                ErrorCode::OutputBudgetTooSmall,
                "Reserve --max-bytes 8388608 for complete deletion recovery receipts; normal output is compact.",
            ));
        }
        if !self.apply {
            return Err(CraftError::new(
                ErrorCode::ApplyRequired,
                "Attachment deletion requires explicit apply.",
            ));
        }
        registry()?
            .prepare(self.request("00000000-1111-2222-3333-444444444444", options.max_bytes)?)?;
        Ok(())
    }
}
pub fn execute(
    args: AttachmentDeleteArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> CommandResult {
    let result = (|| {
        args.validate(fields, options)?;
        let root = resolve_state_root(options.state_root, |key| std::env::var_os(key))?;
        let store = ProfileStore::open(&root, control)?;
        let profile = options
            .profile
            .ok_or_else(|| CraftError::field("/profile", "explicit_attachment_profile_required"))?;
        let tenant = store.get(profile, control)?.tenant_ref()?;
        let prepared = PreparedAttachmentDelete::prepare(
            args.request(&tenant.tenant, options.max_bytes)?,
            &args.operation_id,
            args.apply,
            profile,
            &store,
            control,
        )?;
        let journal = NativeJournal::open(&store, control).map_err(storage)?;
        let credentials = NativeCredentials::open(&store)?;
        let locked = credentials.lock(prepared.grant(), control)?;
        let client = ServiceOAuthClient::new(
            TlsPolicy::Platform,
            control.clone(),
            Arc::new(SystemClock::default()),
        )?;
        Ok(prepared.execute(
            AttachmentDeleteEnvironment {
                store: &store,
                client: &client,
                locked: &locked,
                journal: &journal,
                tls: TlsPolicy::Platform,
                control: control.clone(),
            },
            || Ok(()),
        ))
    })();
    let request_id = options.request_id.to_owned();
    let envelope = match result {
        Ok((data, Some(error))) => {
            Envelope::failure_with_data(native::DELETE, request_id, error, json!(data))
        }
        Ok((data, None)) => Envelope::success(native::DELETE, request_id, json!(data)),
        Err(error) => Envelope::failure(native::DELETE, request_id, error),
    };
    CommandResult {
        envelope,
        output_policy: OutputPolicy::PreserveEvidence,
    }
}
pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    let internal = super::registry()?;
    let mut descriptor = internal.descriptor(native::DELETE)?.clone();
    descriptor.input_schema = schema_for::<AttachmentDeleteArgs>()?;
    descriptor.response_schema = response_schema::<AttachmentDeleteData>()?;
    descriptor.scope_alternatives = vec![vec!["read:jira-work".into(), "write:jira-work".into()]];
    descriptor.example["operation_id"] = json!("remove-design");
    descriptor.example["project_id"] = json!("10000");
    descriptor.example["apply"] = json!(true);
    let binding = OperationBinding {
        operation_id: "remove-design".into(),
        request_digest: format!("sha256:{}", "0".repeat(64)),
        descriptor_digest: format!("sha256:{}", "0".repeat(64)),
        target: ResourceRef {
            provider: "jira".into(),
            deployment: "cloud".into(),
            tenant: "00000000-1111-2222-3333-444444444444".into(),
            resource_type: "project".into(),
            id: "10000".into(),
        },
        grant: GrantRef {
            id: format!("gr_{}", "0".repeat(32)),
        },
        actor: "service-actor".into(),
    };
    descriptor.response_example = json!(Envelope::success(
        native::DELETE,
        "example".into(),
        json!(empty(&binding, "10000".into(), "10001".into()))
    ));
    registry.register_typed_handler::<AttachmentDeleteArgs>(
        descriptor,
        Arc::new(crate::public_commands::VerifiedHostRequired),
    )
}
