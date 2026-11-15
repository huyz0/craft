//! `scan start`, `scan resume` and `scan export`.
use std::path::Path;
use std::sync::Arc;

use clap::{Args, Subcommand};
use craft_core::contracts::{
    CredentialCacheUpdate, EffectClass, Envelope, RequestState, SCHEMA_VERSION,
};
use craft_core::control::{CancellationToken, InvocationControl};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::RetryAfter;
use craft_core::profiles::validate_profile_name;
use craft_core::registry::{
    OperationDescriptor, ReconciliationClass, Registry, RetryClass, response_schema, schema_for,
};
use craft_core::scan::{ScanCaps, ScanLimit, ScanOutcome, ScanTotals, SegmentReceipt};
use craft_core::storage::{ArtifactReceipt, Clock, StoreError};
use craft_platform::{OutputRoot, OverwritePolicy, PrivateRoot};
use craft_runtime::artifacts::NativeArtifacts;
use craft_runtime::control::SystemClock;
use craft_runtime::profiles::{ProfileStore, resolve_state_root};
use craft_runtime::scan::{self, Advanced, ScanCheckpoint, ScanInputs};
use craft_runtime::secrets::NativeCredentials;
use craft_runtime::transport::TlsPolicy;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::dispatch::CommandResult;
use crate::output::OutputPolicy;
use crate::profiles::InvocationOptions;

pub const START: &str = "scan.start";
pub const RESUME: &str = "scan.resume";
pub const EXPORT: &str = "scan.export";
pub const ARTIFACT_DIRECTORY: &str = "scans";
/// List commands whose hosts lend their page source to the scan driver.
pub const SCANNABLE: [&str; 3] = [
    "jira.issue.search",
    "jira.project.list",
    "confluence.page.list",
];
/// The list host still preflights its own envelope; scans never print it, so allow the maximum.
const LIST_ENVELOPE_BYTES: usize = 16_777_216;
const MIN_OUTPUT_BYTES: usize = 4_096;
/// A cancelled scan may still publish what it fetched, inside this fresh bound.
const EVIDENCE_MS: u64 = 5_000;

fn default_items() -> u32 {
    1_000
}
fn default_pages() -> u32 {
    50
}
fn default_bytes() -> u64 {
    8_388_608
}

#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScanStartArgs {
    #[arg(
        long,
        default_value = "",
        help = "Scannable list command, for example jira.issue.search"
    )]
    #[schemars(length(min = 1, max = 128))]
    pub command: String,
    #[arg(long, value_parser = arguments, default_value = "{}", help = "That command's argument object as JSON, without limit or cursor")]
    #[serde(default = "empty_object")]
    pub arguments: Value,
    #[arg(
        long = "record-fields",
        value_delimiter = ',',
        help = "Fields to keep in each stored record, as the list command's --fields"
    )]
    #[serde(default)]
    #[schemars(length(max = 64))]
    pub record_fields: Vec<String>,
    #[command(flatten)]
    #[serde(flatten)]
    pub caps: CapArgs,
}

fn empty_object() -> Value {
    json!({})
}

fn arguments(text: &str) -> Result<Value, String> {
    craft_core::input::parse_object(text.as_bytes()).map_err(|_| "expected a JSON object".into())
}

#[derive(Args, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapArgs {
    #[arg(
        long,
        default_value_t = 1_000,
        help = "Total records the scan may store"
    )]
    #[serde(default = "default_items")]
    #[schemars(range(min = 1, max = 100_000))]
    pub max_items: u32,
    #[arg(
        long,
        default_value_t = 50,
        help = "Total native pages the scan may fetch"
    )]
    #[serde(default = "default_pages")]
    #[schemars(range(min = 1, max = 10_000))]
    pub max_pages: u32,
    #[arg(long, default_value_t = 8_388_608, help = "Total stored record bytes")]
    #[serde(default = "default_bytes")]
    #[schemars(range(min = 1_024, max = 67_108_864))]
    pub max_stored_bytes: u64,
}

impl From<CapArgs> for ScanCaps {
    fn from(caps: CapArgs) -> Self {
        Self {
            max_items: caps.max_items,
            max_pages: caps.max_pages,
            max_stored_bytes: caps.max_stored_bytes,
        }
    }
}

#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScanResumeArgs {
    #[arg(
        long,
        default_value = "",
        help = "Checkpoint artifact name returned by the last scan"
    )]
    #[schemars(length(min = 1, max = 128), regex(pattern = "^[A-Za-z0-9._-]+$"))]
    pub checkpoint: String,
    #[arg(long, default_value = "", help = "That checkpoint's SHA-256")]
    #[schemars(regex(pattern = "^sha256:[0-9a-f]{64}$"))]
    pub checkpoint_digest: String,
    #[arg(long, help = "Raise the scan's total record cap")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1, max = 100_000))]
    pub max_items: Option<u32>,
    #[arg(long, help = "Raise the scan's total page cap")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1, max = 10_000))]
    pub max_pages: Option<u32>,
    #[arg(long, help = "Raise the scan's total stored-byte cap")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1_024, max = 67_108_864))]
    pub max_stored_bytes: Option<u64>,
}

#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScanExportArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 128), regex(pattern = "^[A-Za-z0-9._-]+$"))]
    pub checkpoint: String,
    #[arg(long, default_value = "")]
    #[schemars(regex(pattern = "^sha256:[0-9a-f]{64}$"))]
    pub checkpoint_digest: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 4096))]
    pub output_root: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 4096))]
    pub output: String,
    #[arg(
        long,
        help = "Export a scan that has not completed; the receipt keeps complete: false"
    )]
    #[serde(default)]
    pub allow_partial: bool,
}

#[derive(Subcommand)]
pub enum ScanAction {
    Start(Box<ScanStartArgs>),
    Resume(Box<ScanResumeArgs>),
    Export(Box<ScanExportArgs>),
}

impl ScanAction {
    pub fn command(&self) -> &'static str {
        match self {
            Self::Start(_) => START,
            Self::Resume(_) => RESUME,
            Self::Export(_) => EXPORT,
        }
    }
    pub fn arguments(&self) -> Result<Value, CraftError> {
        match self {
            Self::Start(v) => serde_json::to_value(v),
            Self::Resume(v) => serde_json::to_value(v),
            Self::Export(v) => serde_json::to_value(v),
        }
        .map_err(|_| CraftError::field("/input", "invalid_scan_arguments"))
    }
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckpointRef {
    pub name: String,
    pub digest: String,
    pub bytes: u64,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScanData {
    pub scan_id: Option<String>,
    pub command: String,
    pub revision: Option<u32>,
    pub outcome: Option<ScanOutcome>,
    pub limit: Option<ScanLimit>,
    pub complete: bool,
    pub caps: Option<ScanCaps>,
    pub totals: Option<ScanTotals>,
    /// The resumption point to pass to `scan resume` or `scan export`. Null when nothing new
    /// was published; the earlier checkpoint then remains the resumption point.
    pub checkpoint: Option<CheckpointRef>,
    pub segment: Option<SegmentReceipt>,
    pub retry_after: Option<RetryAfter>,
    #[schemars(extend("const" = "live"))]
    pub consistency: String,
    pub credential_cache_update: Option<CredentialCacheUpdate>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportData {
    pub scan_id: Option<String>,
    pub revision: Option<u32>,
    pub complete: bool,
    pub items: Option<u32>,
    pub artifact: Option<ArtifactReceipt>,
    pub exported: Option<ArtifactReceipt>,
    pub export_write_state: RequestState,
}

fn storage(error: StoreError) -> CraftError {
    let mut failure = CraftError::new(
        match error {
            StoreError::Cancelled => ErrorCode::Cancelled,
            StoreError::DeadlineExceeded => ErrorCode::DeadlineExceeded,
            StoreError::Conflict => ErrorCode::VersionConflict,
            StoreError::Invalid => ErrorCode::InputInvalid,
            _ => ErrorCode::Internal,
        },
        "Scan artifact storage failed.",
    );
    failure.next_action =
        "Inspect the selected state root; the last published checkpoint remains valid.".into();
    failure
}

fn validate_common(fields: &[String], options: &InvocationOptions<'_>) -> Result<(), CraftError> {
    craft_core::contracts::validate_identity("/request_id", options.request_id)?;
    if !fields.is_empty() {
        return Err(CraftError::field("/fields", "projection_not_supported"));
    }
    if let Some(profile) = options.profile {
        validate_profile_name(profile)?;
    }
    if options.max_bytes < MIN_OUTPUT_BYTES {
        let mut error = CraftError::new(
            ErrorCode::OutputBudgetTooSmall,
            "Scan output needs room for its checkpoint and segment receipts.",
        );
        error.next_action = format!("Use --max-bytes {MIN_OUTPUT_BYTES} or more.");
        return Err(error);
    }
    Ok(())
}

fn digest(value: &str, path: &str) -> Result<(), CraftError> {
    if craft_core::pagination::valid_digest(value) {
        Ok(())
    } else {
        Err(CraftError::field(path, "expected_sha256_digest"))
    }
}

impl ScanStartArgs {
    pub fn validate(
        &self,
        fields: &[String],
        options: &InvocationOptions<'_>,
    ) -> Result<(), CraftError> {
        validate_common(fields, options)?;
        ScanCaps::from(self.caps).validate()?;
        if !SCANNABLE.contains(&self.command.as_str()) {
            return Err(CraftError::field("/command", "command_not_scannable"));
        }
        if !self.arguments.is_object() {
            return Err(CraftError::field("/arguments", "expected_object"));
        }
        list_args(&self.command, &self.arguments)?;
        Ok(())
    }
}

impl ScanResumeArgs {
    fn validate(
        &self,
        fields: &[String],
        options: &InvocationOptions<'_>,
    ) -> Result<(), CraftError> {
        validate_common(fields, options)?;
        craft_core::storage::validate_artifact_name(&self.checkpoint)
            .map_err(|_| CraftError::field("/checkpoint", "expected_artifact_name"))?;
        digest(&self.checkpoint_digest, "/checkpoint_digest")
    }
}

impl ScanExportArgs {
    fn validate(
        &self,
        fields: &[String],
        options: &InvocationOptions<'_>,
    ) -> Result<(), CraftError> {
        validate_common(fields, options)?;
        if options.profile.is_some() {
            return Err(CraftError::field(
                "/profile",
                "profile_selection_not_applicable",
            ));
        }
        craft_core::storage::validate_artifact_name(&self.checkpoint)
            .map_err(|_| CraftError::field("/checkpoint", "expected_artifact_name"))?;
        digest(&self.checkpoint_digest, "/checkpoint_digest")?;
        OutputRoot::validate_root(Path::new(&self.output_root))
            .map_err(|_| CraftError::field("/output_root", "expected_safe_absolute_output_root"))?;
        OutputRoot::validate_destination(Path::new(&self.output))
            .map_err(|_| CraftError::field("/output", "expected_safe_relative_output_path"))
    }
}

enum ListArgs {
    IssueSearch(crate::issue_reads::command::IssueSearchArgs),
    ProjectList(crate::project_list::command::ProjectListArgs),
    PageList(crate::confluence_reads::command::ConfluencePageListArgs),
}

/// Strictly decodes the list command's own arguments; continuation stays scan-owned.
fn list_args(command: &str, arguments: &Value) -> Result<ListArgs, CraftError> {
    match command {
        "jira.issue.search" => {
            let args: crate::issue_reads::command::IssueSearchArgs =
                serde_json::from_value(arguments.clone()).map_err(|_| {
                    CraftError::field("/arguments", "invalid_issue_search_arguments")
                })?;
            if args.cursor.is_some() {
                return Err(CraftError::field(
                    "/arguments/cursor",
                    "scan_owns_continuation",
                ));
            }
            Ok(ListArgs::IssueSearch(args))
        }
        "jira.project.list" => {
            let args: crate::project_list::command::ProjectListArgs =
                serde_json::from_value(arguments.clone()).map_err(|_| {
                    CraftError::field("/arguments", "invalid_project_list_arguments")
                })?;
            if args.cursor.is_some() {
                return Err(CraftError::field(
                    "/arguments/cursor",
                    "scan_owns_continuation",
                ));
            }
            Ok(ListArgs::ProjectList(args))
        }
        "confluence.page.list" => {
            let args: crate::confluence_reads::command::ConfluencePageListArgs =
                serde_json::from_value(arguments.clone())
                    .map_err(|_| CraftError::field("/arguments", "invalid_page_list_arguments"))?;
            if args.cursor.is_some() {
                return Err(CraftError::field(
                    "/arguments/cursor",
                    "scan_owns_continuation",
                ));
            }
            Ok(ListArgs::PageList(args))
        }
        _ => Err(CraftError::field("/command", "command_not_scannable")),
    }
}

enum Host {
    IssueSearch(crate::issue_reads::command::PreparedInvocation),
    ProjectList(crate::project_list::command::PreparedInvocation),
    PageList(crate::confluence_reads::command::PreparedInvocation),
}

impl Host {
    fn prepare(
        command: &str,
        arguments: &Value,
        record_fields: &[String],
        options: &InvocationOptions<'_>,
        store: &ProfileStore,
        control: &InvocationControl,
    ) -> Result<Self, CraftError> {
        let list_options = InvocationOptions {
            state_root: options.state_root,
            profile: options.profile,
            request_id: options.request_id,
            max_bytes: LIST_ENVELOPE_BYTES,
        };
        match list_args(command, arguments)? {
            ListArgs::IssueSearch(args) => Ok(Self::IssueSearch(
                crate::issue_reads::command::PreparedInvocation::prepare(
                    crate::issue_reads::command::IssueReadArgs::Search(args),
                    record_fields,
                    &list_options,
                    store,
                    control,
                )?,
            )),
            ListArgs::ProjectList(args) => Ok(Self::ProjectList(
                crate::project_list::command::PreparedInvocation::prepare(
                    args,
                    record_fields,
                    &list_options,
                    store,
                    control,
                )?,
            )),
            ListArgs::PageList(args) => Ok(Self::PageList(
                crate::confluence_reads::command::PreparedInvocation::prepare(
                    crate::confluence_reads::command::ConfluenceReadArgs::PageList(args),
                    record_fields,
                    &list_options,
                    store,
                    control,
                )?,
            )),
        }
    }
    fn binding(&self) -> &craft_core::pagination::ListBinding {
        match self {
            Self::IssueSearch(host) => host.binding(),
            Self::ProjectList(host) => host.binding(),
            Self::PageList(host) => host.binding(),
        }
    }
    fn grant(&self) -> &craft_core::contracts::GrantRef {
        match self {
            Self::IssueSearch(host) => host.grant(),
            Self::ProjectList(host) => host.grant(),
            Self::PageList(host) => host.grant(),
        }
    }
    fn scan(
        &self,
        store: &ProfileStore,
        client: &craft_runtime::auth::ServiceOAuthClient,
        locked: &craft_runtime::secrets::LockedCredentials<'_>,
        control: InvocationControl,
        run: super::ScanRunner<'_>,
    ) -> (Result<Advanced, CraftError>, Option<CredentialCacheUpdate>) {
        match self {
            Self::IssueSearch(host) => {
                host.scan_cached(store, client, locked, TlsPolicy::Platform, control, run)
            }
            Self::ProjectList(host) => {
                host.scan_cached(store, client, locked, TlsPolicy::Platform, control, run)
            }
            Self::PageList(host) => {
                host.scan_cached(store, client, locked, TlsPolicy::Platform, control, run)
            }
        }
    }
}

fn item_id(item: &Value) -> Option<String> {
    match &item["id"] {
        Value::String(id) => Some(id.clone()),
        Value::Number(id) => Some(id.to_string()),
        _ => None,
    }
}

fn artifacts(root: &Path, control: &InvocationControl) -> Result<NativeArtifacts, CraftError> {
    NativeArtifacts::open(
        &root.join(ARTIFACT_DIRECTORY),
        scan::MAX_STORED_BYTES * 2,
        control,
    )
    .map_err(storage)
}

pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    let checkpoint = json!({"name":"scan_0123456789abcdef0123456789abcdef-r00001.checkpoint.json","digest":format!("sha256:{}","0".repeat(64)),"bytes":2048});
    let scan_example = json!({"scan_id":"scan_0123456789abcdef0123456789abcdef","command":"jira.issue.search","revision":1,"outcome":"budget_reached","limit":"items","complete":false,"caps":{"max_items":1000,"max_pages":50,"max_stored_bytes":8388608},"totals":{"items":1000,"pages":20,"stored_bytes":180000,"duplicate_ids":0,"empty_pages":0},"checkpoint":checkpoint,"segment":{"name":"scan_0123456789abcdef0123456789abcdef-r00001.jsonl","digest":format!("sha256:{}","0".repeat(64)),"bytes":180000,"items":1000},"retry_after":null,"consistency":"live","credential_cache_update":null});
    let scan_scopes = vec![
        vec!["read:jira-work".into()],
        vec!["read:page:confluence".into()],
    ];
    for (id, description, effect, input, example) in [
        (
            START,
            "Start an explicit bounded scan of a list command into private record segments and a checkpoint; never prints records.",
            EffectClass::Read,
            schema_for::<ScanStartArgs>()?,
            json!({"command":"jira.issue.search","arguments":{"jql":"project = ED ORDER BY key"},"max_items":1000}),
        ),
        (
            RESUME,
            "Continue a scan from its exact checkpoint name and digest under the same list semantics.",
            EffectClass::Read,
            schema_for::<ScanResumeArgs>()?,
            json!({"checkpoint":"scan_0123456789abcdef0123456789abcdef-r00001.checkpoint.json","checkpoint_digest":format!("sha256:{}","0".repeat(64))}),
        ),
        (
            EXPORT,
            "Export a scan's verified segments as one create-new JSON Lines file; a partial scan needs --allow-partial and stays complete: false.",
            EffectClass::Write,
            schema_for::<ScanExportArgs>()?,
            json!({"checkpoint":"scan_0123456789abcdef0123456789abcdef-r00001.checkpoint.json","checkpoint_digest":format!("sha256:{}","0".repeat(64)),"output_root":"C:/work/out","output":"issues.jsonl"}),
        ),
    ] {
        let export = id == EXPORT;
        let descriptor = OperationDescriptor {
            id: id.into(),
            provider: "craft".into(),
            contract_version: SCHEMA_VERSION.into(),
            api_revision: "bounded-scan-v1".into(),
            description: description.into(),
            effect,
            input_schema: input,
            response_schema: if export {
                response_schema::<ExportData>()?
            } else {
                response_schema::<ScanData>()?
            },
            example,
            response_example: if export {
                json!(Envelope::success(
                    id,
                    "example".into(),
                    json!({"scan_id":"scan_0123456789abcdef0123456789abcdef","revision":2,"complete":true,"items":1500,"artifact":null,"exported":null,"export_write_state":"confirmed"})
                ))
            } else {
                json!(Envelope::success(
                    id,
                    "example".into(),
                    scan_example.clone()
                ))
            },
            projection: None,
            scope_alternatives: if export { vec![] } else { scan_scopes.clone() },
            permissions: if export {
                vec!["Writes only the selected file under the selected output root.".into()]
            } else {
                vec![
                    "The scanned list command's own policy, scopes and native permissions apply."
                        .into(),
                ]
            },
            tenant_conditions: if export {
                vec![]
            } else {
                vec!["A scan is a live traversal, not a snapshot; repeated IDs are counted and missing records cannot be detected.".into()]
            },
            routes: vec![],
            retry_class: if export {
                RetryClass::Never
            } else {
                RetryClass::SafeRead
            },
            reconciliation: if export {
                ReconciliationClass::KnownResource
            } else {
                ReconciliationClass::NotApplicable
            },
        };
        let handler = Arc::new(crate::public_commands::VerifiedHostRequired);
        match id {
            START => registry.register_typed_handler::<ScanStartArgs>(descriptor, handler)?,
            RESUME => registry.register_typed_handler::<ScanResumeArgs>(descriptor, handler)?,
            _ => registry.register_typed_handler::<ScanExportArgs>(descriptor, handler)?,
        }
    }
    Ok(())
}

fn empty(command: &str) -> ScanData {
    ScanData {
        scan_id: None,
        command: command.into(),
        revision: None,
        outcome: None,
        limit: None,
        complete: false,
        caps: None,
        totals: None,
        checkpoint: None,
        segment: None,
        retry_after: None,
        consistency: "live".into(),
        credential_cache_update: None,
    }
}

fn describe(data: &mut ScanData, checkpoint: &ScanCheckpoint) {
    data.scan_id = Some(checkpoint.scan_id.clone());
    data.command = checkpoint.command.clone();
    data.revision = Some(checkpoint.revision);
    data.outcome = Some(checkpoint.outcome);
    data.limit = checkpoint.limit;
    data.complete = checkpoint.complete;
    data.caps = Some(checkpoint.caps);
    data.totals = Some(checkpoint.totals.clone());
}

/// Runs one invocation of a scan from `previous` and publishes its progress.
fn run_scan(
    host: &Host,
    previous: &ScanCheckpoint,
    root: &Path,
    store: &ProfileStore,
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
    data: &mut ScanData,
) -> Result<Option<CraftError>, CraftError> {
    let credentials = NativeCredentials::open(store)?;
    let locked = credentials.lock(host.grant(), control)?;
    let clock = SystemClock::default();
    let client = craft_runtime::auth::ServiceOAuthClient::new(
        TlsPolicy::Platform,
        control.clone(),
        Arc::new(SystemClock::default()),
    )?;
    let mut runner = |session: super::ScanSession<'_>| {
        scan::advance(
            previous,
            ScanInputs {
                source: session.source,
                binding: session.binding,
                project: session.project,
                item_id: &item_id,
                rate_advice: session.rate_advice,
                control,
                clock: &clock,
            },
        )
    };
    let (result, cache) = host.scan(store, &client, &locked, control.clone(), &mut runner);
    data.credential_cache_update = cache;
    let mut advanced = result?;
    describe(data, &advanced.checkpoint);
    data.retry_after = advanced.retry_after;
    // A cancelled invocation still publishes what it fetched, once, within a fresh small bound.
    let evidence;
    let publisher = if control.check().is_ok() {
        artifacts(root, control)?
    } else {
        evidence = InvocationControl::new(
            Arc::new(SystemClock::default()),
            CancellationToken::default(),
            EVIDENCE_MS,
        )?;
        artifacts(root, &evidence)?
    };
    let published = scan::publish(&publisher, &mut advanced)?;
    let _ = options;
    data.checkpoint = Some(CheckpointRef {
        name: published.checkpoint.name,
        digest: published.checkpoint.digest,
        bytes: published.checkpoint.bytes,
    });
    data.segment = published.segment;
    Ok(advanced.error.take())
}

fn finish_scan(
    command: &str,
    data: ScanData,
    result: Result<Option<CraftError>, CraftError>,
    request_id: &str,
) -> CommandResult {
    let error = match result {
        Ok(error) => error,
        Err(error) => Some(error),
    };
    let envelope = match error {
        None => Envelope::success(command, request_id.into(), json!(data)),
        Some(mut error) => {
            if data.outcome == Some(ScanOutcome::RateLimited) {
                error.next_action = "Wait for the reported retry time, then run scan resume with the returned checkpoint.".into();
            }
            Envelope::failure_with_data(command, request_id.into(), error, json!(data))
        }
    };
    CommandResult {
        envelope,
        output_policy: OutputPolicy::PreserveEvidence,
    }
}

pub fn execute(
    action: ScanAction,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> CommandResult {
    match action {
        ScanAction::Start(args) => start(*args, fields, options, control),
        ScanAction::Resume(args) => resume(*args, fields, options, control),
        ScanAction::Export(args) => export(*args, fields, options, control),
    }
}

fn start(
    args: ScanStartArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> CommandResult {
    let mut data = empty(&args.command);
    let result = (|| {
        args.validate(fields, options)?;
        let root = resolve_state_root(options.state_root, |key| std::env::var_os(key))?;
        let store = ProfileStore::open(&root, control)?;
        let host = Host::prepare(
            &args.command,
            &args.arguments,
            &args.record_fields,
            options,
            &store,
            control,
        )?;
        let checkpoint = ScanCheckpoint::start(
            &args.command,
            json!({"input": args.arguments, "record_fields": args.record_fields}),
            host.binding(),
            args.caps.into(),
            SystemClock::default().unix_millis(),
        )?;
        run_scan(
            &host,
            &checkpoint,
            &root,
            &store,
            options,
            control,
            &mut data,
        )
    })();
    finish_scan(START, data, result, options.request_id)
}

fn stored_arguments(checkpoint: &ScanCheckpoint) -> Result<(Value, Vec<String>), CraftError> {
    let input = checkpoint.arguments.get("input").cloned().ok_or_else(|| {
        CraftError::new(
            ErrorCode::CursorCorrupt,
            "Scan checkpoint has no list arguments.",
        )
    })?;
    let fields: Vec<String> = serde_json::from_value(
        checkpoint
            .arguments
            .get("record_fields")
            .cloned()
            .unwrap_or_else(|| json!([])),
    )
    .map_err(|_| {
        CraftError::new(
            ErrorCode::CursorCorrupt,
            "Scan checkpoint fields are invalid.",
        )
    })?;
    Ok((input, fields))
}

fn resume(
    args: ScanResumeArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> CommandResult {
    let mut data = empty("");
    let result = (|| {
        args.validate(fields, options)?;
        let root = resolve_state_root(options.state_root, |key| std::env::var_os(key))?;
        let store = ProfileStore::open(&root, control)?;
        let mut checkpoint = scan::load(
            &artifacts(&root, control)?,
            &args.checkpoint,
            &args.checkpoint_digest,
        )?;
        describe(&mut data, &checkpoint);
        if let Some(items) = args.max_items {
            checkpoint.caps.max_items = items;
        }
        if let Some(pages) = args.max_pages {
            checkpoint.caps.max_pages = pages;
        }
        if let Some(bytes) = args.max_stored_bytes {
            checkpoint.caps.max_stored_bytes = bytes;
        }
        checkpoint.caps.validate()?;
        let (input, record_fields) = stored_arguments(&checkpoint)?;
        let host = Host::prepare(
            &checkpoint.command,
            &input,
            &record_fields,
            options,
            &store,
            control,
        )?;
        run_scan(
            &host,
            &checkpoint,
            &root,
            &store,
            options,
            control,
            &mut data,
        )
    })();
    finish_scan(RESUME, data, result, options.request_id)
}

fn export(
    args: ScanExportArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> CommandResult {
    let mut data = ExportData {
        scan_id: None,
        revision: None,
        complete: false,
        items: None,
        artifact: None,
        exported: None,
        export_write_state: RequestState::NotSent,
    };
    let result = (|| {
        args.validate(fields, options)?;
        let root = resolve_state_root(options.state_root, |key| std::env::var_os(key))?;
        let protected = PrivateRoot::open(&root).map_err(|_| storage(StoreError::Unavailable))?;
        let store = artifacts(&root, control)?;
        let checkpoint = scan::load(&store, &args.checkpoint, &args.checkpoint_digest)?;
        data.scan_id = Some(checkpoint.scan_id.clone());
        data.revision = Some(checkpoint.revision);
        data.complete = checkpoint.complete;
        data.items = Some(checkpoint.totals.items);
        if !checkpoint.complete && !args.allow_partial {
            let mut error = CraftError::new(
                ErrorCode::VersionConflict,
                "This scan has not completed; no export was attempted.",
            );
            error.next_action =
                "Resume the scan, or pass --allow-partial to export the records stored so far."
                    .into();
            return Err(error);
        }
        let assembled = scan::assemble(&store, &checkpoint)?;
        data.artifact = Some(assembled.clone());
        control.check()?;
        let output =
            OutputRoot::open_excluding(Path::new(&args.output_root), protected).map_err(|_| {
                CraftError::field("/output_root", "expected_existing_guarded_output_root")
            })?;
        match store.export_file_checked(
            &assembled.name,
            &output,
            Path::new(&args.output),
            OverwritePolicy::CreateNew,
            &assembled.digest,
        ) {
            Ok(receipt) => {
                data.exported = Some(receipt);
                data.export_write_state = RequestState::Confirmed;
                Ok(())
            }
            Err(error) => {
                data.export_write_state = if error == StoreError::OutcomeUnknown {
                    RequestState::SentUnknown
                } else {
                    RequestState::Failed
                };
                Err(storage(error))
            }
        }
    })();
    let envelope = match result {
        Ok(()) => Envelope::success(EXPORT, options.request_id.into(), json!(data)),
        Err(mut error) => {
            error.details.write_state = data.export_write_state;
            Envelope::failure_with_data(EXPORT, options.request_id.into(), error, json!(data))
        }
    };
    CommandResult {
        envelope,
        output_policy: OutputPolicy::PreserveEvidence,
    }
}
