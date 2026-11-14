//! Explicit local reviewer administration. Configuration is not a verified human approval.
use crate::dispatch::CommandResult;
use crate::output::OutputPolicy;
use crate::profiles::InvocationOptions;
use clap::Args;
use craft_core::contracts::{
    AuthMode, Envelope, GrantRef, RequestState, ResourceRef, validate_identity,
};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::profiles::validate_profile_name;
use craft_core::registry::{
    OperationDescriptor, ReconciliationClass, Registry, RetryClass, response_schema, schema_for,
};
use craft_runtime::profiles::{ProfileStore, resolve_state_root};
use craft_runtime::review_policy::{ReviewPolicyRecord, ReviewPolicyUpdate, validate_eligibility};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::cell::RefCell;
pub const GET: &str = "review.policy.get";
pub const SET: &str = "review.policy.set";
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TargetArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 19), regex(pattern = "^[1-9][0-9]*$"))]
    pub space_id: String,
    #[arg(long)]
    #[schemars(length(min = 36, max = 36))]
    pub cloud_id: Option<String>,
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetArgs {
    #[command(flatten)]
    #[serde(flatten)]
    pub target: TargetArgs,
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetArgs {
    #[command(flatten)]
    #[serde(flatten)]
    pub target: TargetArgs,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 512))]
    pub expected_actor: String,
    #[arg(long,action=clap::ArgAction::Set,num_args=1)]
    pub enabled: Option<bool>,
    #[arg(long, value_delimiter = ',', num_args = 1)]
    #[schemars(length(min = 1, max = 64))]
    pub reviewers: Vec<String>,
    #[arg(long, value_delimiter = ',', num_args = 1)]
    #[serde(default)]
    #[schemars(length(max = 64))]
    pub excluded_actors: Vec<String>,
    #[arg(long, default_value_t = 1)]
    #[serde(default = "one")]
    #[schemars(range(min = 1, max = 64))]
    pub quorum: u16,
    #[arg(long)]
    pub expected_revision: Option<u64>,
    #[arg(long)]
    #[serde(default)]
    pub apply: bool,
}
fn one() -> u16 {
    1
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub revision: u64,
    pub grant: GrantRef,
    pub target: ResourceRef,
    pub expected_actor: String,
    pub enabled: bool,
    pub reviewers: Vec<String>,
    pub excluded_actors: Vec<String>,
    pub quorum: u16,
}
impl From<&ReviewPolicyRecord> for Snapshot {
    fn from(r: &ReviewPolicyRecord) -> Self {
        Self {
            revision: r.revision,
            grant: r.grant.clone(),
            target: r.target.clone(),
            expected_actor: r.expected_actor.clone(),
            enabled: r.enabled,
            reviewers: r.reviewers.clone(),
            excluded_actors: r.excluded_actors.clone(),
            quorum: r.quorum,
        }
    }
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PolicyData {
    pub profile: String,
    pub target: ResourceRef,
    pub policy: Option<Snapshot>,
    pub proposed_policy: Option<Snapshot>,
    pub configuration_write_state: RequestState,
    #[schemars(extend("const"=false))]
    pub live_verified: bool,
    #[schemars(extend("const"=false))]
    pub review_verified: bool,
}
fn context<'a>(options: &InvocationOptions<'a>, fields: &[String]) -> Result<&'a str, CraftError> {
    let name = options
        .profile
        .ok_or_else(|| CraftError::field("/profile", "explicit_administration_profile_required"))?;
    validate_profile_name(name)?;
    validate_identity("/request_id", options.request_id)?;
    if options.request_id.len() > 80
        || !(512..=16_777_216).contains(&options.max_bytes)
        || !fields.is_empty()
    {
        return Err(CraftError::field(
            "/output",
            "expected_complete_bounded_reviewer_policy_receipt",
        ));
    }
    Ok(name)
}
fn space(cloud: &str, id: &str) -> ResourceRef {
    ResourceRef {
        provider: "confluence".into(),
        deployment: "cloud".into(),
        tenant: cloud.into(),
        resource_type: "space".into(),
        id: id.into(),
    }
}
impl TargetArgs {
    fn validate(&self) -> Result<(), CraftError> {
        if !craft_runtime::review_policy::numeric(&self.space_id) {
            return Err(CraftError::field(
                "/space_id",
                "expected_canonical_positive_space_id",
            ));
        }
        if let Some(cloud) = &self.cloud_id {
            craft_runtime::auth::validate_service_target(
                &space(cloud, &self.space_id).tenant_ref(),
            )?;
        }
        Ok(())
    }
    fn provisional(&self) -> ResourceRef {
        space(
            self.cloud_id
                .as_deref()
                .unwrap_or("00000000-1111-2222-3333-444444444444"),
            &self.space_id,
        )
    }
    fn resolve(
        &self,
        name: &str,
        store: &ProfileStore,
        control: &InvocationControl,
    ) -> Result<ResourceRef, CraftError> {
        let profile = store.get(name, control)?;
        if profile.provider != "confluence"
            || profile.deployment != "cloud"
            || profile.auth_mode != AuthMode::Service
        {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Reviewer administration requires an explicit Confluence service profile.",
            ));
        }
        let target = space(
            self.cloud_id
                .as_deref()
                .unwrap_or(&profile.tenant_ref()?.tenant),
            &self.space_id,
        );
        profile.validate_target("space", &target)?;
        Ok(target)
    }
}
fn data(name: &str, target: ResourceRef) -> PolicyData {
    PolicyData {
        profile: name.into(),
        target,
        policy: None,
        proposed_policy: None,
        configuration_write_state: RequestState::NotSent,
        live_verified: false,
        review_verified: false,
    }
}
fn preflight(
    command: &str,
    data: &PolicyData,
    options: &InvocationOptions<'_>,
) -> Result<(), CraftError> {
    let mut error = CraftError::new(ErrorCode::WriteOutcomeUnknown, "x".repeat(500));
    error.details.write_state = RequestState::SentUnknown;
    error.next_action = "x".repeat(512);
    let mut failure =
        Envelope::failure_with_data(command, options.request_id.into(), error, json!(data));
    failure.warnings = vec!["x".repeat(256), "x".repeat(256)];
    for envelope in [
        Envelope::success(command, options.request_id.into(), json!(data)),
        failure,
    ] {
        if serde_json::to_vec(&envelope)
            .map_err(|_| CraftError::field("/output", "invalid_policy_receipt"))?
            .len()
            .saturating_add(1)
            > options.max_bytes
        {
            return Err(CraftError::new(
                ErrorCode::OutputBudgetTooSmall,
                "Increase max-bytes before reviewer administration so complete confirmed or uncertain policy receipts can be retained.",
            ));
        }
    }
    Ok(())
}
impl GetArgs {
    pub fn validate(
        &self,
        fields: &[String],
        options: &InvocationOptions<'_>,
    ) -> Result<(), CraftError> {
        let name = context(options, fields)?;
        self.target.validate()?;
        preflight(GET, &data(name, self.target.provisional()), options)
    }
}
impl SetArgs {
    pub fn validate(
        &self,
        fields: &[String],
        options: &InvocationOptions<'_>,
    ) -> Result<(), CraftError> {
        let name = context(options, fields)?;
        self.target.validate()?;
        validate_eligibility(
            &self.expected_actor,
            &self.reviewers,
            &self.excluded_actors,
            self.quorum,
        )?;
        if self.enabled.is_none() {
            return Err(CraftError::field(
                "/enabled",
                "explicit_enabled_state_required",
            ));
        }
        if self
            .expected_revision
            .is_some_and(|r| r == 0 || r >= i64::MAX as u64)
        {
            return Err(CraftError::field(
                "/expected_revision",
                "expected_incrementable_positive_revision",
            ));
        }
        if !self.apply {
            return Err(CraftError::new(
                ErrorCode::ApplyRequired,
                "Reviewer policy changes require explicit apply.",
            ));
        }
        let mut result = data(name, self.target.provisional());
        result.configuration_write_state = RequestState::SentUnknown;
        result.proposed_policy = Some(Snapshot {
            revision: self.expected_revision.unwrap_or(0) + 1,
            grant: GrantRef {
                id: "g".repeat(512),
            },
            target: result.target.clone(),
            expected_actor: self.expected_actor.clone(),
            enabled: self.enabled == Some(true),
            reviewers: self.reviewers.clone(),
            excluded_actors: self.excluded_actors.clone(),
            quorum: self.quorum,
        });
        preflight(SET, &result, options)
    }
}
pub enum PolicyArgs {
    Get(GetArgs),
    Set(SetArgs),
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
fn finish_set(
    options: &InvocationOptions<'_>,
    mut response: PolicyData,
    proposed: Option<Snapshot>,
    written: Result<ReviewPolicyRecord, CraftError>,
) -> CommandResult {
    match written {
        Ok(record) => {
            response.policy = Some(Snapshot::from(&record));
            response.configuration_write_state = RequestState::Confirmed;
            result(SET, options, response, None)
        }
        Err(error) => {
            if error.details.write_state == RequestState::SentUnknown {
                response.proposed_policy = proposed;
                response.configuration_write_state = RequestState::SentUnknown;
            }
            result(SET, options, response, Some(error))
        }
    }
}
pub fn execute(
    args: PolicyArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> CommandResult {
    let command = match &args {
        PolicyArgs::Get(_) => GET,
        PolicyArgs::Set(_) => SET,
    };
    let output = (|| {
        match &args {
            PolicyArgs::Get(a) => a.validate(fields, options)?,
            PolicyArgs::Set(a) => a.validate(fields, options)?,
        }
        let name = context(options, fields)?;
        let root = resolve_state_root(options.state_root, |k| std::env::var_os(k))?;
        let store = ProfileStore::open(&root, control)?;
        let target = match &args {
            PolicyArgs::Get(a) => a.target.resolve(name, &store, control)?,
            PolicyArgs::Set(a) => a.target.resolve(name, &store, control)?,
        };
        let mut response = data(name, target.clone());
        match args {
            PolicyArgs::Get(_) => {
                response.policy = store
                    .review_policy(name, &target.id, control)?
                    .as_ref()
                    .map(Snapshot::from);
                if let Err(e) = preflight(GET, &response, options) {
                    response.policy = None;
                    return Ok(result(GET, options, response, Some(e)));
                }
                Ok(result(GET, options, response, None))
            }
            PolicyArgs::Set(a) => {
                let proposed = RefCell::new(None);
                let written = store.set_review_policy(
                    name,
                    ReviewPolicyUpdate {
                        target,
                        expected_actor: a.expected_actor,
                        enabled: a.enabled == Some(true),
                        reviewers: a.reviewers,
                        excluded_actors: a.excluded_actors,
                        quorum: a.quorum,
                        expected_revision: a.expected_revision,
                    },
                    a.apply,
                    control,
                    |record| {
                        let snapshot = Snapshot::from(record);
                        let mut probe = data(name, record.target.clone());
                        probe.proposed_policy = Some(snapshot.clone());
                        probe.configuration_write_state = RequestState::SentUnknown;
                        preflight(SET, &probe, options)?;
                        *proposed.borrow_mut() = Some(snapshot);
                        Ok(())
                    },
                );
                Ok(finish_set(
                    options,
                    response,
                    proposed.into_inner(),
                    written,
                ))
            }
        }
    })();
    output.unwrap_or_else(|error| CommandResult {
        envelope: Envelope::failure(command, options.request_id.into(), error),
        output_policy: OutputPolicy::PreserveEvidence,
    })
}
/// Conversion rechecks the complete enabled record in the selected private store.
pub fn trusted_policy(
    store: &ProfileStore,
    record: &ReviewPolicyRecord,
    control: &InvocationControl,
) -> Result<craft_workflows::review::TrustedReviewerPolicy, CraftError> {
    store.verify_review_policy(record, control)?;
    craft_workflows::review::TrustedReviewerPolicy::from_host_configuration(
        record.revision,
        record.target.clone(),
        record.expected_actor.clone(),
        record.reviewers.clone(),
        record.excluded_actors.clone(),
        usize::from(record.quorum),
        record.enabled,
    )
}
pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    for command in [GET, SET] {
        let input = if command == GET {
            json!({"space_id":"100"})
        } else {
            json!({"space_id":"100","expected_actor":"worker-agent","enabled":true,"reviewers":["human-reviewer"],"excluded_actors":["other-agent"],"quorum":1,"expected_revision":null,"apply":true})
        };
        let mut response = data(
            "factory",
            space("00000000-1111-2222-3333-444444444444", "100"),
        );
        if command == SET {
            response.configuration_write_state = RequestState::Confirmed;
            response.policy = Some(Snapshot {
                revision: 1,
                grant: GrantRef {
                    id: "gr_00000000000000000000000000000000".into(),
                },
                target: response.target.clone(),
                expected_actor: "worker-agent".into(),
                enabled: true,
                reviewers: vec!["human-reviewer".into()],
                excluded_actors: vec!["other-agent".into()],
                quorum: 1,
            });
        }
        let descriptor=OperationDescriptor {id:command.into(),provider:"craft".into(),contract_version:craft_core::contracts::SCHEMA_VERSION.into(),api_revision:"local-review-policy-v1".into(),description:if command==GET {"Read explicit-profile operator reviewer configuration; no live verification."}else{"Change explicit-profile local reviewer eligibility with apply and revision CAS; no human approval is created."}.into(),effect:if command==GET {craft_core::contracts::EffectClass::Read}else{craft_core::contracts::EffectClass::AccessChange},input_schema:if command==GET {schema_for::<GetArgs>()?}else{schema_for::<SetArgs>()?},response_schema:response_schema::<PolicyData>()?,example:input,response_example:json!(Envelope::success(command,"example".into(),json!(response))),projection:None,scope_alternatives:vec![],permissions:vec!["Explicit local operator administration; not a provider or human identity proof.".into()],tenant_conditions:vec![],routes:vec![],retry_class:if command==GET {RetryClass::LocalRead}else{RetryClass::ReconcileBeforeReplay},reconciliation:if command==GET {ReconciliationClass::NotApplicable}else{ReconciliationClass::KnownResource}};
        if command == GET {
            registry.register_typed::<GetArgs>(descriptor)?;
        } else {
            registry.register_typed::<SetArgs>(descriptor)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
