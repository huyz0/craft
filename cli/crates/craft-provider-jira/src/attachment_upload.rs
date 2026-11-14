//! Upload preparation binds native project, settings, permission and immutable snapshot facts.
#[cfg(test)]
mod tests;
use crate::attachments::{AttachmentRecord, SettingsRecord};
use crate::create_metadata::MetadataPrincipal;
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
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
pub const UPLOAD: &str = "jira.attachment.upload";
pub const CONTEXT: &str = "jira.attachment.upload-context";
pub const UPLOAD_ROUTE: &str = "jira-attachment-upload";
const ISSUE: &str = "upload-issue-project";
const SETTINGS: &str = "upload-settings";
const PERMISSIONS: &str = "upload-permissions";
const MAX_RESPONSE: usize = 1_048_576;
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UploadInput {
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub issue_id: String,
    #[schemars(length(min = 1, max = 128), regex(pattern = "^[A-Za-z0-9._-]+$"))]
    pub artifact_name: String,
    #[schemars(regex(pattern = "^sha256:[0-9a-f]{64}$"))]
    pub digest: String,
    #[schemars(regex(pattern = "^sha256:[0-9a-f]{64}$"))]
    pub source_selection_digest: String,
    #[schemars(range(max = 1_073_741_824))]
    pub size_bytes: u64,
    #[schemars(length(min = 1, max = 1024))]
    pub filename: String,
    #[schemars(length(min = 3, max = 256))]
    pub media_type: String,
}
impl UploadInput {
    pub fn validate(&self) -> Result<(), CraftError> {
        validate_artifact_name(&self.artifact_name).map_err(|_| invalid())?;
        let media: Vec<_> = self.media_type.split('/').collect();
        if !self
            .source_selection_digest
            .strip_prefix("sha256:")
            .is_some_and(|v| {
                v.len() == 64
                    && v.bytes()
                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            })
        {
            return Err(invalid());
        }
        if !crate::attachments::numeric(&self.issue_id)
            || self.size_bytes > 1_073_741_824
            || !self.digest.strip_prefix("sha256:").is_some_and(|v| {
                v.len() == 64
                    && v.bytes()
                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            })
            || self.filename.is_empty()
            || self.filename.len() > 1024
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
        "expected_exact_numeric_issue_and_bounded_immutable_upload_snapshot",
    )
}
fn malformed() -> CraftError {
    CraftError::new(
        ErrorCode::RemoteUnavailable,
        "Upload preparation or native acknowledgment is malformed or inconsistent.",
    )
}
fn input(request: &CanonicalRequest) -> Result<UploadInput, CraftError> {
    crate::created_issue::validate_target(&request.target)?;
    let value: UploadInput =
        serde_json::from_value(request.input.clone()).map_err(|_| invalid())?;
    value.validate()?;
    if !crate::attachments::numeric(&request.target.id)
        || !matches!(request.command.as_str(), UPLOAD | CONTEXT)
        || request.preconditions.is_some()
        || request.selection.cursor.is_some()
        || !request.selection.fields.is_empty()
    {
        return Err(invalid());
    }
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
            .map(|v| {
                if *v == "issueIdOrKey" {
                    RouteSegment::Parameter((*v).into())
                } else {
                    RouteSegment::Literal((*v).into())
                }
            })
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
    for command in [UPLOAD, CONTEXT] {
        let context = command == CONTEXT;
        let routes = if context {
            vec![
                route(
                    ISSUE,
                    HttpMethod::Get,
                    &["rest", "api", "3", "issue", "issueIdOrKey"],
                    &["fields", "updateHistory", "failFast"],
                    MAX_RESPONSE as u32,
                ),
                route(
                    SETTINGS,
                    HttpMethod::Get,
                    &["rest", "api", "3", "attachment", "meta"],
                    &[],
                    8192,
                ),
                route(
                    PERMISSIONS,
                    HttpMethod::Get,
                    &["rest", "api", "3", "mypermissions"],
                    &["issueId", "permissions"],
                    65536,
                ),
            ]
        } else {
            vec![route(
                UPLOAD_ROUTE,
                HttpMethod::Post,
                &["rest", "api", "3", "issue", "issueIdOrKey", "attachments"],
                &[],
                MAX_RESPONSE as u32,
            )]
        };
        let descriptor=OperationDescriptor {
            id:command.into(),provider:"jira".into(),contract_version:SCHEMA_VERSION.into(),api_revision:"jira-v3-attachment-upload-v1".into(),
            description:if context {"Qualify exact issue/project, enabled attachment size limit and native Browse/Create attachments decisions."}else{"Upload one exact immutable snapshot through the journaled multipart host without automatic retry."}.into(),
            effect:if context{EffectClass::Read}else{EffectClass::Write},input_schema:schema_for::<UploadInput>()?,response_schema:json!({"type":"object"}),
            example:json!({"issue_id":"10000","artifact_name":"upload.bin","digest":format!("sha256:{}","0".repeat(64)),"source_selection_digest":format!("sha256:{}","0".repeat(64)),"size_bytes":12,"filename":"design.bin","media_type":"application/octet-stream"}),response_example:json!({}),projection:None,
            scope_alternatives:vec![vec![if context{"read:jira-work"}else{"write:jira-work"}.into()]],
            permissions:vec!["Browse projects, issue security and Create attachments are required for the exact containing issue.".into()],
            tenant_conditions:vec!["Attachments must be enabled and the snapshot must fit the observed upload limit. Project/settings checks are non-atomic. Remote byte digest remains unverified.".into()],routes,
            retry_class:if context{RetryClass::SafeRead}else{RetryClass::ReconcileBeforeReplay},reconciliation:if context{ReconciliationClass::NotApplicable}else{ReconciliationClass::Correlation},
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
pub struct FrozenUpload {
    call: NativeCall,
    digest: String,
    principal: MetadataPrincipal,
    freshness: Freshness,
    input: UploadInput,
    settings: SettingsRecord,
}
impl FrozenUpload {
    pub fn call(&self) -> &NativeCall {
        &self.call
    }
    pub fn input(&self) -> &UploadInput {
        &self.input
    }
    pub fn settings(&self) -> &SettingsRecord {
        &self.settings
    }
    pub fn verify(
        &self,
        operation: &PreparedOperation,
        principal: &MetadataPrincipal,
    ) -> Result<(), CraftError> {
        if operation.request().command != UPLOAD
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
fn call(route: &str, issue: Option<&str>, query: BTreeMap<String, String>) -> NativeCall {
    NativeCall {
        route: route.into(),
        path_parameters: issue
            .map(|v| BTreeMap::from([("issueIdOrKey".into(), v.into())]))
            .unwrap_or_default(),
        query,
        repeated_query: BTreeMap::new(),
        body: None,
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
    let maximum = match call.route.as_str() {
        SETTINGS => 8192,
        PERMISSIONS => 65536,
        _ => MAX_RESPONSE,
    };
    if !response.body.is_object()
        || serde_json::to_vec(&response.body)
            .map_err(|_| malformed())?
            .len()
            > maximum
        || ["errors", "errorMessages", "warningMessages"]
            .iter()
            .any(|v| response.body.get(v).is_some())
    {
        return Err(malformed());
    }
    Ok(response.body)
}
fn project(
    context: &mut dyn OperationContext,
    issue: &str,
    target: &ResourceRef,
    freshness: &Freshness,
) -> Result<(), CraftError> {
    let raw = read(
        context,
        &call(
            ISSUE,
            Some(issue),
            BTreeMap::from([
                ("fields".into(), "project".into()),
                ("updateHistory".into(), "false".into()),
                ("failFast".into(), "true".into()),
            ]),
        ),
        freshness,
    )?;
    if raw["id"] != issue || raw["fields"]["project"]["id"] != target.id {
        return Err(CraftError::new(
            ErrorCode::VersionConflict,
            "Upload issue or current project differs from the explicit target.",
        ));
    }
    Ok(())
}
fn settings(
    context: &mut dyn OperationContext,
    size: u64,
    freshness: &Freshness,
) -> Result<SettingsRecord, CraftError> {
    let settings = crate::attachments::parse_settings(&read(
        context,
        &call(SETTINGS, None, BTreeMap::new()),
        freshness,
    )?)?;
    match settings.enabled {
        Some(true) => (),
        Some(false) => {
            return Err(CraftError::new(
                ErrorCode::UnsupportedCapability,
                "Jira attachments are disabled.",
            ));
        }
        None => return Err(malformed()),
    }
    let limit = settings.upload_limit_bytes.ok_or_else(malformed)?;
    if size > limit {
        return Err(CraftError::new(
            ErrorCode::ItemTooLarge,
            "Snapshot exceeds the current native attachment upload limit.",
        ));
    }
    Ok(settings)
}
pub fn prepare(
    read_op: &PreparedOperation,
    write_op: &PreparedOperation,
    context: &mut dyn OperationContext,
    principal: MetadataPrincipal,
    clock: Arc<dyn Clock>,
) -> Result<FrozenUpload, CraftError> {
    if read_op.request().command != CONTEXT
        || write_op.request().command != UPLOAD
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
    project(
        context,
        &input.issue_id,
        &write_op.request().target,
        &freshness,
    )?;
    let observed = settings(context, input.size_bytes, &freshness)?;
    let permissions = read(
        context,
        &call(
            PERMISSIONS,
            None,
            BTreeMap::from([
                ("issueId".into(), input.issue_id.clone()),
                (
                    "permissions".into(),
                    "BROWSE_PROJECTS,CREATE_ATTACHMENTS".into(),
                ),
            ]),
        ),
        &freshness,
    )?;
    for key in ["BROWSE_PROJECTS", "CREATE_ATTACHMENTS"] {
        let item = &permissions["permissions"][key];
        if item["key"] != key {
            return Err(malformed());
        }
        match item["havePermission"].as_bool() {
            Some(true) => (),
            Some(false) => {
                return Err(CraftError::new(
                    ErrorCode::PermissionDenied,
                    "Issue-scoped Browse/Create attachments permission is missing.",
                ));
            }
            None => return Err(malformed()),
        }
    }
    let current = settings(context, input.size_bytes, &freshness)?;
    if current.enabled != observed.enabled
        || current.upload_limit_bytes != observed.upload_limit_bytes
    {
        return Err(CraftError::new(
            ErrorCode::VersionConflict,
            "Native attachment settings changed during preparation.",
        ));
    }
    project(
        context,
        &input.issue_id,
        &write_op.request().target,
        &freshness,
    )?;
    context.checkpoint()?;
    freshness.check()?;
    Ok(FrozenUpload {
        call: call(UPLOAD_ROUTE, Some(&input.issue_id), BTreeMap::new()),
        digest: write_op.digest().into(),
        principal,
        freshness,
        input,
        settings: current,
    })
}
pub struct UploadObservation {
    pub state: SubmissionState,
    pub resources: Vec<ResourceRef>,
    pub attachment: Option<AttachmentRecord>,
    pub acknowledged: bool,
    pub acknowledged_ids_complete: bool,
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
        attachment: None,
        acknowledged: false,
        acknowledged_ids_complete: false,
        desired_fields_verified: false,
        error: Some(error),
    }
}
pub fn interpret(
    operation: &PreparedOperation,
    response: &NativeResponse,
) -> Result<UploadObservation, CraftError> {
    let input = input(operation.request())?;
    if operation.request().command != UPLOAD {
        return Err(invalid());
    }
    if response.status != 200 {
        return Ok(failure(
            if matches!(response.status, 401 | 403 | 404 | 413) {
                SubmissionState::Failed
            } else {
                SubmissionState::Unknown
            },
            match response.status {
                401 => ErrorCode::AuthRequired,
                403 => ErrorCode::PermissionDenied,
                413 => ErrorCode::ItemTooLarge,
                429 => ErrorCode::RateLimited,
                _ => ErrorCode::WriteOutcomeUnknown,
            },
            "Upload did not return its documented acknowledgment; retain this operation and do not retry.",
        ));
    }
    let mut observed = UploadObservation {
        state: SubmissionState::Unknown,
        resources: vec![],
        attachment: None,
        acknowledged: true,
        acknowledged_ids_complete: true,
        desired_fields_verified: false,
        error: None,
    };
    let rows = response.body.as_array();
    let bounded = serde_json::to_vec(&response.body)
        .map_err(|_| malformed())?
        .len()
        <= MAX_RESPONSE;
    if let Some(rows) = rows.filter(|_| bounded) {
        if rows.is_empty() {
            observed.acknowledged_ids_complete = false;
        }
        let mut ids = BTreeSet::new();
        for row in rows {
            let id = match &row["id"] {
                Value::String(id) if crate::attachments::numeric(id) => Some(id.clone()),
                Value::Number(id) => id.as_i64().filter(|id| *id > 0).map(|id| id.to_string()),
                _ => None,
            };
            if let Some(id) = id {
                if ids.contains(&id) {
                    continue;
                }
                if ids.len() >= 1000 {
                    observed.acknowledged_ids_complete = false;
                    continue;
                }
                ids.insert(id.clone());
                observed.resources.push(ResourceRef {
                    resource_type: "attachment".into(),
                    id,
                    ..operation.request().target.clone()
                });
            } else {
                observed.acknowledged_ids_complete = false;
            }
        }
        if rows.len() == 1 {
            observed.attachment = crate::attachments::parse_record(&rows[0]).ok();
            observed.desired_fields_verified = observed.attachment.as_ref().is_some_and(|record| {
                record.filename.as_deref() == Some(&input.filename)
                    && record.size_bytes == Some(input.size_bytes)
            });
        }
    } else {
        observed.acknowledged_ids_complete = false;
    }
    if !observed.resources.is_empty() && observed.acknowledged_ids_complete {
        observed.state = SubmissionState::Confirmed;
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
