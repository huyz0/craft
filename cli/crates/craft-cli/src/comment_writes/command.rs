//! Strict command inputs reserve full recovery capacity before opening private state.
use super::*;
use crate::dispatch::CommandResult;
use crate::output::OutputPolicy;
use crate::profiles::InvocationOptions;
use craft_core::contracts::{Envelope, validate_identity};
use craft_core::presence::InputField;
use craft_core::registry::{response_schema, schema_for};
use craft_provider_jira::comments::adf::Document;
use craft_runtime::control::SystemClock;
use craft_runtime::profiles::resolve_state_root;
use craft_runtime::secrets::NativeCredentials;
use native::{CreateInput, DeleteInput, Ownership, UpdateInput, VisibilityChange};
use serde_json::{Value, json};
use std::sync::Arc;

fn parse<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, String> {
    let value = craft_core::input::parse_object(text.as_bytes())
        .map_err(|_| "expected_strict_comment_object")?;
    serde_json::from_value(value).map_err(|_| "expected_typed_comment_object".into())
}
fn parse_body(text: &str) -> Result<Document, String> {
    parse(text)
}
fn parse_visibility(text: &str) -> Result<VisibilityChange, String> {
    parse(text)
}
fn parse_ownership(text: &str) -> Result<Ownership, String> {
    match text {
        "own" => Ok(Ownership::Own),
        "any" => Ok(Ownership::Any),
        _ => Err("expected_own_or_any".into()),
    }
}
fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}
#[derive(clap::Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentCreateArgs {
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
    #[arg(long,value_parser=parse_body)]
    #[schemars(with = "Document", required)]
    pub body: Option<Document>,
    #[arg(long,value_parser=parse_visibility)]
    #[schemars(with = "VisibilityChange", required)]
    pub visibility: Option<VisibilityChange>,
}
#[derive(clap::Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentUpdateArgs {
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
    pub comment_id: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 71, max = 71), regex(pattern = "^sha256:[0-9a-f]{64}$"))]
    pub expected_revision: String,
    #[arg(long,value_parser=parse_ownership,default_value="own")]
    #[serde(default)]
    pub ownership: Ownership,
    #[arg(long,value_parser=parse_body)]
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Document")]
    pub body: Option<Document>,
    #[arg(long,value_parser=parse_visibility)]
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "VisibilityChange")]
    pub visibility: Option<VisibilityChange>,
}
#[derive(clap::Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentDeleteArgs {
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
    pub comment_id: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 71, max = 71), regex(pattern = "^sha256:[0-9a-f]{64}$"))]
    pub expected_revision: String,
    #[arg(long,value_parser=parse_ownership,default_value="own")]
    #[serde(default)]
    pub ownership: Ownership,
}
pub enum WriteArgs {
    Create(CommentCreateArgs),
    Update(CommentUpdateArgs),
    Delete(CommentDeleteArgs),
}
impl WriteArgs {
    pub fn command(&self) -> &'static str {
        match self {
            Self::Create(_) => native::CREATE,
            Self::Update(_) => native::UPDATE,
            Self::Delete(_) => native::DELETE,
        }
    }
    fn controls(&self) -> (&str, &str, bool) {
        match self {
            Self::Create(v) => (&v.operation_id, &v.project_id, v.apply),
            Self::Update(v) => (&v.operation_id, &v.project_id, v.apply),
            Self::Delete(v) => (&v.operation_id, &v.project_id, v.apply),
        }
    }
    fn input(&self) -> Result<Value, CraftError> {
        let required =
            || CraftError::field("/input", "explicit_comment_body_and_visibility_required");
        let patch = |v: &Option<_>| v.clone().map(InputField::Value).unwrap_or_default();
        Ok(match self {
            Self::Create(v) => json!(CreateInput {
                issue_id: v.issue_id.clone(),
                body: v.body.clone().ok_or_else(required)?,
                visibility: v.visibility.clone().ok_or_else(required)?
            }),
            Self::Update(v) => json!(UpdateInput {
                issue_id: v.issue_id.clone(),
                comment_id: v.comment_id.clone(),
                expected_revision: v.expected_revision.clone(),
                ownership: v.ownership,
                body: patch(&v.body),
                visibility: v
                    .visibility
                    .clone()
                    .map(InputField::Value)
                    .unwrap_or_default()
            }),
            Self::Delete(v) => json!(DeleteInput {
                issue_id: v.issue_id.clone(),
                comment_id: v.comment_id.clone(),
                expected_revision: v.expected_revision.clone(),
                ownership: v.ownership
            }),
        })
    }
    fn request(&self, cloud: &str, max: usize) -> Result<CanonicalRequest, CraftError> {
        Ok(CanonicalRequest {
            command: self.command().into(),
            target: ResourceRef {
                provider: "jira".into(),
                deployment: "cloud".into(),
                tenant: cloud.into(),
                resource_type: "project".into(),
                id: self.controls().1.into(),
            },
            input: self.input()?,
            selection: Selection {
                max_bytes: u32::try_from(max)
                    .map_err(|_| CraftError::field("/max_bytes", "comment_budget_out_of_range"))?,
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
        validate_identity("/operation_id", self.controls().0)?;
        validate_identity("/request_id", options.request_id)?;
        craft_core::profiles::validate_profile_name(
            options.profile.ok_or_else(|| {
                CraftError::field("/profile", "explicit_comment_profile_required")
            })?,
        )?;
        if !fields.is_empty() || options.request_id.len() > 80 {
            return Err(CraftError::field(
                "/output",
                "complete_comment_receipts_required",
            ));
        }
        if !(MINIMUM_OUTPUT..=16_777_216).contains(&options.max_bytes) {
            return Err(CraftError::new(
                ErrorCode::OutputBudgetTooSmall,
                "Reserve --max-bytes 8388608 for complete comment recovery receipts; normal output is compact.",
            ));
        }
        if !self.controls().2 {
            return Err(CraftError::new(
                ErrorCode::ApplyRequired,
                "Comment writes require explicit apply.",
            ));
        }
        registry()?
            .prepare(self.request("00000000-1111-2222-3333-444444444444", options.max_bytes)?)?;
        Ok(())
    }
}
pub fn execute(
    args: WriteArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> CommandResult {
    let command = args.command();
    let result = (|| {
        args.validate(fields, options)?;
        let root = resolve_state_root(options.state_root, |key| std::env::var_os(key))?;
        let store = ProfileStore::open(&root, control)?;
        let profile = options
            .profile
            .ok_or_else(|| CraftError::field("/profile", "explicit_comment_profile_required"))?;
        let selected = store.get(profile, control)?;
        let tenant = selected.tenant_ref()?;
        let prepared = PreparedCommentWrite::prepare(
            args.request(&tenant.tenant, options.max_bytes)?,
            args.controls().0,
            args.controls().2,
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
            CommentWriteEnvironment {
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
    let envelope = match result {
        Ok((data, Some(error))) => {
            Envelope::failure_with_data(command, options.request_id.into(), error, json!(data))
        }
        Ok((data, None)) => Envelope::success(command, options.request_id.into(), json!(data)),
        Err(error) => Envelope::failure(command, options.request_id.into(), error),
    };
    CommandResult {
        envelope,
        output_policy: OutputPolicy::PreserveEvidence,
    }
}
pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    let internal = super::registry()?;
    for command in [native::CREATE, native::UPDATE, native::DELETE] {
        let mut descriptor = internal.descriptor(command)?.clone();
        descriptor.input_schema = match command {
            native::CREATE => schema_for::<CommentCreateArgs>()?,
            native::UPDATE => schema_for::<CommentUpdateArgs>()?,
            _ => schema_for::<CommentDeleteArgs>()?,
        };
        descriptor.response_schema = response_schema::<CommentWriteData>()?;
        descriptor.scope_alternatives =
            vec![vec!["read:jira-work".into(), "write:jira-work".into()]];
        descriptor.example["operation_id"] = json!("review-comment");
        descriptor.example["project_id"] = json!("10000");
        descriptor.example["apply"] = json!(true);
        let binding = OperationBinding {
            operation_id: "review-comment".into(),
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
            command,
            "example".into(),
            json!(empty(&binding, "10000".into()))
        ));
        match command {
            native::CREATE => registry.register_typed_handler::<CommentCreateArgs>(
                descriptor,
                Arc::new(crate::public_commands::VerifiedHostRequired),
            )?,
            native::UPDATE => registry.register_typed_handler::<CommentUpdateArgs>(
                descriptor,
                Arc::new(crate::public_commands::VerifiedHostRequired),
            )?,
            _ => registry.register_typed_handler::<CommentDeleteArgs>(
                descriptor,
                Arc::new(crate::public_commands::VerifiedHostRequired),
            )?,
        }
    }
    Ok(())
}
