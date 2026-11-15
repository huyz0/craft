//! Verified local artifact receipts and digest-bound output recovery never acquire cloud authority.
use crate::dispatch::CommandResult;
use crate::output::OutputPolicy;
use crate::profiles::InvocationOptions;
use craft_core::contracts::{EffectClass, Envelope, RequestState, SCHEMA_VERSION};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::registry::{
    OperationDescriptor, ReconciliationClass, Registry, RetryClass, response_schema, schema_for,
};
use craft_core::storage::{ArtifactReceipt, ArtifactStore, StoreError, validate_artifact_name};
use craft_platform::{OutputRoot, OverwritePolicy, PrivateRoot};
use craft_runtime::artifacts::NativeArtifacts;
use craft_runtime::profiles::resolve_state_root;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::Path;

pub const GET: &str = "artifact.get";
pub const EXPORT: &str = "artifact.export";
const MAX_BYTES: u64 = 1_073_741_824;
fn byte_limit() -> u64 {
    16_777_216
}
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Collection {
    #[default]
    Downloads,
    Uploads,
    Requests,
    Reviews,
    Scans,
}
impl Collection {
    pub fn directory(self) -> &'static str {
        match self {
            Self::Downloads => "downloads",
            Self::Uploads => "uploads",
            Self::Requests => "requests",
            Self::Reviews => "reviews",
            Self::Scans => "scans",
        }
    }
}
#[derive(Clone, clap::Args, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 128), regex(pattern = "^[A-Za-z0-9._-]+$"))]
    pub artifact_name: String,
    #[arg(long, value_enum, default_value = "downloads")]
    #[serde(default)]
    pub collection: Collection,
    #[arg(long, default_value_t = 16_777_216)]
    #[serde(default = "byte_limit")]
    #[schemars(range(min = 1, max = 1_073_741_824))]
    pub byte_limit: u64,
}
#[derive(Clone, clap::Args, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportArgs {
    #[command(flatten)]
    #[serde(flatten)]
    pub source: GetArgs,
    #[arg(long, default_value = "")]
    #[schemars(regex(pattern = "^sha256:[0-9a-f]{64}$"))]
    pub expected_digest: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 4096))]
    pub output_root: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 4096))]
    pub output: String,
    #[arg(long)]
    #[serde(default)]
    pub overwrite: bool,
}
#[derive(Clone, clap::Subcommand)]
pub enum ArtifactAction {
    Get(GetArgs),
    Export(ExportArgs),
}
impl ArtifactAction {
    pub fn command(&self) -> &'static str {
        match self {
            Self::Get(_) => GET,
            Self::Export(_) => EXPORT,
        }
    }
    pub fn arguments(&self) -> Result<Value, CraftError> {
        match self {
            Self::Get(v) => serde_json::to_value(v),
            Self::Export(v) => serde_json::to_value(v),
        }
        .map_err(|_| CraftError::field("/input", "invalid_local_artifact_arguments"))
    }
    fn source(&self) -> &GetArgs {
        match self {
            Self::Get(v) => v,
            Self::Export(v) => &v.source,
        }
    }
    pub fn validate(
        &self,
        fields: &[String],
        options: &InvocationOptions<'_>,
    ) -> Result<(), CraftError> {
        let source = self.source();
        validate_artifact_name(&source.artifact_name).map_err(storage)?;
        craft_core::contracts::validate_identity("/request_id", options.request_id)?;
        if !fields.is_empty()
            || options.profile.is_some()
            || options.request_id.len() > 80
            || !(1..=MAX_BYTES).contains(&source.byte_limit)
        {
            return Err(CraftError::field(
                "/input",
                "local_artifacts_require_bounded_source_without_profile_or_projection",
            ));
        }
        if options.max_bytes < 2048 {
            return Err(budget());
        }
        if let Self::Export(v) = self {
            if !valid_digest(&v.expected_digest) {
                return Err(CraftError::field(
                    "/expected_digest",
                    "expected_observed_lowercase_sha256",
                ));
            }
            OutputRoot::validate_root(Path::new(&v.output_root)).map_err(|_| {
                CraftError::field("/output_root", "expected_safe_absolute_output_root")
            })?;
            OutputRoot::validate_destination(Path::new(&v.output))
                .map_err(|_| CraftError::field("/output", "expected_safe_relative_output_path"))?;
        }
        Ok(())
    }
}
fn valid_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|v| {
        v.len() == 64
            && v.bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    })
}
fn budget() -> CraftError {
    CraftError::new(
        ErrorCode::OutputBudgetTooSmall,
        "Increase --max-bytes to retain complete local artifact and export evidence before publication.",
    )
}
fn storage(value: StoreError) -> CraftError {
    let mut error = CraftError::new(
        match value {
            StoreError::Conflict => ErrorCode::VersionConflict,
            StoreError::Invalid => ErrorCode::InputInvalid,
            StoreError::TooLarge => ErrorCode::ItemTooLarge,
            StoreError::Cancelled => ErrorCode::Cancelled,
            StoreError::DeadlineExceeded => ErrorCode::DeadlineExceeded,
            StoreError::OutcomeUnknown => ErrorCode::WriteOutcomeUnknown,
            _ => ErrorCode::Internal,
        },
        "Local artifact inspection or export failed; unreadable state does not establish absence.",
    );
    error.next_action="Inspect the same artifact identity and output under the original state root. Retain publication evidence and do not refetch or retry uncertain publication automatically.".into();
    error
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactData {
    pub collection: Collection,
    pub artifact_name: String,
    pub found: Option<bool>,
    pub artifact: Option<ArtifactReceipt>,
    pub integrity_verified: bool,
    pub exported: Option<ArtifactReceipt>,
    pub export_write_state: RequestState,
    #[schemars(extend("const"=false))]
    pub live_verified: bool,
    #[schemars(extend("const"=false))]
    pub replay_authorized: bool,
}
impl ArtifactData {
    fn empty(source: &GetArgs) -> Self {
        Self {
            collection: source.collection,
            artifact_name: source.artifact_name.clone(),
            found: None,
            artifact: None,
            integrity_verified: false,
            exported: None,
            export_write_state: RequestState::NotSent,
            live_verified: false,
            replay_authorized: false,
        }
    }
}
fn reserve(
    action: &ArtifactAction,
    data: &ArtifactData,
    options: &InvocationOptions<'_>,
) -> Result<(), CraftError> {
    let mut sample = data.clone();
    if let (ArtifactAction::Export(args), Some(receipt)) = (action, &data.artifact) {
        sample.exported = Some(ArtifactReceipt {
            local_path: Some(
                Path::new(&args.output_root)
                    .join(&args.output)
                    .to_str()
                    .ok_or_else(|| CraftError::field("/output", "expected_utf8_output"))?
                    .into(),
            ),
            ..receipt.clone()
        });
        sample.export_write_state = RequestState::SentUnknown;
    }
    let mut error = CraftError::field(format!("/{}", "x".repeat(255)), "x".repeat(256));
    error.message = "x".repeat(500);
    error.next_action = "x".repeat(1000);
    let envelope = Envelope::failure_with_data(
        action.command(),
        options.request_id.into(),
        error,
        json!(sample),
    );
    if serde_json::to_vec(&envelope)
        .map_err(|_| storage(StoreError::Invalid))?
        .len()
        + 1
        > options.max_bytes
    {
        return Err(budget());
    }
    Ok(())
}
pub fn execute(
    action: ArtifactAction,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> CommandResult {
    let mut data = ArtifactData::empty(action.source());
    let result = (|| {
        action.validate(fields, options)?;
        control.check()?;
        let root = resolve_state_root(options.state_root, |key| std::env::var_os(key))?;
        let protected = PrivateRoot::open(&root).map_err(|_| storage(StoreError::Unavailable))?;
        let directory = root.join(action.source().collection.directory());
        let exists = match std::fs::symlink_metadata(&directory) {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(_) => return Err(storage(StoreError::Unavailable)),
        };
        let artifacts = if exists {
            Some(
                NativeArtifacts::open(&directory, action.source().byte_limit, control)
                    .map_err(storage)?,
            )
        } else {
            None
        };
        let receipt = match &artifacts {
            Some(artifacts) => artifacts
                .receipt(&action.source().artifact_name)
                .map_err(storage)?,
            None => None,
        };
        data.found = Some(receipt.is_some());
        data.integrity_verified = receipt.is_some();
        data.artifact = receipt;
        if let ArtifactAction::Export(args) = &action {
            let receipt = data.artifact.as_ref().ok_or_else(|| {
                CraftError::new(
                    ErrorCode::VersionConflict,
                    "The expected local artifact was not found; no export was attempted.",
                )
            })?;
            if receipt.digest != args.expected_digest {
                return Err(CraftError::new(
                    ErrorCode::VersionConflict,
                    "The verified local artifact differs from the observed SHA-256; no export was attempted.",
                ));
            }
            reserve(&action, &data, options)?;
            control.check()?;
            let output = OutputRoot::open_excluding(Path::new(&args.output_root), protected)
                .map_err(|_| {
                    CraftError::field("/output_root", "expected_existing_guarded_output_root")
                })?;
            let artifacts = artifacts
                .as_ref()
                .ok_or_else(|| storage(StoreError::Corrupt))?;
            match artifacts.export_file_checked(
                &args.source.artifact_name,
                &output,
                Path::new(&args.output),
                if args.overwrite {
                    OverwritePolicy::Replace
                } else {
                    OverwritePolicy::CreateNew
                },
                &args.expected_digest,
            ) {
                Ok(receipt) => {
                    data.exported = Some(receipt);
                    data.export_write_state = RequestState::Confirmed;
                }
                Err(error) => {
                    data.export_write_state = if error == StoreError::OutcomeUnknown {
                        RequestState::SentUnknown
                    } else {
                        RequestState::Failed
                    };
                    return Err(storage(error));
                }
            }
        } else {
            reserve(&action, &data, options)?;
        }
        control.check()
    })();
    finish(&action, data, result, options)
}
fn finish(
    action: &ArtifactAction,
    data: ArtifactData,
    result: Result<(), CraftError>,
    options: &InvocationOptions<'_>,
) -> CommandResult {
    let envelope = match result {
        Ok(()) => Envelope::success(action.command(), options.request_id.into(), json!(data)),
        Err(mut error) => {
            error.retryable = false;
            error.details.write_state = if matches!(action, ArtifactAction::Export(_)) {
                data.export_write_state
            } else {
                RequestState::NotSent
            };
            Envelope::failure_with_data(
                action.command(),
                options.request_id.into(),
                error,
                json!(data),
            )
        }
    };
    CommandResult {
        envelope,
        output_policy: if matches!(action, ArtifactAction::Export(_)) {
            OutputPolicy::PreserveEvidence
        } else {
            OutputPolicy::Bounded
        },
    }
}
pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    for id in [GET, EXPORT] {
        let export = id == EXPORT;
        let mut example = json!({"artifact_name":"design.bin"});
        if export {
            example["expected_digest"] = json!(format!("sha256:{}", "0".repeat(64)));
            example["output_root"] = json!(if cfg!(windows) {
                "D:/exports"
            } else {
                "/tmp/exports"
            });
            example["output"] = json!("design.bin");
        }
        let sample = ArtifactData {
            found: Some(false),
            ..ArtifactData::empty(&GetArgs {
                artifact_name: "design.bin".into(),
                collection: Collection::Downloads,
                byte_limit: 16_777_216,
            })
        };
        let response = if export {
            Envelope::failure_with_data(
                id,
                "example".into(),
                CraftError::new(
                    ErrorCode::VersionConflict,
                    "The expected local artifact was not found; no export was attempted.",
                ),
                json!(sample),
            )
        } else {
            Envelope::success(id, "example".into(), json!(sample))
        };
        let descriptor=OperationDescriptor {
            id:id.into(), provider:"craft".into(), contract_version:SCHEMA_VERSION.into(),
            api_revision:"local-artifacts-v1".into(),
            description:if export {"Immediately export a digest-bound local artifact to explicit guarded output without cloud access or automatic retries."} else {"Verify an exact local artifact by streamed size and digest; return its receipt without binary bytes or live authority."}.into(),
            effect:if export{EffectClass::Write}else{EffectClass::Read},
            input_schema:if export{schema_for::<ExportArgs>()?}else{schema_for::<GetArgs>()?},
            response_schema:response_schema::<ArtifactData>()?,example,response_example:json!(response),
            projection:None,scope_alternatives:vec![],permissions:vec![],
            tenant_conditions:vec!["Local state only; profile selection and projection are unavailable. Export requires an observed digest; confirmed local receipts grant no cloud replay authority.".into()],
            routes:vec![],retry_class:if export{RetryClass::ReconcileBeforeReplay}else{RetryClass::LocalRead},
            reconciliation:if export{ReconciliationClass::KnownResource}else{ReconciliationClass::NotApplicable},
        };
        if export {
            registry.register_typed::<ExportArgs>(descriptor)?;
        } else {
            registry.register_typed::<GetArgs>(descriptor)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
