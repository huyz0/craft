//! Page content-state badges. A badge is display metadata chosen by people in the editor; it is
//! not page lifecycle, not human review and never authorizes plan tickets or workflow changes.
use crate::models::{GetData, numeric};
use crate::registration::validate_target;
use craft_core::contracts::{CanonicalRequest, EffectClass, Envelope, SCHEMA_VERSION};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{
    HttpMethod, NativeCall, NativeResponse, NativeRoute, OperationContext, OperationHandler,
    PreparedOperation, RouteSegment,
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

pub const CONTENT_STATE_GET: &str = "confluence.content-state.get";
pub const CONTENT_STATE_AVAILABLE: &str = "confluence.content-state.available";
pub const GET_ROUTE: &str = "confluence-content-state-get";
pub const AVAILABLE_ROUTE: &str = "confluence-content-state-available";

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContentStateInput {
    #[schemars(length(min = 1, max = 19), regex(pattern = "^[1-9][0-9]*$"))]
    pub page_id: String,
}
/// One native badge. IDs 0 through 2 are the space's default states.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContentState {
    pub id: u64,
    pub name: String,
    pub color: String,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContentStateSummary {
    pub page_id: String,
    /// Always `current`: the published page, never an editor draft.
    pub lifecycle_status: String,
    pub state: Option<ContentState>,
    pub last_updated: Option<String>,
    #[schemars(extend("const" = false))]
    pub grants_approval: bool,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AvailableContentStates {
    pub page_id: String,
    /// Suggested space states; empty when the space disables them.
    pub space_states: Vec<ContentState>,
    /// The caller's recently used custom states; empty when custom states are disabled.
    pub custom_states: Vec<ContentState>,
    #[schemars(extend("const" = false))]
    pub grants_approval: bool,
}

fn malformed() -> CraftError {
    CraftError::new(
        ErrorCode::RemoteUnavailable,
        "Confluence did not return a well-formed content state.",
    )
}
pub fn validate_input(input: &ContentStateInput) -> Result<(), CraftError> {
    if !numeric(&input.page_id) {
        return Err(CraftError::field(
            "/page_id",
            "expected_canonical_positive_int64_id",
        ));
    }
    Ok(())
}
fn state(value: &Value) -> Result<ContentState, CraftError> {
    let name = value["name"]
        .as_str()
        .filter(|v| !v.is_empty() && v.chars().count() <= 255 && !v.chars().any(char::is_control))
        .ok_or_else(malformed)?;
    let color = value["color"]
        .as_str()
        .filter(|v| v.len() <= 16 && !v.chars().any(char::is_control))
        .ok_or_else(malformed)?;
    Ok(ContentState {
        id: value["id"].as_u64().ok_or_else(malformed)?,
        name: name.into(),
        color: color.into(),
    })
}
/// Parses a native content-state response; a null `contentState` means no badge is set.
pub fn observe_state(raw: &Value) -> Result<Option<ContentState>, CraftError> {
    match raw.get("contentState") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => state(value).map(Some),
    }
}
/// Parses the available states response into space and custom lists.
pub fn observe_available(
    raw: &Value,
) -> Result<(Vec<ContentState>, Vec<ContentState>), CraftError> {
    let list = |key: &str| -> Result<Vec<ContentState>, CraftError> {
        raw[key]
            .as_array()
            .filter(|items| items.len() <= 256)
            .ok_or_else(malformed)?
            .iter()
            .map(state)
            .collect()
    };
    Ok((list("spaceContentStates")?, list("customContentStates")?))
}
fn path(available: bool) -> Vec<RouteSegment> {
    let mut path: Vec<_> = ["wiki", "rest", "api", "content"]
        .map(|v| RouteSegment::Literal(v.into()))
        .into_iter()
        .chain([
            RouteSegment::Parameter("id".into()),
            RouteSegment::Literal("state".into()),
        ])
        .collect();
    if available {
        path.push(RouteSegment::Literal("available".into()));
    }
    path
}
pub(crate) fn route(command: &str) -> NativeRoute {
    let available = command == CONTENT_STATE_AVAILABLE;
    NativeRoute {
        id: if available {
            AVAILABLE_ROUTE
        } else {
            GET_ROUTE
        }
        .into(),
        method: HttpMethod::Get,
        path: path(available),
        query_parameters: if available {
            vec![]
        } else {
            vec!["status".into()]
        },
        repeated_query_parameters: vec![],
        continuation_parameters: vec![],
        response_max_bytes: 262_144,
    }
}
/// The native call for a content-state read; `status=current` is always explicit.
pub fn read_call(command: &str, page_id: &str) -> NativeCall {
    NativeCall {
        route: route(command).id,
        path_parameters: BTreeMap::from([("id".into(), page_id.into())]),
        query: if command == CONTENT_STATE_GET {
            BTreeMap::from([("status".into(), "current".into())])
        } else {
            BTreeMap::new()
        },
        repeated_query: BTreeMap::new(),
        body: None,
    }
}
pub(crate) fn projection(command: &str) -> ProjectionContract {
    let fields: &[&str] = if command == CONTENT_STATE_GET {
        &[
            "page_id",
            "lifecycle_status",
            "state",
            "last_updated",
            "grants_approval",
        ]
    } else {
        &[
            "page_id",
            "space_states",
            "custom_states",
            "grants_approval",
        ]
    };
    let fields: Vec<String> = fields.iter().map(|f| (*f).into()).collect();
    ProjectionContract {
        item_pointer: "/item".into(),
        always_returned: fields.clone(),
        default_selected: fields,
        mode: ProjectionMode::Local,
    }
}
pub fn response_schema_for(command: &str) -> Result<Value, CraftError> {
    if command == CONTENT_STATE_GET {
        response_schema::<GetData<ContentStateSummary>>()
    } else {
        response_schema::<GetData<AvailableContentStates>>()
    }
}
pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    let target = json!({"provider":"confluence","deployment":"cloud","tenant":"00000000-1111-2222-3333-444444444444","resource_type":"site","id":"00000000-1111-2222-3333-444444444444"});
    let badge = json!({"id":1,"name":"In progress","color":"#2684ff"});
    for command in [CONTENT_STATE_GET, CONTENT_STATE_AVAILABLE] {
        let get = command == CONTENT_STATE_GET;
        let item = if get {
            json!({"page_id":"101","lifecycle_status":"current","state":badge,"last_updated":"2026-11-10T00:00:00.000Z","grants_approval":false})
        } else {
            json!({"page_id":"101","space_states":[badge],"custom_states":[],"grants_approval":false})
        };
        registry.register_typed_handler::<ContentStateInput>(
            OperationDescriptor {
                id: command.into(),
                provider: "confluence".into(),
                contract_version: SCHEMA_VERSION.into(),
                api_revision: "confluence-cloud-v1".into(),
                description: if get {
                    "Read the published page's content-state badge; a badge is never approval or lifecycle."
                } else {
                    "List the content-state badges the caller may set on a page under its space settings."
                }
                .into(),
                effect: EffectClass::Read,
                input_schema: schema_for::<ContentStateInput>()?,
                response_schema: response_schema_for(command)?,
                example: json!({"page_id":"101"}),
                response_example: json!(Envelope::success(
                    command,
                    "example".into(),
                    json!({"target":target,"item":item,"observed_at":1000,"credential_cache_update":null})
                )),
                projection: Some(projection(command)),
                scope_alternatives: if get {
                    vec![
                        vec!["read:confluence-content.summary".into()],
                        vec!["read:content-details:confluence".into()],
                    ]
                } else {
                    vec![
                        vec!["write:confluence-content".into()],
                        vec!["write:content:confluence".into()],
                    ]
                },
                permissions: vec!["View the page; listing available states also needs edit permission.".into()],
                tenant_conditions: vec!["Space settings decide whether suggested or custom states exist; no approval is inferred.".into()],
                routes: vec![route(command)],
                retry_class: RetryClass::SafeRead,
                reconciliation: ReconciliationClass::NotApplicable,
            },
            Arc::new(Handler),
        )?;
    }
    Ok(())
}
/// Exact page read; returns the normalized item for the bounded read host.
pub fn read_get(
    operation: &PreparedOperation,
    context: &mut dyn OperationContext,
) -> Result<Value, CraftError> {
    context.verify_operation(operation)?;
    let command = operation.request().command.as_str();
    if !matches!(command, CONTENT_STATE_GET | CONTENT_STATE_AVAILABLE) {
        return Err(CraftError::field("/command", "expected_content_state_read"));
    }
    let input: ContentStateInput =
        serde_json::from_value(operation.request().input.clone()).map_err(|_| malformed())?;
    validate_input(&input)?;
    context.checkpoint()?;
    let response = context.invoke(&read_call(command, &input.page_id))?;
    let item = normalize(command, &input.page_id, &response)?;
    context.checkpoint()?;
    Ok(item)
}
fn normalize(command: &str, page_id: &str, response: &NativeResponse) -> Result<Value, CraftError> {
    if response.status != 200 {
        return Err(CraftError::new(
            match response.status {
                401 => ErrorCode::AuthRequired,
                403 => ErrorCode::PermissionDenied,
                429 => ErrorCode::RateLimited,
                _ => ErrorCode::RemoteUnavailable,
            },
            "Content state is unavailable or unreadable; no absence or approval was established.",
        ));
    }
    let raw = &response.body;
    if command == CONTENT_STATE_GET {
        let last = match raw.get("lastUpdated") {
            None | Some(Value::Null) => None,
            Some(v) => Some(
                v.as_str()
                    .filter(|v| v.len() <= 64 && !v.chars().any(char::is_control))
                    .ok_or_else(malformed)?,
            ),
        };
        Ok(json!(ContentStateSummary {
            page_id: page_id.into(),
            lifecycle_status: "current".into(),
            state: observe_state(raw)?,
            last_updated: last.map(String::from),
            grants_approval: false,
        }))
    } else {
        let (space_states, custom_states) = observe_available(raw)?;
        Ok(json!(AvailableContentStates {
            page_id: page_id.into(),
            space_states,
            custom_states,
            grants_approval: false,
        }))
    }
}
struct Handler;
impl OperationHandler for Handler {
    fn validate(&self, request: &CanonicalRequest) -> Result<(), CraftError> {
        validate_target(&request.target)?;
        let input: ContentStateInput = serde_json::from_value(request.input.clone())
            .map_err(|_| CraftError::field("/input", "invalid_content_state_read"))?;
        validate_input(&input)?;
        if request.preconditions.is_some()
            || request.selection.cursor.is_some()
            || request.selection.limit != craft_core::contracts::Selection::default().limit
        {
            return Err(CraftError::field(
                "/selection",
                "content_state_read_has_no_paging_or_preconditions",
            ));
        }
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
            "Content-state reads require the verified host and saved policy.",
        ))
    }
}
