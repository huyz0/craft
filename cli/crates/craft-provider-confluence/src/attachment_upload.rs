//! Page attachment upload and ID-based binary replacement. A new upload refuses an existing
//! filename instead of creating a new version, and replacement is bound to one attachment ID and
//! its reviewed version and file, so neither can silently substitute for the other.
use crate::attachments::{AttachmentSummary, observe, valid_attachment_id};
use crate::models::numeric;
use craft_core::contracts::{
    CanonicalRequest, EffectClass, RequestState, ResourceRef, SCHEMA_VERSION,
};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{
    HttpMethod, NativeCall, NativeResponse, NativeRoute, OperationContext, OperationHandler,
    PreparedOperation, RouteSegment,
};
use craft_core::registry::{
    OperationDescriptor, ReconciliationClass, Registry, RetryClass, schema_for,
};
use craft_core::storage::{Clock, SubmissionState, validate_artifact_name};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::Arc;

pub use crate::pages::AttachmentRevision;

pub const UPLOAD: &str = "confluence.attachment.upload";
pub const REPLACE: &str = "confluence.attachment.replace";
pub const CONTEXT: &str = "confluence.attachment.upload-context";
pub const UPLOAD_ROUTE: &str = "confluence-attachment-upload";
pub const REPLACE_ROUTE: &str = "confluence-attachment-replace";
const PAGE: &str = "upload-page";
const BY_NAME: &str = "upload-attachments-by-name";
const ATTACHMENT: &str = "upload-attachment";
const MAX_RESPONSE: usize = 1_048_576;

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UploadInput {
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub space_id: String,
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub page_id: String,
    /// Present only for replacement: the exact attachment whose binary is replaced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(regex(pattern = "^att[1-9][0-9]*$"))]
    pub attachment_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<AttachmentRevision>,
    #[schemars(length(min = 1, max = 128), regex(pattern = "^[A-Za-z0-9._-]+$"))]
    pub artifact_name: String,
    #[schemars(regex(pattern = "^sha256:[0-9a-f]{64}$"))]
    pub digest: String,
    #[schemars(regex(pattern = "^sha256:[0-9a-f]{64}$"))]
    pub source_selection_digest: String,
    #[schemars(range(max = 1_073_741_824))]
    pub size_bytes: u64,
    #[schemars(length(min = 1, max = 255))]
    pub filename: String,
    #[schemars(length(min = 3, max = 256))]
    pub media_type: String,
}
fn digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|v| {
        v.len() == 64
            && v.bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    })
}
impl UploadInput {
    pub fn validate(&self, command: &str) -> Result<(), CraftError> {
        validate_artifact_name(&self.artifact_name).map_err(|_| invalid())?;
        let media: Vec<_> = self.media_type.split('/').collect();
        let replace = command == REPLACE;
        let replace_fields = match (&self.attachment_id, &self.expected) {
            (Some(id), Some(expected)) => {
                valid_attachment_id(id)
                    && (1..i32::MAX as u32).contains(&expected.version)
                    && !expected.file_id.is_empty()
                    && expected.file_id.len() <= 128
                    && !expected.file_id.chars().any(char::is_control)
            }
            _ => false,
        };
        let no_replace_fields = self.attachment_id.is_none() && self.expected.is_none();
        if !numeric(&self.space_id)
            || !numeric(&self.page_id)
            || (replace && !replace_fields)
            || (!replace && !no_replace_fields)
            || self.size_bytes > 1_073_741_824
            || !digest(&self.digest)
            || !digest(&self.source_selection_digest)
            || self.filename.is_empty()
            || self.filename.len() > 255
            || self
                .filename
                .chars()
                .any(|c| c.is_control() || matches!(c, '/' | '\\'))
            || self.media_type.len() > 256
            || media.len() != 2
            || media.iter().any(|v| {
                v.is_empty()
                    || !v
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&c))
            })
        {
            return Err(invalid());
        }
        Ok(())
    }
}
fn invalid() -> CraftError {
    CraftError::field(
        "/input",
        "expected_exact_page_attachment_target_and_bounded_immutable_upload_snapshot",
    )
}
fn malformed() -> CraftError {
    CraftError::new(
        ErrorCode::RemoteUnavailable,
        "Upload preparation or native acknowledgment is malformed or inconsistent.",
    )
}
fn input(request: &CanonicalRequest) -> Result<UploadInput, CraftError> {
    let value: UploadInput =
        serde_json::from_value(request.input.clone()).map_err(|_| invalid())?;
    let command = if request.command == CONTEXT {
        if value.attachment_id.is_some() {
            REPLACE
        } else {
            UPLOAD
        }
    } else {
        request.command.as_str()
    };
    value.validate(command)?;
    let target = &request.target;
    if target.provider != "confluence"
        || target.deployment != "cloud"
        || target.resource_type != "space"
        || target.id != value.space_id
        || !matches!(request.command.as_str(), UPLOAD | REPLACE | CONTEXT)
        || request.preconditions.is_some()
        || request.selection.cursor.is_some()
        || !request.selection.fields.is_empty()
    {
        return Err(invalid());
    }
    target.validate()?;
    Ok(value)
}
fn route(
    id: &str,
    method: HttpMethod,
    parts: &[&str],
    query: &[&str],
    maximum: u32,
) -> NativeRoute {
    NativeRoute {
        id: id.into(),
        method,
        path: parts
            .iter()
            .map(
                |v| match v.strip_prefix('{').and_then(|v| v.strip_suffix('}')) {
                    Some(p) => RouteSegment::Parameter(p.into()),
                    None => RouteSegment::Literal((*v).into()),
                },
            )
            .collect(),
        query_parameters: query.iter().map(|v| (*v).into()).collect(),
        continuation_parameters: vec![],
        repeated_query_parameters: vec![],
        response_max_bytes: maximum,
    }
}
struct Handler;
impl OperationHandler for Handler {
    fn validate(&self, request: &CanonicalRequest) -> Result<(), CraftError> {
        input(request).map(|_| ())
    }
    fn execute(
        &self,
        _: &PreparedOperation,
        _: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "Upload requires the verified snapshot and journaled multipart host.",
        ))
    }
}
pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    let zero = format!("sha256:{}", "0".repeat(64));
    for command in [UPLOAD, REPLACE, CONTEXT] {
        let routes = match command {
            CONTEXT => vec![
                route(
                    PAGE,
                    HttpMethod::Get,
                    &["wiki", "api", "v2", "pages", "{id}"],
                    &["status", "get-draft", "include-version"],
                    MAX_RESPONSE as u32,
                ),
                route(
                    BY_NAME,
                    HttpMethod::Get,
                    &["wiki", "api", "v2", "pages", "{id}", "attachments"],
                    &["filename", "status", "limit"],
                    MAX_RESPONSE as u32,
                ),
                route(
                    ATTACHMENT,
                    HttpMethod::Get,
                    &["wiki", "api", "v2", "attachments", "{id}"],
                    &["include-version"],
                    262_144,
                ),
            ],
            UPLOAD => vec![route(
                UPLOAD_ROUTE,
                HttpMethod::Post,
                &[
                    "wiki",
                    "rest",
                    "api",
                    "content",
                    "{id}",
                    "child",
                    "attachment",
                ],
                &[],
                MAX_RESPONSE as u32,
            )],
            _ => vec![route(
                REPLACE_ROUTE,
                HttpMethod::Post,
                &[
                    "wiki",
                    "rest",
                    "api",
                    "content",
                    "{id}",
                    "child",
                    "attachment",
                    "{attachmentId}",
                    "data",
                ],
                &[],
                MAX_RESPONSE as u32,
            )],
        };
        let mut example = json!({"space_id":"100","page_id":"101","artifact_name":"design-upload.bin","digest":zero,"source_selection_digest":zero,"size_bytes":12,"filename":"design.bin","media_type":"application/octet-stream"});
        if command == REPLACE {
            example["attachment_id"] = json!("att301");
            example["expected"] = json!({"version":1,"file_id":"0b6f6a1c-file"});
        }
        let descriptor = OperationDescriptor {
            id: command.into(),
            provider: "confluence".into(),
            contract_version: SCHEMA_VERSION.into(),
            api_revision: "confluence-cloud-v1-attachment-upload".into(),
            description: match command {
                CONTEXT => "Qualify the current page in its space and either an absent filename or the reviewed attachment revision.",
                UPLOAD => "Add one exact immutable snapshot as a new page attachment; an existing filename is refused, never versioned.",
                _ => "Replace the binary of one reviewed attachment by ID with an exact immutable snapshot.",
            }
            .into(),
            effect: if command == CONTEXT { EffectClass::Read } else { EffectClass::Write },
            input_schema: schema_for::<UploadInput>()?,
            response_schema: json!({"type":"object"}),
            example,
            response_example: json!({}),
            projection: None,
            scope_alternatives: if command == CONTEXT {
                vec![vec!["read:page:confluence".into(), "read:attachment:confluence".into()]]
            } else {
                vec![
                    vec!["write:confluence-file".into()],
                    vec!["read:content-details:confluence".into(), "write:attachment:confluence".into()],
                ]
            },
            permissions: vec!["View the page and add attachments in its space.".into()],
            tenant_conditions: vec!["Page/filename/attachment checks are non-atomic. Remote byte digest remains unverified.".into()],
            routes,
            retry_class: if command == CONTEXT { RetryClass::SafeRead } else { RetryClass::ReconcileBeforeReplay },
            reconciliation: if command == CONTEXT { ReconciliationClass::NotApplicable } else { ReconciliationClass::Correlation },
        };
        registry.register_typed_handler::<UploadInput>(descriptor, Arc::new(Handler))?;
    }
    Ok(())
}
struct Freshness {
    clock: Arc<dyn Clock>,
    wall: u64,
    mono: u64,
    last_wall: Cell<u64>,
    last_mono: Cell<u64>,
}
impl Freshness {
    fn new(clock: Arc<dyn Clock>) -> Self {
        let wall = clock.unix_millis();
        let mono = clock.monotonic_millis();
        Self {
            clock,
            wall,
            mono,
            last_wall: Cell::new(wall),
            last_mono: Cell::new(mono),
        }
    }
    fn check(&self) -> Result<(), CraftError> {
        let wall = self.clock.unix_millis();
        let mono = self.clock.monotonic_millis();
        if wall < self.last_wall.get()
            || mono < self.last_mono.get()
            || wall.saturating_sub(self.wall) >= 30_000
            || mono.saturating_sub(self.mono) >= 30_000
        {
            return Err(CraftError::new(
                ErrorCode::VersionConflict,
                "Upload preparation expired or its clock moved backward.",
            ));
        }
        self.last_wall.set(wall);
        self.last_mono.set(mono);
        Ok(())
    }
}
/// The verified service principal that observed the upload context.
#[derive(Clone, PartialEq, Eq)]
pub struct UploadPrincipal {
    pub grant: craft_core::contracts::GrantRef,
    pub actor: String,
}
pub struct FrozenUpload {
    call: NativeCall,
    digest: String,
    principal: UploadPrincipal,
    freshness: Freshness,
    input: UploadInput,
    route: &'static str,
}
impl FrozenUpload {
    pub fn call(&self) -> &NativeCall {
        &self.call
    }
    pub fn input(&self) -> &UploadInput {
        &self.input
    }
    pub fn route(&self) -> &'static str {
        self.route
    }
    pub fn verify(
        &self,
        operation: &PreparedOperation,
        principal: &UploadPrincipal,
    ) -> Result<(), CraftError> {
        if !matches!(operation.request().command.as_str(), UPLOAD | REPLACE)
            || !matches!(operation.effect(), EffectClass::Write)
            || operation.digest() != self.digest
            || principal != &self.principal
        {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Frozen upload differs from its exact operation or principal.",
            ));
        }
        self.freshness.check()
    }
}
fn read(
    context: &mut dyn OperationContext,
    call: &NativeCall,
    freshness: &Freshness,
) -> Result<Value, CraftError> {
    context.checkpoint()?;
    freshness.check()?;
    let response = context.invoke(call)?;
    context.checkpoint()?;
    freshness.check()?;
    if response.status != 200 {
        return Err(CraftError::new(
            match response.status {
                401 => ErrorCode::AuthRequired,
                403 => ErrorCode::PermissionDenied,
                429 => ErrorCode::RateLimited,
                _ => ErrorCode::RemoteUnavailable,
            },
            "Upload context did not establish required native evidence; no retry was performed.",
        ));
    }
    if !response.body.is_object()
        || serde_json::to_vec(&response.body)
            .map_err(|_| malformed())?
            .len()
            > MAX_RESPONSE
    {
        return Err(malformed());
    }
    Ok(response.body)
}
fn get(route: &str, id: &str, query: &[(&str, &str)]) -> NativeCall {
    NativeCall {
        route: route.into(),
        path_parameters: BTreeMap::from([("id".into(), id.into())]),
        query: query
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect(),
        repeated_query: BTreeMap::new(),
        body: None,
    }
}
fn page(
    context: &mut dyn OperationContext,
    input: &UploadInput,
    freshness: &Freshness,
) -> Result<(), CraftError> {
    let raw = read(
        context,
        &get(
            PAGE,
            &input.page_id,
            &[
                ("status", "current"),
                ("get-draft", "false"),
                ("include-version", "true"),
            ],
        ),
        freshness,
    )?;
    if raw["id"] != input.page_id.as_str()
        || raw["status"] != "current"
        || raw["spaceId"] != input.space_id.as_str()
    {
        return Err(CraftError::new(
            ErrorCode::VersionConflict,
            "Upload page is not current in the selected space.",
        ));
    }
    Ok(())
}
fn existing(
    context: &mut dyn OperationContext,
    input: &UploadInput,
    freshness: &Freshness,
) -> Result<Option<AttachmentSummary>, CraftError> {
    match &input.attachment_id {
        None => {
            let raw = read(
                context,
                &get(
                    BY_NAME,
                    &input.page_id,
                    &[
                        ("filename", &input.filename),
                        ("status", "current"),
                        ("limit", "2"),
                    ],
                ),
                freshness,
            )?;
            let rows = raw["results"].as_array().ok_or_else(malformed)?;
            if !rows.is_empty() {
                return Err(CraftError::new(
                    ErrorCode::VersionConflict,
                    "The page already has an attachment with this filename; replace it by ID instead.",
                ));
            }
            Ok(None)
        }
        Some(id) => {
            let raw = read(
                context,
                &get(ATTACHMENT, id, &[("include-version", "true")]),
                freshness,
            )?;
            let observed = observe(&raw, &input.page_id)?;
            let expected = input.expected.as_ref().ok_or_else(invalid)?;
            if &observed.id != id
                || observed.version != expected.version
                || observed.file_id != expected.file_id
                || observed.title != input.filename
            {
                return Err(CraftError::new(
                    ErrorCode::VersionConflict,
                    "The attachment changed since it was reviewed, or the filename differs. Read it again before replacing it.",
                ));
            }
            Ok(Some(observed))
        }
    }
}
pub fn prepare(
    read_op: &PreparedOperation,
    write_op: &PreparedOperation,
    context: &mut dyn OperationContext,
    principal: UploadPrincipal,
    clock: Arc<dyn Clock>,
) -> Result<FrozenUpload, CraftError> {
    if read_op.request().command != CONTEXT
        || !matches!(write_op.request().command.as_str(), UPLOAD | REPLACE)
        || !matches!(read_op.effect(), EffectClass::Read)
        || !matches!(write_op.effect(), EffectClass::Write)
        || read_op.request().target != write_op.request().target
        || read_op.request().input != write_op.request().input
    {
        return Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "Expected matching sealed upload context and write operations.",
        ));
    }
    let input = input(write_op.request())?;
    principal.grant.validate()?;
    craft_core::contracts::validate_identity("/actor", &principal.actor)?;
    if matches!(principal.actor.as_str(), "anonymous" | "unknown") {
        return Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "Upload context requires an exact verified actor.",
        ));
    }
    context.verify_operation(read_op)?;
    let freshness = Freshness::new(clock);
    page(context, &input, &freshness)?;
    let before = existing(context, &input, &freshness)?;
    let after = existing(context, &input, &freshness)?;
    if before != after {
        return Err(CraftError::new(
            ErrorCode::VersionConflict,
            "Attachment state changed during preparation.",
        ));
    }
    page(context, &input, &freshness)?;
    context.checkpoint()?;
    freshness.check()?;
    let replace = input.attachment_id.is_some();
    let mut path = BTreeMap::from([("id".into(), input.page_id.clone())]);
    if let Some(id) = &input.attachment_id {
        path.insert("attachmentId".into(), id.clone());
    }
    Ok(FrozenUpload {
        call: NativeCall {
            route: if replace { REPLACE_ROUTE } else { UPLOAD_ROUTE }.into(),
            path_parameters: path,
            query: BTreeMap::new(),
            repeated_query: BTreeMap::new(),
            body: None,
        },
        digest: write_op.digest().into(),
        principal,
        freshness,
        route: if replace { REPLACE_ROUTE } else { UPLOAD_ROUTE },
        input,
    })
}
pub struct UploadObservation {
    pub state: SubmissionState,
    pub resources: Vec<ResourceRef>,
    pub attachment_id: Option<String>,
    pub version: Option<u32>,
    pub acknowledged: bool,
    pub desired_fields_verified: bool,
    pub error: Option<CraftError>,
}
fn failure(state: SubmissionState, code: ErrorCode, message: &str) -> UploadObservation {
    let mut error = CraftError::new(code, message);
    error.details.write_state = if state == SubmissionState::Failed {
        RequestState::Failed
    } else {
        RequestState::SentUnknown
    };
    UploadObservation {
        state,
        resources: vec![],
        attachment_id: None,
        version: None,
        acknowledged: false,
        desired_fields_verified: false,
        error: Some(error),
    }
}
/// Upload answers with `{"results":[attachment]}`; replacement answers with the attachment.
/// Confirmation needs the exact ID binding; filename, size and (for replacement) the next
/// version are reported as desired fields.
pub fn interpret(
    operation: &PreparedOperation,
    response: &NativeResponse,
) -> Result<UploadObservation, CraftError> {
    let input = input(operation.request())?;
    let replace = operation.request().command == REPLACE;
    if response.status != 200 {
        return Ok(failure(
            if matches!(response.status, 400 | 401 | 403 | 404 | 413) {
                SubmissionState::Failed
            } else {
                SubmissionState::Unknown
            },
            match response.status {
                400 => ErrorCode::InputInvalid,
                401 => ErrorCode::AuthRequired,
                403 | 404 => ErrorCode::PermissionDenied,
                413 => ErrorCode::ItemTooLarge,
                429 => ErrorCode::RateLimited,
                _ => ErrorCode::WriteOutcomeUnknown,
            },
            "Upload did not return its documented acknowledgment; retain this operation and do not retry.",
        ));
    }
    let bounded = serde_json::to_vec(&response.body)
        .map_err(|_| malformed())?
        .len()
        <= MAX_RESPONSE;
    let row = if !bounded {
        None
    } else if replace {
        Some(&response.body)
    } else {
        response.body["results"]
            .as_array()
            .filter(|rows| rows.len() == 1)
            .map(|rows| &rows[0])
    };
    let mut observed = UploadObservation {
        state: SubmissionState::Unknown,
        resources: vec![],
        attachment_id: None,
        version: None,
        acknowledged: true,
        desired_fields_verified: false,
        error: None,
    };
    if let Some(row) = row {
        let id = row["id"].as_str().filter(|id| valid_attachment_id(id));
        let bound = match (&input.attachment_id, id) {
            (Some(expected), Some(id)) => expected == id,
            (None, Some(_)) => true,
            _ => false,
        };
        if bound && let Some(id) = id {
            observed.attachment_id = Some(id.into());
            observed.resources.push(ResourceRef {
                resource_type: "attachment".into(),
                id: id.into(),
                ..operation.request().target.clone()
            });
            observed.state = SubmissionState::Confirmed;
        }
        let version = row["version"]["number"].as_u64().map(|v| v as u32);
        observed.version = version;
        let next = input.expected.as_ref().map(|e| e.version.saturating_add(1));
        observed.desired_fields_verified = row["title"] == input.filename.as_str()
            && row["extensions"]["fileSize"].as_u64() == Some(input.size_bytes)
            && (!replace || version == next);
    }
    if observed.state != SubmissionState::Confirmed || !observed.desired_fields_verified {
        let mut error = CraftError::new(
            if observed.state == SubmissionState::Confirmed {
                ErrorCode::PartialApply
            } else {
                ErrorCode::WriteOutcomeUnknown
            },
            "Upload acknowledgment lacks complete matching attachment metadata; retain known IDs and the snapshot without replaying.",
        );
        error.details.write_state = if observed.state == SubmissionState::Confirmed {
            RequestState::Confirmed
        } else {
            RequestState::SentUnknown
        };
        observed.error = Some(error);
    }
    Ok(observed)
}
