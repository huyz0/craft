use super::*;
use crate::profiles::{InvocationOptions, selected_profile_context};
use craft_core::contracts::{AuthMode, GrantRef};
use craft_core::profiles::validate_profile_name;
use craft_core::registry::schema_for;
use craft_runtime::control::SystemClock;
use craft_runtime::issue_read_policy::{IssueReadOperation as Op, IssueReadPolicyRecord};
use craft_runtime::profiles::resolve_state_root;
use craft_runtime::secrets::NativeCredentials;
use std::sync::Arc;
fn limit() -> u16 {
    20
}
#[derive(clap::Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LinkGetArgs {
    #[arg(long, default_value = "")]
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub link_id: String,
    #[arg(long)]
    pub cloud_id: Option<String>,
}
#[derive(clap::Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LinkListArgs {
    #[arg(long, default_value = "")]
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub issue_id: String,
    #[arg(long, default_value_t = 20)]
    #[serde(default = "limit")]
    #[schemars(range(min = 1, max = 100))]
    pub limit: u16,
    #[arg(long)]
    pub cursor: Option<String>,
    #[arg(long)]
    pub cloud_id: Option<String>,
}
#[derive(clap::Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LinkTypesArgs {
    #[arg(long, default_value_t = 20)]
    #[serde(default = "limit")]
    #[schemars(range(min = 1, max = 100))]
    pub limit: u16,
    #[arg(long)]
    pub cursor: Option<String>,
    #[arg(long)]
    pub cloud_id: Option<String>,
}
pub enum LinkReadArgs {
    Get(LinkGetArgs),
    List(LinkListArgs),
    Types(LinkTypesArgs),
}
fn site(cloud: &str) -> ResourceRef {
    ResourceRef {
        provider: "jira".into(),
        deployment: "cloud".into(),
        tenant: cloud.into(),
        resource_type: "site".into(),
        id: cloud.into(),
    }
}
impl LinkReadArgs {
    pub fn operation(&self) -> Op {
        match self {
            Self::Get(_) => Op::LinkGet,
            Self::List(_) => Op::LinkList,
            Self::Types(_) => Op::LinkTypes,
        }
    }
    fn cloud(&self) -> Option<&str> {
        match self {
            Self::Get(v) => v.cloud_id.as_deref(),
            Self::List(v) => v.cloud_id.as_deref(),
            Self::Types(v) => v.cloud_id.as_deref(),
        }
    }
    fn input(&self) -> Value {
        match self {
            Self::Get(v) => json!({"link_id":v.link_id}),
            Self::List(v) => json!({"issue_id":v.issue_id}),
            Self::Types(_) => json!({}),
        }
    }
    fn selection(&self, fields: &[String], maximum: usize) -> Result<Selection, CraftError> {
        let (limit, cursor) = match self {
            Self::Get(_) => (20, None),
            Self::List(v) => (v.limit, v.cursor.clone()),
            Self::Types(v) => (v.limit, v.cursor.clone()),
        };
        if cursor.as_ref().is_some_and(|value| {
            !value.strip_prefix("cur_").is_some_and(|s| {
                s.len() == 32
                    && s.bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        }) {
            return Err(CraftError::field(
                "/cursor",
                "expected_opaque_cursor_handle",
            ));
        }
        Ok(Selection {
            fields: fields.to_vec(),
            limit,
            cursor,
            max_bytes: maximum.try_into().map_err(|_| budget())?,
        })
    }
    pub fn validate(
        &self,
        fields: &[String],
        options: &InvocationOptions<'_>,
    ) -> Result<(), CraftError> {
        if let Some(profile) = options.profile {
            validate_profile_name(profile)?;
        }
        if let Some(cloud) = self.cloud() {
            craft_provider_jira::projects::validate_target(&site(cloud))?;
        }
        let target = site("00000000-1111-2222-3333-444444444444");
        PreparedLinkRead::prepare(
            CanonicalRequest {
                command: self.operation().command().into(),
                target: target.clone(),
                input: self.input(),
                selection: self.selection(fields, options.max_bytes)?,
                preconditions: None,
            },
            options.request_id,
            IssueReadPolicy {
                target,
                grant: GrantRef {
                    id: format!("gr_{}", "0".repeat(32)),
                },
                expected_actor: "service-actor".into(),
            },
        )?;
        Ok(())
    }
}
pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    let native = super::registry()?;
    for command in [native::GET, native::LIST, issue_links::TYPES] {
        let mut descriptor = native.descriptor(command)?.clone();
        match command {
            native::GET => {
                descriptor.input_schema = schema_for::<LinkGetArgs>()?;
                descriptor.example = json!({"link_id":"10001"});
                registry.register_typed_handler::<LinkGetArgs>(
                    descriptor,
                    Arc::new(crate::public_commands::VerifiedHostRequired),
                )?;
            }
            native::LIST => {
                descriptor.input_schema = schema_for::<LinkListArgs>()?;
                descriptor.example = json!({"issue_id":"10000","limit":20});
                registry.register_typed_handler::<LinkListArgs>(
                    descriptor,
                    Arc::new(crate::public_commands::VerifiedHostRequired),
                )?;
            }
            _ => {
                descriptor.input_schema = schema_for::<LinkTypesArgs>()?;
                descriptor.example = json!({"limit":20});
                registry.register_typed_handler::<LinkTypesArgs>(
                    descriptor,
                    Arc::new(crate::public_commands::VerifiedHostRequired),
                )?;
            }
        }
    }
    Ok(())
}
pub struct PreparedRead {
    host: PreparedLinkRead,
    policy: IssueReadPolicyRecord,
    operation: Op,
}
impl PreparedRead {
    pub fn prepare(
        args: LinkReadArgs,
        fields: &[String],
        options: &InvocationOptions<'_>,
        store: &ProfileStore,
        control: &InvocationControl,
    ) -> Result<Self, CraftError> {
        args.validate(fields, options)?;
        let (name, _, hints) = selected_profile_context(store, None, options, control)?;
        let profile = store.get(&name, control)?;
        if profile.provider != "jira"
            || profile.deployment != "cloud"
            || profile.auth_mode != AuthMode::Service
        {
            return Err(denied(
                "Link reads require a configured Jira Cloud service profile.",
            ));
        }
        let explicit = args.cloud().map(site);
        let target = if explicit.is_some()
            || hints.targets.contains_key("site")
            || profile.defaults.contains_key("site")
        {
            profile.resolve_target("site", explicit.as_ref(), &hints)?
        } else {
            site(&profile.tenant_ref()?.tenant)
        };
        let policy = store
            .issue_read_policy(&name, control)?
            .ok_or_else(|| denied("No trusted link-read allowance is configured."))?;
        if policy.target != target {
            return Err(denied(
                "Selected link site differs from the trusted read policy.",
            ));
        }
        let operation = args.operation();
        store.verify_issue_read_policy(&policy, operation, control)?;
        let host = PreparedLinkRead::prepare(
            CanonicalRequest {
                command: operation.command().into(),
                target: target.clone(),
                input: args.input(),
                selection: args.selection(fields, options.max_bytes)?,
                preconditions: None,
            },
            options.request_id,
            IssueReadPolicy {
                target,
                grant: policy.grant.clone(),
                expected_actor: policy.expected_actor.clone(),
            },
        )?;
        Ok(Self {
            host,
            policy,
            operation,
        })
    }
    pub fn grant(&self) -> &GrantRef {
        &self.policy.grant
    }
    pub fn execute(&self, environment: ReadEnvironment<'_, '_>) -> CommandResult {
        let store = environment.store;
        let control = environment.control.clone();
        self.host.execute(environment, || {
            store.verify_issue_read_policy(&self.policy, self.operation, &control)
        })
    }
}
pub fn execute(
    args: LinkReadArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> CommandResult {
    let command = args.operation().command();
    let result = (|| {
        args.validate(fields, options)?;
        let root = resolve_state_root(options.state_root, |key| std::env::var_os(key))?;
        let store = ProfileStore::open(&root, control)?;
        let prepared = PreparedRead::prepare(args, fields, options, &store, control)?;
        let credentials = NativeCredentials::open(&store)?;
        let locked = credentials.lock(prepared.grant(), control)?;
        let client = ServiceOAuthClient::new(
            TlsPolicy::Platform,
            control.clone(),
            Arc::new(SystemClock::default()),
        )?;
        Ok(prepared.execute(ReadEnvironment {
            store: &store,
            client: &client,
            locked: &locked,
            tls: TlsPolicy::Platform,
            control: control.clone(),
        }))
    })();
    result.unwrap_or_else(|error| CommandResult {
        envelope: Envelope::failure(command, options.request_id.into(), error),
        output_policy: OutputPolicy::PreserveEvidence,
    })
}
