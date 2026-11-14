//! Attachment metadata is inert and never supplies a download URL or filesystem destination.
#[cfg(test)]
mod tests;
use craft_core::contracts::{
    CanonicalRequest, CredentialCacheUpdate, EffectClass, Envelope, ResourceRef, SCHEMA_VERSION,
};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{
    HttpMethod, NativeCall, NativeRoute, OperationContext, OperationHandler, PreparedOperation,
    RouteSegment,
};
use craft_core::input::canonical_digest;
use craft_core::pagination::{NativePage, NativePosition, PageObservation};
use craft_core::projection::{ProjectionContract, ProjectionMode};
use craft_core::registry::{
    OperationDescriptor, ReconciliationClass, Registry, RetryClass, response_schema, schema_for,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
pub const GET: &str = "jira.attachment.get";
pub const LIST: &str = "jira.attachment.list";
pub const SETTINGS: &str = "jira.attachment.settings";
const GET_ROUTE: &str = "jira-attachment-get";
const LIST_ROUTE: &str = "jira-issue-attachments";
const SETTINGS_ROUTE: &str = "jira-attachment-settings";
const RESPONSE_MAX: usize = 4_194_304;
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetInput {
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub id: String,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListInput {
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub issue_id: String,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SettingsInput {}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttachmentRecord {
    pub id: String,
    pub filename: Option<String>,
    pub size_bytes: Option<u64>,
    pub mime_type: Option<String>,
    pub created: Option<String>,
    pub author_account_id: Option<String>,
    #[schemars(regex(pattern = "^sha256:[0-9a-f]{64}$"))]
    pub metadata_revision: String,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SettingsRecord {
    pub enabled: Option<bool>,
    pub upload_limit_bytes: Option<u64>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetData {
    pub target: ResourceRef,
    pub attachment: Option<AttachmentRecord>,
    pub observation: Option<PageObservation>,
    pub credential_cache_update: Option<CredentialCacheUpdate>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListData {
    pub target: ResourceRef,
    pub source_issue_id: String,
    pub items: Vec<AttachmentRecord>,
    pub observation: Option<PageObservation>,
    pub cached: bool,
    pub credential_cache_update: Option<CredentialCacheUpdate>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SettingsData {
    pub target: ResourceRef,
    pub settings: Option<SettingsRecord>,
    pub observation: Option<PageObservation>,
    pub credential_cache_update: Option<CredentialCacheUpdate>,
}
pub fn numeric(value: &str) -> bool {
    crate::comments::numeric(value)
}
fn invalid() -> CraftError {
    CraftError::new(
        ErrorCode::RemoteUnavailable,
        "Native attachment metadata is malformed or inconsistent; no retry was performed.",
    )
}
fn object(raw: &Value) -> Result<(), CraftError> {
    if !raw.is_object()
        || ["errors", "errorMessages", "warningMessages"]
            .iter()
            .any(|k| raw.get(k).is_some())
    {
        return Err(invalid());
    }
    Ok(())
}
fn optional_text(raw: &Value, path: &str, max: usize) -> Result<Option<String>, CraftError> {
    raw.pointer(path)
        .filter(|v| !v.is_null())
        .map(|v| {
            v.as_str()
                .filter(|s| !s.is_empty() && s.len() <= max && !s.chars().any(char::is_control))
                .map(str::to_owned)
                .ok_or_else(invalid)
        })
        .transpose()
}
fn optional_size(raw: &Value, key: &str) -> Result<Option<u64>, CraftError> {
    raw.get(key)
        .filter(|v| !v.is_null())
        .map(|v| {
            v.as_u64()
                .filter(|n| *n <= i64::MAX as u64)
                .ok_or_else(invalid)
        })
        .transpose()
}
pub fn parse_record(raw: &Value) -> Result<AttachmentRecord, CraftError> {
    object(raw)?;
    let id = match &raw["id"] {
        Value::String(id) if numeric(id) => id.clone(),
        Value::Number(n) => n
            .as_i64()
            .filter(|n| *n > 0)
            .map(|n| n.to_string())
            .ok_or_else(invalid)?,
        _ => return Err(invalid()),
    };
    let mut record = AttachmentRecord {
        id,
        filename: optional_text(raw, "/filename", 4096)?,
        size_bytes: optional_size(raw, "size")?,
        mime_type: optional_text(raw, "/mimeType", 256)?,
        created: optional_text(raw, "/created", 128)?,
        author_account_id: optional_text(raw, "/author/accountId", 512)?,
        metadata_revision: String::new(),
    };
    record.metadata_revision = canonical_digest(
        &json!({"id":record.id,"filename":record.filename,"size_bytes":record.size_bytes,"mime_type":record.mime_type,"created":record.created,"author_account_id":record.author_account_id}),
    )?;
    Ok(record)
}
pub fn parse_settings(raw: &Value) -> Result<SettingsRecord, CraftError> {
    object(raw)?;
    Ok(SettingsRecord {
        enabled: raw
            .get("enabled")
            .filter(|v| !v.is_null())
            .map(|v| v.as_bool().ok_or_else(invalid))
            .transpose()?,
        upload_limit_bytes: optional_size(raw, "uploadLimit")?,
    })
}
pub fn validate(request: &CanonicalRequest) -> Result<(), CraftError> {
    crate::projects::validate_target(&request.target)?;
    if request.preconditions.is_some()
        || request.command != LIST && request.selection.cursor.is_some()
    {
        return Err(CraftError::field(
            "/request",
            "attachment_reads_have_no_write_preconditions_or_single_cursor",
        ));
    }
    let id = match request.command.as_str() {
        GET => {
            let v: GetInput = serde_json::from_value(request.input.clone())
                .map_err(|_| CraftError::field("/input", "invalid_attachment_get"))?;
            Some(v.id)
        }
        LIST => {
            let v: ListInput = serde_json::from_value(request.input.clone())
                .map_err(|_| CraftError::field("/input", "invalid_attachment_list"))?;
            Some(v.issue_id)
        }
        SETTINGS => {
            let _: SettingsInput = serde_json::from_value(request.input.clone())
                .map_err(|_| CraftError::field("/input", "invalid_attachment_settings"))?;
            None
        }
        _ => return Err(CraftError::field("/command", "expected_attachment_read")),
    };
    if id.is_some_and(|id| !numeric(&id)) {
        return Err(CraftError::field(
            "/id",
            "expected_canonical_positive_attachment_or_issue_id",
        ));
    }
    Ok(())
}
fn projection(command: &str) -> ProjectionContract {
    ProjectionContract {
        item_pointer: match command {
            GET => "/attachment",
            SETTINGS => "/settings",
            _ => "/items",
        }
        .into(),
        always_returned: if command == SETTINGS {
            vec!["enabled".into(), "upload_limit_bytes".into()]
        } else {
            vec!["id".into(), "metadata_revision".into()]
        },
        default_selected: if command == SETTINGS {
            vec!["enabled".into(), "upload_limit_bytes".into()]
        } else {
            [
                "id",
                "filename",
                "size_bytes",
                "mime_type",
                "created",
                "metadata_revision",
            ]
            .map(String::from)
            .into()
        },
        mode: ProjectionMode::Mixed,
    }
}
struct Handler;
impl OperationHandler for Handler {
    fn validate(&self, request: &CanonicalRequest) -> Result<(), CraftError> {
        validate(request)
    }
    fn execute(
        &self,
        _: &PreparedOperation,
        _: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "Attachment reads require the verified buffered host.",
        ))
    }
}
pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    for command in [GET, LIST, SETTINGS] {
        let (route, parts, query) = match command {
            GET => (
                GET_ROUTE,
                vec!["rest", "api", "3", "attachment", "id"],
                vec![],
            ),
            LIST => (
                LIST_ROUTE,
                vec!["rest", "api", "3", "issue", "issueIdOrKey"],
                vec!["fields", "updateHistory", "failFast"],
            ),
            _ => (
                SETTINGS_ROUTE,
                vec!["rest", "api", "3", "attachment", "meta"],
                vec![],
            ),
        };
        let target = json!({"provider":"jira","deployment":"cloud","tenant":"00000000-1111-2222-3333-444444444444","resource_type":"site","id":"00000000-1111-2222-3333-444444444444"});
        let data = match command {
            GET => {
                json!({"target":target,"attachment":null,"observation":null,"credential_cache_update":null})
            }
            LIST => {
                json!({"target":target,"source_issue_id":"10000","items":[],"observation":null,"cached":false,"credential_cache_update":null})
            }
            _ => {
                json!({"target":target,"settings":null,"observation":null,"credential_cache_update":null})
            }
        };
        let descriptor=OperationDescriptor {id:command.into(),provider:"jira".into(),contract_version:SCHEMA_VERSION.into(),api_revision:"jira-v3-attachment-metadata-v1".into(),description:match command {GET=>"Read inert attachment metadata without following or returning content URLs.",LIST=>"Read visible issue attachment metadata with opaque local remainder paging.",_=>"Read attachment enablement and upload-size settings; project write permission stays separate."}.into(),effect:EffectClass::Read,input_schema:match command {GET=>schema_for::<GetInput>()?,LIST=>schema_for::<ListInput>()?,_=>schema_for::<SettingsInput>()?},response_schema:match command {GET=>response_schema::<GetData>()?,LIST=>response_schema::<ListData>()?,_=>response_schema::<SettingsData>()?},example:match command {GET=>json!({"id":"10001"}),LIST=>json!({"issue_id":"10000"}),_=>json!({})},response_example:json!(Envelope::success(command,"example".into(),data)),projection:Some(projection(command)),scope_alternatives:vec![vec!["read:jira-work".into()]],permissions:vec!["Native Browse projects, issue security and private-comment restrictions apply to metadata; settings has no native permission requirement.".into()],tenant_conditions:vec!["Metadata is not downloaded bytes or replay identity; cached observations do not prove current native access.".into()],routes:vec![NativeRoute {id:route.into(),method:HttpMethod::Get,path:parts.into_iter().map(|s|if matches!(s,"id"|"issueIdOrKey") {RouteSegment::Parameter(s.into())}else{RouteSegment::Literal(s.into())}).collect(),query_parameters:query.into_iter().map(String::from).collect(),continuation_parameters:vec![],repeated_query_parameters:vec![],response_max_bytes:if command==SETTINGS {8192}else{RESPONSE_MAX as u32}}],retry_class:RetryClass::SafeRead,reconciliation:ReconciliationClass::NotApplicable};
        match command {
            GET => registry.register_typed_handler::<GetInput>(descriptor, Arc::new(Handler))?,
            LIST => registry.register_typed_handler::<ListInput>(descriptor, Arc::new(Handler))?,
            _ => registry.register_typed_handler::<SettingsInput>(descriptor, Arc::new(Handler))?,
        }
    }
    Ok(())
}
pub fn read_page(
    operation: &PreparedOperation,
    context: &mut dyn OperationContext,
    position: Option<&NativePosition>,
) -> Result<NativePage, CraftError> {
    validate(operation.request())?;
    context.verify_operation(operation)?;
    if position.is_some() {
        return Err(invalid());
    }
    let command = operation.request().command.as_str();
    let input = &operation.request().input;
    let mut call = NativeCall {
        route: match command {
            GET => GET_ROUTE,
            LIST => LIST_ROUTE,
            _ => SETTINGS_ROUTE,
        }
        .into(),
        path_parameters: BTreeMap::new(),
        query: BTreeMap::new(),
        repeated_query: BTreeMap::new(),
        body: None,
    };
    if command == GET {
        call.path_parameters.insert(
            "id".into(),
            input["id"].as_str().ok_or_else(invalid)?.into(),
        );
    }
    if command == LIST {
        call.path_parameters.insert(
            "issueIdOrKey".into(),
            input["issue_id"].as_str().ok_or_else(invalid)?.into(),
        );
        call.query = BTreeMap::from([
            ("fields".into(), "attachment".into()),
            ("updateHistory".into(), "false".into()),
            ("failFast".into(), "true".into()),
        ]);
    }
    context.checkpoint()?;
    let response = context.invoke(&call)?;
    context.checkpoint()?;
    if response.status != 200 {
        return Err(CraftError::new(
            match response.status {
                401 => ErrorCode::AuthRequired,
                403 => ErrorCode::PermissionDenied,
                429 => ErrorCode::RateLimited,
                _ => ErrorCode::RemoteUnavailable,
            },
            "Attachment read did not establish visible evidence; no retry was performed.",
        ));
    }
    if serde_json::to_vec(&response.body)
        .map_err(|_| invalid())?
        .len()
        > if command == SETTINGS {
            8192
        } else {
            RESPONSE_MAX
        }
    {
        return Err(invalid());
    }
    object(&response.body)?;
    let items = if command == SETTINGS {
        vec![json!(parse_settings(&response.body)?)]
    } else if command == GET {
        let record = parse_record(&response.body)?;
        if record.id != input["id"] {
            return Err(invalid());
        }
        vec![json!(record)]
    } else {
        if response.body["id"] != input["issue_id"]
            || [
                "nextPage",
                "nextPageToken",
                "isLast",
                "startAt",
                "maxResults",
            ]
            .iter()
            .any(|k| response.body.get(k).is_some())
        {
            return Err(invalid());
        }
        let rows = response.body["fields"]["attachment"]
            .as_array()
            .filter(|rows| rows.len() <= 1000)
            .ok_or_else(invalid)?;
        let mut ids = BTreeSet::new();
        let mut items = Vec::new();
        for raw in rows {
            let record = parse_record(raw)?;
            if !ids.insert(record.id.clone()) {
                return Err(invalid());
            }
            items.push(json!(record));
        }
        items
    };
    Ok(NativePage {
        items,
        context: if command == LIST {
            Some(
                json!({"source_issue_id":input["issue_id"],"coverage":"visible_issue_attachment_field"}),
            )
        } else {
            None
        },
        next: None,
    })
}
