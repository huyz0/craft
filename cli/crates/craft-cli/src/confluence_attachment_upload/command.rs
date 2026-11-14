//! Strict file-or-snapshot selection precedes profiles, credentials and journaled page upload.
use super::*;
use craft_core::profiles::validate_profile_name;
use craft_core::registry::{response_schema, schema_for};
use craft_runtime::control::SystemClock;
use craft_runtime::profiles::resolve_state_root;
use craft_runtime::secrets::NativeCredentials;
use std::sync::Arc;
fn media() -> String {
    "application/octet-stream".into()
}
fn byte_limit() -> u64 {
    16_777_216
}
fn revision(text: &str) -> Result<native::AttachmentRevision, String> {
    let value = craft_core::input::parse_object(text.as_bytes())
        .map_err(|_| "expected_strict_json_object")?;
    serde_json::from_value(value).map_err(|_| "expected_version_and_file_id".into())
}
#[derive(Clone, clap::Args, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UploadArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 512))]
    pub operation_id: String,
    /// Optional when trusted profile context supplies the space.
    #[arg(long)]
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub space_id: Option<String>,
    #[arg(long, default_value = "")]
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub page_id: String,
    /// Replacement only: the exact attachment whose binary is replaced.
    #[arg(long)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(regex(pattern = "^att[1-9][0-9]*$"))]
    pub attachment_id: Option<String>,
    /// Replacement only: the reviewed version and file ID from `confluence attachment get`.
    #[arg(long, value_parser = revision)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<native::AttachmentRevision>,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 128), regex(pattern = "^[A-Za-z0-9._-]+$"))]
    pub artifact_name: String,
    #[arg(long)]
    #[schemars(length(min = 1, max = 4096))]
    pub file_root: Option<String>,
    #[arg(long)]
    #[schemars(length(min = 1, max = 4096))]
    pub file: Option<String>,
    #[arg(long)]
    #[serde(default)]
    pub from_artifact: bool,
    #[arg(long)]
    #[schemars(regex(pattern = "^sha256:[0-9a-f]{64}$"))]
    pub expected_digest: Option<String>,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 1024))]
    pub filename: String,
    #[arg(long, default_value = "application/octet-stream")]
    #[serde(default = "media")]
    #[schemars(length(min = 3, max = 256))]
    pub media_type: String,
    #[arg(long, default_value_t = 16_777_216)]
    #[serde(default = "byte_limit")]
    #[schemars(range(min = 1, max = 1_073_741_824))]
    pub byte_limit: u64,
    #[arg(long)]
    #[schemars(length(min = 36, max = 36))]
    pub cloud_id: Option<String>,
    #[arg(long)]
    #[serde(default)]
    pub apply: bool,
}
impl UploadArgs {
    /// Replacement is selected only by an explicit attachment ID; a filename never selects it.
    pub fn command(&self) -> &'static str {
        if self.attachment_id.is_some() {
            native::REPLACE
        } else {
            native::UPLOAD
        }
    }
    pub fn route(&self) -> &'static str {
        if self.attachment_id.is_some() {
            native::REPLACE_ROUTE
        } else {
            native::UPLOAD_ROUTE
        }
    }
    pub fn operation(&self) -> Op {
        if self.attachment_id.is_some() {
            Op::AttachmentReplace
        } else {
            Op::AttachmentUpload
        }
    }
    pub(super) fn selection_digest(&self) -> Result<String, CraftError> {
        craft_core::input::canonical_digest(&if self.from_artifact {
            json!({"mode":"snapshot","artifact_name":self.artifact_name,"digest":self.expected_digest})
        } else {
            json!({"mode":"workspace","root":self.file_root,"file":self.file})
        })
    }
    pub fn validate(
        &self,
        fields: &[String],
        options: &InvocationOptions<'_>,
    ) -> Result<(), CraftError> {
        validate_profile_name(
            options
                .profile
                .ok_or_else(|| CraftError::field("/profile", "explicit_upload_profile_required"))?,
        )?;
        craft_core::contracts::validate_identity("/operation_id", &self.operation_id)?;
        craft_core::contracts::validate_identity("/request_id", options.request_id)?;
        if !self.apply {
            return Err(CraftError::new(
                ErrorCode::ApplyRequired,
                "Attachment upload requires explicit apply.",
            ));
        }
        if !fields.is_empty() || options.request_id.len() > 80 {
            return Err(CraftError::field(
                "/output",
                "complete_upload_receipts_required",
            ));
        }
        if !(MINIMUM_OUTPUT..=16_777_216).contains(&options.max_bytes) {
            return Err(CraftError::new(
                ErrorCode::OutputBudgetTooSmall,
                "Reserve --max-bytes 8388608 for complete upload, snapshot and recovery evidence; normal output is compact.",
            ));
        }
        if !(1..=1_073_741_824).contains(&self.byte_limit) {
            return Err(CraftError::field(
                "/byte_limit",
                "expected_bounded_snapshot_limit",
            ));
        }
        if self.from_artifact {
            if self.expected_digest.is_none() || self.file_root.is_some() || self.file.is_some() {
                return Err(CraftError::field(
                    "/source",
                    "artifact_source_requires_digest_without_workspace_paths",
                ));
            }
        } else {
            InputRoot::validate_root(Path::new(self.file_root.as_deref().ok_or_else(|| {
                CraftError::field("/file_root", "workspace_source_requires_root")
            })?))
            .map_err(|_| CraftError::field("/file_root", "expected_safe_absolute_input_root"))?;
            InputRoot::validate_path(Path::new(self.file.as_deref().ok_or_else(|| {
                CraftError::field("/file", "workspace_source_requires_relative_file")
            })?))
            .map_err(|_| CraftError::field("/file", "expected_safe_relative_input_file"))?;
        }
        let space = self.space_id.clone().unwrap_or_else(|| "1".into());
        let target = ResourceRef {
            provider: "confluence".into(),
            deployment: "cloud".into(),
            tenant: self
                .cloud_id
                .clone()
                .unwrap_or("00000000-1111-2222-3333-444444444444".into()),
            resource_type: "space".into(),
            id: space.clone(),
        };
        let mut registry = Registry::default();
        native::register(&mut registry)?;
        registry.prepare(CanonicalRequest {
            command: self.command().into(),
            target,
            input: json!(UploadInput {
                space_id: space,
                page_id: self.page_id.clone(),
                attachment_id: self.attachment_id.clone(),
                expected: self.expected.clone(),
                artifact_name: self.artifact_name.clone(),
                digest: self
                    .expected_digest
                    .clone()
                    .unwrap_or_else(|| format!("sha256:{}", "0".repeat(64))),
                source_selection_digest: self.selection_digest()?,
                size_bytes: 0,
                filename: self.filename.clone(),
                media_type: self.media_type.clone()
            }),
            selection: Selection::default(),
            preconditions: None,
        })?;
        Ok(())
    }
}
fn source_schema(schema: &mut Value) {
    schema["allOf"] = json!([{"if":{"required":["from_artifact"],"properties":{"from_artifact":{"const":true}}},"then":{"required":["expected_digest"],"properties":{"expected_digest":{"type":"string","pattern":"^sha256:[0-9a-f]{64}$"},"file_root":{"type":"null"},"file":{"type":"null"}}},"else":{"required":["file_root","file"],"properties":{"file_root":{"type":"string","minLength":1},"file":{"type":"string","minLength":1}}}}]);
}
fn example(replace: bool) -> UploadArgs {
    UploadArgs {
        operation_id: "upload-design".into(),
        space_id: Some("100".into()),
        page_id: "101".into(),
        attachment_id: replace.then(|| "att301".into()),
        expected: replace.then(|| native::AttachmentRevision {
            version: 1,
            file_id: "0b6f6a1c-file".into(),
        }),
        artifact_name: "design-upload.bin".into(),
        file_root: None,
        file: None,
        from_artifact: false,
        expected_digest: None,
        filename: "design.bin".into(),
        media_type: media(),
        byte_limit: byte_limit(),
        cloud_id: None,
        apply: true,
    }
}
pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    let mut native_registry = Registry::default();
    native::register(&mut native_registry)?;
    for command in [native::UPLOAD, native::REPLACE] {
        let replace = command == native::REPLACE;
        let mut descriptor = native_registry.descriptor(command)?.clone();
        descriptor.input_schema = schema_for::<UploadArgs>()?;
        source_schema(&mut descriptor.input_schema);
        let required = descriptor.input_schema["required"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut required: Vec<Value> = required;
        if replace {
            required.extend([json!("attachment_id"), json!("expected")]);
        } else {
            descriptor.input_schema["properties"]["attachment_id"] = json!({"type":"null"});
            descriptor.input_schema["properties"]["expected"] = json!({"type":"null"});
        }
        descriptor.input_schema["required"] = json!(required);
        descriptor.response_schema = response_schema::<UploadData>()?;
        descriptor.scope_alternatives = vec![
            vec![
                "read:page:confluence".into(),
                "read:attachment:confluence".into(),
                "write:confluence-file".into(),
            ],
            vec![
                "read:page:confluence".into(),
                "read:attachment:confluence".into(),
                "read:content-details:confluence".into(),
                "write:attachment:confluence".into(),
            ],
        ];
        let mut sample = json!({"operation_id":"upload-design","space_id":"100","page_id":"101","artifact_name":"design-upload.bin","file_root":if cfg!(windows){"D:/workspace"}else{"/tmp/workspace"},"file":"design.bin","filename":"design.bin","apply":true});
        if replace {
            sample["attachment_id"] = json!("att301");
            sample["expected"] = json!({"version":1,"file_id":"0b6f6a1c-file"});
        }
        descriptor.example = sample;
        descriptor.response_example = json!(Envelope::success(
            command,
            "example".into(),
            json!(empty(
                &example(replace),
                ResourceRef {
                    provider: "confluence".into(),
                    deployment: "cloud".into(),
                    tenant: "00000000-1111-2222-3333-444444444444".into(),
                    resource_type: "space".into(),
                    id: "100".into()
                }
            ))
        ));
        registry.register_typed_handler::<UploadArgs>(
            descriptor,
            Arc::new(crate::public_commands::VerifiedHostRequired),
        )?;
    }
    Ok(())
}
pub fn execute(
    args: UploadArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> CommandResult {
    let command = args.command();
    let result = (|| {
        args.validate(fields, options)?;
        let root = resolve_state_root(options.state_root, |key| std::env::var_os(key))?;
        let store = ProfileStore::open(&root, control)?;
        let prepared = PreparedUpload::prepare(args, fields, options, &store, control)?;
        let credentials = NativeCredentials::open(&store)?;
        let locked = credentials.lock(prepared.grant(), control)?;
        let client = ServiceOAuthClient::new(
            TlsPolicy::Platform,
            control.clone(),
            Arc::new(SystemClock::default()),
        )?;
        let journal = NativeJournal::open(&store, control).map_err(storage)?;
        Ok(prepared.execute(
            UploadEnvironment {
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
    result.unwrap_or_else(|error| CommandResult {
        envelope: Envelope::failure(command, options.request_id.into(), error),
        output_policy: OutputPolicy::PreserveEvidence,
    })
}
