//! Serialized review packs constrain live observations; they never restore approval authority.
use super::*;
use crate::confluence_reads::inventory::PreparedInventory;
use crate::dispatch::CommandResult;
use crate::output::OutputPolicy;
use crate::profiles::InvocationOptions;
use craft_core::contracts::{CanonicalRequest, EffectClass, Envelope, SCHEMA_VERSION, Selection};
use craft_core::registry::{
    OperationDescriptor, ReconciliationClass, Registry, RetryClass, response_schema, schema_for,
};
use craft_core::storage::{ArtifactReceipt, ArtifactSpec, ArtifactStore, Clock, StoreError};
use craft_runtime::artifacts::NativeArtifacts;
use craft_runtime::control::SystemClock;
use craft_runtime::profiles::resolve_state_root;
use craft_workflows::review::{
    self, CommentInventory, CommentObservation, ReviewEvidencePort, TrustedReviewerPolicy,
};
use std::cell::Cell;
use std::io::Write;
use std::sync::Arc;
pub const VERIFY: &str = "review.verify";

#[cfg(test)]
mod window_tests {
    use super::*;
    use craft_core::control::CancellationToken;
    use craft_test_support::state::ManualClock;
    #[test]
    fn policy_checks_cannot_extend_or_outlive_the_original_batch_window()
    -> Result<(), Box<dyn std::error::Error>> {
        let clock = Arc::new(ManualClock::new(1000));
        let control = InvocationControl::new(clock.clone(), CancellationToken::default(), 60_000)?;
        let window = Window::new(clock.clone(), control);
        let advance = |millis| {
            clock
                .advance(std::time::Duration::from_millis(millis))
                .map_err(|_| CraftError::new(ErrorCode::Internal, "Clock fixture failed."))
        };
        window.around(|| advance(29_999))?;
        assert_eq!(
            window.around(|| advance(1)).err().map(|e| e.code),
            Some(ErrorCode::VersionConflict)
        );
        Ok(())
    }
}
#[derive(clap::Args, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VerifyArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 64), regex(pattern = "^[A-Za-z0-9._-]+$"))]
    pub review_id: String,
    #[arg(long)]
    #[serde(default)]
    pub apply: bool,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VerificationData {
    pub review_id: String,
    pub preparation_artifact: Option<ArtifactReceipt>,
    pub verification_artifact_name: String,
    pub verification_artifact: Option<ArtifactReceipt>,
    pub artifact_write_state: RequestState,
    pub review_verified: bool,
    pub approval_count: u16,
    pub distinct_reviewers: u16,
    pub binding_digest: Option<String>,
    #[schemars(extend("const"=0))]
    pub tickets_created: u32,
    pub credential_cache_updates: Vec<CredentialCacheUpdate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket_preview: Option<tickets::PreviewSummary>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredPack {
    format: String,
    review_id: String,
    profile: String,
    request: BindingRequest,
    snapshot: StoredSnapshot,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredSnapshot {
    binding: Value,
    statement: ReviewStatement,
    plan: Value,
    designs: Vec<Value>,
    tasks: Vec<Value>,
    reviewer_policy_digest: String,
}
struct Loaded {
    pack: StoredPack,
    receipt: ArtifactReceipt,
    binding_digest: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredTicketManifest {
    format: String,
    binding_digest: String,
    batches: Vec<Vec<String>>,
    links: Vec<tickets::DependencyLink>,
    context: StoredSnapshot,
}
impl StoredTicketManifest {
    fn summary(&self) -> Result<tickets::PreviewSummary, CraftError> {
        tickets::material_summary(
            &self.format,
            &self.binding_digest,
            &self.batches,
            &self.links,
            &self.context.tasks,
        )
    }
}
impl VerifyArgs {
    pub fn validate(
        &self,
        fields: &[String],
        options: &InvocationOptions<'_>,
    ) -> Result<(), CraftError> {
        if !identifier(&self.review_id) || options.request_id.len() > 80 {
            return Err(invalid("invalid_review_or_request_identity"));
        }
        craft_core::contracts::validate_identity("/request_id", options.request_id)?;
        validate_profile_name(
            options
                .profile
                .ok_or_else(|| invalid("explicit_confluence_profile_required"))?,
        )?;
        if !fields.is_empty() {
            return Err(CraftError::field(
                "/fields",
                "verification_returns_compact_receipt_read_full_evidence_from_artifact",
            ));
        }
        if options.max_bytes < 1_048_576 {
            let mut error = CraftError::new(
                ErrorCode::OutputBudgetTooSmall,
                "Verification needs capacity for complete multi-profile and artifact effect receipts.",
            );
            error.next_action =
                "Use --max-bytes 1048576; normal output is a compact verification receipt.".into();
            return Err(error);
        }
        if !self.apply {
            return Err(CraftError::new(
                ErrorCode::ApplyRequired,
                "Verification requires apply to publish its immutable local evidence artifact.",
            ));
        }
        Ok(())
    }
}
fn artifact_name(review_id: &str, request_id: &str) -> Result<String, CraftError> {
    Ok(format!(
        "verified-{}.json",
        &digest(&json!({"review_id":review_id,"request_id":request_id}))?[7..]
    ))
}
fn empty(args: &VerifyArgs, request_id: &str) -> VerificationData {
    VerificationData {
        review_id: args.review_id.clone(),
        preparation_artifact: None,
        verification_artifact_name: artifact_name(&args.review_id, request_id).unwrap_or_default(),
        verification_artifact: None,
        artifact_write_state: RequestState::NotSent,
        review_verified: false,
        approval_count: 0,
        distinct_reviewers: 0,
        binding_digest: None,
        tickets_created: 0,
        credential_cache_updates: vec![],
        ticket_preview: None,
    }
}
fn changed(message: &str) -> CraftError {
    CraftError::new(ErrorCode::VersionConflict, message)
}
fn load(
    args: &VerifyArgs,
    profile: &str,
    artifacts: &dyn ArtifactStore,
) -> Result<Loaded, CraftError> {
    let name = format!("review-{}.json", args.review_id);
    let blob = artifacts
        .read(&name, SNAPSHOT_BYTES as u64)
        .map_err(command::local_error)?
        .ok_or_else(|| invalid("prepared_review_artifact_not_found"))?;
    let value = craft_core::input::parse_document_json(&blob.bytes, SNAPSHOT_BYTES)?;
    let pack: StoredPack =
        serde_json::from_value(value).map_err(|_| invalid("invalid_prepared_review_pack"))?;
    pack.request.validate()?;
    let binding_digest = digest(&pack.snapshot.binding)?;
    let statement = &pack.snapshot.statement;
    if pack.format != "craft.review-pack/v1"
        || pack.review_id != args.review_id
        || pack.profile != profile
        || !pack.snapshot.binding.is_object()
        || !pack.snapshot.plan.is_object()
        || pack.snapshot.designs.len() > 64
        || pack.snapshot.tasks.len() != pack.request.tasks.len()
        || !craft_core::pagination::valid_digest(&pack.snapshot.reviewer_policy_digest)
        || statement.format != "craft.review/v1"
        || !matches!(statement.decision, Decision::Approve)
        || statement.binding_digest != binding_digest
        || pack.snapshot.binding["page"] != json!(statement.page)
        || pack.snapshot.binding["plan_id"] != statement.plan_id
        || statement.page.id != pack.request.page_id
        || pack.snapshot.binding["space"]["id"] != pack.request.space_id
        || blob.receipt.name != name
        || blob.receipt.media_type != "application/vnd.craft.review+json"
        || blob.receipt.source.as_ref() != Some(&statement.page)
    {
        return Err(invalid("prepared_review_context_is_inconsistent"));
    }
    Ok(Loaded {
        pack,
        receipt: blob.receipt,
        binding_digest,
    })
}
struct Window {
    clock: Arc<dyn Clock>,
    control: InvocationControl,
    wall: u64,
    mono: u64,
    last_wall: Cell<u64>,
    last_mono: Cell<u64>,
}
impl Window {
    fn new(clock: Arc<dyn Clock>, control: InvocationControl) -> Self {
        let wall = clock.unix_millis();
        let mono = clock.monotonic_millis();
        Self {
            clock,
            control,
            wall,
            mono,
            last_wall: Cell::new(wall),
            last_mono: Cell::new(mono),
        }
    }
    fn check(&self) -> Result<(), CraftError> {
        self.control.check()?;
        let wall = self.clock.unix_millis();
        let mono = self.clock.monotonic_millis();
        if wall < self.last_wall.get()
            || mono < self.last_mono.get()
            || wall.saturating_sub(self.wall) >= 30_000
            || mono.saturating_sub(self.mono) >= 30_000
        {
            return Err(changed(
                "Native review verification expired or a clock moved backward.",
            ));
        }
        self.last_wall.set(wall);
        self.last_mono.set(mono);
        Ok(())
    }
    fn around(&self, check: impl FnOnce() -> Result<(), CraftError>) -> Result<(), CraftError> {
        self.check()?;
        check()?;
        self.check()
    }
}
struct NativePort<'a> {
    host: LiveBindingHost<'a>,
    inventory: PreparedInventory,
    expected_binding: String,
    expected_policy: String,
    window: Window,
    first_comments: Option<Vec<CommentObservation>>,
    last_snapshot: Option<BindingSnapshot>,
}
impl NativePort<'_> {
    fn check(&self) -> Result<(), CraftError> {
        self.window.around(|| self.host.check_all())
    }
}
impl ReviewEvidencePort for NativePort<'_> {
    fn policy(&mut self) -> Result<TrustedReviewerPolicy, CraftError> {
        self.check()?;
        let policy = crate::review_policy::trusted_policy(
            self.host.store,
            &self.host.review_policy,
            &self.host.control,
        )?;
        if digest(&policy)? != self.expected_policy {
            return Err(changed(
                "Reviewer configuration differs from the prepared review context.",
            ));
        }
        self.check()?;
        Ok(policy)
    }
    fn execution_actor(&mut self) -> Result<String, CraftError> {
        self.check()?;
        self.host.reserve_open()?;
        let profile = self.host.profile.clone();
        let policy = self.host.capture_cf(&profile)?;
        let locked = self
            .host
            .credentials
            .lock(&policy.grant, &self.host.control)?;
        let client = self.host.client.for_invocation(self.host.control.clone())?;
        let access = match client.open_service_access(&locked, &policy.target.tenant_ref()) {
            Ok(access) => access,
            Err(failure) => {
                return Err(self.host.record_auth_failure(policy.grant.clone(), failure));
            }
        };
        if let Some(r) = access.cache_update() {
            self.host.receipts.push(CredentialCacheUpdate {
                grant: access.grant().clone(),
                write_state: RequestState::Confirmed,
                cleanup_pending: Some(r.cleanup_pending),
            });
        }
        let mut registry = Registry::default();
        registry.register_provider(&craft_provider_confluence::ConfluenceContext)?;
        let page = registry.prepare(CanonicalRequest {
            command: craft_provider_confluence::PAGE_GET.into(),
            target: policy.target,
            input: json!({"id":self.host.request.page_id}),
            selection: Selection::default(),
            preconditions: None,
        })?;
        access.check_operation(&registry, &page)?;
        self.inventory.check_access(&access, &self.host.control)?;
        let actor = access.identity().actor().to_owned();
        self.check()?;
        Ok(actor)
    }
    fn binding(&mut self) -> Result<ReviewBinding, CraftError> {
        self.check()?;
        let snapshot = self.host.compile()?;
        if digest(&snapshot.binding)? != self.expected_binding {
            return Err(changed(
                "Live plan, design or material field mapping differs from the prepared review context.",
            ));
        }
        self.check()?;
        let binding = snapshot.binding.clone();
        self.last_snapshot = Some(snapshot);
        Ok(binding)
    }
    fn comments(&mut self) -> Result<CommentInventory, CraftError> {
        self.check()?;
        self.host.reserve_open()?;
        let locked = self
            .host
            .credentials
            .lock(self.inventory.grant(), &self.host.control)?;
        let result = self.inventory.execute_cached_with_check(
            self.host.store,
            self.host.client,
            &locked,
            self.host.tls.clone(),
            self.host.control.clone(),
            || self.check(),
        );
        if let Some(receipt) = result.credential_cache_update {
            self.host.receipts.push(receipt);
        }
        let inventory = result.inventory?;
        if result.execution_actor.as_deref()
            != Some(self.host.review_policy.expected_actor.as_str())
        {
            return Err(denied(
                "Inventory execution identity differs from the captured review service actor.",
            ));
        }
        self.check()?;
        if self.first_comments.is_none() {
            self.first_comments = Some(inventory.comments.clone());
        }
        Ok(inventory)
    }
}
pub fn verify_native(
    args: VerifyArgs,
    profile: &str,
    request_id: &str,
    environment: BindingEnvironment<'_>,
    artifacts: &dyn ArtifactStore,
) -> (VerificationData, Option<CraftError>) {
    run_native(args, profile, request_id, environment, artifacts, false)
}

/// A fresh live observation constrained by the original preview, not restored from its JSON.
pub struct RevalidatedManifest<'a> {
    manifest: tickets::TicketManifest,
    review: review::VerifiedReview,
    host: LiveBindingHost<'a>,
    window: Window,
}
impl RevalidatedManifest<'_> {
    pub fn manifest(&self) -> &tickets::TicketManifest {
        &self.manifest
    }
    pub fn review(&self) -> &review::VerifiedReview {
        &self.review
    }
    /// Keeps the original observation window and captured policies; it performs no native revalidation.
    pub fn check(&self) -> Result<(), CraftError> {
        self.window.around(|| self.host.check_all())
    }
}
pub struct TicketRevalidation<'a> {
    pub validated: Option<RevalidatedManifest<'a>>,
    pub credential_cache_updates: Vec<CredentialCacheUpdate>,
    pub error: Option<CraftError>,
}

/// Repeat for every batch. The result does not grant an issue or link send claim.
pub fn revalidate_ticket_manifest<'a>(
    review_id: &str,
    preview_artifact: &str,
    profile: &str,
    request_id: &str,
    environment: BindingEnvironment<'a>,
    artifacts: &dyn ArtifactStore,
) -> TicketRevalidation<'a> {
    let prepared = (|| {
        let args = VerifyArgs {
            review_id: review_id.into(),
            apply: true,
        };
        args.validate(
            &[],
            &InvocationOptions {
                profile: Some(profile),
                state_root: Some(environment.store.path()),
                request_id,
                max_bytes: 1_048_576,
            },
        )?;
        let loaded = load(&args, profile, artifacts)?;
        let blob = artifacts
            .read(preview_artifact, SNAPSHOT_BYTES as u64)
            .map_err(command::local_error)?
            .ok_or_else(|| invalid("ticket_preview_artifact_not_found"))?;
        let expected = craft_core::input::parse_document_json(&blob.bytes, SNAPSHOT_BYTES)?;
        let manifest: StoredTicketManifest =
            serde_json::from_value(expected["manifest"].clone())
                .map_err(|_| invalid("invalid_stored_ticket_manifest"))?;
        if expected["format"] != "craft.ticket-preview/v1"
            || expected["review_id"] != review_id
            || expected["preparation_artifact"] != json!(loaded.receipt)
            || manifest.format != "craft.ticket-manifest/v1"
            || manifest.binding_digest != loaded.binding_digest
            || json!(manifest.summary()?) != expected["ticket_preview"]
            || blob.receipt.media_type != "application/vnd.craft.ticket-preview+json"
            || blob.receipt.source != loaded.receipt.source
            || blob.receipt.name != preview_artifact
        {
            return Err(invalid("ticket_preview_binding_is_inconsistent"));
        }
        let expected_summary = manifest.summary()?;
        let expected_review = expected["review"].clone();
        let window = Window::new(
            environment.client.clock().clone(),
            environment.control.clone(),
        );
        let host = LiveBindingHost::new(environment, profile, request_id, loaded.pack.request)?;
        let inventory = PreparedInventory::prepare(
            loaded.pack.snapshot.statement.page,
            profile,
            request_id,
            host.store,
            &host.control,
        )?;
        Ok((
            NativePort {
                host,
                inventory,
                expected_binding: loaded.binding_digest,
                expected_policy: loaded.pack.snapshot.reviewer_policy_digest,
                window,
                first_comments: None,
                last_snapshot: None,
            },
            expected_summary,
            expected_review,
        ))
    })();
    let (mut port, expected_summary, expected_review) = match prepared {
        Ok(value) => value,
        Err(error) => {
            return TicketRevalidation {
                validated: None,
                credential_cache_updates: vec![],
                error: Some(error),
            };
        }
    };
    let clock = port.host.client.clock().clone();
    let control = port.host.control.clone();
    let result = (|| {
        let verified = review::verify(&mut port, clock.as_ref(), &control)?;
        port.check()?;
        if json!(verified) != expected_review {
            return Err(changed(
                "Original approval identities, revisions, decisions or reviewer policy changed; new approvals cannot substitute for the preview's original approvals.",
            ));
        }
        let manifest = tickets::compile(
            port.last_snapshot
                .take()
                .ok_or_else(|| invalid("missing_revalidated_manifest"))?,
        )?;
        if manifest.summary()?.manifest_digest != expected_summary.manifest_digest {
            return Err(changed(
                "Live ticket material differs from the original preview.",
            ));
        }
        port.check()?;
        Ok((verified, manifest))
    })();
    let receipts = std::mem::take(&mut port.host.receipts);
    match result {
        Ok((review, manifest)) => TicketRevalidation {
            validated: Some(RevalidatedManifest {
                manifest,
                review,
                host: port.host,
                window: port.window,
            }),
            credential_cache_updates: receipts,
            error: None,
        },
        Err(error) => TicketRevalidation {
            validated: None,
            credential_cache_updates: receipts,
            error: Some(error),
        },
    }
}
pub(super) fn run_native(
    args: VerifyArgs,
    profile: &str,
    request_id: &str,
    environment: BindingEnvironment<'_>,
    artifacts: &dyn ArtifactStore,
    preview: bool,
) -> (VerificationData, Option<CraftError>) {
    let mut data = empty(&args, request_id);
    if preview {
        data.verification_artifact_name =
            data.verification_artifact_name
                .replacen("verified-", "tickets-", 1);
    }
    let options = InvocationOptions {
        profile: Some(profile),
        request_id,
        state_root: Some(environment.store.path()),
        max_bytes: 1_048_576,
    };
    if let Err(error) = args.validate(&[], &options) {
        return (data, Some(error));
    }
    let loaded = match load(&args, profile, artifacts) {
        Ok(loaded) => loaded,
        Err(error) => return (data, Some(error)),
    };
    data.preparation_artifact = Some(loaded.receipt.clone());
    match artifacts.receipt(&data.verification_artifact_name) {
        Ok(None) => {}
        Ok(Some(receipt)) => {
            data.verification_artifact = Some(receipt);
            return (data, Some(command::local_error(StoreError::Conflict)));
        }
        Err(error) => return (data, Some(command::local_error(error))),
    }
    let window = Window::new(
        environment.client.clock().clone(),
        environment.control.clone(),
    );
    let host = match LiveBindingHost::new(environment, profile, request_id, loaded.pack.request) {
        Ok(host) => host,
        Err(error) => return (data, Some(error)),
    };
    let page = loaded.pack.snapshot.statement.page;
    let inventory =
        match PreparedInventory::prepare(page, profile, request_id, host.store, &host.control) {
            Ok(inventory) => inventory,
            Err(error) => return (data, Some(error)),
        };
    let clock = host.client.clock().clone();
    let control = host.control.clone();
    let mut port = NativePort {
        host,
        inventory,
        expected_binding: loaded.binding_digest,
        expected_policy: loaded.pack.snapshot.reviewer_policy_digest,
        window,
        first_comments: None,
        last_snapshot: None,
    };
    let result = (|| {
        let verified = review::verify(&mut port, clock.as_ref(), &control)?;
        port.check()?;
        let observed_at = clock.unix_millis();
        port.check()?;
        let manifest = if preview {
            let snapshot = port
                .last_snapshot
                .take()
                .ok_or_else(|| invalid("missing_live_manifest_context"))?;
            if digest(&snapshot.binding)? != digest(verified.binding())? {
                return Err(changed(
                    "Manifest context differs from the current verified binding.",
                ));
            }
            Some(tickets::compile(snapshot)?)
        } else {
            None
        };
        let summary = manifest
            .as_ref()
            .map(tickets::TicketManifest::summary)
            .transpose()?;
        let format = if preview {
            "craft.ticket-preview/v1"
        } else {
            "craft.verified-review/v1"
        };
        let mut evidence = json!({"format":format,"review_id":args.review_id,"preparation_artifact":loaded.receipt,"observed_at":observed_at,"review":verified,"comments":port.first_comments});
        if let Some(manifest) = &manifest {
            evidence["manifest"] = serde_json::to_value(manifest)
                .map_err(|_| invalid("cannot_encode_ticket_manifest"))?;
            evidence["ticket_preview"] = json!(summary);
        }
        let bytes = serde_json::to_vec(&evidence)
            .map_err(|_| invalid("cannot_encode_native_verification_evidence"))?;
        craft_core::input::parse_document_json(&bytes, SNAPSHOT_BYTES)?;
        if bytes.len() > SNAPSHOT_BYTES {
            return Err(invalid("verification_artifact_byte_limit"));
        }
        port.check()?;
        let mut writer = artifacts
            .begin(ArtifactSpec {
                name: data.verification_artifact_name.clone(),
                media_type: if preview {
                    "application/vnd.craft.ticket-preview+json"
                } else {
                    "application/vnd.craft.verified-review+json"
                }
                .into(),
                max_bytes: SNAPSHOT_BYTES as u64,
                source: Some(verified.binding().page.clone()),
            })
            .map_err(command::local_error)?;
        writer
            .write_all(&bytes)
            .map_err(|_| command::local_error(StoreError::Unavailable))?;
        match writer.finish() {
            Ok(receipt) => {
                data.artifact_write_state = RequestState::Confirmed;
                data.verification_artifact = Some(receipt);
            }
            Err(error) => {
                if error == StoreError::OutcomeUnknown {
                    data.artifact_write_state = RequestState::SentUnknown;
                }
                return Err(command::local_error(error));
            }
        }
        port.check()?;
        data.ticket_preview = summary;
        data.review_verified = true;
        data.approval_count = verified
            .approvals()
            .len()
            .try_into()
            .map_err(|_| invalid("approval_count_overflow"))?;
        data.distinct_reviewers = verified
            .approvals()
            .iter()
            .map(|a| a.actor.as_str())
            .collect::<BTreeSet<_>>()
            .len()
            .try_into()
            .map_err(|_| invalid("reviewer_count_overflow"))?;
        data.binding_digest = Some(digest(verified.binding())?);
        Ok(())
    })();
    data.credential_cache_updates = port.host.receipts;
    let error = result.err().map(|mut error| {
        data.ticket_preview = None;
        data.review_verified = false;
        data.approval_count = 0;
        data.distinct_reviewers = 0;
        data.binding_digest = None;
        error.details.write_state = data.artifact_write_state;
        error
    });
    (data, error)
}
pub fn execute(
    args: VerifyArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> CommandResult {
    execute_command(VERIFY, args, fields, options, control)
}
pub(super) fn execute_command(
    command: &str,
    args: VerifyArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> CommandResult {
    let preview = command == tickets::PREVIEW;
    let mut fallback = empty(&args, options.request_id);
    if preview {
        fallback.verification_artifact_name =
            fallback
                .verification_artifact_name
                .replacen("verified-", "tickets-", 1);
    }
    let result = (|| {
        args.validate(fields, options)?;
        let root = resolve_state_root(options.state_root, |key| std::env::var_os(key))?;
        let store = ProfileStore::open(&root, control)?;
        let profile = options
            .profile
            .ok_or_else(|| invalid("explicit_confluence_profile_required"))?;
        let artifacts = NativeArtifacts::open(
            &store.path().join("reviews"),
            SNAPSHOT_BYTES as u64,
            control,
        )
        .map_err(command::local_error)?;
        let loaded = load(&args, profile, &artifacts)?;
        let policy = store
            .review_policy(profile, &loaded.pack.request.space_id, control)?
            .ok_or_else(|| denied("No trusted reviewer policy is configured."))?;
        store.verify_review_policy(&policy, control)?;
        let credentials = NativeCredentials::open(&store)?;
        let client = ServiceOAuthClient::new(
            TlsPolicy::Platform,
            control.clone(),
            Arc::new(SystemClock::default()),
        )?;
        Ok(run_native(
            args,
            profile,
            options.request_id,
            BindingEnvironment {
                store: &store,
                credentials: &credentials,
                client: &client,
                tls: TlsPolicy::Platform,
                control: control.clone(),
            },
            &artifacts,
            preview,
        ))
    })();
    let (data, error) = match result {
        Ok(value) => value,
        Err(error) => (fallback, Some(error)),
    };
    CommandResult {
        envelope: match error {
            Some(error) => {
                Envelope::failure_with_data(command, options.request_id.into(), error, json!(data))
            }
            None => Envelope::success(command, options.request_id.into(), json!(data)),
        },
        output_policy: OutputPolicy::PreserveEvidence,
    }
}
pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    register_command(registry, VERIFY)
}
pub(super) fn register_command(registry: &mut Registry, command: &str) -> Result<(), CraftError> {
    let example = json!({"review_id":"payment-review","apply":true});
    let args: VerifyArgs = serde_json::from_value(example.clone())
        .map_err(|_| invalid("invalid_verification_example"))?;
    registry.register_typed_handler::<VerifyArgs>(OperationDescriptor {id:command.into(),provider:"craft".into(),contract_version:SCHEMA_VERSION.into(),api_revision:"native-review-verification-v1".into(),description:if command == tickets::PREVIEW { "Preview exact reviewed issue payloads, dependency batches and native links in an immutable local artifact; no ticket write." } else { "Verify current native human approval against prepared context and record immutable local evidence; no ticket write." }.into(),effect:EffectClass::Write,input_schema:schema_for::<VerifyArgs>()?,response_schema:response_schema::<VerificationData>()?,example,response_example:json!(Envelope::success(command,"example".into(),json!(empty(&args,"example")))),projection:None,scope_alternatives:vec![],permissions:vec!["Explicit reviewer/page/comment policies and native project create-preparation reads; current eligible human quorum.".into()],tenant_conditions:vec!["Serialized evidence is data; future ticket writes require fresh native review checks.".into()],routes:vec![],retry_class:RetryClass::ReconcileBeforeReplay,reconciliation:ReconciliationClass::KnownResource},Arc::new(crate::public_commands::VerifiedHostRequired))
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_core::control::CancellationToken;
    use craft_test_support::state::ManualClock;
    use std::time::Duration;

    #[test]
    fn verification_window_rejects_expiry_rollback_and_cancellation()
    -> Result<(), Box<dyn std::error::Error>> {
        for mode in ["expiry", "rollback", "cancel"] {
            let clock = Arc::new(ManualClock::new(100_000));
            let cancellation = CancellationToken::default();
            let control = InvocationControl::new(clock.clone(), cancellation.clone(), 60_000)?;
            let window = Window::new(clock.clone(), control);
            clock.advance(Duration::from_millis(20_000))?;
            window.check()?;
            match mode {
                "expiry" => clock.advance(Duration::from_millis(10_000))?,
                "rollback" => clock.set_wall(119_999)?,
                _ => cancellation.cancel(),
            }
            assert!(window.check().is_err(), "{mode}");
        }
        Ok(())
    }
}
