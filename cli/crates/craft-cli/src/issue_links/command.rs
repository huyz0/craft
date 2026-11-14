//! Local input validation and explicit-profile administration precede link authentication.
use super::*;
use crate::dispatch::CommandResult;
use crate::output::OutputPolicy;
use crate::profiles::InvocationOptions;
use craft_core::contracts::{Envelope, SCHEMA_VERSION};
use craft_core::profiles::validate_profile_name;
use craft_core::registry::{response_schema, schema_for};
use craft_provider_jira::issue_links::LinkType;
use craft_runtime::control::SystemClock;
use craft_runtime::profiles::resolve_state_root;
use craft_runtime::secrets::NativeCredentials;
use std::sync::Arc;

fn parse_type(text: &str) -> Result<LinkType, String> {
    let value = craft_core::input::parse_object(text.as_bytes())
        .map_err(|_| "expected_strict_link_type_json")?;
    if value == json!({}) {
        return Ok(LinkType {
            id: String::new(),
            name: String::new(),
            inward: String::new(),
            outward: String::new(),
        });
    }
    serde_json::from_value(value).map_err(|_| "expected_complete_native_link_type".into())
}
#[derive(clap::Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LinkArgs {
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
    pub inward_project_id: String,
    #[arg(long, default_value = "")]
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub inward_issue_id: String,
    #[arg(long, default_value = "")]
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub outward_issue_id: String,
    #[arg(long,default_value="{}",value_parser=parse_type)]
    pub link_type: LinkType,
}
impl LinkArgs {
    fn input(&self) -> LinkInput {
        LinkInput {
            inward_issue_id: self.inward_issue_id.clone(),
            outward_issue_id: self.outward_issue_id.clone(),
            inward_project_id: self.inward_project_id.clone(),
            link_type: self.link_type.clone(),
        }
    }
    pub fn validate(
        &self,
        fields: &[String],
        options: &InvocationOptions<'_>,
    ) -> Result<(), CraftError> {
        craft_core::contracts::validate_identity("/operation_id", &self.operation_id)?;
        craft_core::contracts::validate_identity("/request_id", options.request_id)?;
        if options.request_id.len() > 80 || !fields.is_empty() {
            return Err(CraftError::field(
                "/output",
                "complete_link_effect_receipts_required",
            ));
        }
        validate_profile_name(
            options
                .profile
                .ok_or_else(|| CraftError::field("/profile", "explicit_link_profile_required"))?,
        )?;
        if !(MINIMUM_OUTPUT..=16_777_216).contains(&options.max_bytes) {
            return Err(CraftError::new(
                ErrorCode::OutputBudgetTooSmall,
                "Use --max-bytes 8388608 to reserve complete link journal and credential receipts; normal output remains compact.",
            ));
        }
        if !self.apply {
            return Err(CraftError::new(
                ErrorCode::ApplyRequired,
                "Link writes require explicit apply.",
            ));
        }
        self.input().validate()?;
        let target = ResourceRef {
            provider: "jira".into(),
            deployment: "cloud".into(),
            tenant: "00000000-1111-2222-3333-444444444444".into(),
            resource_type: "project".into(),
            id: self.project_id.clone(),
        };
        registry()?.prepare(CanonicalRequest {
            command: CREATE.into(),
            target,
            input: json!(self.input()),
            selection: Selection::default(),
            preconditions: None,
        })?;
        Ok(())
    }
}
pub fn prepare(
    args: &LinkArgs,
    profile: &str,
    store: &ProfileStore,
    control: &InvocationControl,
) -> Result<PreparedLink, CraftError> {
    let selected = store.get(profile, control)?;
    let tenant = selected.tenant_ref()?;
    PreparedLink::prepare(
        args.input(),
        ResourceRef {
            provider: tenant.provider,
            deployment: tenant.deployment,
            tenant: tenant.tenant,
            resource_type: "project".into(),
            id: args.project_id.clone(),
        },
        profile,
        &args.operation_id,
        args.apply,
        store,
        control,
    )
}
fn parse_expected(text: &str) -> Result<LinkInput, String> {
    let value = craft_core::input::parse_object(text.as_bytes())
        .map_err(|_| "expected_strict_link_expectation")?;
    if value == json!({}) {
        return Ok(LinkInput {
            inward_issue_id: String::new(),
            outward_issue_id: String::new(),
            inward_project_id: String::new(),
            link_type: LinkType {
                id: String::new(),
                name: String::new(),
                inward: String::new(),
                outward: String::new(),
            },
        });
    }
    serde_json::from_value(value).map_err(|_| "expected_complete_link_expectation".into())
}
#[derive(clap::Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LinkDeleteArgs {
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
    pub link_id: String,
    #[arg(long,default_value="{}",value_parser=parse_expected)]
    pub expected: LinkInput,
}
impl LinkDeleteArgs {
    fn validate(
        &self,
        fields: &[String],
        options: &InvocationOptions<'_>,
    ) -> Result<(), CraftError> {
        let input = write::LinkDeleteInput {
            link_id: self.link_id.clone(),
            expected: self.expected.clone(),
        };
        input.validate()?;
        LinkArgs {
            operation_id: self.operation_id.clone(),
            apply: self.apply,
            project_id: self.project_id.clone(),
            inward_project_id: self.expected.inward_project_id.clone(),
            inward_issue_id: self.expected.inward_issue_id.clone(),
            outward_issue_id: self.expected.outward_issue_id.clone(),
            link_type: self.expected.link_type.clone(),
        }
        .validate(fields, options)
    }
}
pub fn prepare_delete(
    args: &LinkDeleteArgs,
    profile: &str,
    store: &ProfileStore,
    control: &InvocationControl,
) -> Result<PreparedLink, CraftError> {
    let tenant = store.get(profile, control)?.tenant_ref()?;
    PreparedLink::prepare_delete(
        write::LinkDeleteInput {
            link_id: args.link_id.clone(),
            expected: args.expected.clone(),
        },
        ResourceRef {
            provider: tenant.provider,
            deployment: tenant.deployment,
            tenant: tenant.tenant,
            resource_type: "project".into(),
            id: args.project_id.clone(),
        },
        profile,
        &args.operation_id,
        args.apply,
        store,
        control,
    )
}
enum WriteArgs {
    Create(LinkArgs),
    Delete(LinkDeleteArgs),
}
impl WriteArgs {
    fn command(&self) -> &'static str {
        match self {
            Self::Create(_) => CREATE,
            Self::Delete(_) => write::DELETE,
        }
    }
    fn validate(
        &self,
        fields: &[String],
        options: &InvocationOptions<'_>,
    ) -> Result<(), CraftError> {
        match self {
            Self::Create(v) => v.validate(fields, options),
            Self::Delete(v) => v.validate(fields, options),
        }
    }
    fn prepare(
        &self,
        profile: &str,
        store: &ProfileStore,
        control: &InvocationControl,
    ) -> Result<PreparedLink, CraftError> {
        match self {
            Self::Create(v) => prepare(v, profile, store, control),
            Self::Delete(v) => prepare_delete(v, profile, store, control),
        }
    }
}
pub fn execute_delete(
    args: LinkDeleteArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> CommandResult {
    execute_write(WriteArgs::Delete(args), fields, options, control)
}
pub fn execute(
    args: LinkArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> CommandResult {
    execute_write(WriteArgs::Create(args), fields, options, control)
}
fn execute_write(
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
            .ok_or_else(|| CraftError::field("/profile", "explicit_link_profile_required"))?;
        let prepared = args.prepare(profile, &store, control)?;
        let journal = NativeJournal::open(&store, control).map_err(storage)?;
        let credentials = NativeCredentials::open(&store)?;
        let locked = credentials.lock(prepared.grant(), control)?;
        let client = ServiceOAuthClient::new(
            TlsPolicy::Platform,
            control.clone(),
            Arc::new(SystemClock::default()),
        )?;
        Ok(prepared.execute(
            LinkEnvironment {
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
    let mut descriptor = internal.descriptor(CREATE)?.clone();
    descriptor.contract_version = SCHEMA_VERSION.into();
    descriptor.scope_alternatives = vec![vec!["read:jira-work".into(), "write:jira-work".into()]];
    descriptor.input_schema = schema_for::<LinkArgs>()?;
    descriptor.response_schema = response_schema::<LinkData>()?;
    descriptor.example = json!({"operation_id":"dependency-link","apply":true,"project_id":"10000","inward_project_id":"10000","inward_issue_id":"10001","outward_issue_id":"10002","link_type":{"id":"10000","name":"Blocks","inward":"is blocked by","outward":"blocks"}});
    let args: LinkArgs = serde_json::from_value(descriptor.example.clone())
        .map_err(|_| CraftError::field("/example", "invalid_link_example"))?;
    let binding = OperationBinding {
        operation_id: args.operation_id.clone(),
        request_digest: format!("sha256:{}", "0".repeat(64)),
        descriptor_digest: format!("sha256:{}", "0".repeat(64)),
        target: ResourceRef {
            provider: "jira".into(),
            deployment: "cloud".into(),
            tenant: "00000000-1111-2222-3333-444444444444".into(),
            resource_type: "project".into(),
            id: args.project_id.clone(),
        },
        grant: craft_core::contracts::GrantRef {
            id: format!("gr_{}", "0".repeat(32)),
        },
        actor: "service-actor".into(),
    };
    descriptor.response_example = json!(Envelope::success(
        CREATE,
        "example".into(),
        json!(empty(&binding, args.input()))
    ));
    registry.register_typed_handler::<LinkArgs>(
        descriptor,
        Arc::new(crate::public_commands::VerifiedHostRequired),
    )?;
    let mut descriptor = internal.descriptor(write::DELETE)?.clone();
    descriptor.input_schema = schema_for::<LinkDeleteArgs>()?;
    descriptor.response_schema = response_schema::<LinkData>()?;
    descriptor.scope_alternatives = vec![vec!["read:jira-work".into(), "write:jira-work".into()]];
    descriptor.example = json!({"operation_id":"delete-dependency","apply":true,"project_id":"10000","link_id":"10003","expected":args.input()});
    let mut data = empty(&binding, args.input());
    data.link_id = Some("10003".into());
    descriptor.response_example = json!(Envelope::success(
        write::DELETE,
        "example".into(),
        json!(data)
    ));
    registry.register_typed_handler::<LinkDeleteArgs>(
        descriptor,
        Arc::new(crate::public_commands::VerifiedHostRequired),
    )
}
