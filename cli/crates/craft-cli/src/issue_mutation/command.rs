//! Typed candidates capture explicit private project policy before opening service access.
use super::host::{IssueMutationPolicy, MutationPermission, PreparedIssueMutation};
use super::*;
use crate::profiles::{InvocationOptions, selected_profile_context};
use clap::Args;
use craft_core::contracts::{AuthMode, CanonicalRequest, GrantRef, Selection};
use craft_core::control::InvocationControl;
use craft_core::profiles::validate_profile_name;
use craft_core::registry::{Registry, schema_for};
use craft_provider_jira::issue_context::IssueState;
use craft_provider_jira::issue_context::mutation::{
    IssueChanges, IssueEditInput, IssueTransitionInput,
};
use craft_runtime::auth::ServiceOAuthClient;
use craft_runtime::control::SystemClock;
use craft_runtime::issue_mutation_policy::{
    IssueMutationOperation as Operation, IssueMutationPolicyRecord,
};
use craft_runtime::journal::NativeJournal;
use craft_runtime::profiles::{ProfileStore, resolve_state_root};
use craft_runtime::secrets::{LockedCredentials, NativeCredentials};
use craft_runtime::transport::TlsPolicy;
use std::sync::Arc;
mod actions;
pub use actions::{IssueAssignArgs, IssueDeleteArgs, IssueUnassignArgs};
use craft_provider_jira::issue_context::mutation::actions::{
    IssueAssignmentInput, IssueDeleteInput, IssueUnassignInput, SubtaskDeletion,
};
#[cfg(test)]
mod tests;

fn empty_expected() -> IssueState {
    IssueState {
        id: String::new(),
        key: String::new(),
        project_id: String::new(),
        issue_type_id: String::new(),
        status_id: String::new(),
        updated: String::new(),
    }
}
fn expected(text: &str) -> Result<IssueState, String> {
    if text == "{}" {
        return Ok(empty_expected());
    }
    let value = craft_core::input::parse_object(text.as_bytes())
        .map_err(|_| "invalid expected-state JSON")?;
    serde_json::from_value(value).map_err(|_| "invalid expected-state fields".into())
}
fn changes(text: &str) -> Result<IssueChanges, String> {
    let value =
        craft_core::input::parse_object(text.as_bytes()).map_err(|_| "invalid change JSON")?;
    serde_json::from_value(value).map_err(|_| "invalid change fields".into())
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueUpdateArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 512))]
    pub operation_id: String,
    #[arg(long)]
    #[serde(default)]
    pub apply: bool,
    #[arg(long)]
    #[schemars(length(min = 36, max = 36))]
    pub cloud_id: Option<String>,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 512), regex(pattern = "^[0-9]+$"))]
    pub issue: String,
    #[arg(long,value_parser=expected,default_value="{}")]
    pub expected: IssueState,
    #[arg(long,value_parser=changes,default_value="{}")]
    pub changes: IssueChanges,
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransitionApplyArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 512))]
    pub operation_id: String,
    #[arg(long)]
    #[serde(default)]
    pub apply: bool,
    #[arg(long)]
    #[schemars(length(min = 36, max = 36))]
    pub cloud_id: Option<String>,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 512), regex(pattern = "^[0-9]+$"))]
    pub issue: String,
    #[arg(long,value_parser=expected,default_value="{}")]
    pub expected: IssueState,
    #[arg(long,value_parser=changes,default_value="{}")]
    pub changes: IssueChanges,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 512))]
    pub transition_id: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 512), regex(pattern = "^[0-9]+$"))]
    pub expected_status_id: String,
}
pub enum IssueMutationArgs {
    Edit(IssueUpdateArgs),
    Transition(TransitionApplyArgs),
    Assign(IssueAssignArgs),
    Unassign(IssueUnassignArgs),
    Delete(IssueDeleteArgs),
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
impl IssueMutationArgs {
    pub fn operation(&self) -> Operation {
        match self {
            Self::Edit(_) => Operation::Edit,
            Self::Transition(_) => Operation::Transition,
            Self::Assign(_) => Operation::Assign,
            Self::Unassign(_) => Operation::Unassign,
            Self::Delete(args) => {
                if matches!(args.subtasks, Some(SubtaskDeletion::DeleteAll { .. })) {
                    Operation::DeleteAllSubtasks
                } else {
                    Operation::Delete
                }
            }
        }
    }
    fn issue(&self) -> &str {
        match self {
            Self::Edit(args) => &args.issue,
            Self::Transition(args) => &args.issue,
            Self::Assign(args) => &args.issue,
            Self::Unassign(args) => &args.issue,
            Self::Delete(args) => &args.issue,
        }
    }
    fn cloud(&self) -> Option<&str> {
        match self {
            Self::Edit(args) => args.cloud_id.as_deref(),
            Self::Transition(args) => args.cloud_id.as_deref(),
            Self::Assign(args) => args.cloud_id.as_deref(),
            Self::Unassign(args) => args.cloud_id.as_deref(),
            Self::Delete(args) => args.cloud_id.as_deref(),
        }
    }
    fn operation_id(&self) -> &str {
        match self {
            Self::Edit(args) => &args.operation_id,
            Self::Transition(args) => &args.operation_id,
            Self::Assign(args) => &args.operation_id,
            Self::Unassign(args) => &args.operation_id,
            Self::Delete(args) => &args.operation_id,
        }
    }
    fn apply(&self) -> bool {
        match self {
            Self::Edit(args) => args.apply,
            Self::Transition(args) => args.apply,
            Self::Assign(args) => args.apply,
            Self::Unassign(args) => args.apply,
            Self::Delete(args) => args.apply,
        }
    }
    fn expected(&self) -> &IssueState {
        match self {
            Self::Edit(args) => &args.expected,
            Self::Transition(args) => &args.expected,
            Self::Assign(args) => &args.expected,
            Self::Unassign(args) => &args.expected,
            Self::Delete(args) => &args.expected,
        }
    }
    fn input(&self) -> Result<Value, CraftError> {
        match self {
            Self::Edit(args) => serde_json::to_value(IssueEditInput {
                issue: args.issue.clone(),
                expected: args.expected.clone(),
                changes: args.changes.clone(),
            }),
            Self::Transition(args) => serde_json::to_value(IssueTransitionInput {
                issue: args.issue.clone(),
                expected: args.expected.clone(),
                changes: args.changes.clone(),
                transition_id: args.transition_id.clone(),
                expected_status_id: args.expected_status_id.clone(),
            }),
            Self::Assign(args) => serde_json::to_value(IssueAssignmentInput {
                issue: args.issue.clone(),
                expected: args.expected.clone(),
                expected_assignee: args.expected_assignee.clone().ok_or_else(|| {
                    CraftError::field("/expected_assignee", "expected_explicit_assignee")
                })?,
                account_id: args.account_id.clone(),
            }),
            Self::Unassign(args) => serde_json::to_value(IssueUnassignInput {
                issue: args.issue.clone(),
                expected: args.expected.clone(),
                expected_assignee: args.expected_assignee.clone().ok_or_else(|| {
                    CraftError::field("/expected_assignee", "expected_explicit_assignee")
                })?,
            }),
            Self::Delete(args) => serde_json::to_value(IssueDeleteInput {
                issue: args.issue.clone(),
                expected: args.expected.clone(),
                subtasks: args.subtasks.clone().ok_or_else(|| {
                    CraftError::field("/subtasks", "expected_explicit_subtask_mode")
                })?,
            }),
        }
        .map_err(|_| CraftError::field("/input", "invalid_mutation_arguments"))
    }
    fn request(
        &self,
        target: ResourceRef,
        fields: &[String],
        maximum: usize,
    ) -> Result<CanonicalRequest, CraftError> {
        Ok(CanonicalRequest {
            command: self.operation().command().into(),
            target,
            input: self.input()?,
            selection: Selection {
                fields: fields.to_vec(),
                max_bytes: maximum
                    .try_into()
                    .map_err(|_| CraftError::field("/max_bytes", "invalid_budget"))?,
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
        self.validate_budget(fields, options, true)
    }
    fn validate_budget(
        &self,
        fields: &[String],
        options: &InvocationOptions<'_>,
        saved: bool,
    ) -> Result<(), CraftError> {
        let fields =
            crate::saved_requests::native_fields::<IssueMutationData>(projection(), fields)?;
        let maximum = if saved {
            crate::saved_requests::native_budget(options.max_bytes)?
        } else {
            options.max_bytes
        };
        craft_core::contracts::validate_identity("/operation_id", self.operation_id())?;
        if let Some(profile) = options.profile {
            validate_profile_name(profile)?;
        }
        let site = site(
            self.cloud()
                .unwrap_or("00000000-1111-2222-3333-444444444444"),
        );
        craft_provider_jira::projects::validate_target(&site)?;
        let target = ResourceRef {
            resource_type: "issue".into(),
            id: self.issue().into(),
            ..site
        };
        let request = self.request(target.clone(), &fields, options.max_bytes)?;
        match self {
            Self::Edit(_) | Self::Transition(_) => mutation::validate_request(&request)?,
            _ => craft_provider_jira::issue_context::mutation::actions::validate_request(&request)?,
        }
        if !self.apply() {
            return Err(CraftError::new(
                ErrorCode::ApplyRequired,
                "Issue mutations require explicit apply.",
            ));
        }
        MutationCompletion::prepare(
            OperationBinding {
                operation_id: self.operation_id().into(),
                request_digest: format!("sha256:{}", "0".repeat(64)),
                descriptor_digest: format!("sha256:{}", "0".repeat(64)),
                target,
                grant: GrantRef {
                    id: "g".repeat(512),
                },
                actor: "a".repeat(512),
            },
            self.operation().command(),
            options.request_id,
            &fields,
            maximum,
        )?;
        Ok(())
    }
}
pub fn register_candidates(registry: &mut Registry) -> Result<(), CraftError> {
    let native = super::host::registry()?;
    let public_response = crate::saved_requests::public_schema::<IssueMutationData>()?;
    let public_projection = crate::saved_requests::projection(projection());
    for command in [
        EDIT_ISSUE,
        TRANSITION_ISSUE,
        ASSIGN_ISSUE,
        UNASSIGN_ISSUE,
        DELETE_ISSUE,
    ] {
        let mut descriptor = native.descriptor(command)?.clone();
        descriptor.response_schema = public_response.clone();
        descriptor.projection = Some(public_projection.clone());
        descriptor.response_example["data"]["saved_request"] = Value::Null;
        descriptor.example["operation_id"] = json!("issue-change");
        descriptor.example["apply"] = json!(true);
        match command {
            EDIT_ISSUE => {
                descriptor.input_schema = schema_for::<IssueUpdateArgs>()?;
                registry.register_typed::<IssueUpdateArgs>(descriptor)?;
            }
            TRANSITION_ISSUE => {
                descriptor.input_schema = schema_for::<TransitionApplyArgs>()?;
                registry.register_typed::<TransitionApplyArgs>(descriptor)?;
            }
            ASSIGN_ISSUE => {
                descriptor.input_schema = schema_for::<IssueAssignArgs>()?;
                registry.register_typed::<IssueAssignArgs>(descriptor)?;
            }
            UNASSIGN_ISSUE => {
                descriptor.input_schema = schema_for::<IssueUnassignArgs>()?;
                registry.register_typed::<IssueUnassignArgs>(descriptor)?;
            }
            _ => {
                descriptor.input_schema = schema_for::<IssueDeleteArgs>()?;
                registry.register_typed::<IssueDeleteArgs>(descriptor)?;
            }
        }
    }
    Ok(())
}
pub struct PreparedInvocation {
    host: PreparedIssueMutation,
    policy: IssueMutationPolicyRecord,
    operation: Operation,
}
impl PreparedInvocation {
    pub fn prepare(
        args: IssueMutationArgs,
        fields: &[String],
        options: &InvocationOptions<'_>,
        store: &ProfileStore,
        control: &InvocationControl,
    ) -> Result<Self, CraftError> {
        args.validate_budget(fields, options, false)?;
        let fields =
            crate::saved_requests::native_fields::<IssueMutationData>(projection(), fields)?;
        control.check()?;
        let (name, _, hints) = selected_profile_context(store, None, options, control)?;
        let profile = store.get(&name, control)?;
        if profile.provider != "jira"
            || profile.deployment != "cloud"
            || profile.auth_mode != AuthMode::Service
        {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Mutations require a Jira Cloud service profile.",
            ));
        }
        let explicit = args.cloud().map(site);
        let site = if explicit.is_some()
            || hints.targets.contains_key("site")
            || profile.defaults.contains_key("site")
        {
            profile.resolve_target("site", explicit.as_ref(), &hints)?
        } else {
            site(&profile.tenant_ref()?.tenant)
        };
        profile.validate_target("site", &site)?;
        craft_provider_jira::projects::validate_target(&site)?;
        let project = ResourceRef {
            resource_type: "project".into(),
            id: args.expected().project_id.clone(),
            ..site.clone()
        };
        profile.validate_target("project", &project)?;
        let operation = args.operation();
        let policy = store
            .issue_mutation_policy(&name, &project.id, control)?
            .ok_or_else(|| {
                CraftError::new(
                    ErrorCode::PolicyDenied,
                    "No explicit mutation policy is configured for this project.",
                )
            })?;
        if policy.target != project {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Selected project differs from trusted mutation policy.",
            ));
        }
        store.verify_issue_mutation_policy(&policy, operation, control)?;
        let host = PreparedIssueMutation::prepare(
            args.request(
                ResourceRef {
                    resource_type: "issue".into(),
                    id: args.issue().into(),
                    ..site
                },
                &fields,
                options.max_bytes,
            )?,
            args.operation_id(),
            args.apply(),
            options.request_id,
            IssueMutationPolicy {
                enabled: policy.enabled,
                project,
                grant: policy.grant.clone(),
                expected_actor: policy.expected_actor.clone(),
                permissions: policy
                    .operations
                    .iter()
                    .filter_map(|op| match op {
                        Operation::Edit => Some(MutationPermission::Edit),
                        Operation::Transition => Some(MutationPermission::Transition),
                        Operation::Assign => Some(MutationPermission::Assign),
                        Operation::Unassign => Some(MutationPermission::Unassign),
                        Operation::Delete => Some(MutationPermission::Delete),
                        Operation::DeleteAllSubtasks => Some(MutationPermission::DeleteAllSubtasks),
                        Operation::Link
                        | Operation::LinkDelete
                        | Operation::CommentCreate
                        | Operation::CommentUpdateOwn
                        | Operation::CommentUpdateAny
                        | Operation::CommentDeleteOwn
                        | Operation::CommentDeleteAny
                        | Operation::AttachmentUpload => None,
                    })
                    .collect(),
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
    pub fn execute_saved(
        self,
        store: &ProfileStore,
        client: &ServiceOAuthClient,
        locked: &LockedCredentials<'_>,
        journal: &NativeJournal,
        tls: TlsPolicy,
        control: InvocationControl,
    ) -> CommandResult {
        let (original, binding) = self.host.saved_source();
        if let Err(error) = self.host.preflight_saved() {
            let (prior, unavailable) =
                crate::saved_requests::failure_prior(journal, binding, &control);
            return crate::saved_requests::attach_failure(
                self.host.reject_capture(error, prior.as_ref()),
                None,
                unavailable,
            );
        }
        let step = route(&original.command).unwrap_or("");
        match crate::saved_requests::capture(store, journal, original, binding, step, &control) {
            Ok(receipt) => crate::saved_requests::attach(
                self.execute_cached(store, client, locked, journal, tls, control),
                receipt,
            ),
            Err(failure) => crate::saved_requests::attach_failure(
                self.host
                    .reject_capture(failure.error, failure.prior.as_ref()),
                failure.receipt,
                failure.evidence_unavailable,
            ),
        }
    }
    pub fn execute_cached(
        self,
        store: &ProfileStore,
        client: &ServiceOAuthClient,
        locked: &LockedCredentials<'_>,
        journal: &NativeJournal,
        tls: TlsPolicy,
        control: InvocationControl,
    ) -> CommandResult {
        let check = control.clone();
        self.host
            .execute_cached_with_policy_check(client, locked, journal, tls, control, || {
                store.verify_issue_mutation_policy(&self.policy, self.operation, &check)
            })
    }
}
pub fn execute(
    args: IssueMutationArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> CommandResult {
    let command = args.operation().command();
    let result = (|| {
        args.validate(fields, options)?;
        let root = resolve_state_root(options.state_root, |key| std::env::var_os(key))?;
        let store = ProfileStore::open(&root, control)?;
        let prepared = PreparedInvocation::prepare(args, fields, options, &store, control)?;
        let credentials = NativeCredentials::open(&store)?;
        let locked = credentials.lock(prepared.grant(), control)?;
        let journal = NativeJournal::open(&store, control).map_err(|_| {
            CraftError::new(
                ErrorCode::Internal,
                "Cannot open the selected mutation journal.",
            )
        })?;
        let client = ServiceOAuthClient::new(
            TlsPolicy::Platform,
            control.clone(),
            Arc::new(SystemClock::default()),
        )?;
        Ok(prepared.execute_saved(
            &store,
            &client,
            &locked,
            &journal,
            TlsPolicy::Platform,
            control.clone(),
        ))
    })();
    result.unwrap_or_else(|error| CommandResult {
        envelope: Envelope::failure(command, options.request_id.into(), error),
        output_policy: OutputPolicy::PreserveEvidence,
    })
}
