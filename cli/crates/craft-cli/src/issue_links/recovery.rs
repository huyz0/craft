//! Matching current relations are observations, never attribution or a new send claim.
use super::*;
use crate::dispatch::CommandResult;
use crate::issue_recovery::Assessment;
use crate::output::OutputPolicy;
use crate::profiles::InvocationOptions;
use craft_core::contracts::{Envelope, GrantRef};
use craft_core::profiles::validate_profile_name;
use craft_core::registry::{
    OperationDescriptor, ReconciliationClass, RetryClass, response_schema, schema_for,
};
use craft_provider_jira::issue_links::reads::inspection::{
    self, CurrentRelation, InspectionInput, LinkObservation,
};
use craft_runtime::control::SystemClock;
use craft_runtime::issue_recovery_policy::{
    IssueRecoveryOperation as RecoveryOp, IssueRecoveryPolicyRecord,
};
use craft_runtime::profiles::resolve_state_root;
use craft_runtime::secrets::NativeCredentials;
use std::sync::Arc;
pub const COMMAND: &str = "jira.link.reconcile";
const MINIMUM: usize = 4_194_304;
fn original(text: &str) -> Result<CanonicalRequest, String> {
    serde_json::from_value(
        craft_core::input::parse_object(text.as_bytes())
            .map_err(|_| "expected_strict_original_request")?,
    )
    .map_err(|_| "expected_original_canonical_request".into())
}
fn required<'de, D: serde::Deserializer<'de>>(
    decoder: D,
) -> Result<Option<CanonicalRequest>, D::Error> {
    CanonicalRequest::deserialize(decoder).map(Some)
}
#[derive(clap::Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReconcileArgs {
    #[arg(long, default_value = "")]
    pub operation_id: String,
    #[arg(long,value_parser=original)]
    #[serde(deserialize_with = "required")]
    #[schemars(required, with = "CanonicalRequest")]
    pub original: Option<CanonicalRequest>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecoveryData {
    pub operation_id: String,
    pub initial_record: Option<JournalRecord>,
    pub latest_record: Option<JournalRecord>,
    pub journal_matches_initial: Option<bool>,
    pub assessment: Assessment,
    pub current_relation: CurrentRelation,
    pub observations: Vec<LinkObservation>,
    pub observations_complete: bool,
    pub http_status: Option<u16>,
    pub retry_after: Option<RetryAfter>,
    pub credential_cache_update: Option<CredentialCacheUpdate>,
    pub inspection_error: Option<CraftError>,
    #[schemars(extend("const"=false))]
    pub attribution_upgraded: bool,
    #[schemars(extend("const"=false))]
    pub journal_updated: bool,
    #[schemars(extend("const"=false))]
    pub replay_authorized: bool,
    #[schemars(extend("const"=false))]
    pub atomic_snapshot: bool,
}
fn empty(operation_id: &str) -> RecoveryData {
    RecoveryData {
        operation_id: operation_id.into(),
        initial_record: None,
        latest_record: None,
        journal_matches_initial: None,
        assessment: Assessment::LocalEvidenceMissing,
        current_relation: CurrentRelation::Uninspected,
        observations: vec![],
        observations_complete: false,
        http_status: None,
        retry_after: None,
        credential_cache_update: None,
        inspection_error: None,
        attribution_upgraded: false,
        journal_updated: false,
        replay_authorized: false,
        atomic_snapshot: false,
    }
}
fn reconstruct(
    args: &ReconcileArgs,
) -> Result<(PreparedOperation, InspectionInput, RecoveryOp), CraftError> {
    let original = args
        .original
        .as_ref()
        .ok_or_else(|| CraftError::field("/original", "expected_original_native_link_request"))?;
    let operation = super::registry()?.prepare(original.clone())?;
    let (input, kind) = match original.command.as_str() {
        CREATE => {
            let expected: LinkInput =
                serde_json::from_value(original.input.clone()).map_err(|_| {
                    CraftError::field("/original/input", "invalid_original_link_creation")
                })?;
            (
                InspectionInput {
                    expected,
                    link_id: None,
                },
                RecoveryOp::LinkCreate,
            )
        }
        write::DELETE => {
            let input: write::LinkDeleteInput = serde_json::from_value(original.input.clone())
                .map_err(|_| {
                    CraftError::field("/original/input", "invalid_original_link_deletion")
                })?;
            (
                InspectionInput {
                    expected: input.expected,
                    link_id: Some(input.link_id),
                },
                RecoveryOp::LinkDelete,
            )
        }
        _ => {
            return Err(CraftError::field(
                "/original/command",
                "expected_original_link_create_or_delete",
            ));
        }
    };
    Ok((operation, input, kind))
}
impl ReconcileArgs {
    pub fn validate(
        &self,
        fields: &[String],
        options: &InvocationOptions<'_>,
    ) -> Result<(), CraftError> {
        craft_core::contracts::validate_identity("/operation_id", &self.operation_id)?;
        craft_core::contracts::validate_identity("/request_id", options.request_id)?;
        validate_profile_name(options.profile.ok_or_else(|| {
            CraftError::field("/profile", "explicit_link_recovery_profile_required")
        })?)?;
        if !fields.is_empty() || options.request_id.len() > 80 {
            return Err(CraftError::field(
                "/output",
                "complete_link_recovery_evidence_required",
            ));
        }
        if !(MINIMUM..=16_777_216).contains(&options.max_bytes) {
            return Err(CraftError::new(
                ErrorCode::OutputBudgetTooSmall,
                "Use --max-bytes 4194304 to reserve complete link journal and inspection receipts.",
            ));
        }
        reconstruct(self)?;
        Ok(())
    }
}
pub struct PreparedRecovery {
    binding: OperationBinding,
    step: &'static str,
    read: PreparedOperation,
    registry: Registry,
    policies: Vec<IssueRecoveryPolicyRecord>,
    operation: RecoveryOp,
}
impl PreparedRecovery {
    pub fn prepare(
        args: &ReconcileArgs,
        profile: &str,
        store: &ProfileStore,
        control: &InvocationControl,
    ) -> Result<Self, CraftError> {
        let (original, input, operation) = reconstruct(args)?;
        let mut policies = Vec::new();
        for project in [
            &original.request().target.id,
            &input.expected.inward_project_id,
        ] {
            let policy = store
                .issue_recovery_policy(profile, project, control)?
                .ok_or_else(|| {
                    denied(
                        "Both original link endpoint projects require explicit recovery allowance.",
                    )
                })?;
            store.verify_issue_recovery_policy(&policy, operation, control)?;
            if policy.target.tenant_ref() != original.request().target.tenant_ref() {
                return Err(denied(
                    "Recovery project is outside the original link site.",
                ));
            }
            policies.push(policy);
        }
        if policies[0].grant != policies[1].grant
            || policies[0].expected_actor != policies[1].expected_actor
            || policies[0].installation != policies[1].installation
        {
            return Err(denied(
                "Both recovery allowances must identify the original grant and actor.",
            ));
        }
        let binding = original.mutation_binding(
            &args.operation_id,
            &policies[0].grant,
            &policies[0].expected_actor,
        )?;
        let step = if operation == RecoveryOp::LinkDelete {
            write::DELETE_ROUTE
        } else {
            CREATE_ROUTE
        };
        let mut registry = Registry::default();
        inspection::register(&mut registry)?;
        let read = registry.prepare(CanonicalRequest {
            command: inspection::COMMAND.into(),
            target: original.request().target.clone(),
            input: json!(input),
            selection: Selection::default(),
            preconditions: None,
        })?;
        Ok(Self {
            binding,
            step,
            read,
            registry,
            policies,
            operation,
        })
    }
    pub fn grant(&self) -> &GrantRef {
        &self.binding.grant
    }
    pub fn execute(
        &self,
        environment: LinkEnvironment<'_, '_>,
        extra: impl Fn() -> Result<(), CraftError>,
    ) -> (RecoveryData, Option<CraftError>) {
        let LinkEnvironment {
            store,
            client,
            locked,
            journal,
            tls,
            control,
        } = environment;
        let journal = journal.for_invocation(&control);
        let mut data = empty(&self.binding.operation_id);
        let result: Result<(), CraftError> = (|| {
            let policy = || {
                control.check()?;
                extra()?;
                for p in &self.policies {
                    store.verify_issue_recovery_policy(p, self.operation, &control)?;
                }
                control.check()
            };
            policy()?;
            let initial = journal.get(&self.binding.operation_id).map_err(storage)?;
            data.initial_record = initial.clone();
            let Some(initial) = initial else {
                return Ok(());
            };
            if initial.binding != self.binding {
                return Err(CraftError::new(
                    ErrorCode::OperationIdReused,
                    "Original link request does not match this journal binding.",
                ));
            }
            let step = initial.steps.get(self.step);
            data.assessment = match step.map(|s| s.state) {
                None | Some(SubmissionState::Prepared) => Assessment::NotLocallyStarted,
                Some(SubmissionState::Confirmed) => Assessment::RecordedConfirmation,
                Some(SubmissionState::Failed) => Assessment::RecordedFailure,
                _ => Assessment::Unresolved,
            };
            if step.is_none_or(|s| s.state == SubmissionState::Prepared) {
                return Ok(());
            }
            policy()?;
            if locked.grant() != self.binding.grant {
                return Err(denied(
                    "Recovery locked grant differs from the original journal actor.",
                ));
            }
            let client = client.for_invocation(control.clone())?;
            let access = match client.open_service_access(locked, &self.binding.target.tenant_ref())
            {
                Ok(access) => access,
                Err(failure) => {
                    data.credential_cache_update = failure
                        .credential_update
                        .map(|r| CredentialCacheUpdate {
                            grant: locked.grant(),
                            write_state: RequestState::Confirmed,
                            cleanup_pending: Some(r.cleanup_pending),
                        })
                        .or_else(|| {
                            failure
                                .credential_write_state
                                .map(|s| CredentialCacheUpdate {
                                    grant: locked.grant(),
                                    write_state: s,
                                    cleanup_pending: None,
                                })
                        });
                    let mut error = failure.error;
                    error.details.write_state = RequestState::NotSent;
                    return Err(error);
                }
            };
            data.credential_cache_update = access.cache_update().map(|r| CredentialCacheUpdate {
                grant: access.grant().clone(),
                write_state: RequestState::Confirmed,
                cleanup_pending: Some(r.cleanup_pending),
            });
            let check = || {
                policy()?;
                if access.tenant() != &self.binding.target.tenant_ref()
                    || access.grant() != &self.binding.grant
                    || access.identity().actor() != self.binding.actor
                {
                    return Err(denied(
                        "Recovery execution identity differs from the original binding.",
                    ));
                }
                access.check_operation(&self.registry, &self.read)
            };
            check()?;
            let mut native =
                access.read_context(&self.registry, &self.read, tls, control.clone())?;
            let mut guard = Guard {
                native: &mut native,
                check: &check,
            };
            let batch = inspection::inspect(&self.read, &mut guard, access.clock().as_ref());
            data.observations = batch.observations;
            data.current_relation = batch.current;
            data.observations_complete = batch.complete;
            data.http_status = batch.http_status;
            data.retry_after = batch.retry_after;
            if let Some(error) = batch.error {
                return Err(error);
            }
            check()?;
            Ok(())
        })();
        let mut error = result.err();
        match journal.get(&self.binding.operation_id) {
            Ok(latest) => {
                data.journal_matches_initial = Some(latest == data.initial_record);
                data.latest_record = latest;
                if data
                    .latest_record
                    .as_ref()
                    .is_some_and(|r| r.binding != self.binding)
                {
                    error.get_or_insert_with(|| {
                        CraftError::new(
                            ErrorCode::VersionConflict,
                            "Original link journal binding changed during investigation.",
                        )
                    });
                }
            }
            Err(storage_error) => {
                error.get_or_insert_with(|| storage(storage_error));
            }
        }
        if error.is_none()
            && let Err(failure) = control.check().and_then(|()| {
                extra()?;
                for p in &self.policies {
                    store.verify_issue_recovery_policy(p, self.operation, &control)?;
                }
                Ok(())
            })
        {
            error = Some(failure);
        }
        if let Some(failure) = &error {
            data.observations_complete = false;
            if matches!(
                failure.code,
                ErrorCode::PolicyDenied
                    | ErrorCode::PermissionDenied
                    | ErrorCode::AuthRequired
                    | ErrorCode::VersionConflict
            ) {
                data.observations.clear();
                data.current_relation = CurrentRelation::Uninspected;
            }
            data.inspection_error = Some(failure.clone());
        }
        (data, error)
    }
}
pub fn execute(
    args: ReconcileArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> CommandResult {
    let mut fallback = empty(&args.operation_id);
    let result: Result<(RecoveryData, Option<CraftError>), CraftError> = (|| {
        args.validate(fields, options)?;
        let root = resolve_state_root(options.state_root, |key| std::env::var_os(key))?;
        let store = ProfileStore::open(&root, control)?;
        let profile = options
            .profile
            .ok_or_else(|| denied("Explicit recovery profile required."))?;
        let host = PreparedRecovery::prepare(&args, profile, &store, control)?;
        let journal = NativeJournal::open(&store, control).map_err(storage)?;
        let credentials = NativeCredentials::open(&store)?;
        let locked = credentials.lock(host.grant(), control)?;
        let client = ServiceOAuthClient::new(
            TlsPolicy::Platform,
            control.clone(),
            Arc::new(SystemClock::default()),
        )?;
        Ok(host.execute(
            LinkEnvironment {
                store: &store,
                client: &client,
                locked: &locked,
                journal: &journal,
                tls: TlsPolicy::Platform,
                control: control.clone(),
            },
            || Ok(()),
        ))
    })();
    let (data, error) = match result {
        Ok(result) => result,
        Err(error) => {
            fallback.inspection_error = Some(error.clone());
            (fallback, Some(error))
        }
    };
    CommandResult {
        envelope: match error {
            Some(error) => {
                Envelope::failure_with_data(COMMAND, options.request_id.into(), error, json!(data))
            }
            None => Envelope::success(COMMAND, options.request_id.into(), json!(data)),
        },
        output_policy: OutputPolicy::PreserveEvidence,
    }
}
pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    let internal = super::registry()?;
    let descriptor = internal.descriptor(CREATE)?;
    let example = json!({"operation_id":"dependency-link","original":{"command":CREATE,"target":{"provider":"jira","deployment":"cloud","tenant":"00000000-1111-2222-3333-444444444444","resource_type":"project","id":"10000"},"input":descriptor.example,"selection":Selection::default(),"preconditions":null}});
    registry.register_typed_handler::<ReconcileArgs>(OperationDescriptor {id:COMMAND.into(),provider:"jira".into(),contract_version:craft_core::contracts::SCHEMA_VERSION.into(),api_revision:"journal-bound-link-recovery-v1".into(),description:"Inspect an original link write without changing attribution or replay authority.".into(),effect:craft_core::contracts::EffectClass::Read,input_schema:schema_for::<ReconcileArgs>()?,response_schema:response_schema::<RecoveryData>()?,example,response_example:json!(Envelope::success(COMMAND,"example".into(),json!(empty("dependency-link")))),projection:None,scope_alternatives:vec![vec!["read:jira-work".into()]],permissions:vec!["Separate original-operation recovery allowances on both endpoint projects.".into()],tenant_conditions:vec!["Exact local journal binding required before authentication; observed presence or absence is not attribution.".into()],routes:vec![],retry_class:RetryClass::Never,reconciliation:ReconciliationClass::NotApplicable},Arc::new(crate::public_commands::VerifiedHostRequired))
}
