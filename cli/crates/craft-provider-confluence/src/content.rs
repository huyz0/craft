//! Folder and whiteboard reads. Both are v2 space content with the same native shape: identity,
//! space, title, status, parent and version. Whiteboard drawing content is not exposed by the API.
use crate::models::{GetData, numeric};
use crate::registration::validate_target;
use craft_core::contracts::{CanonicalRequest, EffectClass, Envelope, SCHEMA_VERSION};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{
    HttpMethod, NativeCall, NativeRoute, OperationContext, OperationHandler, PreparedOperation,
    RouteSegment,
};
use craft_core::projection::{ProjectionContract, ProjectionMode};
use craft_core::registry::{
    OperationDescriptor, ReconciliationClass, Registry, RetryClass, response_schema, schema_for,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

pub const FOLDER_GET: &str = "confluence.folder.get";
pub const WHITEBOARD_GET: &str = "confluence.whiteboard.get";
pub const FOLDER_GET_ROUTE: &str = "confluence-folder-get";
pub const WHITEBOARD_GET_ROUTE: &str = "confluence-whiteboard-get";

/// The two v2 space content types that share this contract.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ContentKind {
    Folder,
    Whiteboard,
}
impl ContentKind {
    pub const ALL: [Self; 2] = [Self::Folder, Self::Whiteboard];
    /// The native `type` value and Craft resource type.
    pub fn native(self) -> &'static str {
        match self {
            Self::Folder => "folder",
            Self::Whiteboard => "whiteboard",
        }
    }
    pub fn collection(self) -> &'static str {
        match self {
            Self::Folder => "folders",
            Self::Whiteboard => "whiteboards",
        }
    }
    pub fn get_command(self) -> &'static str {
        match self {
            Self::Folder => FOLDER_GET,
            Self::Whiteboard => WHITEBOARD_GET,
        }
    }
    pub fn get_route(self) -> &'static str {
        match self {
            Self::Folder => FOLDER_GET_ROUTE,
            Self::Whiteboard => WHITEBOARD_GET_ROUTE,
        }
    }
    pub fn read_scope(self) -> &'static str {
        match self {
            Self::Folder => "read:folder:confluence",
            Self::Whiteboard => "read:whiteboard:confluence",
        }
    }
    pub fn from_get(command: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.get_command() == command)
    }
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContentGetInput {
    #[schemars(length(min = 1, max = 19), regex(pattern = "^[1-9][0-9]*$"))]
    pub id: String,
}
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContentSummary {
    pub id: String,
    pub space_id: String,
    pub status: String,
    pub title: String,
    /// The parent page, folder or whiteboard; absent at the space root.
    pub parent_id: Option<String>,
    pub parent_type: Option<String>,
    pub version: u32,
}

fn malformed() -> CraftError {
    CraftError::new(
        ErrorCode::RemoteUnavailable,
        "Confluence did not return well-formed folder or whiteboard metadata.",
    )
}
fn bounded<'a>(raw: &'a Value, key: &str, max: usize) -> Result<Option<&'a str>, CraftError> {
    match raw.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_str()
            .filter(|v| !v.is_empty() && v.len() <= max && !v.chars().any(char::is_control))
            .map(Some)
            .ok_or_else(malformed),
    }
}
/// Normalizes one folder or whiteboard of the expected kind. Any status is reported; writes check
/// `current` themselves. `createdAt` is not read: the test site returns epoch milliseconds where
/// the reference documents a string.
pub fn observe(raw: &Value, kind: ContentKind) -> Result<ContentSummary, CraftError> {
    let required = |key, max| bounded(raw, key, max)?.ok_or_else(malformed);
    let id = required("id", 19)?;
    let space_id = required("spaceId", 19)?;
    if !numeric(id) || !numeric(space_id) || raw.get("type").is_some_and(|t| t != kind.native()) {
        return Err(malformed());
    }
    let status = required("status", 32)?;
    let version = raw
        .pointer("/version/number")
        .and_then(Value::as_u64)
        .filter(|n| (1..=i32::MAX as u64).contains(n))
        .ok_or_else(malformed)?;
    let parent_id = bounded(raw, "parentId", 19)?;
    if parent_id.is_some_and(|p| !numeric(p)) {
        return Err(malformed());
    }
    Ok(ContentSummary {
        id: id.into(),
        space_id: space_id.into(),
        status: status.into(),
        title: required("title", 4096)?.into(),
        parent_id: parent_id.map(Into::into),
        parent_type: bounded(raw, "parentType", 32)?.map(Into::into),
        version: version as u32,
    })
}
pub(crate) fn route(kind: ContentKind) -> NativeRoute {
    NativeRoute {
        id: kind.get_route().into(),
        method: HttpMethod::Get,
        path: ["wiki", "api", "v2", kind.collection()]
            .into_iter()
            .map(|p| RouteSegment::Literal(p.into()))
            .chain([RouteSegment::Parameter("id".into())])
            .collect(),
        query_parameters: vec!["include-direct-children".into()],
        repeated_query_parameters: vec![],
        continuation_parameters: vec![],
        response_max_bytes: 262_144,
    }
}
/// Whether a folder or whiteboard response lists any direct child. Requires the read to send
/// `include-direct-children=true`; a missing or truncated listing is not evidence of emptiness.
pub fn has_children(raw: &Value) -> Result<bool, CraftError> {
    let children = raw.get("directChildren").ok_or_else(malformed)?;
    let results = children
        .get("results")
        .and_then(Value::as_array)
        .ok_or_else(malformed)?;
    let more = children
        .pointer("/meta/hasMore")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok(more || !results.is_empty())
}
/// The native single-item read used by readers and write hosts alike.
pub fn get_call(kind: ContentKind, id: &str) -> NativeCall {
    NativeCall {
        route: kind.get_route().into(),
        path_parameters: BTreeMap::from([("id".into(), id.into())]),
        query: BTreeMap::new(),
        repeated_query: BTreeMap::new(),
        body: None,
    }
}
const FIELDS: [&str; 7] = [
    "id",
    "space_id",
    "status",
    "title",
    "parent_id",
    "parent_type",
    "version",
];
pub(crate) fn projection() -> ProjectionContract {
    let fields: Vec<String> = FIELDS.map(String::from).to_vec();
    ProjectionContract {
        item_pointer: "/item".into(),
        always_returned: fields.clone(),
        default_selected: fields,
        mode: ProjectionMode::Local,
    }
}
pub fn response_schema_for() -> Result<Value, CraftError> {
    response_schema::<GetData<ContentSummary>>()
}
pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    let target = json!({"provider":"confluence","deployment":"cloud","tenant":"00000000-1111-2222-3333-444444444444","resource_type":"site","id":"00000000-1111-2222-3333-444444444444"});
    for kind in ContentKind::ALL {
        let command = kind.get_command();
        let item = json!({"id":"401","space_id":"100","status":"current","title":"Plans","parent_id":null,"parent_type":null,"version":1});
        let descriptor = OperationDescriptor {
            id: command.into(),
            provider: "confluence".into(),
            contract_version: SCHEMA_VERSION.into(),
            api_revision: "confluence-cloud-v2".into(),
            description: match kind {
                ContentKind::Folder => "Read one folder's metadata.",
                ContentKind::Whiteboard => "Read one whiteboard's metadata; no drawing content.",
            }
            .into(),
            effect: EffectClass::Read,
            input_schema: schema_for::<ContentGetInput>()?,
            response_schema: response_schema_for()?,
            example: json!({"id":"401"}),
            response_example: json!(Envelope::success(
                command,
                "example".into(),
                json!({"target":target,"item":item,"observed_at":1000,"credential_cache_update":null})
            )),
            projection: Some(projection()),
            scope_alternatives: vec![vec![kind.read_scope().into()]],
            permissions: vec![format!("View the {} and its space.", kind.native())],
            tenant_conditions: vec![format!(
                "Exact site; the item must have type {}.",
                kind.native()
            )],
            routes: vec![route(kind)],
            retry_class: RetryClass::SafeRead,
            reconciliation: ReconciliationClass::NotApplicable,
        };
        registry.register_typed_handler::<ContentGetInput>(descriptor, Arc::new(Handler))?;
    }
    Ok(())
}
pub fn read_get(
    operation: &PreparedOperation,
    context: &mut dyn OperationContext,
) -> Result<Value, CraftError> {
    context.verify_operation(operation)?;
    let kind = ContentKind::from_get(&operation.request().command)
        .ok_or_else(|| CraftError::field("/command", "expected_folder_or_whiteboard_get"))?;
    let input: ContentGetInput =
        serde_json::from_value(operation.request().input.clone()).map_err(|_| malformed())?;
    context.checkpoint()?;
    let response = context.invoke(&get_call(kind, &input.id))?;
    if response.status != 200 {
        return Err(CraftError::new(
            match response.status {
                401 => ErrorCode::AuthRequired,
                403 => ErrorCode::PermissionDenied,
                429 => ErrorCode::RateLimited,
                _ => ErrorCode::RemoteUnavailable,
            },
            "Folder or whiteboard metadata is unavailable; absence was not established.",
        ));
    }
    let item = observe(&response.body, kind)?;
    if item.id != input.id {
        return Err(malformed());
    }
    context.checkpoint()?;
    Ok(json!(item))
}
struct Handler;
impl OperationHandler for Handler {
    fn validate(&self, request: &CanonicalRequest) -> Result<(), CraftError> {
        validate_target(&request.target)?;
        if request.preconditions.is_some()
            || request.selection.cursor.is_some()
            || request.selection.limit != craft_core::contracts::Selection::default().limit
        {
            return Err(CraftError::field(
                "/selection",
                "unsupported_get_paging_or_preconditions",
            ));
        }
        let input: ContentGetInput = serde_json::from_value(request.input.clone())
            .map_err(|_| CraftError::field("/input", "invalid_content_selector"))?;
        if !numeric(&input.id) {
            return Err(CraftError::field(
                "/id",
                "expected_canonical_positive_int64_id",
            ));
        }
        projection().compile(&response_schema_for()?, &request.selection.fields)?;
        Ok(())
    }
    fn execute(
        &self,
        _: &PreparedOperation,
        _: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "Folder and whiteboard reads require the verified host and saved policy.",
        ))
    }
}
