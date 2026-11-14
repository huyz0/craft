//! Candidate arguments choose context; separate private configuration supplies exact read authority.

use clap::Args;
use craft_core::contracts::{
    AuthMode, CanonicalRequest, Envelope, GrantRef, ResourceRef, Selection,
};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::input::parse_object;
use craft_core::profiles::validate_profile_name;
use craft_core::registry::{Registry, schema_for};
use craft_provider_jira::issue_reads::{GET, IssueGetInput, IssueSearchInput, JiraIssues, SEARCH};
use craft_runtime::auth::ServiceOAuthClient;
use craft_runtime::control::SystemClock;
use craft_runtime::issue_read_policy::{IssueReadOperation, IssueReadPolicyRecord};
use craft_runtime::profiles::{ProfileStore, resolve_state_root};
use craft_runtime::secrets::{LockedCredentials, NativeCredentials};
use craft_runtime::transport::TlsPolicy;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

use super::{IssueReadPolicy, PreparedIssueRead};
use crate::dispatch::CommandResult;
use crate::output::OutputPolicy;
use crate::profiles::{InvocationOptions, selected_profile_context};

#[cfg(test)]
mod tests;

#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueGetArgs {
    #[arg(long)]
    #[schemars(length(min = 36, max = 36))]
    pub cloud_id: Option<String>,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 512))]
    pub issue: String,
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueSearchArgs {
    #[arg(long)]
    #[schemars(length(min = 36, max = 36))]
    pub cloud_id: Option<String>,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 16_384))]
    pub jql: String,
    #[arg(long,default_value_t=50,value_parser=clap::value_parser!(u16).range(1..=100))]
    #[serde(default = "page_size")]
    #[schemars(range(min = 1, max = 100))]
    pub page_size: u16,
    #[arg(long, value_delimiter = ',')]
    #[serde(default)]
    #[schemars(length(max = 50))]
    pub reconcile_issue_ids: Vec<String>,
    #[arg(long,default_value_t=20,value_parser=clap::value_parser!(u16).range(1..=100))]
    #[serde(default = "limit")]
    #[schemars(range(min = 1, max = 100))]
    pub limit: u16,
    #[arg(long)]
    #[schemars(length(min = 36, max = 36))]
    pub cursor: Option<String>,
}
fn page_size() -> u16 {
    50
}
fn limit() -> u16 {
    20
}
pub enum IssueReadArgs {
    Get(IssueGetArgs),
    Search(IssueSearchArgs),
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
fn encoded<T: Serialize>(value: &T) -> Result<Value, CraftError> {
    serde_json::to_value(value)
        .map_err(|_| CraftError::field("/input", "invalid_issue_read_arguments"))
}
impl IssueReadArgs {
    pub fn operation(&self) -> IssueReadOperation {
        match self {
            Self::Get(_) => IssueReadOperation::Get,
            Self::Search(_) => IssueReadOperation::Search,
        }
    }
    fn cloud(&self) -> Option<&str> {
        match self {
            Self::Get(args) => args.cloud_id.as_deref(),
            Self::Search(args) => args.cloud_id.as_deref(),
        }
    }
    fn input(&self) -> Result<Value, CraftError> {
        match self {
            Self::Get(args) => {
                let input = IssueGetInput {
                    issue: args.issue.clone(),
                };
                input.validate()?;
                encoded(&input)
            }
            Self::Search(args) => {
                let input = IssueSearchInput {
                    jql: args.jql.clone(),
                    page_size: args.page_size,
                    reconcile_issue_ids: args.reconcile_issue_ids.clone(),
                };
                input.validate()?;
                encoded(&input)
            }
        }
    }
    fn selection(&self, fields: &[String], bytes: usize) -> Result<Selection, CraftError> {
        let (limit, cursor) = match self {
            Self::Get(_) => (20, None),
            Self::Search(args) => (args.limit, args.cursor.clone()),
        };
        Ok(Selection {
            fields: fields.to_vec(),
            limit,
            cursor,
            max_bytes: bytes
                .try_into()
                .map_err(|_| CraftError::field("/max_bytes", "invalid_output_budget"))?,
        })
    }
    fn validate(
        &self,
        fields: &[String],
        options: &InvocationOptions<'_>,
    ) -> Result<(), CraftError> {
        if let Some(profile) = options.profile {
            validate_profile_name(profile)?;
        }
        let input = self.input()?;
        if let Some(cloud) = self.cloud() {
            craft_provider_jira::projects::validate_target(&site(cloud))?;
        }
        let selection = self.selection(fields, options.max_bytes)?;
        if selection.cursor.as_ref().is_some_and(|cursor| {
            !cursor.strip_prefix("cur_").is_some_and(|suffix| {
                suffix.len() == 32
                    && suffix
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
        }) {
            return Err(CraftError::field(
                "/cursor",
                "expected_opaque_cursor_handle",
            ));
        }
        let raw = match self {
            Self::Get(args) => encoded(args)?,
            Self::Search(args) => encoded(args)?,
        };
        parse_object(
            &serde_json::to_vec(&raw)
                .map_err(|_| CraftError::field("/input", "invalid_issue_read_arguments"))?,
        )?;
        let target = site("00000000-1111-2222-3333-444444444444");
        // Fixed UUID/grant lengths let pure preflight reject invalid budgets without opening state.
        PreparedIssueRead::prepare(
            CanonicalRequest {
                command: self.operation().command().into(),
                target: target.clone(),
                input,
                selection,
                preconditions: None,
            },
            options.request_id,
            IssueReadPolicy {
                target,
                grant: GrantRef {
                    id: "gr_00000000000000000000000000000000".into(),
                },
                expected_actor: "actor-one".into(),
            },
        )?;
        Ok(())
    }
}
/// Adds typed candidate contracts only to an explicitly supplied internal registry.
pub fn register_candidates(registry: &mut Registry) -> Result<(), CraftError> {
    let mut native = Registry::default();
    native.register_provider(&JiraIssues)?;
    for command in [GET, SEARCH] {
        let mut descriptor = native.descriptor(command)?.clone();
        if command == GET {
            descriptor.input_schema = schema_for::<IssueGetArgs>()?;
            descriptor.example = json!({"issue":"10000"});
            registry.register_typed::<IssueGetArgs>(descriptor)?;
        } else {
            descriptor.input_schema = schema_for::<IssueSearchArgs>()?;
            descriptor.example = json!({"jql":"project = ED ORDER BY key","page_size":50,"limit":20,"reconcile_issue_ids":[]});
            registry.register_typed::<IssueSearchArgs>(descriptor)?;
        }
    }
    Ok(())
}
pub struct PreparedInvocation {
    host: PreparedIssueRead,
    policy: IssueReadPolicyRecord,
    operation: IssueReadOperation,
}
impl PreparedInvocation {
    /// Resolves context once and captures exact enabled operation-specific private configuration.
    pub fn prepare(
        args: IssueReadArgs,
        fields: &[String],
        options: &InvocationOptions<'_>,
        store: &ProfileStore,
        control: &InvocationControl,
    ) -> Result<Self, CraftError> {
        args.validate(fields, options)?;
        control.check()?;
        let (name, _, hints) = selected_profile_context(store, None, options, control)?;
        let profile = store.get(&name, control)?;
        if profile.provider != "jira"
            || profile.deployment != "cloud"
            || profile.auth_mode != AuthMode::Service
        {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Issue reads require a configured Jira Cloud service profile.",
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
        profile.validate_target("site", &target)?;
        craft_provider_jira::projects::validate_target(&target)?;
        let policy = store.issue_read_policy(&name, control)?.ok_or_else(|| {
            CraftError::new(
                ErrorCode::PolicyDenied,
                "No trusted issue-read configuration is present for this profile.",
            )
        })?;
        if policy.target != target {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Selected site differs from trusted issue-read configuration.",
            ));
        }
        let operation = args.operation();
        store.verify_issue_read_policy(&policy, operation, control)?;
        let host = PreparedIssueRead::prepare(
            CanonicalRequest {
                command: operation.command().into(),
                target: target.clone(),
                input: args.input()?,
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
    /// Rechecks the captured configuration before auth and before native/cached output.
    pub fn execute_cached(
        &self,
        store: &ProfileStore,
        client: &ServiceOAuthClient,
        locked: &LockedCredentials<'_>,
        tls: TlsPolicy,
        control: InvocationControl,
    ) -> CommandResult {
        let check_control = control.clone();
        self.host
            .execute_cached_with_policy_check(client, locked, store, tls, control, || {
                store.verify_issue_read_policy(&self.policy, self.operation, &check_control)
            })
    }
}
/// Validates before opening state, then uses only configured native credentials and declared hosts.
/// This candidate entry point is absent from public startup/registration.
pub fn execute(
    args: IssueReadArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> CommandResult {
    let command = args.operation().command();
    let result = (|| {
        args.validate(fields, options)?;
        let root = resolve_state_root(options.state_root, |name| std::env::var_os(name))?;
        let store = ProfileStore::open(&root, control)?;
        let prepared = PreparedInvocation::prepare(args, fields, options, &store, control)?;
        let credentials = NativeCredentials::open(&store)?;
        let locked = credentials.lock(prepared.grant(), control)?;
        let client = ServiceOAuthClient::new(
            TlsPolicy::Platform,
            control.clone(),
            Arc::new(SystemClock::default()),
        )?;
        Ok(prepared.execute_cached(
            &store,
            &client,
            &locked,
            TlsPolicy::Platform,
            control.clone(),
        ))
    })();
    result.unwrap_or_else(|error| CommandResult {
        envelope: Envelope::failure(command, options.request_id.into(), error),
        output_policy: OutputPolicy::Bounded,
    })
}
