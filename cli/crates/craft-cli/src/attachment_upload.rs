//! Upload capture, current authority and native acknowledgment keep independent recovery evidence.
pub mod command;
use crate::dispatch::CommandResult;
use crate::issue_create::PersistenceState;
use crate::output::OutputPolicy;
use crate::profiles::InvocationOptions;
use crate::saved_requests::{self, SavedReceipt};
use craft_core::contracts::{
    AuthMode, CanonicalRequest, CredentialCacheUpdate, Envelope, GrantRef, OperationReceipt,
    RequestState, ResourceRef, Selection,
};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{
    NativeCall, NativeResponse, OperationContext, PreparedOperation, RetryAfter,
};
use craft_core::registry::Registry;
use craft_core::storage::{
    ArtifactReceipt, ArtifactSpec, ArtifactStore, JournalRecord, OperationBinding,
    OperationJournal, StoreError, SubmissionState,
};
use craft_platform::InputRoot;
use craft_provider_jira::attachment_upload::{self as native, UploadInput};
use craft_provider_jira::attachments::{AttachmentRecord, SettingsRecord};
use craft_provider_jira::create_metadata::MetadataPrincipal;
use craft_runtime::artifacts::NativeArtifacts;
use craft_runtime::auth::{ServiceOAuthClient, VerifiedServiceAccess};
use craft_runtime::issue_mutation_policy::{
    IssueMutationOperation as Op, IssueMutationPolicyRecord,
};
use craft_runtime::journal::{MutationAuthority, NativeJournal, StepOutcome};
use craft_runtime::profiles::ProfileStore;
use craft_runtime::secrets::LockedCredentials;
use craft_runtime::transport::{MultipartFile, TlsPolicy};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::Path;
pub const MINIMUM_OUTPUT: usize = 8_388_608;
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UploadData {
    pub receipt: OperationReceipt,
    pub target: ResourceRef,
    pub issue_id: String,
    pub artifact_name: String,
    #[schemars(extend("const"="uploads"))]
    pub snapshot_collection: String,
    pub snapshot: Option<ArtifactReceipt>,
    pub snapshot_write_state: RequestState,
    pub snapshot_reused: bool,
    pub saved_request: Option<SavedReceipt>,
    pub attachment: Option<AttachmentRecord>,
    pub acknowledged: bool,
    pub acknowledged_ids_complete: bool,
    pub desired_fields_verified: bool,
    #[schemars(extend("const"=false))]
    pub remote_digest_verified: bool,
    #[schemars(extend("const"=false))]
    pub atomic_project_check: bool,
    pub settings: Option<SettingsRecord>,
    pub unqualified_acknowledgments: Vec<ResourceRef>,
    pub prior_record: Option<JournalRecord>,
    pub latest_record: Option<JournalRecord>,
    pub pending_record: Option<JournalRecord>,
    pub persistence: PersistenceState,
    pub pending_record_state: PersistenceState,
    pub http_status: Option<u16>,
    pub provider_request_id: Option<String>,
    pub retry_after: Option<RetryAfter>,
    pub credential_cache_update: Option<CredentialCacheUpdate>,
    pub reconciliation_required: bool,
    #[schemars(extend("const"=false))]
    pub replay_authorized: bool,
}
fn empty(args: &command::UploadArgs, target: ResourceRef) -> UploadData {
    UploadData {
        receipt: OperationReceipt {
            operation_id: args.operation_id.clone(),
            request_state: RequestState::NotSent,
            confirmed_resources: vec![],
            job: None,
        },
        target,
        issue_id: args.issue_id.clone(),
        artifact_name: args.artifact_name.clone(),
        snapshot_collection: "uploads".into(),
        snapshot: None,
        snapshot_write_state: RequestState::NotSent,
        snapshot_reused: false,
        saved_request: None,
        attachment: None,
        acknowledged: false,
        acknowledged_ids_complete: false,
        desired_fields_verified: false,
        remote_digest_verified: false,
        atomic_project_check: false,
        settings: None,
        unqualified_acknowledgments: vec![],
        prior_record: None,
        latest_record: None,
        pending_record: None,
        persistence: PersistenceState::Unrecorded,
        pending_record_state: PersistenceState::Unrecorded,
        http_status: None,
        provider_request_id: None,
        retry_after: None,
        credential_cache_update: None,
        reconciliation_required: false,
        replay_authorized: false,
    }
}
fn denied(message: &str) -> CraftError {
    CraftError::new(ErrorCode::PolicyDenied, message)
}
fn storage(error: StoreError) -> CraftError {
    crate::attachment_download::storage(error)
}
fn state(value: SubmissionState) -> RequestState {
    match value {
        SubmissionState::Prepared => RequestState::NotSent,
        SubmissionState::Confirmed => RequestState::Confirmed,
        SubmissionState::Failed => RequestState::Failed,
        _ => RequestState::SentUnknown,
    }
}
fn absorb(data: &mut UploadData, record: &JournalRecord) {
    for step in record.steps.values() {
        for resource in &step.confirmed_resources {
            if !data.receipt.confirmed_resources.contains(resource) {
                data.receipt.confirmed_resources.push(resource.clone());
            }
        }
        data.receipt.job = data.receipt.job.clone().or(step.job.clone());
    }
    let recorded = if record.steps.values().any(|s| {
        matches!(
            s.state,
            SubmissionState::SendStarted | SubmissionState::Unknown
        )
    }) {
        RequestState::SentUnknown
    } else if record
        .steps
        .values()
        .any(|s| s.state == SubmissionState::Confirmed)
    {
        RequestState::Confirmed
    } else if record
        .steps
        .values()
        .any(|s| s.state == SubmissionState::Failed)
    {
        RequestState::Failed
    } else {
        RequestState::NotSent
    };
    if data.receipt.request_state == RequestState::NotSent
        || data.receipt.request_state == RequestState::SentUnknown
            && recorded == RequestState::Confirmed
    {
        data.receipt.request_state = recorded;
    }
    if data.receipt.request_state == RequestState::Failed
        && (!data.receipt.confirmed_resources.is_empty()
            || data.receipt.job.is_some()
            || recorded == RequestState::Confirmed)
    {
        data.receipt.request_state = RequestState::SentUnknown;
    }
}
pub struct UploadEnvironment<'a, 'credentials> {
    pub store: &'a ProfileStore,
    pub client: &'a ServiceOAuthClient,
    pub locked: &'a LockedCredentials<'credentials>,
    pub journal: &'a NativeJournal,
    pub tls: TlsPolicy,
    pub control: InvocationControl,
}
pub struct PreparedUpload {
    args: command::UploadArgs,
    target: ResourceRef,
    policy: IssueMutationPolicyRecord,
    request_id: String,
    max_bytes: usize,
}
impl PreparedUpload {
    pub fn prepare(
        args: command::UploadArgs,
        fields: &[String],
        options: &InvocationOptions<'_>,
        store: &ProfileStore,
        control: &InvocationControl,
    ) -> Result<Self, CraftError> {
        args.validate(fields, options)?;
        let (name, _, hints) =
            crate::profiles::selected_profile_context(store, None, options, control)?;
        let profile = store.get(&name, control)?;
        if profile.provider != "jira"
            || profile.deployment != "cloud"
            || profile.auth_mode != AuthMode::Service
        {
            return Err(denied("Upload requires a Jira Cloud service profile."));
        }
        let cloud = args
            .cloud_id
            .as_deref()
            .unwrap_or(&profile.tenant_ref()?.tenant)
            .to_owned();
        let target = ResourceRef {
            provider: "jira".into(),
            deployment: "cloud".into(),
            tenant: cloud,
            resource_type: "project".into(),
            id: args.project_id.clone(),
        };
        let target = profile.resolve_target("project", Some(&target), &hints)?;
        let policy = store
            .issue_mutation_policy(&name, &target.id, control)?
            .ok_or_else(|| denied("No exact project upload allowance is configured."))?;
        if policy.target != target {
            return Err(denied(
                "Upload target differs from its captured project allowance.",
            ));
        }
        store.verify_issue_mutation_policy(&policy, Op::AttachmentUpload, control)?;
        Ok(Self {
            args,
            target,
            policy,
            request_id: options.request_id.into(),
            max_bytes: options.max_bytes,
        })
    }
    pub fn grant(&self) -> &GrantRef {
        &self.policy.grant
    }
    fn check_policy(
        &self,
        store: &ProfileStore,
        control: &InvocationControl,
        extra: &dyn Fn() -> Result<(), CraftError>,
    ) -> Result<(), CraftError> {
        control.check()?;
        extra()?;
        store.verify_issue_mutation_policy(&self.policy, Op::AttachmentUpload, control)?;
        extra()?;
        control.check()
    }
    fn prior(&self, journal: &NativeJournal, data: &mut UploadData) -> Result<(), CraftError> {
        let record = journal.get(&self.args.operation_id).map_err(|error| {
            data.persistence = PersistenceState::Uncertain;
            data.receipt.request_state = RequestState::SentUnknown;
            data.reconciliation_required = true;
            storage(error)
        })?;
        if data.prior_record.is_none() {
            data.prior_record = record.clone();
        } else {
            data.latest_record = record.clone();
        }
        if let Some(record) = record {
            data.persistence = PersistenceState::Recorded;
            absorb(data, &record);
            if record.binding.target != self.target
                || record.binding.grant != self.policy.grant
                || record.binding.actor != self.policy.expected_actor
                || record.steps.iter().any(|(name, s)| {
                    name != native::UPLOAD_ROUTE || s.state != SubmissionState::Prepared
                })
            {
                return Err(CraftError::new(
                    ErrorCode::OperationIdReused,
                    "Upload operation changed or already started; retain its original source and never replay it.",
                ));
            }
        }
        Ok(())
    }
    fn capture(
        &self,
        store: &ProfileStore,
        journal: &NativeJournal,
        control: &InvocationControl,
        data: &mut UploadData,
    ) -> Result<(Registry, PreparedOperation, PreparedOperation), CraftError> {
        let saved = saved_requests::inspect(store, &self.args.operation_id, control)?.map(
            |(saved, receipt)| {
                data.saved_request = Some(receipt);
                saved
            },
        );
        let old = if let Some(saved) = &saved {
            if saved.original.command != native::UPLOAD
                || saved.binding.target != self.target
                || saved.binding.grant != self.policy.grant
                || saved.binding.actor != self.policy.expected_actor
            {
                return Err(CraftError::new(
                    ErrorCode::OperationIdReused,
                    "Saved source belongs to another upload intent or authority.",
                ));
            }
            let input: UploadInput = serde_json::from_value(saved.original.input.clone())
                .map_err(|_| denied("Saved upload input is invalid."))?;
            let selection_matches = self.args.from_artifact
                || input.source_selection_digest == self.args.selection_digest()?;
            if input.issue_id != self.args.issue_id
                || input.artifact_name != self.args.artifact_name
                || input.filename != self.args.filename
                || input.media_type != self.args.media_type
                || !selection_matches
                || self
                    .args
                    .expected_digest
                    .as_ref()
                    .is_some_and(|v| v != &input.digest)
            {
                return Err(CraftError::new(
                    ErrorCode::OperationIdReused,
                    "Upload source selection or file metadata differs from the saved operation.",
                ));
            }
            Some(input)
        } else {
            None
        };
        if old.is_none() && data.prior_record.is_some() {
            return Err(denied(
                "An existing journal without its canonical source cannot authorize upload recovery.",
            ));
        }
        let artifacts =
            NativeArtifacts::open(&store.path().join("uploads"), self.args.byte_limit, control)
                .map_err(storage)?;
        let existing = artifacts
            .receipt(&self.args.artifact_name)
            .map_err(storage)?;
        if let Some(receipt) = existing {
            data.snapshot = Some(receipt.clone());
            data.snapshot_write_state = RequestState::Confirmed;
            if old.is_none() && !self.args.from_artifact {
                return Err(CraftError::new(
                    ErrorCode::VersionConflict,
                    "Snapshot name exists without a matching saved source; inspect it or explicitly select a digest-bound artifact.",
                ));
            }
            if self
                .args
                .expected_digest
                .as_ref()
                .is_some_and(|v| v != &receipt.digest)
                || old
                    .as_ref()
                    .is_some_and(|v| v.digest != receipt.digest || v.size_bytes != receipt.bytes)
            {
                return Err(CraftError::new(
                    ErrorCode::VersionConflict,
                    "Existing snapshot differs from the exact expected digest or size.",
                ));
            }
            data.snapshot_reused = true;
        } else {
            if self.args.from_artifact {
                return Err(CraftError::new(
                    ErrorCode::VersionConflict,
                    "The selected upload snapshot is absent; no native upload was attempted.",
                ));
            }
            let root = InputRoot::open(Path::new(
                self.args
                    .file_root
                    .as_deref()
                    .ok_or_else(|| denied("Missing source root"))?,
            ))
            .map_err(|_| CraftError::field("/file_root", "expected_existing_input_root"))?;
            data.snapshot_write_state = RequestState::SentUnknown;
            let expected = old
                .as_ref()
                .map(|v| v.digest.as_str())
                .or(self.args.expected_digest.as_deref());
            match artifacts.import_file(
                &root,
                Path::new(
                    self.args
                        .file
                        .as_deref()
                        .ok_or_else(|| denied("Missing source file"))?,
                ),
                ArtifactSpec {
                    name: self.args.artifact_name.clone(),
                    media_type: self.args.media_type.clone(),
                    max_bytes: self.args.byte_limit,
                    source: Some(self.target.clone()),
                },
                expected,
            ) {
                Ok(receipt) => {
                    data.snapshot = Some(receipt);
                    data.snapshot_write_state = RequestState::Confirmed;
                }
                Err(error) => {
                    data.snapshot_write_state = if error == StoreError::OutcomeUnknown {
                        RequestState::SentUnknown
                    } else {
                        RequestState::Failed
                    };
                    return Err(storage(error));
                }
            }
        }
        let snapshot = data
            .snapshot
            .as_ref()
            .ok_or_else(|| denied("Missing verified snapshot"))?;
        let input = UploadInput {
            issue_id: self.args.issue_id.clone(),
            artifact_name: self.args.artifact_name.clone(),
            digest: snapshot.digest.clone(),
            source_selection_digest: old
                .as_ref()
                .map(|v| v.source_selection_digest.clone())
                .unwrap_or(self.args.selection_digest()?),
            size_bytes: snapshot.bytes,
            filename: self.args.filename.clone(),
            media_type: self.args.media_type.clone(),
        };
        let mut registry = Registry::default();
        native::register(&mut registry)?;
        let make = |command: &str| {
            registry.prepare(CanonicalRequest {
                command: command.into(),
                target: self.target.clone(),
                input: json!(input),
                selection: Selection {
                    max_bytes: self.max_bytes as u32,
                    ..Selection::default()
                },
                preconditions: None,
            })
        };
        let write = make(native::UPLOAD)?;
        let read = make(native::CONTEXT)?;
        let binding = write.mutation_binding(
            &self.args.operation_id,
            &self.policy.grant,
            &self.policy.expected_actor,
        )?;
        if saved.as_ref().is_some_and(|saved| saved.binding != binding)
            || data
                .prior_record
                .as_ref()
                .is_some_and(|record| record.binding != binding)
        {
            return Err(CraftError::new(
                ErrorCode::OperationIdReused,
                "Upload binding differs from its retained original source or journal.",
            ));
        }
        match saved_requests::capture(
            store,
            journal,
            write.request(),
            &binding,
            native::UPLOAD_ROUTE,
            control,
        ) {
            Ok(Some(receipt)) => data.saved_request = Some(receipt),
            Ok(None) => {
                self.prior(journal, data)?;
                return Err(CraftError::new(
                    ErrorCode::OperationIdReused,
                    "Upload was claimed concurrently; inspect its retained evidence.",
                ));
            }
            Err(failure) => {
                data.reconciliation_required = failure.evidence_unavailable
                    || failure
                        .receipt
                        .as_ref()
                        .is_some_and(|r| r.write_state == RequestState::SentUnknown);
                data.saved_request = failure.receipt;
                data.latest_record = failure.prior;
                if let Some(record) = data.latest_record.clone() {
                    absorb(data, &record);
                }
                return Err(failure.error);
            }
        }
        Ok((registry, read, write))
    }
    pub fn execute(
        &self,
        environment: UploadEnvironment<'_, '_>,
        extra: impl Fn() -> Result<(), CraftError>,
    ) -> CommandResult {
        let UploadEnvironment {
            store,
            client,
            locked,
            journal,
            tls,
            control,
        } = environment;
        let journal = journal.for_invocation(&control);
        let mut data = empty(&self.args, self.target.clone());
        let mut claim_attempted = false;
        let result = (|| {
            let policy = || self.check_policy(store, &control, &extra);
            policy()?;
            self.prior(&journal, &mut data)?;
            let (registry, read, write) = self.capture(store, &journal, &control, &mut data)?;
            policy()?;
            self.prior(&journal, &mut data)?;
            if locked.grant() != self.policy.grant {
                return Err(denied("Locked upload grant differs from its allowance."));
            }
            let client = client.for_invocation(control.clone())?;
            let access = match client.open_service_access(locked, &self.target.tenant_ref()) {
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
                                .map(|state| CredentialCacheUpdate {
                                    grant: locked.grant(),
                                    write_state: state,
                                    cleanup_pending: None,
                                })
                        });
                    return Err(failure.error);
                }
            };
            data.credential_cache_update = access.cache_update().map(|r| CredentialCacheUpdate {
                grant: access.grant().clone(),
                write_state: RequestState::Confirmed,
                cleanup_pending: Some(r.cleanup_pending),
            });
            let check = || {
                policy()?;
                self.access(&access, &registry, &read, &write)?;
                policy()
            };
            check()?;
            self.prior(&journal, &mut data)?;
            let principal = MetadataPrincipal {
                grant: access.grant().clone(),
                actor: access.identity().actor().into(),
            };
            let mut context =
                access.read_context(&registry, &read, tls.clone(), control.clone())?;
            let frozen = native::prepare(
                &read,
                &write,
                &mut Guard {
                    native: &mut context,
                    check: &check,
                },
                principal.clone(),
                access.clock(),
            )?;
            data.settings = Some(frozen.settings().clone());
            frozen.verify(&write, &principal)?;
            check()?;
            let artifacts = NativeArtifacts::open(
                &store.path().join("uploads"),
                self.args.byte_limit,
                &control,
            )
            .map_err(storage)?;
            let file = MultipartFile {
                input: artifacts
                    .open_verified_stream(&self.args.artifact_name, &frozen.input().digest)
                    .map_err(storage)?,
                filename: self.args.filename.clone(),
                media_type: self.args.media_type.clone(),
            };
            let mutation = access.prepare_jira_upload(
                &registry,
                &write,
                tls,
                control.clone(),
                frozen.call(),
                file,
            )?;
            claim_attempted = true;
            let attempt = mutation.send_with_guards(
                &journal,
                &MutationAuthority {
                    apply: true,
                    operation_id: &self.args.operation_id,
                    grant: access.grant(),
                    actor: access.identity().actor(),
                },
                native::UPLOAD_ROUTE,
                || {
                    check()?;
                    frozen.verify(&write, &principal)?;
                    check()
                },
                check,
            )?;
            complete(
                &write,
                &write.mutation_binding(
                    &self.args.operation_id,
                    &self.policy.grant,
                    &self.policy.expected_actor,
                )?,
                &attempt,
                &mut data,
            )
        })();
        if let Err(mut error) = result {
            if claim_attempted {
                match journal.get(&self.args.operation_id) {
                    Ok(record) => {
                        if let Some(record) = record {
                            if record.binding.target == self.target
                                && record.binding.grant == self.policy.grant
                                && record.binding.actor == self.policy.expected_actor
                            {
                                absorb(&mut data, &record);
                            }
                            data.latest_record = Some(record);
                        }
                    }
                    Err(_) => data.persistence = PersistenceState::Uncertain,
                }
            }
            if error.details.write_state == RequestState::SentUnknown
                && data.receipt.request_state == RequestState::NotSent
            {
                data.receipt.request_state = RequestState::SentUnknown;
            }
            data.reconciliation_required = data.reconciliation_required
                || data.receipt.request_state != RequestState::NotSent
                || data.prior_record.is_some()
                || data.latest_record.is_some()
                || data.snapshot_write_state == RequestState::SentUnknown
                || data
                    .saved_request
                    .as_ref()
                    .is_some_and(|r| r.write_state == RequestState::SentUnknown);
            error.details.write_state = data.receipt.request_state;
            error.retryable = false;
            error.next_action="Retain this operation, snapshot and saved request. Inspect known effects and uncertain local publication; no upload retry or replay was performed.".into();
            return CommandResult {
                envelope: Envelope::failure_with_data(
                    native::UPLOAD,
                    self.request_id.clone(),
                    error,
                    json!(data),
                ),
                output_policy: OutputPolicy::PreserveEvidence,
            };
        }
        CommandResult {
            envelope: Envelope::success(native::UPLOAD, self.request_id.clone(), json!(data)),
            output_policy: OutputPolicy::PreserveEvidence,
        }
    }
    fn access(
        &self,
        access: &VerifiedServiceAccess,
        registry: &Registry,
        read: &PreparedOperation,
        write: &PreparedOperation,
    ) -> Result<(), CraftError> {
        if access.tenant() != &self.target.tenant_ref()
            || access.grant() != &self.policy.grant
            || access.identity().actor() != self.policy.expected_actor
        {
            return Err(denied(
                "Upload access differs from its exact site, grant or actor.",
            ));
        }
        access.check_operation(registry, read)?;
        access.check_operation(registry, write)
    }
}
struct Guard<'a> {
    native: &'a mut dyn OperationContext,
    check: &'a dyn Fn() -> Result<(), CraftError>,
}
impl OperationContext for Guard<'_> {
    fn checkpoint(&self) -> Result<(), CraftError> {
        (self.check)()?;
        self.native.checkpoint()
    }
    fn verify_operation(&self, op: &PreparedOperation) -> Result<(), CraftError> {
        self.checkpoint()?;
        self.native.verify_operation(op)
    }
    fn invoke(&mut self, call: &NativeCall) -> Result<NativeResponse, CraftError> {
        self.checkpoint()?;
        let response = self.native.invoke(call)?;
        self.checkpoint()?;
        Ok(response)
    }
}
struct Completion<'a> {
    binding: &'a OperationBinding,
    command: &'a str,
    route: &'a str,
    step: &'a str,
    response: Result<&'a NativeResponse, &'a CraftError>,
    incomplete: Option<&'a NativeResponse>,
    pending: Result<&'a JournalRecord, &'a StoreError>,
}
fn complete(
    operation: &PreparedOperation,
    binding: &OperationBinding,
    attempt: &craft_runtime::transport::MutationAttempt,
    data: &mut UploadData,
) -> Result<(), CraftError> {
    finish_with(
        operation,
        binding,
        Completion {
            binding: attempt.binding(),
            command: attempt.command(),
            route: attempt.route(),
            step: attempt.step(),
            response: attempt.response.as_ref(),
            incomplete: attempt.incomplete_upload_response.as_ref(),
            pending: attempt.pending_evidence.as_ref(),
        },
        data,
        |outcome| attempt.record_outcome(outcome),
    )
}
fn finish_with(
    operation: &PreparedOperation,
    binding: &OperationBinding,
    view: Completion<'_>,
    data: &mut UploadData,
    record: impl FnOnce(&StepOutcome) -> Result<JournalRecord, StoreError>,
) -> Result<(), CraftError> {
    if view.binding != binding
        || view.command != native::UPLOAD
        || view.route != native::UPLOAD_ROUTE
        || view.step != native::UPLOAD_ROUTE
    {
        let mut error = denied("Upload completion has a foreign operation or route binding.");
        error.details.write_state = RequestState::SentUnknown;
        data.receipt.request_state = RequestState::SentUnknown;
        data.reconciliation_required = true;
        return Err(error);
    }
    let mut observed = if let Ok(response) = view.response {
        native::interpret(operation, response)?
    } else {
        native::UploadObservation {
            state: SubmissionState::Unknown,
            resources: vec![],
            attachment: None,
            acknowledged: false,
            acknowledged_ids_complete: false,
            desired_fields_verified: false,
            error: Some(
                view.response
                    .err()
                    .cloned()
                    .unwrap_or_else(|| denied("Unqualified upload response")),
            ),
        }
    };
    let response = view.response.ok().or(view.incomplete);
    if let Some(response) = response {
        data.http_status = Some(response.status);
        data.provider_request_id = response.request_id.clone();
        data.retry_after = response.retry_after;
    }
    if let Some(response) = view.incomplete {
        data.unqualified_acknowledgments = native::interpret(operation, response)?.resources;
    }
    data.pending_record = view.pending.ok().cloned();
    data.pending_record_state = match view.pending {
        Ok(_) => PersistenceState::Recorded,
        Err(StoreError::OutcomeUnknown) => PersistenceState::Uncertain,
        Err(_) => PersistenceState::Unrecorded,
    };
    data.receipt.request_state = state(observed.state);
    data.receipt.confirmed_resources = observed.resources.clone();
    if let Some(record) = data.pending_record.clone() {
        if record.binding == *binding && record.validate().is_ok() {
            absorb(data, &record);
        } else {
            observed.error = Some(CraftError::new(
                ErrorCode::PartialApply,
                "Pending upload evidence has a foreign or invalid binding.",
            ));
        }
    }
    match record(&StepOutcome {
        state: observed.state,
        confirmed_resources: observed.resources.clone(),
        job: None,
    }) {
        Ok(record) => {
            let consistent = record.binding == *binding
                && record.validate().is_ok()
                && record.steps.len() == 1
                && record.steps.get(native::UPLOAD_ROUTE).is_some_and(|step| {
                    step.state == observed.state
                        && step.job.is_none()
                        && step.confirmed_resources == observed.resources
                });
            data.persistence = if consistent {
                PersistenceState::Recorded
            } else {
                PersistenceState::Uncertain
            };
            if record.binding == *binding && record.validate().is_ok() {
                absorb(data, &record);
            }
            data.latest_record = Some(record);
            if !consistent {
                observed.error = Some(CraftError::new(
                    ErrorCode::PartialApply,
                    "Upload journal acknowledgment differs from observed native effects.",
                ));
            }
        }
        Err(error) => {
            data.persistence = if error == StoreError::OutcomeUnknown {
                PersistenceState::Uncertain
            } else {
                PersistenceState::Unrecorded
            };
            observed.error.get_or_insert_with(|| {
                CraftError::new(
                    ErrorCode::Internal,
                    "Observed upload effects could not be durably acknowledged locally.",
                )
            });
        }
    }
    if data
        .receipt
        .confirmed_resources
        .iter()
        .any(|resource| !observed.resources.contains(resource))
        || data.receipt.job.is_some()
        || data
            .pending_record
            .as_ref()
            .is_some_and(|r| r.steps.len() != 1)
    {
        observed.error = Some(CraftError::new(
            ErrorCode::PartialApply,
            "Additional recorded upload effects require inspection.",
        ));
    }
    data.attachment = observed.attachment;
    data.acknowledged = observed.acknowledged;
    data.acknowledged_ids_complete = observed.acknowledged_ids_complete;
    data.desired_fields_verified = observed.desired_fields_verified;
    data.reconciliation_required = observed.error.is_some();
    observed.error.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests;
