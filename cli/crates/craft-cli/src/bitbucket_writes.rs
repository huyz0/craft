//! Bitbucket writes through a provisioned static grant, a separately enabled `bitbucket-write`
//! policy and the local operation journal. One command is one claimed request: it is recorded
//! before it is sent, never repeated automatically, and every outcome keeps its receipt.

use crate::dispatch::CommandResult;
use crate::issue_create::PersistenceState;
use crate::output::OutputPolicy;
use crate::profiles::InvocationOptions;
use clap::Args;
use craft_core::contracts::{
    CanonicalRequest, EffectClass, Envelope, GrantRef, OperationReceipt, RequestState, ResourceRef,
    Selection, validate_identity,
};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{NativeResponse, PreparedOperation, RetryAfter};
use craft_core::registry::{Registry, response_schema, schema_for};
use craft_core::storage::{
    JournalRecord, OperationBinding, OperationJournal, StoreError, SubmissionState,
};
use craft_provider_bitbucket::{self as provider, BitbucketContext};
use craft_runtime::auth::ServiceOAuthClient;
use craft_runtime::control::SystemClock;
use craft_runtime::journal::{MutationAuthority, NativeJournal, StepOutcome};
use craft_runtime::profiles::{ProfileStore, resolve_state_root};
use craft_runtime::provider_policy::{BITBUCKET_WRITE, ProviderPolicyRecord};
use craft_runtime::secrets::{LockedCredentials, NativeCredentials};
use craft_runtime::transport::{PreparedHttpMutation, TlsPolicy};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

/// Receipts carry journal records, so reserve room for them.
pub const MINIMUM_OUTPUT: usize = 65_536;

/// Declares one write's arguments: the native fields plus the host's operation controls.
macro_rules! args {
    ($name:ident { $( $(#[$m:meta])* $field:ident : $ty:ty ),* $(,)? }) => {
        #[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            $( $(#[$m])* pub $field: $ty, )*
            #[arg(long, default_value = "")]
            #[schemars(length(min = 1, max = 128))]
            pub operation_id: String,
            #[arg(long)]
            #[serde(default)]
            pub apply: bool,
        }
    };
}

args!(BranchCreateArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 100), regex(pattern = "^[a-z0-9][a-z0-9._-]*$"))]
    repo: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 250))]
    name: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 40, max = 64), regex(pattern = "^[0-9a-fA-F]{40,64}$"))]
    source_commit: String,
});
args!(PullRequestCreateArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 100), regex(pattern = "^[a-z0-9][a-z0-9._-]*$"))]
    repo: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 255))]
    title: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 250))]
    source_branch: String,
    #[arg(long)]
    #[schemars(length(min = 1, max = 250))]
    destination_branch: Option<String>,
    #[arg(long)]
    #[schemars(length(max = 32768))]
    description: Option<String>,
    #[arg(long)]
    close_source_branch: Option<bool>,
    #[arg(long, value_delimiter = ',', num_args = 1)]
    #[schemars(length(max = 10))]
    reviewer_uuids: Option<Vec<String>>,
});
args!(PullRequestUpdateArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 100), regex(pattern = "^[a-z0-9][a-z0-9._-]*$"))]
    repo: String,
    #[arg(long, default_value_t = 0)]
    #[schemars(range(min = 1, max = 2147483647))]
    pull_request_id: u64,
    #[arg(long)]
    #[schemars(length(min = 1, max = 255))]
    title: Option<String>,
    #[arg(long)]
    #[schemars(length(max = 32768))]
    description: Option<String>,
    #[arg(long)]
    #[schemars(length(min = 1, max = 250))]
    destination_branch: Option<String>,
    #[arg(long)]
    close_source_branch: Option<bool>,
});
args!(CommentAddArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 100), regex(pattern = "^[a-z0-9][a-z0-9._-]*$"))]
    repo: String,
    #[arg(long, default_value_t = 0)]
    #[schemars(range(min = 1, max = 2147483647))]
    pull_request_id: u64,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 32768))]
    text: String,
    #[arg(long)]
    #[schemars(range(min = 1))]
    parent_id: Option<u64>,
    #[arg(long)]
    #[schemars(length(min = 1, max = 1024))]
    path: Option<String>,
    #[arg(long)]
    #[schemars(range(min = 1, max = 10000000))]
    line: Option<u32>,
});
args!(PullRequestChangeArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 100), regex(pattern = "^[a-z0-9][a-z0-9._-]*$"))]
    repo: String,
    #[arg(long, default_value_t = 0)]
    #[schemars(range(min = 1, max = 2147483647))]
    pull_request_id: u64,
});
args!(PullRequestMergeArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 100), regex(pattern = "^[a-z0-9][a-z0-9._-]*$"))]
    repo: String,
    #[arg(long, default_value_t = 0)]
    #[schemars(range(min = 1, max = 2147483647))]
    pull_request_id: u64,
    #[arg(long, default_value = "")]
    #[schemars(regex(pattern = "^(merge_commit|squash|fast_forward)$"))]
    merge_strategy: String,
    #[arg(long)]
    #[schemars(length(max = 4096))]
    message: Option<String>,
    #[arg(long)]
    close_source_branch: Option<bool>,
});
args!(PullRequestDeclineArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 100), regex(pattern = "^[a-z0-9][a-z0-9._-]*$"))]
    repo: String,
    #[arg(long, default_value_t = 0)]
    #[schemars(range(min = 1, max = 2147483647))]
    pull_request_id: u64,
    #[arg(long)]
    #[schemars(length(max = 1000))]
    reason: Option<String>,
});

/// One row per write: the flat variant, its argument type and its command id. Everything else
/// (command id, operation controls, schema registration) derives from the rows.
macro_rules! writes {
    ($( $variant:ident($ty:ty) => $command:literal ),* $(,)?) => {
        /// Flat view of every Bitbucket write, shared by parsing, JSON input and the host.
        #[derive(Clone)]
        pub enum BitbucketWriteArgs {
            $( $variant($ty), )*
        }

        impl BitbucketWriteArgs {
            pub fn command(&self) -> &'static str {
                match self { $( Self::$variant(_) => $command, )* }
            }

            pub fn arguments(&self) -> Result<Value, CraftError> {
                match self { $( Self::$variant(v) => encoded(v), )* }
            }

            /// Decodes JSON input for the command the parser selected.
            pub fn decode(command: &str, input: Value) -> Result<Self, CraftError> {
                match command {
                    $( $command => Ok(Self::$variant(decode(input)?)), )*
                    _ => Err(CraftError::field("/command", "unsupported_bitbucket_write")),
                }
            }

            pub fn operation_id(&self) -> &str {
                match self { $( Self::$variant(v) => &v.operation_id, )* }
            }

            pub fn apply(&self) -> bool {
                match self { $( Self::$variant(v) => v.apply, )* }
            }
        }

        /// Registers the CLI-level schemas (with operation id and apply) over the provider
        /// descriptors.
        pub fn register_candidates(registry: &mut Registry) -> Result<(), CraftError> {
            let mut native = Registry::default();
            native.register_provider(&BitbucketContext)?;
            for command in provider::write_commands() {
                let mut descriptor = native.descriptor(command)?.clone();
                descriptor.response_schema = response_schema::<BitbucketWriteData>()?;
                descriptor.example["operation_id"] = json!("agent-change");
                descriptor.example["apply"] = json!(true);
                descriptor.response_example =
                    json!(Envelope::success(command, "example".into(), example_data()));
                match command {
                    $( $command => {
                        descriptor.input_schema = schema_for::<$ty>()?;
                        registry.register_typed::<$ty>(descriptor)?
                    } )*
                    _ => return Err(CraftError::field("/command", "unknown_bitbucket_write")),
                }
            }
            Ok(())
        }
    };
}

writes! {
    BranchCreate(BranchCreateArgs) => "bitbucket.branch.create",
    PullRequestCreate(PullRequestCreateArgs) => "bitbucket.pullrequest.create",
    PullRequestUpdate(PullRequestUpdateArgs) => "bitbucket.pullrequest.update",
    CommentAdd(CommentAddArgs) => "bitbucket.comment.add",
    PullRequestApprove(PullRequestChangeArgs) => "bitbucket.pullrequest.approve",
    PullRequestUnapprove(PullRequestChangeArgs) => "bitbucket.pullrequest.unapprove",
    PullRequestRequestChanges(PullRequestChangeArgs) => "bitbucket.pullrequest.request-changes",
    PullRequestUnrequestChanges(PullRequestChangeArgs) => "bitbucket.pullrequest.unrequest-changes",
    PullRequestMerge(PullRequestMergeArgs) => "bitbucket.pullrequest.merge",
    PullRequestDecline(PullRequestDeclineArgs) => "bitbucket.pullrequest.decline",
}

fn encoded(value: &impl Serialize) -> Result<Value, CraftError> {
    serde_json::to_value(value)
        .map_err(|_| CraftError::field("/input", "invalid_bitbucket_arguments"))
}
fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, CraftError> {
    serde_json::from_value(value)
        .map_err(|_| CraftError::field("/input", "invalid_typed_bitbucket_arguments"))
}

impl BitbucketWriteArgs {
    /// The provider's input: the native fields without the host's operation controls.
    fn input(&self) -> Result<Value, CraftError> {
        let mut value = self.arguments()?;
        let object = value
            .as_object_mut()
            .ok_or_else(|| CraftError::field("/input", "invalid_bitbucket_arguments"))?;
        object.remove("operation_id");
        object.remove("apply");
        object.retain(|_, field| !field.is_null());
        Ok(value)
    }
}

/// A complete, schema-valid receipt used as the documented response example.
fn example_data() -> Value {
    json!({
        "receipt": {
            "operation_id": "agent-change",
            "request_state": "confirmed",
            "confirmed_resources": [{
                "provider": "bitbucket", "deployment": "cloud", "tenant": "team",
                "resource_type": "pullrequest", "id": "1"
            }]
        },
        "target": {
            "provider": "bitbucket", "deployment": "cloud", "tenant": "team",
            "resource_type": "workspace", "id": "team"
        },
        "item": {"id": "1", "title": "Fix login"},
        "acknowledged": true,
        "desired_fields_verified": true,
        "persistence": "recorded",
        "prior_record": null,
        "latest_record": null,
        "pending_record": null,
        "pending_record_state": "recorded",
        "http_status": 200,
        "provider_request_id": null,
        "retry_after": null,
        "reconciliation_required": false,
        "replay_authorized": false
    })
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BitbucketWriteData {
    pub receipt: OperationReceipt,
    pub target: ResourceRef,
    pub item: Option<provider::WriteItem>,
    pub acknowledged: bool,
    pub desired_fields_verified: bool,
    pub persistence: PersistenceState,
    pub prior_record: Option<JournalRecord>,
    pub latest_record: Option<JournalRecord>,
    pub pending_record: Option<JournalRecord>,
    pub pending_record_state: PersistenceState,
    pub http_status: Option<u16>,
    pub provider_request_id: Option<String>,
    pub retry_after: Option<RetryAfter>,
    pub reconciliation_required: bool,
    #[schemars(extend("const" = false))]
    pub replay_authorized: bool,
}

fn denied(message: &str) -> CraftError {
    CraftError::new(ErrorCode::PolicyDenied, message)
}

fn storage(_: StoreError) -> CraftError {
    CraftError::new(
        ErrorCode::Internal,
        "Cannot inspect the exact Bitbucket operation journal.",
    )
}

fn request_state(state: SubmissionState) -> RequestState {
    match state {
        SubmissionState::Prepared => RequestState::NotSent,
        SubmissionState::Confirmed => RequestState::Confirmed,
        SubmissionState::Failed => RequestState::Failed,
        _ => RequestState::SentUnknown,
    }
}

fn empty(binding: &OperationBinding) -> BitbucketWriteData {
    BitbucketWriteData {
        receipt: OperationReceipt {
            operation_id: binding.operation_id.clone(),
            request_state: RequestState::NotSent,
            confirmed_resources: vec![],
            job: None,
        },
        target: binding.target.clone(),
        item: None,
        acknowledged: false,
        desired_fields_verified: false,
        persistence: PersistenceState::Unrecorded,
        prior_record: None,
        latest_record: None,
        pending_record: None,
        pending_record_state: PersistenceState::Unrecorded,
        http_status: None,
        provider_request_id: None,
        retry_after: None,
        reconciliation_required: false,
        replay_authorized: false,
    }
}

fn workspace(tenant: &str) -> ResourceRef {
    ResourceRef {
        provider: "bitbucket".into(),
        deployment: "cloud".into(),
        tenant: tenant.into(),
        resource_type: "workspace".into(),
        id: tenant.into(),
    }
}

pub struct PreparedBitbucketWrite {
    registry: Registry,
    write: PreparedOperation,
    binding: OperationBinding,
    policy: ProviderPolicyRecord,
    step: String,
}

pub struct BitbucketWriteEnvironment<'a, 'credentials> {
    pub store: &'a ProfileStore,
    pub client: &'a ServiceOAuthClient,
    pub locked: &'a LockedCredentials<'credentials>,
    pub journal: &'a NativeJournal,
    pub tls: TlsPolicy,
    pub control: InvocationControl,
}

impl PreparedBitbucketWrite {
    /// Validates the request, the enabled operation and the exact policy binding. Nothing is
    /// sent, claimed or read from the credential store.
    pub fn prepare(
        request: CanonicalRequest,
        operation_id: &str,
        apply: bool,
        profile: &str,
        store: &ProfileStore,
        control: &InvocationControl,
    ) -> Result<Self, CraftError> {
        if !apply {
            return Err(CraftError::new(
                ErrorCode::ApplyRequired,
                "Bitbucket writes require explicit apply.",
            ));
        }
        if !(MINIMUM_OUTPUT..=16_777_216).contains(&(request.selection.max_bytes as usize)) {
            return Err(CraftError::new(
                ErrorCode::OutputBudgetTooSmall,
                "Reserve --max-bytes 65536 for complete journal and recovery receipts.",
            ));
        }
        let mut registry = Registry::default();
        registry.register_provider(&BitbucketContext)?;
        let write = registry.prepare(request)?;
        let command = write.request().command.clone();
        let policy = store
            .provider_policy(profile, BITBUCKET_WRITE, control)?
            .ok_or_else(|| denied("No trusted Bitbucket write policy is present."))?;
        store.verify_provider_policy(&policy, &command, control)?;
        if policy.target != write.request().target {
            return Err(denied(
                "Bitbucket write target differs from the configured workspace.",
            ));
        }
        let binding =
            write.mutation_binding(operation_id, &policy.grant, &policy.expected_actor)?;
        let step = provider::step(&command)
            .ok_or_else(|| CraftError::field("/command", "unknown_bitbucket_write"))?;
        Ok(Self {
            registry,
            write,
            binding,
            policy,
            step,
        })
    }

    pub fn grant(&self) -> &GrantRef {
        &self.binding.grant
    }

    pub fn binding(&self) -> &OperationBinding {
        &self.binding
    }

    /// An existing record may only be an untouched prepared step for this exact request.
    fn prior(
        &self,
        journal: &NativeJournal,
        data: &mut BitbucketWriteData,
    ) -> Result<(), CraftError> {
        let record = journal.get(&self.binding.operation_id).map_err(|error| {
            data.persistence = PersistenceState::Uncertain;
            data.reconciliation_required = true;
            storage(error)
        })?;
        if data.prior_record.is_none() {
            data.prior_record = record.clone();
        } else {
            data.latest_record = record.clone();
        }
        if record.is_some() {
            data.persistence = PersistenceState::Recorded;
        }
        if let Some(record) = record
            && (record.binding != self.binding
                || record
                    .steps
                    .iter()
                    .any(|(name, s)| name != &self.step || s.state != SubmissionState::Prepared))
        {
            return Err(CraftError::new(
                ErrorCode::OperationIdReused,
                "Bitbucket operation changed or already started; retain its journal and do not replay it.",
            ));
        }
        Ok(())
    }

    pub fn execute(
        self,
        environment: BitbucketWriteEnvironment<'_, '_>,
        extra: impl Fn() -> Result<(), CraftError>,
    ) -> (BitbucketWriteData, Option<CraftError>) {
        let BitbucketWriteEnvironment {
            store,
            client,
            locked,
            journal,
            tls,
            control,
        } = environment;
        let journal = journal.for_invocation(&control);
        let mut data = empty(&self.binding);
        let mut claim_attempted = false;
        let command = self.write.request().command.clone();
        let result = (|| {
            let policy_check = || {
                control.check()?;
                extra()?;
                store.verify_provider_policy(&self.policy, &command, &control)?;
                extra()?;
                control.check()
            };
            self.prior(&journal, &mut data)?;
            policy_check()?;
            if locked.grant() != self.binding.grant {
                return Err(denied(
                    "Locked grant differs from the captured Bitbucket write policy.",
                ));
            }
            let call = provider::build(self.write.request())?;
            let client = client.for_invocation(control.clone())?;
            let session = client
                .open_static_session(
                    locked,
                    &self.binding.target.tenant_ref(),
                    &self.binding.actor,
                )
                .map_err(|failure| {
                    let mut error = failure.error;
                    error.details.write_state = RequestState::NotSent;
                    error
                })?;
            policy_check()?;
            self.prior(&journal, &mut data)?;
            let mutation = PreparedHttpMutation::prepare(
                &self.registry,
                &self.write,
                session,
                tls,
                control.clone(),
                &call,
            )?;
            claim_attempted = true;
            let attempt = mutation.send_checked(
                &journal,
                &MutationAuthority {
                    apply: true,
                    operation_id: &self.binding.operation_id,
                    grant: &self.binding.grant,
                    actor: &self.binding.actor,
                },
                &self.step,
                policy_check,
            )?;
            finish(
                &self.write,
                &self.binding,
                &self.step,
                Completion {
                    binding: attempt.binding(),
                    command: attempt.command(),
                    route: attempt.route(),
                    step: attempt.step(),
                    response: attempt.response.as_ref(),
                    pending: attempt.pending_evidence.as_ref(),
                },
                &mut data,
                |outcome| attempt.record_outcome(outcome),
            )
        })();
        let error = result.err().map(|mut error| {
            if data.latest_record.is_none() && claim_attempted {
                data.latest_record = journal.get(&self.binding.operation_id).ok().flatten();
                if data.latest_record.is_some() {
                    data.persistence = PersistenceState::Recorded;
                } else if error.details.write_state == RequestState::SentUnknown {
                    data.persistence = PersistenceState::Uncertain;
                }
            }
            if data.http_status.is_none() {
                if let Some(record) = data.latest_record.as_ref().or(data.prior_record.as_ref())
                    && record.binding == self.binding
                    && record.validate().is_ok()
                    && let Some(saved) = record.steps.get(&self.step)
                {
                    data.receipt.request_state = request_state(saved.state);
                    data.receipt.confirmed_resources = saved.confirmed_resources.clone();
                    data.receipt.job = saved.job.clone();
                }
                if error.details.write_state == RequestState::SentUnknown
                    && data.receipt.request_state == RequestState::NotSent
                {
                    data.receipt.request_state = RequestState::SentUnknown;
                }
            }
            data.reconciliation_required = data.reconciliation_required
                || data.receipt.request_state != RequestState::NotSent
                || data.prior_record.is_some()
                || data.latest_record.is_some();
            error.details.write_state = data.receipt.request_state;
            error.next_action = "Retain all Bitbucket receipts and inspect this operation ID; read the resource before any new attempt. No automatic retry or replay is authorized.".into();
            error
        });
        (data, error)
    }
}

struct Completion<'a> {
    binding: &'a OperationBinding,
    command: &'a str,
    route: &'a str,
    step: &'a str,
    response: Result<&'a NativeResponse, &'a CraftError>,
    pending: Result<&'a JournalRecord, &'a StoreError>,
}

fn merge(
    record: &JournalRecord,
    step: &str,
    state: &mut SubmissionState,
    resources: &mut Vec<ResourceRef>,
) {
    if let Some(saved) = record.steps.get(step) {
        for resource in &saved.confirmed_resources {
            if !resources.contains(resource) {
                resources.push(resource.clone());
            }
        }
        if saved.state == SubmissionState::Confirmed {
            *state = SubmissionState::Confirmed;
        }
        if *state == SubmissionState::Failed && !resources.is_empty() {
            *state = SubmissionState::Unknown;
        }
    }
}

/// Records the interpreted outcome durably and assembles the receipt. A response that cannot be
/// attributed to the claimed step is recorded as unknown, never as a clean failure.
fn finish(
    operation: &PreparedOperation,
    binding: &OperationBinding,
    step: &str,
    view: Completion<'_>,
    data: &mut BitbucketWriteData,
    record: impl FnOnce(&StepOutcome) -> Result<JournalRecord, StoreError>,
) -> Result<(), CraftError> {
    let command = &operation.request().command;
    let recognized = view.binding == binding
        && view.command == command
        && view.route == step
        && view.step == step;
    let mut observation = match view.response {
        Ok(response) if recognized => provider::interpret(operation, response)?,
        _ => provider::Observation {
            state: SubmissionState::Unknown,
            resources: vec![],
            item: None,
            acknowledged: false,
            desired_fields_verified: false,
            error: Some(CraftError::new(
                view.response
                    .err()
                    .map_or(ErrorCode::PolicyDenied, |e| e.code),
                "Bitbucket attempt lacks qualified response evidence.",
            )),
        },
    };
    let mut state = observation.state;
    let mut resources = observation.resources.clone();
    let expected = resources.clone();
    data.pending_record = view.pending.ok().cloned();
    data.pending_record_state = match view.pending {
        Ok(_) => PersistenceState::Recorded,
        Err(StoreError::OutcomeUnknown) => PersistenceState::Uncertain,
        Err(_) => PersistenceState::Unrecorded,
    };
    if let Some(pending) = &data.pending_record {
        if pending.binding == *binding && pending.validate().is_ok() {
            merge(pending, step, &mut state, &mut resources);
        } else {
            observation.error = Some(CraftError::new(
                ErrorCode::PartialApply,
                "Initial Bitbucket journal evidence has a different or invalid binding.",
            ));
        }
    }
    match record(&StepOutcome {
        state,
        confirmed_resources: resources.clone(),
        job: None,
    }) {
        Ok(saved) => {
            let valid = saved.binding == *binding && saved.validate().is_ok();
            let consistent = valid
                && saved.steps.get(step).is_some_and(|s| {
                    s.state == state
                        && s.job.is_none()
                        && resources.iter().all(|r| s.confirmed_resources.contains(r))
                });
            if valid {
                merge(&saved, step, &mut state, &mut resources);
            }
            data.persistence = if consistent {
                PersistenceState::Recorded
            } else {
                PersistenceState::Uncertain
            };
            data.latest_record = Some(saved);
            if !consistent {
                observation.error = Some(CraftError::new(
                    ErrorCode::PartialApply,
                    "Bitbucket journal acknowledgment conflicts with observed native effects.",
                ));
            }
        }
        Err(error) => {
            data.persistence = if error == StoreError::OutcomeUnknown {
                PersistenceState::Uncertain
            } else {
                PersistenceState::Unrecorded
            };
            observation.error.get_or_insert_with(|| {
                CraftError::new(
                    ErrorCode::Internal,
                    "Observed Bitbucket effects could not be durably acknowledged locally.",
                )
            });
        }
    }
    if let Ok(response) = view.response {
        data.http_status = Some(response.status);
        data.provider_request_id = response.request_id.clone();
        data.retry_after = response.retry_after;
    }
    if resources.iter().any(|r| !expected.contains(r))
        || data
            .latest_record
            .as_ref()
            .is_some_and(|r| r.steps.len() != 1)
        || data
            .pending_record
            .as_ref()
            .is_some_and(|r| r.steps.len() != 1)
    {
        observation.error = Some(CraftError::new(
            ErrorCode::PartialApply,
            "Additional recorded Bitbucket effects require inspection.",
        ));
    }
    data.receipt.request_state = request_state(state);
    data.receipt.confirmed_resources = resources;
    data.acknowledged = observation.acknowledged;
    data.desired_fields_verified = observation.desired_fields_verified;
    data.item = observation
        .item
        .and_then(|item| serde_json::from_value(item).ok());
    data.reconciliation_required = observation.error.is_some();
    observation.error.map_or(Ok(()), Err)
}

/// Runs one write end to end for the selected profile. Validation precedes any private state.
pub fn execute(
    args: BitbucketWriteArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> CommandResult {
    let command = args.command();
    let result = (|| {
        validate_identity("/operation_id", args.operation_id())?;
        validate_identity("/request_id", options.request_id)?;
        let name = options
            .profile
            .ok_or_else(|| CraftError::field("/profile", "explicit_bitbucket_profile_required"))?;
        craft_core::profiles::validate_profile_name(name)?;
        if !fields.is_empty() || options.request_id.len() > 80 {
            return Err(CraftError::field(
                "/output",
                "complete_bitbucket_receipts_required",
            ));
        }
        let root = resolve_state_root(options.state_root, |key| std::env::var_os(key))?;
        let store = ProfileStore::open(&root, control)?;
        let profile = store.get(name, control)?;
        if profile.provider != "bitbucket" {
            return Err(denied(
                "Bitbucket writes require a Bitbucket service profile.",
            ));
        }
        let tenant = profile.tenant_ref()?.tenant;
        let prepared = PreparedBitbucketWrite::prepare(
            CanonicalRequest {
                command: command.into(),
                target: workspace(&tenant),
                input: args.input()?,
                selection: Selection {
                    max_bytes: u32::try_from(options.max_bytes)
                        .map_err(|_| CraftError::field("/max_bytes", "invalid_output_budget"))?,
                    ..Selection::default()
                },
                preconditions: None,
            },
            args.operation_id(),
            args.apply(),
            name,
            &store,
            control,
        )?;
        let journal = NativeJournal::open(&store, control).map_err(storage)?;
        let credentials = NativeCredentials::open(&store)?;
        let locked = credentials.lock(prepared.grant(), control)?;
        let client = ServiceOAuthClient::new(
            TlsPolicy::Platform,
            control.clone(),
            Arc::new(SystemClock::default()),
        )?;
        Ok(prepared.execute(
            BitbucketWriteEnvironment {
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
    let envelope = match result {
        Ok((data, Some(error))) => {
            Envelope::failure_with_data(command, options.request_id.into(), error, json!(data))
        }
        Ok((data, None)) => Envelope::success(command, options.request_id.into(), json!(data)),
        Err(error) => Envelope::failure(command, options.request_id.into(), error),
    };
    CommandResult {
        envelope,
        output_policy: OutputPolicy::PreserveEvidence,
    }
}

const _: fn() = || {
    let _ = EffectClass::Write;
};
