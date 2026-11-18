//! Explicit local permission administration. No credential or provider access is performed here.
mod records;
#[cfg(test)]
mod tests;
use crate::dispatch::CommandResult;
use crate::output::OutputPolicy;
use crate::profiles::InvocationOptions;
use clap::{Args, ValueEnum};
use craft_core::contracts::{Envelope, GrantRef, RequestState, ResourceRef, validate_identity};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::profiles::validate_profile_name;
use craft_core::registry::{
    OperationDescriptor, ReconciliationClass, Registry, RetryClass, response_schema, schema_for,
};
use craft_runtime::profiles::{ProfileStore, resolve_state_root};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::cell::RefCell;

pub const GET: &str = "policy.get";
pub const SET: &str = "policy.set";
#[derive(Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PolicyKind {
    ConfluenceRead,
    ConfluenceWrite,
    ConfluenceAdmin,
    JiraCreate,
    JiraRead,
    JiraProjectRead,
    JiraProjectContext,
    JiraFieldRead,
    JiraMutation,
    JiraRecovery,
    TrelloRead,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PolicySnapshot {
    pub revision: u64,
    pub grant: GrantRef,
    pub target: ResourceRef,
    pub expected_actor: String,
    pub enabled: bool,
    pub operations: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_site: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_reads: Option<Vec<ProjectPermissionSnapshot>>,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectPermissionSnapshot {
    pub target: ResourceRef,
    pub operations: Vec<String>,
    pub enabled: bool,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PolicyData {
    pub profile: String,
    pub kind: PolicyKind,
    pub target: ResourceRef,
    pub policy: Option<PolicySnapshot>,
    pub proposed_policy: Option<PolicySnapshot>,
    pub configuration_write_state: RequestState,
    #[schemars(extend("const" = false))]
    pub live_verified: bool,
}
fn invalid(path: &str, rule: &str) -> CraftError {
    CraftError::field(path, rule)
}
fn parse_target(text: &str) -> Result<ResourceRef, String> {
    let value = craft_core::input::parse_object(text.as_bytes())
        .map_err(|_| "expected_strict_target_json")?;
    if value == json!({}) {
        return Ok(ResourceRef {
            provider: String::new(),
            deployment: String::new(),
            tenant: String::new(),
            resource_type: String::new(),
            id: String::new(),
        });
    }
    serde_json::from_value(value).map_err(|_| "expected_exact_resource_reference".into())
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PolicyGetArgs {
    #[arg(long, value_enum, default_value = "confluence-read")]
    pub kind: PolicyKind,
    #[arg(long,value_parser=parse_target,default_value="{}")]
    pub target: ResourceRef,
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PolicySetArgs {
    #[arg(long, value_enum, default_value = "confluence-read")]
    pub kind: PolicyKind,
    #[arg(long,value_parser=parse_target,default_value="{}")]
    pub target: ResourceRef,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 512))]
    pub expected_actor: String,
    #[arg(long,action=clap::ArgAction::Set,num_args=1)]
    pub enabled: Option<bool>,
    #[arg(long, value_delimiter = ',', num_args = 1)]
    #[schemars(length(min = 1, max = 16))]
    pub operations: Vec<String>,
    #[arg(long)]
    #[schemars(length(min = 1, max = 512))]
    pub continuation_site: Option<String>,
    #[arg(long)]
    pub expected_revision: Option<u64>,
    #[arg(long)]
    #[serde(default)]
    pub apply: bool,
}
fn profile<'a>(
    options: &'a InvocationOptions<'_>,
    fields: &[String],
) -> Result<&'a str, CraftError> {
    let profile = options
        .profile
        .ok_or_else(|| invalid("/profile", "explicit_administration_profile_required"))?;
    validate_profile_name(profile)?;
    validate_identity("/request_id", options.request_id)?;
    if options.request_id.len() > 80
        || !(512..=16_777_216).contains(&options.max_bytes)
        || !fields.is_empty()
    {
        return Err(invalid(
            "/output",
            "expected_complete_bounded_policy_receipt",
        ));
    }
    Ok(profile)
}
fn validate_target(kind: PolicyKind, target: &ResourceRef) -> Result<(), CraftError> {
    target.validate()?;
    craft_runtime::auth::validate_service_target(&target.tenant_ref())?;
    let (provider, resource) = match kind {
        PolicyKind::ConfluenceRead | PolicyKind::ConfluenceAdmin => ("confluence", "site"),
        PolicyKind::ConfluenceWrite => ("confluence", "space"),
        PolicyKind::JiraRead | PolicyKind::JiraProjectRead | PolicyKind::JiraFieldRead => {
            ("jira", "site")
        }
        PolicyKind::TrelloRead => ("trello", "account"),
        _ => ("jira", "project"),
    };
    if target.provider != provider
        || target.deployment != "cloud"
        || target.resource_type != resource
        || (matches!(resource, "site" | "account") && target.id != target.tenant)
        || (!matches!(resource, "site" | "account")
            && kind != PolicyKind::JiraProjectContext
            && !target
                .id
                .parse::<i64>()
                .is_ok_and(|id| id > 0 && id.to_string() == target.id))
    {
        return Err(invalid("/target", "expected_exact_policy_kind_target"));
    }
    if kind == PolicyKind::JiraProjectContext {
        craft_provider_jira::project_context::validate_target(target)?;
    }
    Ok(())
}
fn data(kind: PolicyKind, target: &ResourceRef, profile: &str) -> PolicyData {
    PolicyData {
        profile: profile.into(),
        kind,
        target: target.clone(),
        policy: None,
        proposed_policy: None,
        configuration_write_state: RequestState::NotSent,
        live_verified: false,
    }
}
fn preflight(
    data: &PolicyData,
    command: &str,
    options: &InvocationOptions<'_>,
) -> Result<(), CraftError> {
    let success = Envelope::success(command, options.request_id.into(), json!(data));
    let mut error = CraftError::new(ErrorCode::WriteOutcomeUnknown, "x".repeat(500));
    error.details.write_state = RequestState::SentUnknown;
    error.next_action = "x".repeat(512);
    let mut failure =
        Envelope::failure_with_data(command, options.request_id.into(), error, json!(data));
    failure.warnings = vec!["x".repeat(256)];
    for e in [success, failure] {
        if serde_json::to_vec(&e)
            .map_err(|_| invalid("/output", "cannot_encode_policy_receipt"))?
            .len()
            .saturating_add(1)
            > options.max_bytes
        {
            return Err(CraftError::new(
                ErrorCode::OutputBudgetTooSmall,
                "The output budget cannot carry the complete policy and uncertain-commit receipt. Increase max-bytes before changing configuration.",
            ));
        }
    }
    Ok(())
}
impl PolicyGetArgs {
    pub fn validate(
        &self,
        fields: &[String],
        options: &InvocationOptions<'_>,
    ) -> Result<(), CraftError> {
        profile(options, fields)?;
        validate_target(self.kind, &self.target)
    }
}
impl PolicySetArgs {
    pub fn validate(
        &self,
        fields: &[String],
        options: &InvocationOptions<'_>,
    ) -> Result<(), CraftError> {
        let name = profile(options, fields)?;
        validate_target(self.kind, &self.target)?;
        validate_identity("/expected_actor", &self.expected_actor)?;
        if matches!(self.expected_actor.as_str(), "unknown" | "anonymous") || self.enabled.is_none()
        {
            return Err(invalid(
                "/input",
                "explicit_actor_and_enabled_state_required",
            ));
        }
        if self
            .expected_revision
            .is_some_and(|v| v == 0 || v >= i64::MAX as u64)
        {
            return Err(invalid(
                "/expected_revision",
                "expected_incrementable_positive_revision",
            ));
        }
        records::validate_operations(self.kind, &self.operations)?;
        if matches!(
            self.kind,
            PolicyKind::JiraProjectRead | PolicyKind::JiraFieldRead
        ) {
            let origin = self.continuation_site.as_deref().ok_or_else(|| {
                invalid(
                    "/continuation_site",
                    "explicit_canonical_jira_origin_required",
                )
            })?;
            craft_runtime::transport::JiraProjectSite::parse(origin)?;
        } else if self.continuation_site.is_some() {
            return Err(invalid(
                "/continuation_site",
                "origin_only_for_project_or_field_site_policy",
            ));
        }
        if self.kind == PolicyKind::JiraProjectContext && self.expected_revision.is_none() {
            return Err(invalid(
                "/expected_revision",
                "existing_project_site_revision_required",
            ));
        }
        if !self.apply {
            return Err(CraftError::new(
                ErrorCode::ApplyRequired,
                "Local permission changes require explicit apply.",
            ));
        }
        let mut proposed = data(self.kind, &self.target, name);
        proposed.proposed_policy = Some(PolicySnapshot {
            revision: self.expected_revision.unwrap_or(0) + 1,
            grant: GrantRef {
                id: "g".repeat(512),
            },
            target: self.target.clone(),
            expected_actor: self.expected_actor.clone(),
            enabled: self.enabled == Some(true),
            operations: self.operations.clone(),
            continuation_site: self.continuation_site.clone(),
            context_reads: None,
        });
        proposed.configuration_write_state = RequestState::SentUnknown;
        preflight(&proposed, SET, options)
    }
}
fn result(
    command: &str,
    options: &InvocationOptions<'_>,
    data: PolicyData,
    error: Option<CraftError>,
) -> CommandResult {
    CommandResult {
        envelope: match error {
            Some(e) => {
                Envelope::failure_with_data(command, options.request_id.into(), e, json!(data))
            }
            None => Envelope::success(command, options.request_id.into(), json!(data)),
        },
        output_policy: OutputPolicy::PreserveEvidence,
    }
}
/// Uses only the explicit profile. Repository hints never choose an administration principal.
pub fn get_with_store(
    args: &PolicyGetArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    store: &ProfileStore,
    control: &InvocationControl,
) -> Result<CommandResult, CraftError> {
    args.validate(fields, options)?;
    let name = profile(options, fields)?;
    let selected = store.get(name, control)?;
    selected.validate_target(&args.target.resource_type, &args.target)?;
    if selected.provider != args.target.provider
        || selected.auth_mode != craft_core::contracts::AuthMode::Service
    {
        return Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "Policy administration requires the exact configured service profile.",
        ));
    }
    let mut output = data(args.kind, &args.target, name);
    output.policy = records::get(store, name, args.kind, &args.target, control)?;
    if output.policy.as_ref().is_some_and(|p| {
        if args.kind == PolicyKind::JiraProjectContext {
            p.target.provider != args.target.provider
                || p.target.deployment != args.target.deployment
                || p.target.tenant != args.target.tenant
                || p.target.resource_type != "site"
                || p.target.id != args.target.tenant
        } else {
            p.target != args.target
        }
    }) {
        return Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "The saved policy does not match the exact requested target.",
        ));
    }
    let envelope = Envelope::success(GET, options.request_id.into(), json!(output));
    if serde_json::to_vec(&envelope)
        .map_err(|_| invalid("/output", "cannot_encode_policy"))?
        .len()
        .saturating_add(1)
        > options.max_bytes
    {
        return Err(CraftError::new(
            ErrorCode::OutputBudgetTooSmall,
            "The selected budget cannot carry the complete saved policy.",
        ));
    }
    Ok(CommandResult {
        envelope,
        output_policy: OutputPolicy::Bounded,
    })
}
/// Freezes complete output in the existing setter's preflight before any policy commit.
pub fn set_with_store(
    args: &PolicySetArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    store: &ProfileStore,
    control: &InvocationControl,
) -> Result<CommandResult, CraftError> {
    args.validate(fields, options)?;
    let name = profile(options, fields)?;
    let proposed = RefCell::new(None);
    let update = records::set(store, name, args, control, |snapshot| {
        let mut output = data(args.kind, &args.target, name);
        output.proposed_policy = Some(snapshot.clone());
        output.configuration_write_state = RequestState::SentUnknown;
        preflight(&output, SET, options)?;
        *proposed.borrow_mut() = Some(snapshot);
        Ok(())
    });
    Ok(finish_set(
        args,
        options,
        name,
        proposed.into_inner(),
        update,
    ))
}
fn finish_set(
    args: &PolicySetArgs,
    options: &InvocationOptions<'_>,
    name: &str,
    proposed: Option<PolicySnapshot>,
    update: Result<PolicySnapshot, CraftError>,
) -> CommandResult {
    let mut output = data(args.kind, &args.target, name);
    match update {
        Ok(snapshot) => {
            output.policy = Some(snapshot);
            output.configuration_write_state = RequestState::Confirmed;
            result(SET, options, output, None)
        }
        Err(mut error) => {
            if error.code == ErrorCode::WriteOutcomeUnknown
                || error.details.write_state == RequestState::SentUnknown
            {
                output.configuration_write_state = RequestState::SentUnknown;
                output.proposed_policy = proposed;
                error.details.write_state = RequestState::SentUnknown;
                error.next_action="Read the same profile/kind/target under the same state root before retrying. Preserve competing revisions; no automatic retry is authorized.".into();
            } else {
                error.details.write_state = RequestState::NotSent;
            }
            result(SET, options, output, Some(error))
        }
    }
}
pub enum PolicyArgs {
    Get(PolicyGetArgs),
    Set(PolicySetArgs),
}
impl PolicyArgs {
    pub fn command(&self) -> &'static str {
        match self {
            Self::Get(_) => GET,
            Self::Set(_) => SET,
        }
    }
}
pub fn execute(
    args: PolicyArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> CommandResult {
    let command = args.command();
    let outcome = (|| {
        match &args {
            PolicyArgs::Get(v) => v.validate(fields, options)?,
            PolicyArgs::Set(v) => v.validate(fields, options)?,
        }
        let root = resolve_state_root(options.state_root, |key| std::env::var_os(key))?;
        let store = ProfileStore::open(&root, control)?;
        match &args {
            PolicyArgs::Get(v) => get_with_store(v, fields, options, &store, control),
            PolicyArgs::Set(v) => set_with_store(v, fields, options, &store, control),
        }
    })();
    outcome.unwrap_or_else(|error| CommandResult {
        envelope: Envelope::failure(command, options.request_id.into(), error),
        output_policy: OutputPolicy::PreserveEvidence,
    })
}
/// Separate candidate registration permits complete review without enabling the public root.
pub fn register_candidates(registry: &mut Registry) -> Result<(), CraftError> {
    for command in [GET, SET] {
        let target = json!({"provider":"confluence","deployment":"cloud","tenant":"00000000-1111-2222-3333-444444444444","resource_type":"site","id":"00000000-1111-2222-3333-444444444444"});
        let example = if command == GET {
            json!({"kind":"confluence-read","target":target})
        } else {
            json!({"kind":"confluence-read","target":target,"expected_actor":"service-actor","enabled":true,"operations":["confluence.page.get"],"expected_revision":null,"apply":true})
        };
        let mut output = json!({"profile":"factory","kind":"confluence-read","target":target,"policy":null,"proposed_policy":null,"configuration_write_state":"not_sent","live_verified":false});
        if command == SET {
            output["configuration_write_state"] = json!("confirmed");
            output["policy"] = json!({"revision":1,"grant":{"id":"example-profile-grant"},"target":target,"expected_actor":"service-actor","enabled":true,"operations":["confluence.page.get"]});
        }
        let descriptor=OperationDescriptor{id:command.into(),provider:"craft".into(),contract_version:craft_core::contracts::SCHEMA_VERSION.into(),api_revision:"local-policy-v1".into(),description:if command==GET {"Read explicit-profile saved permissions without credentials or live verification."}else{"Change explicit-profile local permission intent with revision CAS; this does not grant remote permission."}.into(),effect:if command==GET {craft_core::contracts::EffectClass::Read}else{craft_core::contracts::EffectClass::AccessChange},input_schema:if command==GET {schema_for::<PolicyGetArgs>()?}else{schema_for::<PolicySetArgs>()?},response_schema:response_schema::<PolicyData>()?,example,response_example:json!(Envelope::success(command,"example".into(),output)),projection:None,scope_alternatives:vec![],permissions:vec!["Explicit local trusted-profile administration; no server permission proof.".into()],tenant_conditions:vec![],routes:vec![],retry_class:if command==GET {RetryClass::LocalRead}else{RetryClass::ReconcileBeforeReplay},reconciliation:if command==GET {ReconciliationClass::NotApplicable}else{ReconciliationClass::KnownResource}};
        if command == GET {
            registry.register_typed::<PolicyGetArgs>(descriptor)?;
        } else {
            registry.register_typed::<PolicySetArgs>(descriptor)?;
        }
    }
    Ok(())
}
