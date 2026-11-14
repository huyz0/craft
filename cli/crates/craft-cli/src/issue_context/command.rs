//! Candidate issue metadata uses explicit site allowances and exact issue-bound preparation.

use clap::Args;
use craft_core::contracts::{
    AuthMode, CanonicalRequest, Envelope, GrantRef, ResourceRef, Selection,
};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::input::parse_object;
use craft_core::profiles::validate_profile_name;
use craft_core::registry::{Registry, schema_for};
use craft_provider_jira::issue_context::{
    EDIT, EditInput, JiraIssueContext, TRANSITIONS, TransitionInput,
};
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

use super::{IssueContextPolicy, PreparedIssueContext};
use crate::dispatch::CommandResult;
use crate::output::OutputPolicy;
use crate::profiles::{InvocationOptions, selected_profile_context};

#[cfg(test)]
mod tests;

#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditMetadataArgs {
    #[arg(long)]
    #[schemars(length(min = 36, max = 36))]
    pub cloud_id: Option<String>,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 512))]
    pub issue: String,
    #[arg(long,default_value_t=20,value_parser=clap::value_parser!(u16).range(1..=100))]
    #[serde(default = "limit")]
    #[schemars(range(min = 1, max = 100))]
    pub limit: u16,
    #[arg(long)]
    #[schemars(length(min = 36, max = 36))]
    pub cursor: Option<String>,
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransitionListArgs {
    #[arg(long)]
    #[schemars(length(min = 36, max = 36))]
    pub cloud_id: Option<String>,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 512))]
    pub issue: String,
    #[arg(long)]
    #[schemars(length(min = 1, max = 512))]
    pub transition_id: Option<String>,
    #[arg(long)]
    #[serde(default)]
    pub include_unavailable: bool,
    #[arg(long)]
    #[serde(default)]
    pub sort_by_status: bool,
    #[arg(long,default_value_t=20,value_parser=clap::value_parser!(u16).range(1..=100))]
    #[serde(default = "limit")]
    #[schemars(range(min = 1, max = 100))]
    pub limit: u16,
    #[arg(long)]
    #[schemars(length(min = 36, max = 36))]
    pub cursor: Option<String>,
}
fn limit() -> u16 {
    20
}
pub enum IssueContextArgs {
    Edit(EditMetadataArgs),
    Transitions(TransitionListArgs),
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
fn issue_target(site: &ResourceRef, issue: &str) -> ResourceRef {
    ResourceRef {
        resource_type: "issue".into(),
        id: issue.into(),
        ..site.clone()
    }
}
fn encoded<T: Serialize>(value: &T) -> Result<Value, CraftError> {
    serde_json::to_value(value)
        .map_err(|_| CraftError::field("/input", "invalid_issue_metadata_arguments"))
}
impl IssueContextArgs {
    pub fn operation(&self) -> IssueReadOperation {
        match self {
            Self::Edit(_) => IssueReadOperation::EditMetadata,
            Self::Transitions(_) => IssueReadOperation::Transitions,
        }
    }
    fn cloud(&self) -> Option<&str> {
        match self {
            Self::Edit(args) => args.cloud_id.as_deref(),
            Self::Transitions(args) => args.cloud_id.as_deref(),
        }
    }
    fn issue(&self) -> &str {
        match self {
            Self::Edit(args) => &args.issue,
            Self::Transitions(args) => &args.issue,
        }
    }
    fn input(&self) -> Result<Value, CraftError> {
        match self {
            Self::Edit(args) => encoded(&EditInput {
                issue: args.issue.clone(),
            }),
            Self::Transitions(args) => {
                let input = TransitionInput {
                    issue: args.issue.clone(),
                    transition_id: args.transition_id.clone(),
                    include_unavailable: args.include_unavailable,
                    sort_by_status: args.sort_by_status,
                };
                input.validate()?;
                encoded(&input)
            }
        }
    }
    fn selection(&self, fields: &[String], bytes: usize) -> Result<Selection, CraftError> {
        let (limit, cursor) = match self {
            Self::Edit(args) => (args.limit, args.cursor.clone()),
            Self::Transitions(args) => (args.limit, args.cursor.clone()),
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
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        }) {
            return Err(CraftError::field(
                "/cursor",
                "expected_opaque_cursor_handle",
            ));
        }
        let raw = match self {
            Self::Edit(args) => encoded(args)?,
            Self::Transitions(args) => encoded(args)?,
        };
        parse_object(
            &serde_json::to_vec(&raw)
                .map_err(|_| CraftError::field("/input", "invalid_issue_metadata_arguments"))?,
        )?;
        let target = issue_target(&site("00000000-1111-2222-3333-444444444444"), self.issue());
        PreparedIssueContext::prepare(
            CanonicalRequest {
                command: self.operation().command().into(),
                target: target.clone(),
                input,
                selection,
                preconditions: None,
            },
            options.request_id,
            IssueContextPolicy {
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
/// Adds typed metadata contracts to the supplied registry without granting execution authority.
pub fn register_candidates(registry: &mut Registry) -> Result<(), CraftError> {
    let mut native = Registry::default();
    native.register_provider(&JiraIssueContext)?;
    for command in [EDIT, TRANSITIONS] {
        let mut descriptor = native.descriptor(command)?.clone();
        descriptor.example = json!({"issue":"10000","limit":20});
        if command == EDIT {
            descriptor.input_schema = schema_for::<EditMetadataArgs>()?;
            registry.register_typed::<EditMetadataArgs>(descriptor)?;
        } else {
            descriptor.input_schema = schema_for::<TransitionListArgs>()?;
            registry.register_typed::<TransitionListArgs>(descriptor)?;
        }
    }
    Ok(())
}
pub struct PreparedInvocation {
    host: PreparedIssueContext,
    policy: IssueReadPolicyRecord,
    operation: IssueReadOperation,
}
impl PreparedInvocation {
    /// Resolves context once and captures exact enabled operation-specific private configuration.
    pub fn prepare(
        args: IssueContextArgs,
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
                "Issue metadata reads require a configured Jira Cloud service profile.",
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
        let host = PreparedIssueContext::prepare(
            CanonicalRequest {
                command: operation.command().into(),
                target: issue_target(&target, args.issue()),
                input: args.input()?,
                selection: args.selection(fields, options.max_bytes)?,
                preconditions: None,
            },
            options.request_id,
            IssueContextPolicy {
                target: issue_target(&target, args.issue()),
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
/// Executes through the verified native host after typed input and policy preparation.
pub fn execute(
    args: IssueContextArgs,
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
