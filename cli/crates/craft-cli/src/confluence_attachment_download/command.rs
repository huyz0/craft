//! Strict download arguments resolve one Confluence service profile and explicit read allowance.
use super::*;
use crate::profiles::{InvocationOptions, selected_profile_context};
use craft_core::contracts::{AuthMode, GrantRef, ResourceRef, Selection};
use craft_core::profiles::validate_profile_name;
use craft_core::registry::schema_for;
use craft_runtime::auth::ServiceOAuthClient;
use craft_runtime::confluence_read_policy::{
    ConfluenceReadOperation as ReadOperation, ConfluenceReadPolicyRecord,
};
use craft_runtime::control::SystemClock;
use craft_runtime::profiles::{ProfileStore, resolve_state_root};
use craft_runtime::secrets::NativeCredentials;
use craft_runtime::transport::TlsPolicy;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
fn byte_limit() -> u64 {
    16_777_216
}
#[derive(clap::Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttachmentDownloadArgs {
    #[arg(long, default_value = "")]
    #[schemars(regex(pattern = "^att[1-9][0-9]*$"))]
    pub id: String,
    #[arg(long, default_value = "")]
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub page_id: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 128), regex(pattern = "^[A-Za-z0-9._-]+$"))]
    pub artifact_name: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 4096))]
    pub output_root: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 4096))]
    pub output: String,
    #[arg(long, default_value_t = 16_777_216)]
    #[serde(default = "byte_limit")]
    #[schemars(range(min = 1, max = 1_073_741_824))]
    pub byte_limit: u64,
    #[arg(long)]
    #[schemars(regex(pattern = "^sha256:[0-9a-f]{64}$"))]
    pub expected_digest: Option<String>,
    #[arg(long)]
    #[serde(default)]
    pub overwrite: bool,
    #[arg(long)]
    #[schemars(length(min = 36, max = 36))]
    pub cloud_id: Option<String>,
}
fn site(cloud: &str) -> ResourceRef {
    ResourceRef {
        provider: "confluence".into(),
        deployment: "cloud".into(),
        tenant: cloud.into(),
        resource_type: "site".into(),
        id: cloud.into(),
    }
}
impl AttachmentDownloadArgs {
    pub fn validate_local(&self) -> Result<(), CraftError> {
        craft_core::storage::validate_artifact_name(&self.artifact_name).map_err(storage)?;
        OutputRoot::validate_root(Path::new(&self.output_root))
            .map_err(|_| CraftError::field("/output_root", "expected_safe_absolute_output_root"))?;
        OutputRoot::validate_destination(Path::new(&self.output))
            .map_err(|_| CraftError::field("/output", "expected_safe_relative_output_path"))?;
        if !(1..=MAX_BYTES).contains(&self.byte_limit)
            || !craft_provider_confluence::attachments::valid_attachment_id(&self.id)
            || self
                .page_id
                .parse::<u64>()
                .map_or(true, |n| n == 0 || n.to_string() != self.page_id)
            || self.output_root.len() > 4096
            || !Path::new(&self.output_root).is_absolute()
            || self.output_root.chars().any(char::is_control)
            || self.expected_digest.as_ref().is_some_and(|v| {
                !v.strip_prefix("sha256:").is_some_and(|v| {
                    v.len() == 64
                        && v.bytes()
                            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
                })
            })
        {
            return Err(CraftError::field(
                "/input",
                "expected_bounded_attachment_id_absolute_output_root_and_optional_lowercase_sha256",
            ));
        }
        if let Some(id) = &self.cloud_id {
            site(id).validate()?;
            craft_runtime::auth::validate_service_target(&site(id).tenant_ref())?;
        }
        Ok(())
    }
    fn host(
        &self,
        fields: &[String],
        options: &InvocationOptions<'_>,
        policy: DownloadPolicy,
    ) -> Result<PreparedAttachmentDownload, CraftError> {
        PreparedAttachmentDownload::prepare(
            CanonicalRequest {
                command: native::DOWNLOAD.into(),
                target: policy.target.clone(),
                input: json!({"id":self.id,"page_id":self.page_id}),
                selection: Selection {
                    fields: fields.to_vec(),
                    cursor: None,
                    limit: 20,
                    max_bytes: u32::try_from(options.max_bytes).map_err(|_| {
                        CraftError::field("/max_bytes", "download_output_budget_out_of_range")
                    })?,
                },
                preconditions: None,
            },
            options.request_id,
            policy,
            self.clone(),
        )
    }
    pub fn validate(
        &self,
        fields: &[String],
        options: &InvocationOptions<'_>,
    ) -> Result<(), CraftError> {
        if let Some(profile) = options.profile {
            validate_profile_name(profile)?;
        }
        self.host(
            fields,
            options,
            DownloadPolicy {
                target: site(
                    self.cloud_id
                        .as_deref()
                        .unwrap_or("00000000-1111-2222-3333-444444444444"),
                ),
                grant: GrantRef {
                    id: format!("gr_{}", "0".repeat(32)),
                },
                expected_actor: "service-actor".into(),
            },
        )?;
        Ok(())
    }
}
pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    let mut native_registry = Registry::default();
    native::register(&mut native_registry)?;
    let mut descriptor = native_registry.descriptor(native::DOWNLOAD)?.clone();
    descriptor.input_schema = schema_for::<AttachmentDownloadArgs>()?;
    descriptor.example = json!({"id":"att301","page_id":"101","artifact_name":"attachment.bin","output_root":if cfg!(windows){"D:/exports"}else{"/tmp/exports"},"output":"attachment.bin"});
    registry.register_typed_handler::<AttachmentDownloadArgs>(
        descriptor,
        Arc::new(crate::public_commands::VerifiedHostRequired),
    )
}
pub struct PreparedDownload {
    host: PreparedAttachmentDownload,
    policy: ConfluenceReadPolicyRecord,
}
impl PreparedDownload {
    pub fn prepare(
        args: AttachmentDownloadArgs,
        fields: &[String],
        options: &InvocationOptions<'_>,
        store: &ProfileStore,
        control: &InvocationControl,
    ) -> Result<Self, CraftError> {
        args.validate(fields, options)?;
        let (name, _, hints) = selected_profile_context(store, None, options, control)?;
        let profile = store.get(&name, control)?;
        if profile.provider != "confluence"
            || profile.deployment != "cloud"
            || profile.auth_mode != AuthMode::Service
        {
            return Err(denied());
        }
        let explicit = args.cloud_id.as_deref().map(site);
        let target = if explicit.is_some()
            || hints.targets.contains_key("site")
            || profile.defaults.contains_key("site")
        {
            profile.resolve_target("site", explicit.as_ref(), &hints)?
        } else {
            site(&profile.tenant_ref()?.tenant)
        };
        let policy = store
            .confluence_read_policy(&name, control)?
            .ok_or_else(denied)?;
        if policy.target != target {
            return Err(denied());
        }
        store.verify_confluence_read_policy(&policy, ReadOperation::AttachmentDownload, control)?;
        let host = args.host(
            fields,
            options,
            DownloadPolicy {
                target,
                grant: policy.grant.clone(),
                expected_actor: policy.expected_actor.clone(),
            },
        )?;
        Ok(Self { host, policy })
    }
    pub fn grant(&self) -> &GrantRef {
        &self.policy.grant
    }
    #[cfg(feature = "synthetic-acceptance")]
    pub fn with_media_origin(mut self, origin: MediaOrigin) -> Self {
        self.host = self.host.with_media_origin(origin);
        self
    }
    pub fn execute(&self, environment: ReadEnvironment<'_, '_>) -> CommandResult {
        let store = environment.store;
        let control = environment.control.clone();
        self.host.execute(environment, || {
            store.verify_confluence_read_policy(
                &self.policy,
                ReadOperation::AttachmentDownload,
                &control,
            )
        })
    }
}
pub fn execute(
    args: AttachmentDownloadArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> CommandResult {
    let result = (|| {
        args.validate(fields, options)?;
        let root = resolve_state_root(options.state_root, |key| std::env::var_os(key))?;
        let store = ProfileStore::open(&root, control)?;
        let prepared = PreparedDownload::prepare(args, fields, options, &store, control)?;
        let credentials = NativeCredentials::open(&store)?;
        let locked = credentials.lock(prepared.grant(), control)?;
        let client = ServiceOAuthClient::new(
            TlsPolicy::Platform,
            control.clone(),
            Arc::new(SystemClock::default()),
        )?;
        Ok(prepared.execute(ReadEnvironment {
            store: &store,
            client: &client,
            locked: &locked,
            tls: TlsPolicy::Platform,
            control: control.clone(),
        }))
    })();
    result.unwrap_or_else(|error| CommandResult {
        envelope: Envelope::failure(native::DOWNLOAD, options.request_id.into(), error),
        output_policy: OutputPolicy::PreserveEvidence,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input() -> AttachmentDownloadArgs {
        AttachmentDownloadArgs {
            id: "att101".into(),
            page_id: "100".into(),
            artifact_name: "valid.bin".into(),
            output_root: if cfg!(windows) {
                "D:/exports".into()
            } else {
                "/tmp/exports".into()
            },
            output: "file.bin".into(),
            byte_limit: 100,
            expected_digest: None,
            overwrite: false,
            cloud_id: None,
        }
    }
    #[test]
    fn invalid_download_contracts_fail_without_state_or_authentication() -> Result<(), CraftError> {
        let options = InvocationOptions {
            state_root: None,
            profile: None,
            request_id: "validate-download",
            max_bytes: 16384,
        };
        let good = input();
        good.validate(&[], &options)?;
        for mode in ["id", "name", "relative", "root", "limit", "digest", "cloud"] {
            let mut bad = good.clone();
            match mode {
                "id" => bad.id = "101".into(),
                "name" => bad.artifact_name = "..".into(),
                "relative" => bad.output = "../file".into(),
                "root" => bad.output_root = "relative".into(),
                "limit" => bad.byte_limit = 0,
                "digest" => bad.expected_digest = Some(format!("sha256:{}", "A".repeat(64))),
                _ => bad.cloud_id = Some("invalid".into()),
            };
            assert!(bad.validate(&[], &options).is_err(), "{mode}");
        }
        assert!(good.validate(&["artifact".into()], &options).is_err());
        assert!(
            good.validate(
                &[],
                &InvocationOptions {
                    max_bytes: 512,
                    ..options
                }
            )
            .is_err()
        );
        let mut explicit = good;
        explicit.cloud_id = Some("11111111-2222-3333-4444-555555555555".into());
        explicit.validate(&[], &options)?;
        Ok(())
    }
}
