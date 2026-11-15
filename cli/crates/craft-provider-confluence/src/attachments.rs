//! Page attachment metadata reads. Metadata never carries file bytes or download authority; the
//! native download link is not returned because it is not a trusted routing input.
use crate::comments::paging::{continuation, validate_position};
use crate::models::{GetData, ListData, numeric};
use crate::registration::validate_target;
use craft_core::contracts::{CanonicalRequest, EffectClass, Envelope, SCHEMA_VERSION};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{
    HttpMethod, NativeCall, NativeResponse, NativeRoute, OperationContext, OperationHandler,
    PreparedOperation, RouteSegment,
};
use craft_core::pagination::{NativePage, NativePosition};
use craft_core::projection::{ProjectionContract, ProjectionMode};
use craft_core::registry::{
    OperationDescriptor, ReconciliationClass, Registry, RetryClass, response_schema, schema_for,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub const ATTACHMENT_GET: &str = "confluence.attachment.get";
pub const ATTACHMENT_LIST: &str = "confluence.attachment.list";
pub const GET_ROUTE: &str = "confluence-attachment-get";
pub const LIST_ROUTE: &str = "confluence-attachment-list";

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttachmentGetInput {
    /// Native v2 attachment ID, such as `att123`.
    #[schemars(length(min = 4, max = 22), regex(pattern = "^att[1-9][0-9]*$"))]
    pub id: String,
    #[schemars(length(min = 1, max = 19), regex(pattern = "^[1-9][0-9]*$"))]
    pub page_id: String,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttachmentListInput {
    #[schemars(length(min = 1, max = 19), regex(pattern = "^[1-9][0-9]*$"))]
    pub page_id: String,
    #[serde(default = "crate::models::page_size")]
    #[schemars(range(min = 1, max = 250))]
    pub page_size: u16,
}
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttachmentSummary {
    pub id: String,
    pub page_id: String,
    pub status: String,
    pub title: String,
    pub media_type: String,
    pub file_size: u64,
    pub file_id: String,
    pub version: u32,
    pub comment: Option<String>,
}

pub fn valid_attachment_id(id: &str) -> bool {
    id.strip_prefix("att").is_some_and(numeric)
}
fn malformed() -> CraftError {
    CraftError::new(
        ErrorCode::RemoteUnavailable,
        "Confluence did not return well-formed current attachment metadata.",
    )
}
fn text<'a>(raw: &'a Value, key: &str, max: usize) -> Result<&'a str, CraftError> {
    raw[key]
        .as_str()
        .filter(|v| !v.is_empty() && v.len() <= max && !v.chars().any(char::is_control))
        .ok_or_else(malformed)
}
/// Normalizes one current page attachment and checks its page binding.
pub fn observe(raw: &Value, page_id: &str) -> Result<AttachmentSummary, CraftError> {
    let id = text(raw, "id", 22)?;
    if !valid_attachment_id(id)
        || raw["pageId"] != page_id
        || raw["status"] != "current"
        || ["blogPostId", "customContentId"]
            .iter()
            .any(|k| raw.get(k).is_some_and(|v| !v.is_null()))
    {
        return Err(malformed());
    }
    let version = raw["version"]["number"]
        .as_u64()
        .filter(|n| (1..=i32::MAX as u64).contains(n))
        .ok_or_else(malformed)?;
    let comment = match raw.get("comment") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_str()
                .filter(|v| v.len() <= 4096 && !v.contains('\0'))
                .ok_or_else(malformed)?
                .to_owned(),
        ),
    };
    Ok(AttachmentSummary {
        id: id.into(),
        page_id: page_id.into(),
        status: "current".into(),
        title: text(raw, "title", 1024)?.into(),
        media_type: text(raw, "mediaType", 255)?.into(),
        file_size: raw["fileSize"].as_u64().ok_or_else(malformed)?,
        file_id: text(raw, "fileId", 128)?.into(),
        version: version as u32,
        comment: comment.filter(|c| !c.is_empty()),
    })
}
fn segments(parts: &[&str]) -> Vec<RouteSegment> {
    parts
        .iter()
        .map(
            |s| match s.strip_prefix('{').and_then(|v| v.strip_suffix('}')) {
                Some(p) => RouteSegment::Parameter(p.into()),
                None => RouteSegment::Literal((*s).into()),
            },
        )
        .collect()
}
pub(crate) fn route(command: &str) -> NativeRoute {
    if command == ATTACHMENT_GET {
        NativeRoute {
            id: GET_ROUTE.into(),
            method: HttpMethod::Get,
            path: segments(&["wiki", "api", "v2", "attachments", "{id}"]),
            query_parameters: vec!["include-version".into()],
            repeated_query_parameters: vec![],
            continuation_parameters: vec![],
            response_max_bytes: 262_144,
        }
    } else {
        NativeRoute {
            id: LIST_ROUTE.into(),
            method: HttpMethod::Get,
            path: segments(&["wiki", "api", "v2", "pages", "{id}", "attachments"]),
            query_parameters: ["status", "sort", "cursor", "limit"]
                .map(String::from)
                .to_vec(),
            repeated_query_parameters: vec![],
            continuation_parameters: vec!["cursor".into()],
            response_max_bytes: 1_048_576,
        }
    }
}
/// The native single-attachment read used by readers and write hosts alike.
pub fn get_call(id: &str) -> NativeCall {
    NativeCall {
        route: GET_ROUTE.into(),
        path_parameters: BTreeMap::from([("id".into(), id.into())]),
        query: BTreeMap::from([("include-version".into(), "true".into())]),
        repeated_query: BTreeMap::new(),
        body: None,
    }
}
const FIELDS: [&str; 9] = [
    "id",
    "page_id",
    "status",
    "title",
    "media_type",
    "file_size",
    "file_id",
    "version",
    "comment",
];
pub(crate) fn projection(command: &str) -> ProjectionContract {
    let fields: Vec<String> = FIELDS.map(String::from).to_vec();
    ProjectionContract {
        item_pointer: if command == ATTACHMENT_GET {
            "/item"
        } else {
            "/items"
        }
        .into(),
        always_returned: fields.clone(),
        default_selected: fields,
        mode: ProjectionMode::Local,
    }
}
pub fn response_schema_for(command: &str) -> Result<Value, CraftError> {
    if command == ATTACHMENT_GET {
        response_schema::<GetData<AttachmentSummary>>()
    } else {
        response_schema::<ListData<AttachmentSummary>>()
    }
}
pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    let target = json!({"provider":"confluence","deployment":"cloud","tenant":"00000000-1111-2222-3333-444444444444","resource_type":"site","id":"00000000-1111-2222-3333-444444444444"});
    let item = json!({"id":"att301","page_id":"101","status":"current","title":"design.png","media_type":"image/png","file_size":2048,"file_id":"0b6f6a1c-file","version":1,"comment":null});
    for command in [ATTACHMENT_GET, ATTACHMENT_LIST] {
        let get = command == ATTACHMENT_GET;
        let descriptor = OperationDescriptor {
            id: command.into(),
            provider: "confluence".into(),
            contract_version: SCHEMA_VERSION.into(),
            api_revision: "confluence-cloud-v2".into(),
            description: if get {
                "Read one page attachment's metadata; no bytes or links."
            } else {
                "List current attachment metadata for a page."
            }
            .into(),
            effect: EffectClass::Read,
            input_schema: if get {
                schema_for::<AttachmentGetInput>()?
            } else {
                schema_for::<AttachmentListInput>()?
            },
            response_schema: response_schema_for(command)?,
            example: if get {
                json!({"id":"att301","page_id":"101"})
            } else {
                json!({"page_id":"101","page_size":25})
            },
            response_example: json!(Envelope::success(
                command,
                "example".into(),
                if get {
                    json!({"target":target,"item":item,"observed_at":1000,"credential_cache_update":null})
                } else {
                    json!({"target":target,"items":[item],"observation":null,"cached":false,"credential_cache_update":null})
                }
            )),
            projection: Some(projection(command)),
            scope_alternatives: vec![vec!["read:attachment:confluence".into()]],
            permissions: vec!["View the page that holds the attachment.".into()],
            tenant_conditions: vec![
                "Exact page binding; only current page attachments are reported.".into(),
            ],
            routes: vec![route(command)],
            retry_class: RetryClass::SafeRead,
            reconciliation: ReconciliationClass::NotApplicable,
        };
        if get {
            registry.register_typed_handler::<AttachmentGetInput>(descriptor, Arc::new(Handler))?;
        } else {
            registry
                .register_typed_handler::<AttachmentListInput>(descriptor, Arc::new(Handler))?;
        }
    }
    Ok(())
}
fn validate(request: &CanonicalRequest) -> Result<(), CraftError> {
    let bad = || CraftError::field("/input", "invalid_attachment_selectors");
    if request.command == ATTACHMENT_GET {
        let input: AttachmentGetInput =
            serde_json::from_value(request.input.clone()).map_err(|_| bad())?;
        if !valid_attachment_id(&input.id) || !numeric(&input.page_id) {
            return Err(bad());
        }
    } else {
        let input: AttachmentListInput =
            serde_json::from_value(request.input.clone()).map_err(|_| bad())?;
        if !numeric(&input.page_id) || !(1..=250).contains(&input.page_size) {
            return Err(bad());
        }
    }
    Ok(())
}
fn invoke(
    context: &mut dyn OperationContext,
    call: &NativeCall,
) -> Result<NativeResponse, CraftError> {
    context.checkpoint()?;
    let response = context.invoke(call)?;
    if response.status != 200 {
        return Err(CraftError::new(
            match response.status {
                401 => ErrorCode::AuthRequired,
                403 => ErrorCode::PermissionDenied,
                429 => ErrorCode::RateLimited,
                _ => ErrorCode::RemoteUnavailable,
            },
            "Attachment metadata is unavailable; absence was not established.",
        ));
    }
    Ok(response)
}
pub fn read_get(
    operation: &PreparedOperation,
    context: &mut dyn OperationContext,
) -> Result<Value, CraftError> {
    context.verify_operation(operation)?;
    if operation.request().command != ATTACHMENT_GET {
        return Err(CraftError::field("/command", "expected_attachment_get"));
    }
    let input: AttachmentGetInput =
        serde_json::from_value(operation.request().input.clone()).map_err(|_| malformed())?;
    let response = invoke(context, &get_call(&input.id))?;
    let item = observe(&response.body, &input.page_id)?;
    if item.id != input.id {
        return Err(malformed());
    }
    context.checkpoint()?;
    Ok(json!(item))
}
pub fn read_page(
    operation: &PreparedOperation,
    context: &mut dyn OperationContext,
    position: Option<&NativePosition>,
) -> Result<NativePage, CraftError> {
    context.verify_operation(operation)?;
    if operation.request().command != ATTACHMENT_LIST {
        return Err(CraftError::field("/command", "expected_attachment_list"));
    }
    let input: AttachmentListInput =
        serde_json::from_value(operation.request().input.clone()).map_err(|_| malformed())?;
    let mut call = NativeCall {
        route: LIST_ROUTE.into(),
        path_parameters: BTreeMap::from([("id".into(), input.page_id.clone())]),
        query: BTreeMap::from([
            ("limit".into(), input.page_size.to_string()),
            ("status".into(), "current".into()),
            ("sort".into(), "created-date".into()),
        ]),
        repeated_query: BTreeMap::new(),
        body: None,
    };
    if let Some(p) = position {
        validate_position(p)?;
        call.query.extend(p.0.clone());
    }
    let response = invoke(context, &call)?;
    let values = response.body["results"]
        .as_array()
        .filter(|v| v.len() <= input.page_size as usize)
        .ok_or_else(malformed)?;
    let next = continuation(context, &call, &route(ATTACHMENT_LIST), &response)?;
    let mut ids = BTreeSet::new();
    let mut items = Vec::new();
    for raw in values {
        context.checkpoint()?;
        let item = observe(raw, &input.page_id)?;
        if !ids.insert(item.id.clone()) {
            return Err(malformed());
        }
        items.push(json!(item));
    }
    context.checkpoint()?;
    Ok(NativePage {
        items,
        next,
        context: Some(json!({"page_id":input.page_id})),
    })
}
struct Handler;
impl OperationHandler for Handler {
    fn validate(&self, request: &CanonicalRequest) -> Result<(), CraftError> {
        validate_target(&request.target)?;
        if request.preconditions.is_some()
            || (request.command == ATTACHMENT_GET
                && (request.selection.cursor.is_some()
                    || request.selection.limit
                        != craft_core::contracts::Selection::default().limit))
        {
            return Err(CraftError::field(
                "/selection",
                "unsupported_attachment_paging_or_preconditions",
            ));
        }
        validate(request)?;
        projection(&request.command).compile(
            &response_schema_for(&request.command)?,
            &request.selection.fields,
        )?;
        Ok(())
    }
    fn execute(
        &self,
        _: &PreparedOperation,
        _: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "Attachment reads require the verified host and saved policy.",
        ))
    }
}
