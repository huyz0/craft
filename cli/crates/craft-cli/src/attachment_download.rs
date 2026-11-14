//! Service-authorized binary download retains independent artifact and output evidence.
pub mod command;
use crate::attachment_reads::ReadEnvironment;
use crate::dispatch::CommandResult;
use crate::issue_reads::IssueReadPolicy;
use crate::output::OutputPolicy;
use craft_core::contracts::{CanonicalRequest, CredentialCacheUpdate, Envelope, RequestState};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::PreparedOperation;
use craft_core::registry::Registry;
use craft_core::storage::{ArtifactReceipt, ArtifactSpec, StoreError};
use craft_platform::{OutputRoot, OverwritePolicy};
use craft_provider_jira::attachment_download::{self as native, DownloadData};
use craft_runtime::artifacts::NativeArtifacts;
use craft_runtime::auth::VerifiedServiceAccess;
use craft_runtime::transport::DownloadRequest;
use serde_json::json;
use std::path::Path;

pub const ARTIFACT_DIRECTORY: &str = "downloads";
pub const MAX_BYTES: u64 = 1_073_741_824;
pub fn storage(error: StoreError) -> CraftError {
    let mut result = CraftError::new(
        match error {
            StoreError::Invalid => ErrorCode::InputInvalid,
            StoreError::Conflict => ErrorCode::VersionConflict,
            StoreError::TooLarge => ErrorCode::ItemTooLarge,
            StoreError::Cancelled => ErrorCode::Cancelled,
            StoreError::DeadlineExceeded => ErrorCode::DeadlineExceeded,
            StoreError::OutcomeUnknown => ErrorCode::WriteOutcomeUnknown,
            _ => ErrorCode::Internal,
        },
        "Local artifact or output operation failed; retain its independent publication state.",
    );
    result.next_action="Inspect the named local artifact and explicit output before retrying. Never infer absence from an uncertain publication.".into();
    result
}
fn denied() -> CraftError {
    CraftError::new(
        ErrorCode::PolicyDenied,
        "Attachment download differs from the current trusted site, actor, grant or allowance.",
    )
}
pub struct PreparedAttachmentDownload {
    registry: Registry,
    operation: PreparedOperation,
    policy: IssueReadPolicy,
    args: command::AttachmentDownloadArgs,
    request_id: String,
}
impl PreparedAttachmentDownload {
    pub fn prepare(
        request: CanonicalRequest,
        request_id: &str,
        policy: IssueReadPolicy,
        args: command::AttachmentDownloadArgs,
    ) -> Result<Self, CraftError> {
        args.validate_local()?;
        craft_core::contracts::validate_identity("/request_id", request_id)?;
        craft_core::contracts::validate_identity("/actor", &policy.expected_actor)?;
        policy.grant.validate()?;
        if request_id.len() > 80
            || request.target != policy.target
            || matches!(policy.expected_actor.as_str(), "anonymous" | "unknown")
            || request.input != json!({"id":args.id})
        {
            return Err(denied());
        }
        if args
            .cloud_id
            .as_ref()
            .is_some_and(|id| id != &request.target.tenant)
        {
            return Err(denied());
        }
        let mut registry = Registry::default();
        native::register(&mut registry)?;
        let operation = registry.prepare(request)?;
        let host = Self {
            registry,
            operation,
            policy,
            args,
            request_id: request_id.into(),
        };
        host.preflight()?;
        Ok(host)
    }
    fn data(&self) -> DownloadData {
        DownloadData {
            target: self.policy.target.clone(),
            attachment_id: self.args.id.clone(),
            artifact_name: self.args.artifact_name.clone(),
            artifact: None,
            artifact_write_state: RequestState::NotSent,
            exported: None,
            export_write_state: RequestState::NotSent,
            received_bytes: 0,
            response_media_type: None,
            credential_cache_update: None,
        }
    }
    fn preflight(&self) -> Result<(), CraftError> {
        let mut data = self.data();
        let mut source = self.policy.target.clone();
        source.resource_type = "attachment".into();
        source.id = self.args.id.clone();
        let receipt = ArtifactReceipt {
            name: self.args.artifact_name.clone(),
            media_type: "application/octet-stream".into(),
            digest: format!("sha256:{}", "f".repeat(64)),
            bytes: MAX_BYTES,
            source: Some(source),
            local_path: Some("\\".repeat(4096)),
        };
        data.artifact = Some(receipt.clone());
        data.exported = Some(ArtifactReceipt {
            local_path: Some(format!("{}/{}", self.args.output_root, self.args.output)),
            ..receipt
        });
        data.response_media_type = Some("x".repeat(256));
        data.credential_cache_update = Some(CredentialCacheUpdate {
            grant: self.policy.grant.clone(),
            write_state: RequestState::SentUnknown,
            cleanup_pending: Some(false),
        });
        let mut error = CraftError::new(ErrorCode::Internal, "x".repeat(500));
        error.next_action = "x".repeat(1000);
        let envelope = Envelope::failure_with_data(
            native::DOWNLOAD,
            self.request_id.clone(),
            error,
            json!(data),
        );
        if serde_json::to_vec(&envelope)
            .map_err(|_| storage(StoreError::Invalid))?
            .len()
            + 1
            > self.operation.request().selection.max_bytes as usize
        {
            return Err(CraftError::new(
                ErrorCode::OutputBudgetTooSmall,
                "Increase --max-bytes before authentication to retain complete artifact, export and credential receipts.",
            ));
        }
        Ok(())
    }
    fn check(
        &self,
        access: &VerifiedServiceAccess,
        control: &InvocationControl,
        policy: &dyn Fn() -> Result<(), CraftError>,
    ) -> Result<(), CraftError> {
        control.check()?;
        policy()?;
        if access.tenant() != &self.policy.target.tenant_ref()
            || access.grant() != &self.policy.grant
            || access.identity().actor() != self.policy.expected_actor
        {
            return Err(denied());
        }
        access.check_operation(&self.registry, &self.operation)?;
        control.check()?;
        policy()
    }
    pub fn execute(
        &self,
        environment: ReadEnvironment<'_, '_>,
        policy: impl Fn() -> Result<(), CraftError>,
    ) -> CommandResult {
        let ReadEnvironment {
            store,
            client,
            locked,
            tls,
            control,
        } = environment;
        let mut data = self.data();
        let result = (|| {
            control.check()?;
            policy()?;
            if locked.grant() != self.policy.grant {
                return Err(denied());
            }
            // Bound the actual private receipt path against the pre-authentication reserve.
            if store
                .path()
                .join(ARTIFACT_DIRECTORY)
                .to_str()
                .is_none_or(|path| path.len() > 3900)
            {
                return Err(CraftError::field(
                    "/state_root",
                    "download_receipt_path_too_long",
                ));
            }
            let artifacts = NativeArtifacts::open(
                &store.path().join(ARTIFACT_DIRECTORY),
                self.args.byte_limit,
                &control,
            )
            .map_err(storage)?;
            let protected = craft_platform::PrivateRoot::open(store.path())
                .map_err(|_| storage(StoreError::Unavailable))?;
            let output = OutputRoot::open_excluding(Path::new(&self.args.output_root), protected)
                .map_err(|_| {
                CraftError::field("/output_root", "expected_existing_guarded_output_root")
            })?;
            let client = client.for_invocation(control.clone())?;
            let access = match client.open_service_access(locked, &self.policy.target.tenant_ref())
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
            let check = || self.check(&access, &control, &policy);
            check()?;
            let mut source = self.policy.target.clone();
            source.resource_type = "attachment".into();
            source.id = self.args.id.clone();
            let outcome = access
                .prepare_download(
                    &self.registry,
                    &self.operation,
                    tls,
                    control.clone(),
                    DownloadRequest {
                        call: native::call(&self.operation)?,
                        artifact: ArtifactSpec {
                            name: self.args.artifact_name.clone(),
                            media_type: "application/octet-stream".into(),
                            max_bytes: self.args.byte_limit,
                            source: Some(source),
                        },
                        expected_digest: self.args.expected_digest.clone(),
                    },
                )?
                .receive(&artifacts, check);
            data.artifact = outcome.artifact;
            data.artifact_write_state = outcome.artifact_write_state;
            data.received_bytes = outcome.received_bytes;
            data.response_media_type = outcome.response_media_type;
            if let Some(error) = outcome.error {
                return Err(error);
            }
            check()?;
            let observed_digest = &data
                .artifact
                .as_ref()
                .ok_or_else(|| storage(StoreError::Corrupt))?
                .digest;
            match artifacts.export_file_checked(
                &self.args.artifact_name,
                &output,
                Path::new(&self.args.output),
                if self.args.overwrite {
                    OverwritePolicy::Replace
                } else {
                    OverwritePolicy::CreateNew
                },
                observed_digest,
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
            // A refusal after confirmed local publication retains both receipts.
            check()
        })();
        let envelope = match result {
            Ok(()) => Envelope::success(native::DOWNLOAD, self.request_id.clone(), json!(data)),
            Err(mut error) => {
                error.retryable = false;
                error.details.write_state = RequestState::NotSent;
                Envelope::failure_with_data(
                    native::DOWNLOAD,
                    self.request_id.clone(),
                    error,
                    json!(data),
                )
            }
        };
        CommandResult {
            envelope,
            output_policy: OutputPolicy::PreserveEvidence,
        }
    }
}
