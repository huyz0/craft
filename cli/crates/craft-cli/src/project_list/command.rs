//! Typed candidate arguments select context; exact private configuration supplies the read binding.

use std::sync::Arc;

use clap::Args;
use craft_core::contracts::{AuthMode, CanonicalRequest, GrantRef, ResourceRef, Selection};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::input::parse_object;
use craft_core::profiles::validate_profile_name;
use craft_core::registry::{Registry, schema_for};
use craft_provider_jira::projects::{self, COMMAND, JiraProjects, ProjectListInput};
use craft_runtime::auth::ServiceOAuthClient;
use craft_runtime::control::SystemClock;
use craft_runtime::profiles::{ProfileStore, resolve_state_root};
use craft_runtime::project_read_policy::ProjectReadPolicyRecord;
use craft_runtime::secrets::{LockedCredentials, NativeCredentials};
use craft_runtime::transport::TlsPolicy;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{PreparedProjectList, ProjectReadPolicy};
use crate::dispatch::CommandResult;
use crate::output::OutputPolicy;
use crate::profiles::{InvocationOptions, selected_profile_context};
use craft_core::contracts::Envelope;

#[cfg(test)]
mod tests;

#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectListArgs {
    #[arg(long)]
    #[schemars(length(min = 36, max = 36))]
    pub cloud_id: Option<String>,
    #[arg(long)]
    #[schemars(length(min = 1, max = 256))]
    pub query: Option<String>,
    #[arg(long,default_value_t=50,value_parser=clap::value_parser!(u16).range(1..=100))]
    #[serde(default = "page_size")]
    #[schemars(range(min = 1, max = 100))]
    pub page_size: u16,
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
impl ProjectListArgs {
    fn input(&self) -> ProjectListInput {
        ProjectListInput {
            query: self.query.clone(),
            page_size: self.page_size,
        }
    }
    fn selection(&self, fields: &[String], bytes: usize) -> Result<Selection, CraftError> {
        Ok(Selection {
            fields: fields.to_vec(),
            limit: self.limit,
            cursor: self.cursor.clone(),
            max_bytes: bytes
                .try_into()
                .map_err(|_| CraftError::field("/max_bytes", "invalid_output_budget"))?,
        })
    }
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
pub fn register_candidate(registry: &mut Registry) -> Result<(), CraftError> {
    let mut native = Registry::default();
    native.register_provider(&JiraProjects)?;
    let mut descriptor = native.descriptor(COMMAND)?.clone();
    descriptor.input_schema = schema_for::<ProjectListArgs>()?;
    descriptor.example = json!({"page_size":50,"limit":20});
    registry.register_typed::<ProjectListArgs>(descriptor)
}
fn validate(
    args: &ProjectListArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
) -> Result<(), CraftError> {
    if let Some(profile) = options.profile {
        validate_profile_name(profile)?;
    }
    args.input().validate()?;
    if let Some(cloud) = &args.cloud_id {
        projects::validate_target(&site(cloud))?;
    }
    if args.cursor.as_ref().is_some_and(|cursor| {
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
    let encoded = serde_json::to_vec(args)
        .map_err(|_| CraftError::field("/input", "invalid_project_list_arguments"))?;
    parse_object(&encoded)?;
    let target = site("00000000-1111-2222-3333-444444444444");
    // Site UUID and issued-grant lengths are fixed; this pure probe never opens state or emits a target.
    PreparedProjectList::prepare(
        CanonicalRequest {
            command: COMMAND.into(),
            target: target.clone(),
            input: serde_json::to_value(args.input())
                .map_err(|_| CraftError::field("/input", "invalid_project_list_input"))?,
            selection: args.selection(fields, options.max_bytes)?,
            preconditions: None,
        },
        options.request_id,
        ProjectReadPolicy {
            target,
            grant: GrantRef {
                id: "gr_00000000000000000000000000000000".into(),
            },
            expected_actor: "actor-one".into(),
            continuation_site: "https://example.atlassian.net/".into(),
        },
    )?;
    Ok(())
}
pub struct PreparedInvocation {
    host: PreparedProjectList,
    policy: ProjectReadPolicyRecord,
}
impl PreparedInvocation {
    pub fn prepare(
        args: ProjectListArgs,
        fields: &[String],
        options: &InvocationOptions<'_>,
        store: &ProfileStore,
        control: &InvocationControl,
    ) -> Result<Self, CraftError> {
        validate(&args, fields, options)?;
        control.check()?;
        let (name, _, hints) = selected_profile_context(store, None, options, control)?;
        let profile = store.get(&name, control)?;
        if profile.provider != "jira"
            || profile.deployment != "cloud"
            || profile.auth_mode != AuthMode::Service
        {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Project listing requires a configured Jira Cloud service profile.",
            ));
        }
        let explicit = args.cloud_id.as_deref().map(site);
        let target = if explicit.is_some()
            || hints.targets.contains_key("site")
            || profile.defaults.contains_key("site")
        {
            profile.resolve_target("site", explicit.as_ref(), &hints)?
        } else {
            site(&profile.tenant_ref()?.tenant)
        };
        profile.validate_target("site", &target)?;
        projects::validate_target(&target)?;
        let policy = store.project_read_policy(&name, control)?.ok_or_else(|| {
            CraftError::new(
                ErrorCode::PolicyDenied,
                "No trusted project-read configuration is present for this profile.",
            )
        })?;
        if policy.target != target {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Selected site differs from the trusted project-read configuration.",
            ));
        }
        store.verify_project_read_policy(&policy, control)?;
        let host = PreparedProjectList::prepare(
            CanonicalRequest {
                command: COMMAND.into(),
                target: target.clone(),
                input: serde_json::to_value(args.input())
                    .map_err(|_| CraftError::field("/input", "invalid_project_list_input"))?,
                selection: args.selection(fields, options.max_bytes)?,
                preconditions: None,
            },
            options.request_id,
            ProjectReadPolicy {
                target,
                grant: policy.grant.clone(),
                expected_actor: policy.expected_actor.clone(),
                continuation_site: policy.continuation_site.clone(),
            },
        )?;
        Ok(Self { host, policy })
    }
    pub fn grant(&self) -> &GrantRef {
        &self.policy.grant
    }
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
                store.verify_project_read_policy(&self.policy, &check_control)
            })
    }
}
pub fn execute(
    args: ProjectListArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> CommandResult {
    let result = (|| {
        validate(&args, fields, options)?;
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
        envelope: Envelope::failure(COMMAND, options.request_id.into(), error),
        output_policy: OutputPolicy::Bounded,
    })
}
