//! Deletion binds one attachment to its issue, project, metadata fingerprint and native permission.
//!
//! Jira's attachment metadata does not name its issue, so membership comes from the issue's own
//! attachment field. The fingerprint is rechecked before the send, which narrows but cannot close
//! the window for a concurrent change: Jira offers no conditional attachment delete.
#[cfg(test)]
mod tests;
use crate::attachments::{AttachmentRecord, parse_record};
use crate::comments::mutation::Ownership;
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
use craft_core::storage::{Clock, SubmissionState};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::Arc;

pub const DELETE: &str = "jira.attachment.delete";
pub const CONTEXT: &str = "jira.attachment.delete-context";
pub const DELETE_ROUTE: &str = "jira-attachment-delete";
const ISSUE: &str = "attachment-delete-issue";
const METADATA: &str = "attachment-delete-metadata";
const PERMISSIONS: &str = "attachment-delete-permissions";
const RESPONSE_MAX: usize = 4_194_304;

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteInput {
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub issue_id: String,
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub attachment_id: String,
    #[schemars(length(min = 71, max = 71), regex(pattern = "^sha256:[0-9a-f]{64}$"))]
    pub expected_revision: String,
    #[serde(default)]
    pub ownership: Ownership,
}
fn invalid() -> CraftError {
    CraftError::field(
        "/input",
        "expected_numeric_issue_attachment_and_metadata_revision",
    )
}
fn malformed() -> CraftError {
    CraftError::new(
        ErrorCode::RemoteUnavailable,
        "Attachment deletion evidence is malformed or inconsistent; no retry was performed.",
    )
}
fn digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|v| {
        v.len() == 64
            && v.bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    })
}
pub fn input(request: &CanonicalRequest) -> Result<DeleteInput, CraftError> {
    crate::created_issue::validate_target(&request.target)?;
    let value: DeleteInput =
        serde_json::from_value(request.input.clone()).map_err(|_| invalid())?;
    if !crate::attachments::numeric(&request.target.id)
        || !crate::attachments::numeric(&value.issue_id)
        || !crate::attachments::numeric(&value.attachment_id)
        || !digest(&value.expected_revision)
        || !matches!(request.command.as_str(), DELETE | CONTEXT)
        || request.preconditions.is_some()
        || request.selection.cursor.is_some()
        || !request.selection.fields.is_empty()
    {
        return Err(invalid());
    }
    Ok(value)
}
fn route(id: &str, method: HttpMethod, parts: &[&str], query: &[&str]) -> NativeRoute {
    NativeRoute {
        id: id.into(),
        method,
        path: parts
            .iter()
            .map(|s| {
                if matches!(*s, "issueIdOrKey" | "id") {
                    RouteSegment::Parameter((*s).into())
                } else {
                    RouteSegment::Literal((*s).into())
                }
            })
            .collect(),
        query_parameters: query.iter().map(|s| (*s).into()).collect(),
        continuation_parameters: vec![],
        repeated_query_parameters: vec![],
        response_max_bytes: RESPONSE_MAX as u32,
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
            "Attachment deletion requires private preparation and the journaled host.",
        ))
    }
}
pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    for command in [DELETE, CONTEXT] {
        let context = command == CONTEXT;
        let routes = if context {
            vec![
                route(
                    ISSUE,
                    HttpMethod::Get,
                    &["rest", "api", "3", "issue", "issueIdOrKey"],
                    &["fields", "updateHistory", "failFast"],
                ),
                route(
                    METADATA,
                    HttpMethod::Get,
                    &["rest", "api", "3", "attachment", "id"],
                    &[],
                ),
                route(
                    PERMISSIONS,
                    HttpMethod::Get,
                    &["rest", "api", "3", "mypermissions"],
                    &["issueId", "permissions"],
                ),
            ]
        } else {
            vec![route(
                DELETE_ROUTE,
                HttpMethod::Delete,
                &["rest", "api", "3", "attachment", "id"],
                &[],
            )]
        };
        let descriptor = OperationDescriptor {
            id: command.into(),
            provider: "jira".into(),
            contract_version: SCHEMA_VERSION.into(),
            api_revision: "jira-cloud-v3-attachment-delete-v1".into(),
            description: if context {
                "Qualify issue/project membership, attachment fingerprint, uploader and native delete permission."
            } else {
                "Permanently delete one exact attachment through the journaled host; no automatic retry."
            }
            .into(),
            effect: if context {
                EffectClass::Read
            } else {
                EffectClass::Permanent
            },
            input_schema: schema_for::<DeleteInput>()?,
            response_schema: json!({"type":"object"}),
            example: json!({"issue_id":"10000","attachment_id":"10001","expected_revision":format!("sha256:{}", "0".repeat(64)),"ownership":"own"}),
            response_example: json!({}),
            projection: None,
            scope_alternatives: vec![vec![
                if context {
                    "read:jira-work"
                } else {
                    "write:jira-work"
                }
                .into(),
            ]],
            permissions: vec![
                "Browse projects, issue security and native Delete own/all attachments.".into(),
            ],
            tenant_conditions: vec![
                "Fingerprint rechecks are non-atomic; Jira has no conditional attachment delete.".into(),
            ],
            routes,
            retry_class: if context {
                RetryClass::SafeRead
            } else {
                RetryClass::ReconcileBeforeReplay
            },
            reconciliation: if context {
                ReconciliationClass::NotApplicable
            } else {
                ReconciliationClass::KnownResource
            },
        };
        registry.register_typed_handler::<DeleteInput>(descriptor, Arc::new(Handler))?;
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
                "Attachment deletion preparation expired or its clock moved backward.",
            ));
        }
        self.last_wall.set(wall);
        self.last_mono.set(mono);
        Ok(())
    }
}
pub struct FrozenDelete {
    call: NativeCall,
    digest: String,
    principal: MetadataPrincipal,
    freshness: Freshness,
    before: AttachmentRecord,
}
impl FrozenDelete {
    pub fn call(&self) -> &NativeCall {
        &self.call
    }
    pub fn before(&self) -> &AttachmentRecord {
        &self.before
    }
    pub fn verify(
        &self,
        operation: &PreparedOperation,
        principal: &MetadataPrincipal,
    ) -> Result<(), CraftError> {
        if operation.request().command != DELETE
            || self.digest != operation.digest()
            || &self.principal != principal
        {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Frozen attachment deletion differs from the exact operation or authenticated principal.",
            ));
        }
        self.freshness.check()
    }
}
fn call(route: &str, param: Option<(&str, &str)>, query: &[(&str, &str)]) -> NativeCall {
    NativeCall {
        route: route.into(),
        path_parameters: param
            .map(|(k, v)| BTreeMap::from([(k.to_owned(), v.to_owned())]))
            .unwrap_or_default(),
        query: query
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
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
                404 => ErrorCode::RemoteUnavailable,
                429 => ErrorCode::RateLimited,
                _ => ErrorCode::RemoteUnavailable,
            },
            "Attachment deletion could not establish native evidence; no retry was performed.",
        ));
    }
    if !response.body.is_object()
        || serde_json::to_vec(&response.body)
            .map_err(|_| malformed())?
            .len()
            > RESPONSE_MAX
        || ["errors", "errorMessages", "warningMessages"]
            .iter()
            .any(|k| response.body.get(k).is_some())
    {
        return Err(malformed());
    }
    Ok(response.body)
}
/// Confirms the issue is in the target project and currently lists the attachment.
fn membership(
    context: &mut dyn OperationContext,
    value: &DeleteInput,
    target: &ResourceRef,
    freshness: &Freshness,
) -> Result<(), CraftError> {
    let raw = read(
        context,
        &call(
            ISSUE,
            Some(("issueIdOrKey", &value.issue_id)),
            &[
                ("fields", "project,attachment"),
                ("updateHistory", "false"),
                ("failFast", "true"),
            ],
        ),
        freshness,
    )?;
    if raw["id"] != value.issue_id.as_str() || raw["fields"]["project"]["id"] != target.id.as_str()
    {
        return Err(CraftError::new(
            ErrorCode::VersionConflict,
            "Attachment issue or its current project differs from the explicit target.",
        ));
    }
    let rows = raw["fields"]["attachment"]
        .as_array()
        .filter(|rows| rows.len() <= 1000)
        .ok_or_else(malformed)?;
    let listed = rows
        .iter()
        .map(parse_record)
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .any(|record| record.id == value.attachment_id);
    if !listed {
        return Err(CraftError::new(
            ErrorCode::VersionConflict,
            "The issue does not currently list this attachment; nothing was deleted.",
        ));
    }
    Ok(())
}
fn metadata(
    context: &mut dyn OperationContext,
    value: &DeleteInput,
    principal: &MetadataPrincipal,
    freshness: &Freshness,
) -> Result<AttachmentRecord, CraftError> {
    let raw = read(
        context,
        &call(METADATA, Some(("id", &value.attachment_id)), &[]),
        freshness,
    )?;
    let record = parse_record(&raw)?;
    if record.id != value.attachment_id || record.metadata_revision != value.expected_revision {
        return Err(CraftError::new(
            ErrorCode::VersionConflict,
            "Current attachment metadata differs from its expected revision.",
        ));
    }
    if value.ownership == Ownership::Own
        && record.author_account_id.as_deref() != Some(&principal.actor)
    {
        return Err(CraftError::new(
            ErrorCode::PermissionDenied,
            "Own-attachment intent requires the authenticated actor to be the uploader.",
        ));
    }
    Ok(record)
}
fn permissions(
    context: &mut dyn OperationContext,
    value: &DeleteInput,
    freshness: &Freshness,
) -> Result<(), CraftError> {
    let required = [
        "BROWSE_PROJECTS",
        "DELETE_OWN_ATTACHMENTS",
        "DELETE_ALL_ATTACHMENTS",
    ];
    let raw = read(
        context,
        &call(
            PERMISSIONS,
            None,
            &[
                ("issueId", &value.issue_id),
                ("permissions", &required.join(",")),
            ],
        ),
        freshness,
    )?;
    let flags = required
        .iter()
        .map(|key| {
            let item = &raw["permissions"][*key];
            if item["key"] != *key {
                return Err(malformed());
            }
            item["havePermission"].as_bool().ok_or_else(malformed)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let allowed = flags[0] && (flags[2] || value.ownership == Ownership::Own && flags[1]);
    if !allowed {
        return Err(CraftError::new(
            ErrorCode::PermissionDenied,
            "Issue-scoped native attachment delete permission is missing or insufficient for own/any intent.",
        ));
    }
    Ok(())
}
pub fn prepare(
    read_op: &PreparedOperation,
    write_op: &PreparedOperation,
    context: &mut dyn OperationContext,
    principal: MetadataPrincipal,
    clock: Arc<dyn Clock>,
) -> Result<FrozenDelete, CraftError> {
    if read_op.request().command != CONTEXT
        || write_op.request().command != DELETE
        || !matches!(read_op.effect(), EffectClass::Read)
        || !matches!(write_op.effect(), EffectClass::Permanent)
        || read_op.request().input != write_op.request().input
        || read_op.request().target != write_op.request().target
    {
        return Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "Expected matching sealed attachment deletion context and mutation operations.",
        ));
    }
    let value = input(write_op.request())?;
    principal.grant.validate()?;
    craft_core::contracts::validate_identity("/actor", &principal.actor)?;
    context.verify_operation(read_op)?;
    let freshness = Freshness::new(clock);
    let target = &write_op.request().target;
    membership(context, &value, target, &freshness)?;
    let before = metadata(context, &value, &principal, &freshness)?;
    permissions(context, &value, &freshness)?;
    metadata(context, &value, &principal, &freshness)?;
    membership(context, &value, target, &freshness)?;
    context.checkpoint()?;
    freshness.check()?;
    Ok(FrozenDelete {
        call: call(DELETE_ROUTE, Some(("id", &value.attachment_id)), &[]),
        digest: write_op.digest().into(),
        principal,
        freshness,
        before,
    })
}
pub struct DeleteObservation {
    pub state: SubmissionState,
    pub resources: Vec<ResourceRef>,
    pub acknowledged: bool,
    pub error: Option<CraftError>,
}
/// 204 confirms deletion. 403 and 404 are documented refusals that leave nothing to reconcile;
/// any other status leaves the outcome unknown.
pub fn interpret(
    operation: &PreparedOperation,
    response: &NativeResponse,
) -> Result<DeleteObservation, CraftError> {
    let value = input(operation.request())?;
    if operation.request().command != DELETE {
        return Err(CraftError::field("/command", "expected_attachment_delete"));
    }
    if response.status == 204 {
        return Ok(DeleteObservation {
            state: SubmissionState::Confirmed,
            resources: vec![ResourceRef {
                resource_type: "attachment".into(),
                id: value.attachment_id,
                ..operation.request().target.clone()
            }],
            acknowledged: true,
            error: None,
        });
    }
    let refused = matches!(response.status, 400 | 401 | 403 | 404);
    let mut error = CraftError::new(
        match response.status {
            400 => ErrorCode::InputInvalid,
            401 => ErrorCode::AuthRequired,
            403 => ErrorCode::PermissionDenied,
            404 => ErrorCode::RemoteUnavailable,
            429 => ErrorCode::RateLimited,
            _ => ErrorCode::WriteOutcomeUnknown,
        },
        "Attachment deletion did not return its documented acknowledgment; retain the receipt and do not retry.",
    );
    error.details.write_state = if refused {
        RequestState::Failed
    } else {
        RequestState::SentUnknown
    };
    Ok(DeleteObservation {
        state: if refused {
            SubmissionState::Failed
        } else {
            SubmissionState::Unknown
        },
        resources: vec![],
        acknowledged: false,
        error: Some(error),
    })
}
