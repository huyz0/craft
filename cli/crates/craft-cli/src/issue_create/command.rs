//! Typed candidate dispatch; trusted private policy supplies authority, not command arguments.

use std::collections::BTreeMap;
use std::sync::Arc;

use clap::Args;
use craft_core::contracts::{AuthMode, CanonicalRequest, Selection};
use craft_core::control::InvocationControl;
use craft_core::presence::InputField;
use craft_core::profiles::validate_profile_name;
use craft_core::registry::{Registry, schema_for};
use craft_provider_jira::create_metadata::CreateInput;
use craft_runtime::auth::ServiceOAuthClient;
use craft_runtime::control::SystemClock;
use craft_runtime::create_policy::CreatePolicyRecord;
use craft_runtime::journal::NativeJournal;
use craft_runtime::profiles::{ProfileStore, resolve_state_root};
use craft_runtime::secrets::{LockedCredentials, NativeCredentials};
use craft_runtime::transport::TlsPolicy;

use super::host::{CreatePolicy, PreparedCreate};
use super::*;
use crate::profiles::{InvocationOptions, selected_profile_context};

#[cfg(test)]
mod tests;

fn literal(value: &str) -> Result<InputField<String>, String> {
    Ok(InputField::Value(value.into()))
}
fn provided<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    decoder: D,
) -> Result<Option<InputField<T>>, D::Error> {
    InputField::deserialize(decoder).map(Some)
}

#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueCreateArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 512))]
    pub operation_id: String,
    #[arg(long)]
    #[serde(default)]
    pub apply: bool,
    #[arg(long,value_parser=literal)]
    #[serde(
        default,
        deserialize_with = "provided",
        skip_serializing_if = "Option::is_none"
    )]
    pub project_id: Option<InputField<String>>,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 512), regex(pattern = "^[0-9]+$"))]
    pub issue_type_id: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 255))]
    pub summary: String,
    #[arg(long,value_parser=literal)]
    #[serde(
        default,
        deserialize_with = "provided",
        skip_serializing_if = "Option::is_none"
    )]
    pub description: Option<InputField<String>>,
    #[arg(long,value_parser=literal)]
    #[serde(
        default,
        deserialize_with = "provided",
        skip_serializing_if = "Option::is_none"
    )]
    pub parent_issue_id: Option<InputField<String>>,
    #[arg(long,value_parser=literal)]
    #[serde(
        default,
        deserialize_with = "provided",
        skip_serializing_if = "Option::is_none"
    )]
    pub assignee_account_id: Option<InputField<String>>,
    #[arg(skip)]
    #[serde(
        default,
        deserialize_with = "provided",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(extend("x-craft-input-channel"="json_only"))]
    pub labels: Option<InputField<Vec<String>>>,
    #[arg(skip)]
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(extend("x-craft-input-channel"="json_only"))]
    pub custom_fields: BTreeMap<String, Value>,
}

impl IssueCreateArgs {
    fn input(&self) -> CreateInput {
        CreateInput {
            issue_type_id: self.issue_type_id.clone(),
            summary: self.summary.clone(),
            description: self.description.clone().unwrap_or_default(),
            labels: self.labels.clone().unwrap_or_default(),
            custom_fields: self.custom_fields.clone(),
            parent_issue_id: self.parent_issue_id.clone().unwrap_or_default(),
            assignee_account_id: self.assignee_account_id.clone().unwrap_or_default(),
        }
    }
}

/// Candidate registration remains separate from startup/public provider registration.
pub fn register_candidate(registry: &mut Registry) -> Result<(), CraftError> {
    let mut descriptor = super::host::descriptor()?;
    descriptor.input_schema = schema_for::<IssueCreateArgs>()?;
    descriptor.response_schema = crate::saved_requests::public_schema::<IssueCreateData>()?;
    descriptor.projection = Some(crate::saved_requests::projection(projection()));
    descriptor.response_example["data"]["saved_request"] = Value::Null;
    descriptor.example = json!({"operation_id":"create-report","apply":true,"issue_type_id":"10001","summary":"Example"});
    registry.register_typed::<IssueCreateArgs>(descriptor)
}

fn validate(
    args: &IssueCreateArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
) -> Result<(), CraftError> {
    validate_budget(args, fields, options, true)
}
fn validate_budget(
    args: &IssueCreateArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    saved: bool,
) -> Result<(), CraftError> {
    let fields = crate::saved_requests::native_fields::<IssueCreateData>(projection(), fields)?;
    let maximum = if saved {
        crate::saved_requests::native_budget(options.max_bytes)?
    } else {
        options.max_bytes
    };
    craft_core::contracts::validate_identity("/operation_id", &args.operation_id)?;
    if let Some(profile) = options.profile {
        validate_profile_name(profile)?;
    }
    args.input().validate_basic()?;
    if let Some(project) = &args.project_id
        && !matches!(project,InputField::Value(id) if !id.is_empty() && id.len()<=512 && id.bytes().all(|byte|byte.is_ascii_digit()))
    {
        return Err(CraftError::field(
            "/project_id",
            "expected_numeric_project_id",
        ));
    }
    if !args.apply {
        return Err(CraftError::new(
            ErrorCode::ApplyRequired,
            "Issue creation requires explicit apply.",
        ));
    }
    let encoded = serde_json::to_vec(args)
        .map_err(|_| CraftError::field("/input", "invalid_create_arguments"))?;
    craft_core::input::parse_object(&encoded)?;
    let target = ResourceRef {
        provider: "jira".into(),
        deployment: "cloud".into(),
        tenant: "00000000-1111-2222-3333-444444444444".into(),
        resource_type: "project".into(),
        id: "9".repeat(512),
    };
    // Reserve the maximum target size before even opening private state. No placeholder is emitted.
    CreateCompletion::prepare(
        OperationBinding {
            operation_id: args.operation_id.clone(),
            request_digest: format!("sha256:{}", "0".repeat(64)),
            descriptor_digest: format!("sha256:{}", "0".repeat(64)),
            target,
            grant: GrantRef {
                id: "g".repeat(512),
            },
            actor: "a".repeat(512),
        },
        options.request_id,
        &fields,
        maximum,
    )?;
    Ok(())
}

pub struct PreparedInvocation {
    host: PreparedCreate,
    policy: CreatePolicyRecord,
}
impl PreparedInvocation {
    /// Validates before profile/hint reads, then requires one exact enabled private policy.
    pub fn prepare(
        args: IssueCreateArgs,
        fields: &[String],
        options: &InvocationOptions<'_>,
        store: &ProfileStore,
        control: &InvocationControl,
    ) -> Result<Self, CraftError> {
        validate_budget(&args, fields, options, false)?;
        let fields = crate::saved_requests::native_fields::<IssueCreateData>(projection(), fields)?;
        control.check()?;
        let (name, _, hints) = selected_profile_context(store, None, options, control)?;
        let profile = store.get(&name, control)?;
        if profile.provider != "jira"
            || profile.deployment != "cloud"
            || profile.auth_mode != AuthMode::Service
        {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Create requires a configured Jira Cloud service profile.",
            ));
        }
        let explicit = match &args.project_id {
            Some(InputField::Value(id)) => Some(ResourceRef {
                resource_type: "project".into(),
                id: id.clone(),
                provider: profile.provider.clone(),
                deployment: profile.deployment.clone(),
                tenant: profile.tenant_ref()?.tenant,
            }),
            _ => None,
        };
        let target = profile.resolve_target("project", explicit.as_ref(), &hints)?;
        let policy = store.create_policy(&name, control)?.ok_or_else(|| {
            CraftError::new(
                ErrorCode::PolicyDenied,
                "No trusted create policy is configured for this profile.",
            )
        })?;
        if policy.target != target {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Selected project differs from the trusted create policy.",
            ));
        }
        store.verify_create_policy(&policy, control)?;
        let request = CanonicalRequest {
            command: COMMAND.into(),
            target: target.clone(),
            input: serde_json::to_value(args.input())
                .map_err(|_| CraftError::field("/input", "invalid_typed_create_input"))?,
            selection: Selection {
                fields,
                max_bytes: options
                    .max_bytes
                    .try_into()
                    .map_err(|_| CraftError::field("/max_bytes", "invalid_output_budget"))?,
                ..Default::default()
            },
            preconditions: None,
        };
        let host = PreparedCreate::prepare(
            request,
            &args.operation_id,
            args.apply,
            options.request_id,
            CreatePolicy {
                enabled: policy.enabled,
                target,
                grant: policy.grant.clone(),
                expected_actor: policy.expected_actor.clone(),
            },
        )?;
        Ok(Self { host, policy })
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
        let Self { host, policy } = self;
        match host.retain_source(store, journal, &control) {
            Ok((host, receipt)) => crate::saved_requests::attach(
                Self { host, policy }.execute_cached(store, client, locked, journal, tls, control),
                receipt,
            ),
            Err(failure) => *failure,
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
        let policy_control = control.clone();
        self.host
            .execute_cached_with_policy_check(client, locked, journal, tls, control, || {
                store.verify_create_policy(&self.policy, &policy_control)
            })
    }
}

pub fn execute(
    args: IssueCreateArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> CommandResult {
    let result = (|| {
        validate(&args, fields, options)?;
        let root = resolve_state_root(options.state_root, |key| std::env::var_os(key))?;
        let store = ProfileStore::open(&root, control)?;
        let prepared = PreparedInvocation::prepare(args, fields, options, &store, control)?;
        let credentials = NativeCredentials::open(&store)?;
        let locked = credentials.lock(prepared.grant(), control)?;
        let journal = NativeJournal::open(&store, control).map_err(|_| {
            CraftError::new(
                ErrorCode::Internal,
                "Cannot open the selected operation journal.",
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
        envelope: Envelope::failure(COMMAND, options.request_id.into(), error),
        output_policy: OutputPolicy::PreserveEvidence,
    })
}
