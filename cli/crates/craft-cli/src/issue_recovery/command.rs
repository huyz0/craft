//! Typed recovery candidates preserve original requests and capture separate private authority.
use super::*;
use crate::profiles::{InvocationOptions, selected_profile_context};
use clap::Args;
use craft_core::contracts::AuthMode;
use craft_core::profiles::validate_profile_name;
use craft_core::registry::{OperationDescriptor, ReconciliationClass, RetryClass, schema_for};
use craft_runtime::control::SystemClock;
use craft_runtime::issue_recovery_policy::{
    IssueRecoveryOperation as Operation, IssueRecoveryPolicyRecord,
};
use craft_runtime::profiles::{ProfileStore, resolve_state_root};
use craft_runtime::secrets::NativeCredentials;
use std::sync::Arc;
#[cfg(test)]
mod tests;

fn original(text: &str) -> Result<CanonicalRequest, String> {
    let value = craft_core::input::parse_object(text.as_bytes())
        .map_err(|_| "invalid original request JSON")?;
    serde_json::from_value(value).map_err(|_| "invalid original request fields".into())
}
fn required<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<CanonicalRequest>, D::Error> {
    CanonicalRequest::deserialize(deserializer).map(Some)
}
fn is_false(value: &bool) -> bool {
    !*value
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationReconcileArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 512))]
    pub operation_id: String,
    #[arg(long,value_parser=original)]
    #[serde(
        default,
        deserialize_with = "required",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "CanonicalRequest")]
    pub original: Option<CanonicalRequest>,
    #[arg(long)]
    #[serde(default, skip_serializing_if = "is_false")]
    pub saved: bool,
    #[arg(long,value_delimiter=',',num_args=1..)]
    #[serde(default)]
    #[schemars(length(max=20),extend("uniqueItems"=true))]
    pub candidate_issue_ids: Vec<String>,
}
impl OperationReconcileArgs {
    fn input(&self) -> Result<RecoveryInput, CraftError> {
        Ok(RecoveryInput {
            operation_id: self.operation_id.clone(),
            original: self.original.clone().ok_or_else(|| {
                CraftError::field("/original", "expected_original_canonical_request")
            })?,
            candidate_issue_ids: self.candidate_issue_ids.clone(),
        })
    }
    fn operation(&self) -> Result<Operation, CraftError> {
        serde_json::from_value(json!(self.input()?.original.command))
            .map_err(|_| CraftError::field("/original/command", "unsupported_recovery_operation"))
    }
    fn project(&self) -> Result<ResourceRef, CraftError> {
        let original = self.input()?.original;
        let id = if original.command == created_issue::COMMAND {
            original.target.id.clone()
        } else {
            original.input["expected"]["project_id"]
                .as_str()
                .ok_or_else(|| {
                    CraftError::field("/original/input/expected/project_id", "expected_project")
                })?
                .into()
        };
        Ok(ResourceRef {
            resource_type: "project".into(),
            id,
            ..original.target
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
        if self.saved {
            if self.original.is_some() {
                return Err(CraftError::field(
                    "/original",
                    "original_and_saved_are_exclusive",
                ));
            }
            craft_core::contracts::validate_identity("/operation_id", &self.operation_id)?;
            craft_core::contracts::validate_identity("/request_id", options.request_id)?;
            if options.request_id.len() > 80 || !(512..=16_777_216).contains(&options.max_bytes) {
                return Err(CraftError::field(
                    "/output",
                    "invalid_recovery_budget_or_identity",
                ));
            }
            projection().compile(&schema()?, fields)?;
            if self.candidate_issue_ids.len() > 20
                || self.candidate_issue_ids.iter().any(|id| !numeric_id(id))
                || self
                    .candidate_issue_ids
                    .iter()
                    .collect::<BTreeSet<_>>()
                    .len()
                    != self.candidate_issue_ids.len()
            {
                return Err(CraftError::field(
                    "/candidate_issue_ids",
                    "expected_bounded_unique_numeric_candidates",
                ));
            }
            if options.max_bytes < 1024 {
                return Err(CraftError::new(
                    ErrorCode::OutputBudgetTooSmall,
                    "Increase max-bytes before loading saved recovery evidence.",
                ));
            }
            return Ok(());
        }
        let operation = self.operation()?;
        let prepared = PreparedRecovery::prepare(
            self.input()?,
            fields,
            options.max_bytes,
            options.request_id,
            RecoveryPolicy {
                enabled: true,
                project: self.project()?,
                grant: GrantRef {
                    id: "g".repeat(512),
                },
                expected_actor: "a".repeat(512),
                allowed_commands: vec![operation.command().into()],
            },
        )?;
        prepared.preflight(&prepared.data(None), &[])
    }
    fn resolve_saved(
        mut self,
        store: &ProfileStore,
        control: &InvocationControl,
    ) -> Result<Self, CraftError> {
        if self.saved {
            self.original =
                Some(crate::saved_requests::load(store, &self.operation_id, control)?.original);
            self.saved = false;
        }
        Ok(self)
    }
}

/// Registers the typed Jira investigation contract; native access still requires the verified host.
pub fn register_candidate(registry: &mut Registry) -> Result<(), CraftError> {
    let mut input_schema = schema_for::<OperationReconcileArgs>()?;
    input_schema["oneOf"] = json!([
        {"required":["original"],"properties":{"saved":{"const":false}}},
        {"required":["saved"],"properties":{"saved":{"const":true},"original":false}}
    ]);
    registry.register_typed::<OperationReconcileArgs>(OperationDescriptor {
        id:COMMAND.into(),provider:"jira".into(),contract_version:craft_core::contracts::SCHEMA_VERSION.into(),
        api_revision:"journal-bound-jira-recovery-v1".into(),
        description:"Inspect bound Jira evidence from an explicit or saved source without changing attribution or authorizing replay.".into(),
        effect:craft_core::contracts::EffectClass::Read,
        input_schema,response_schema:schema()?,
        example:json!({"operation_id":"create-work","original":{"command":"jira.issue.create","target":{"provider":"jira","deployment":"cloud","tenant":"00000000-1111-2222-3333-444444444444","resource_type":"project","id":"100"},"input":{"issue_type_id":"101","summary":"Reviewed"},"selection":Selection::default(),"preconditions":null},"candidate_issue_ids":[]}),
        response_example:json!(Envelope::success(COMMAND,"recovery-example".into(),json!({"operation_id":"create-work","initial_record":null,"latest_record":null,"assessment":"local_evidence_missing","reconciliation_required":null,"journal_matches_initial":true,"observations":[],"observations_complete":false,"inspection_error":null,"http_status":null,"retry_after":null,"credential_cache_update":null,"atomic_snapshot":false,"attribution_upgraded":false,"journal_updated":false,"replay_authorized":false}))),
        projection:Some(projection()),scope_alternatives:vec![],permissions:vec![],
        tenant_conditions:vec!["Explicit project recovery policy and exact original journal binding; native reads require separately verified service access.".into()],
        routes:vec![],retry_class:RetryClass::Never,reconciliation:ReconciliationClass::NotApplicable,
    })
}
pub struct PreparedInvocation {
    host: PreparedRecovery,
    policy: IssueRecoveryPolicyRecord,
    operation: Operation,
}
impl PreparedInvocation {
    pub fn prepare(
        args: OperationReconcileArgs,
        fields: &[String],
        options: &InvocationOptions<'_>,
        store: &ProfileStore,
        control: &InvocationControl,
    ) -> Result<Self, CraftError> {
        args.validate(fields, options)?;
        let args = args.resolve_saved(store, control)?;
        args.validate(fields, options)?;
        let (name, _, hints) = selected_profile_context(store, None, options, control)?;
        let profile = store.get(&name, control)?;
        if profile.provider != "jira"
            || profile.deployment != "cloud"
            || profile.auth_mode != AuthMode::Service
        {
            return Err(failure(
                ErrorCode::PolicyDenied,
                "Recovery requires a Jira Cloud service profile.",
            ));
        }
        let original = args.input()?.original;
        let site = ResourceRef {
            resource_type: "site".into(),
            id: original.target.tenant.clone(),
            ..original.target.clone()
        };
        let resolved = profile.resolve_target("site", Some(&site), &hints)?;
        if resolved != site {
            return Err(failure(
                ErrorCode::PolicyDenied,
                "Selected site differs from the original recovery target.",
            ));
        }
        profile.validate_target("site", &site)?;
        let project = args.project()?;
        profile.validate_target("project", &project)?;
        let operation = args.operation()?;
        let policy = store
            .issue_recovery_policy(&name, &project.id, control)?
            .ok_or_else(|| {
                failure(
                    ErrorCode::PolicyDenied,
                    "No explicit recovery policy is configured for this project.",
                )
            })?;
        if policy.target != project {
            return Err(failure(
                ErrorCode::PolicyDenied,
                "Selected project differs from trusted recovery policy.",
            ));
        }
        store.verify_issue_recovery_policy(&policy, operation, control)?;
        let host = PreparedRecovery::prepare(
            args.input()?,
            fields,
            options.max_bytes,
            options.request_id,
            RecoveryPolicy {
                enabled: policy.enabled,
                project,
                grant: policy.grant.clone(),
                expected_actor: policy.expected_actor.clone(),
                allowed_commands: policy
                    .operations
                    .iter()
                    .filter(|op| !matches!(op, Operation::LinkCreate | Operation::LinkDelete))
                    .map(|op| op.command().into())
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
    pub fn execute_cached(
        &self,
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
                store.verify_issue_recovery_policy(&self.policy, self.operation, &check)
            })
    }
    pub(crate) fn execute_with_credentials(
        &self,
        store: &ProfileStore,
        journal: &NativeJournal,
        tls: TlsPolicy,
        control: InvocationControl,
        credentials: impl FnOnce(&ProfileStore) -> Result<NativeCredentials, CraftError>,
    ) -> CommandResult {
        let check = || store.verify_issue_recovery_policy(&self.policy, self.operation, &control);
        let plan = match self.host.begin(journal, control.clone(), check) {
            Ok(plan) => plan,
            Err(result) => return *result,
        };
        let resources = (|| {
            check()?;
            let credentials = credentials(store)?;
            let client = ServiceOAuthClient::new(
                TlsPolicy::Platform,
                control.clone(),
                Arc::new(SystemClock::default()),
            )?;
            Ok((credentials, client))
        })();
        let (credentials, client) = match resources {
            Ok(resources) => resources,
            Err(error) => return self.host.fail_plan(plan, error),
        };
        if let Err(error) = check() {
            return self.host.fail_plan(plan, error);
        }
        let locked = match credentials.lock(self.grant(), &control) {
            Ok(locked) => locked,
            Err(error) => return self.host.fail_plan(plan, error),
        };
        self.host.execute_plan(plan, &client, &locked, tls, check)
    }
}
pub fn execute(
    args: OperationReconcileArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> CommandResult {
    let result = (|| {
        args.validate(fields, options)?;
        let root = resolve_state_root(options.state_root, |key| std::env::var_os(key))?;
        let store = ProfileStore::open(&root, control)?;
        let prepared = PreparedInvocation::prepare(args, fields, options, &store, control)?;
        let journal = NativeJournal::open(&store, control).map_err(|_| store_error())?;
        Ok(prepared.execute_with_credentials(
            &store,
            &journal,
            TlsPolicy::Platform,
            control.clone(),
            NativeCredentials::open,
        ))
    })();
    result.unwrap_or_else(|error| CommandResult {
        envelope: Envelope::failure(COMMAND, options.request_id.into(), error),
        output_policy: OutputPolicy::PreserveEvidence,
    })
}
