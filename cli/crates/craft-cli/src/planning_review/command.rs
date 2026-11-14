use super::*;
use crate::dispatch::CommandResult;
use crate::output::OutputPolicy;
use crate::profiles::InvocationOptions;
use craft_core::contracts::{EffectClass, Envelope, SCHEMA_VERSION};
use craft_core::registry::{
    OperationDescriptor, ReconciliationClass, Registry, RetryClass, response_schema, schema_for,
};
use craft_core::storage::{ArtifactReceipt, ArtifactSpec, ArtifactStore, StoreError};
use craft_runtime::artifacts::NativeArtifacts;
use craft_runtime::control::SystemClock;
use craft_runtime::profiles::resolve_state_root;
use std::io::Write;
use std::sync::Arc;
pub const PREPARE: &str = "review.prepare";
#[derive(clap::Args, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrepareArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 64), regex(pattern = "^[A-Za-z0-9._-]+$"))]
    pub review_id: String,
    #[arg(long)]
    #[serde(default)]
    pub apply: bool,
    #[command(flatten)]
    #[serde(flatten)]
    pub binding: BindingRequest,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreparationData {
    pub review_id: String,
    pub artifact_name: String,
    pub artifact: Option<ArtifactReceipt>,
    pub artifact_write_state: RequestState,
    pub binding_digest: Option<String>,
    pub statement: Option<ReviewStatement>,
    pub prepared: bool,
    #[schemars(extend("const"=false))]
    pub review_verified: bool,
    #[schemars(extend("const"=0))]
    pub tickets_created: u32,
    pub credential_cache_updates: Vec<CredentialCacheUpdate>,
}
impl PrepareArgs {
    pub fn validate(
        &self,
        fields: &[String],
        options: &InvocationOptions<'_>,
    ) -> Result<(), CraftError> {
        self.binding.validate()?;
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
                "review_preparation_returns_compact_receipt_read_full_context_from_artifact",
            ));
        }
        if options.max_bytes < 1_048_576 {
            let mut error = CraftError::new(
                ErrorCode::OutputBudgetTooSmall,
                "Review preparation needs capacity for complete multi-profile credential and artifact receipts.",
            );
            error.next_action="Use --max-bytes 1048576; normal output contains only a compact receipt and statement.".into();
            return Err(error);
        }
        if !self.apply {
            return Err(CraftError::new(
                ErrorCode::ApplyRequired,
                "Review preparation requires apply to publish its immutable local artifact.",
            ));
        }
        Ok(())
    }
}
fn empty(args: &PrepareArgs) -> PreparationData {
    PreparationData {
        review_id: args.review_id.clone(),
        artifact_name: format!("review-{}.json", args.review_id),
        artifact: None,
        artifact_write_state: RequestState::NotSent,
        binding_digest: None,
        statement: None,
        prepared: false,
        review_verified: false,
        tickets_created: 0,
        credential_cache_updates: vec![],
    }
}
pub(super) fn local_error(error: StoreError) -> CraftError {
    let mut result = CraftError::new(
        match error {
            StoreError::Conflict => ErrorCode::VersionConflict,
            StoreError::Cancelled => ErrorCode::Cancelled,
            StoreError::DeadlineExceeded => ErrorCode::DeadlineExceeded,
            _ => ErrorCode::Internal,
        },
        "Review artifact is unavailable, already exists or could not be published.",
    );
    result.next_action="Inspect the returned artifact name under the same state root before another attempt. A stored review pack is data, not approval authority.".into();
    result
}
/// Native-only testable execution. Artifact bytes retain the full context; the envelope stays compact.
pub fn prepare_native(
    args: PrepareArgs,
    profile: &str,
    request_id: &str,
    environment: BindingEnvironment<'_>,
    artifacts: &dyn ArtifactStore,
) -> (PreparationData, Option<CraftError>) {
    let store = environment.store;
    let mut data = empty(&args);
    let options = InvocationOptions {
        state_root: Some(store.path()),
        profile: Some(profile),
        request_id,
        max_bytes: 1_048_576,
    };
    if let Err(error) = args.validate(&[], &options) {
        return (data, Some(error));
    }
    match artifacts.receipt(&data.artifact_name) {
        Ok(None) => {}
        Ok(Some(receipt)) => {
            data.artifact = Some(receipt);
            return (data, Some(local_error(StoreError::Conflict)));
        }
        Err(error) => return (data, Some(local_error(error))),
    }
    let mut host = match LiveBindingHost::new(environment, profile, request_id, args.binding) {
        Ok(host) => host,
        Err(error) => return (data, Some(error)),
    };
    let outcome = (|| {
        let snapshot = host.compile()?;
        let bytes=serde_json::to_vec(&json!({"format":"craft.review-pack/v1","review_id":data.review_id,"profile":profile,"request":host.request(),"snapshot":snapshot})).map_err(|_|invalid("cannot_encode_review_pack"))?;
        if bytes.len() > SNAPSHOT_BYTES {
            return Err(invalid("review_pack_byte_limit"));
        }
        craft_core::input::parse_document_json(&bytes, SNAPSHOT_BYTES)?;
        let binding_digest = digest(&snapshot.binding)?;
        host.check_all()?;
        let mut writer = artifacts
            .begin(ArtifactSpec {
                name: data.artifact_name.clone(),
                media_type: "application/vnd.craft.review+json".into(),
                max_bytes: SNAPSHOT_BYTES as u64,
                source: Some(snapshot.binding.page.clone()),
            })
            .map_err(local_error)?;
        writer
            .write_all(&bytes)
            .map_err(|_| local_error(StoreError::Unavailable))?;
        match writer.finish() {
            Ok(receipt) => {
                data.artifact_write_state = RequestState::Confirmed;
                data.artifact = Some(receipt);
            }
            Err(error) => {
                if error == StoreError::OutcomeUnknown {
                    data.artifact_write_state = RequestState::SentUnknown;
                }
                return Err(local_error(error));
            }
        }
        host.check_all()?;
        data.binding_digest = Some(binding_digest);
        data.statement = Some(snapshot.statement);
        data.prepared = true;
        Ok(())
    })();
    data.credential_cache_updates = host.receipts().to_vec();
    let error = outcome.err().map(|mut error| {
        error.details.write_state = data.artifact_write_state;
        error
    });
    (data, error)
}
pub fn execute(
    args: PrepareArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> CommandResult {
    let fallback = empty(&args);
    let result = (|| {
        args.validate(fields, options)?;
        let root = resolve_state_root(options.state_root, |key| std::env::var_os(key))?;
        let store = ProfileStore::open(&root, control)?;
        let profile = options
            .profile
            .ok_or_else(|| invalid("explicit_confluence_profile_required"))?;
        let policy = store
            .review_policy(profile, &args.binding.space_id, control)?
            .ok_or_else(|| denied("No reviewer policy is configured."))?;
        store.verify_review_policy(&policy, control)?;
        let artifacts = NativeArtifacts::open(
            &store.path().join("reviews"),
            SNAPSHOT_BYTES as u64,
            control,
        )
        .map_err(local_error)?;
        let credentials = NativeCredentials::open(&store)?;
        let client = ServiceOAuthClient::new(
            TlsPolicy::Platform,
            control.clone(),
            Arc::new(SystemClock::default()),
        )?;
        Ok(prepare_native(
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
        ))
    })();
    let (data, error) = match result {
        Ok(value) => value,
        Err(error) => (fallback, Some(error)),
    };
    CommandResult {
        envelope: match error {
            Some(error) => {
                Envelope::failure_with_data(PREPARE, options.request_id.into(), error, json!(data))
            }
            None => Envelope::success(PREPARE, options.request_id.into(), json!(data)),
        },
        output_policy: OutputPolicy::PreserveEvidence,
    }
}
pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    let input = json!({"review_id":"payment-plan","apply":true,"page_id":"101","space_id":"100","tasks":[{"task_id":"define-contract","profile":"jira-test","project_id":"10000","input":{"issue_type_id":"10001","summary":"Define the contract"}}]});
    let args: PrepareArgs =
        serde_json::from_value(input.clone()).map_err(|_| invalid("invalid_review_example"))?;
    registry.register_typed_handler::<PrepareArgs>(OperationDescriptor {id:PREPARE.into(),provider:"craft".into(),contract_version:SCHEMA_VERSION.into(),api_revision:"native-review-preparation-v1".into(),description:"Read live plan/design/create metadata and publish immutable local human-review context; no cloud write or approval.".into(),effect:EffectClass::Write,input_schema:schema_for::<PrepareArgs>()?,response_schema:response_schema::<PreparationData>()?,example:input,response_example:json!(Envelope::success(PREPARE,"example".into(),json!(empty(&args)))),projection:None,scope_alternatives:vec![],permissions:vec!["Explicit Confluence reviewer/page-read policy and exact Jira create-preparation policies; native scopes checked per service.".into()],tenant_conditions:vec!["Local immutable review context; human review, verification and ticket authority remain separate.".into()],routes:vec![],retry_class:RetryClass::ReconcileBeforeReplay,reconciliation:ReconciliationClass::KnownResource},Arc::new(crate::public_commands::VerifiedHostRequired))
}
