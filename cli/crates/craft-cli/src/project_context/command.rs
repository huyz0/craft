//! Typed project context selects a resource; explicit stored permission authorizes the read.

use super::{PreparedProjectContext, ProjectContextPolicy};
use crate::dispatch::CommandResult;
use crate::output::OutputPolicy;
use crate::profiles::{InvocationOptions, selected_profile_context};
use clap::Args;
use craft_core::contracts::{
    AuthMode, CanonicalRequest, Envelope, GrantRef, ResourceRef, Selection,
};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::input::parse_object;
use craft_core::profiles::validate_profile_name;
use craft_core::registry::{Registry, schema_for};
use craft_provider_jira::project_context::{self, GET, JiraProjectContext, STATUSES};
use craft_runtime::auth::ServiceOAuthClient;
use craft_runtime::control::SystemClock;
use craft_runtime::profiles::{ProfileStore, resolve_state_root};
use craft_runtime::project_read_policy::{ProjectContextOperation, ProjectReadPolicyRecord};
use craft_runtime::secrets::{LockedCredentials, NativeCredentials};
use craft_runtime::transport::TlsPolicy;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

#[cfg(test)]
mod tests;

#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectGetArgs {
    #[arg(long)]
    #[schemars(length(min = 36, max = 36))]
    pub cloud_id: Option<String>,
    #[arg(long)]
    #[schemars(length(min = 1, max = 512))]
    pub project: Option<String>,
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectStatusesArgs {
    #[arg(long)]
    #[schemars(length(min = 36, max = 36))]
    pub cloud_id: Option<String>,
    #[arg(long)]
    #[schemars(length(min = 1, max = 512))]
    pub project: Option<String>,
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
pub enum ProjectContextArgs {
    Get(ProjectGetArgs),
    Statuses(ProjectStatusesArgs),
}
fn target(cloud: &str, project: &str) -> ResourceRef {
    ResourceRef {
        provider: "jira".into(),
        deployment: "cloud".into(),
        tenant: cloud.into(),
        resource_type: "project".into(),
        id: project.into(),
    }
}
fn value<T: Serialize>(value: &T) -> Result<Value, CraftError> {
    serde_json::to_value(value)
        .map_err(|_| CraftError::field("/input", "invalid_project_context_arguments"))
}
impl ProjectContextArgs {
    pub fn operation(&self) -> ProjectContextOperation {
        match self {
            Self::Get(_) => ProjectContextOperation::Get,
            Self::Statuses(_) => ProjectContextOperation::Statuses,
        }
    }
    fn context(&self) -> (Option<&str>, Option<&str>) {
        match self {
            Self::Get(args) => (args.cloud_id.as_deref(), args.project.as_deref()),
            Self::Statuses(args) => (args.cloud_id.as_deref(), args.project.as_deref()),
        }
    }
    fn selection(&self, fields: &[String], bytes: usize) -> Result<Selection, CraftError> {
        let (limit, cursor) = match self {
            Self::Get(_) => (20, None),
            Self::Statuses(args) => (args.limit, args.cursor.clone()),
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
        let (cloud, project) = self.context();
        // Reserve the maximum selector length when context will only be known after state is opened.
        let maximum = "X".repeat(512);
        let target = target(
            cloud.unwrap_or("00000000-1111-2222-3333-444444444444"),
            project.unwrap_or(&maximum),
        );
        project_context::validate_target(&target)?;
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
            Self::Get(args) => value(args)?,
            Self::Statuses(args) => value(args)?,
        };
        parse_object(
            &serde_json::to_vec(&raw)
                .map_err(|_| CraftError::field("/input", "invalid_project_context_arguments"))?,
        )?;
        PreparedProjectContext::prepare(
            CanonicalRequest {
                command: self.operation().command().into(),
                target: target.clone(),
                input: json!({}),
                selection,
                preconditions: None,
            },
            options.request_id,
            ProjectContextPolicy {
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
/// Registers typed contracts without granting execution authority.
pub fn register_candidates(registry: &mut Registry) -> Result<(), CraftError> {
    let mut native = Registry::default();
    native.register_provider(&JiraProjectContext)?;
    for command in [GET, STATUSES] {
        let mut descriptor = native.descriptor(command)?.clone();
        if command == GET {
            descriptor.input_schema = schema_for::<ProjectGetArgs>()?;
            descriptor.example = json!({"project":"10000"});
            registry.register_typed::<ProjectGetArgs>(descriptor)?;
        } else {
            descriptor.input_schema = schema_for::<ProjectStatusesArgs>()?;
            descriptor.example = json!({"project":"10000","limit":20});
            registry.register_typed::<ProjectStatusesArgs>(descriptor)?;
        }
    }
    Ok(())
}
pub struct PreparedInvocation {
    host: PreparedProjectContext,
    policy: ProjectReadPolicyRecord,
    target: ResourceRef,
    operation: ProjectContextOperation,
}
impl PreparedInvocation {
    /// Resolves profile/project once and captures the exact operation permission and shared identity.
    pub fn prepare(
        args: ProjectContextArgs,
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
                "Project context requires a configured Jira Cloud service profile.",
            ));
        }
        let tenant = profile.tenant_ref()?;
        let (cloud, project) = args.context();
        if cloud.is_some_and(|cloud| cloud != tenant.tenant) {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Explicit cloud differs from the selected profile tenant.",
            ));
        }
        let explicit = project.map(|project| target(&tenant.tenant, project));
        let target=profile.resolve_target("project",explicit.as_ref(),&hints).map_err(|mut error|{if error.code==ErrorCode::TargetAmbiguous{error.next_action="Supply --project or configure an exact project target in the selected profile/repository.".into();}error})?;
        project_context::validate_target(&target)?;
        let policy = store.project_read_policy(&name, control)?.ok_or_else(|| {
            CraftError::new(
                ErrorCode::PolicyDenied,
                "No trusted project-read configuration is present for this profile.",
            )
        })?;
        let operation = args.operation();
        store.verify_project_context_policy(&policy, &target, operation, control)?;
        let host = PreparedProjectContext::prepare(
            CanonicalRequest {
                command: operation.command().into(),
                target: target.clone(),
                input: json!({}),
                selection: args.selection(fields, options.max_bytes)?,
                preconditions: None,
            },
            options.request_id,
            ProjectContextPolicy {
                target: target.clone(),
                grant: policy.grant.clone(),
                expected_actor: policy.expected_actor.clone(),
            },
        )?;
        Ok(Self {
            host,
            policy,
            target,
            operation,
        })
    }
    pub fn grant(&self) -> &GrantRef {
        &self.policy.grant
    }
    /// Rechecks the whole captured configuration before auth and native/cached result release.
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
                store.verify_project_context_policy(
                    &self.policy,
                    &self.target,
                    self.operation,
                    &check_control,
                )
            })
    }
}
/// Performs pure validation before opening local state or credentials; never implicitly provisions.
pub fn execute(
    args: ProjectContextArgs,
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
