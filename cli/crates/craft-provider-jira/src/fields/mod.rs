//! Native field metadata is descriptive context, never issue-write schema authority.

mod input;
mod models;
mod search;
mod selection;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use craft_core::contracts::{CanonicalRequest, EffectClass, Envelope, SCHEMA_VERSION};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{
    HttpMethod, NativeCall, NativeRoute, OperationContext, OperationHandler, PreparedOperation,
    RouteSegment,
};
use craft_core::pagination::{NativePage, NativePosition};
use craft_core::registry::{
    OperationDescriptor, ProviderManifest, ProviderModule, ReconciliationClass, Registry,
    RetryClass, response_schema, schema_for,
};
use serde_json::{Value, json};

pub use input::{FieldListInput, FieldOrder, FieldSearchInput, FieldType};
pub use models::*;
pub use selection::{FieldPlan, projection};

pub const LIST: &str = "jira.field.list";
pub const SEARCH: &str = "jira.field.search";
pub const LIST_ROUTE: &str = "jira-field-list";
pub const SEARCH_ROUTE: &str = "jira-field-search";
pub const MAX_RESPONSE_BYTES: usize = 1_048_576;

pub fn route(command: &str) -> Result<NativeRoute, CraftError> {
    if !matches!(command, LIST | SEARCH) {
        return Err(CraftError::field("/command", "expected_field_discovery"));
    }
    let mut path: Vec<_> = ["rest", "api", "3", "field"]
        .map(|value| RouteSegment::Literal(value.into()))
        .into();
    if command == SEARCH {
        path.push(RouteSegment::Literal("search".into()));
    }
    Ok(NativeRoute {
        id: if command == LIST {
            LIST_ROUTE
        } else {
            SEARCH_ROUTE
        }
        .into(),
        method: HttpMethod::Get,
        path,
        query_parameters: if command == LIST {
            vec![]
        } else {
            [
                "startAt",
                "maxResults",
                "type",
                "id",
                "query",
                "orderBy",
                "expand",
                "projectIds",
            ]
            .map(String::from)
            .into()
        },
        continuation_parameters: if command == LIST {
            vec![]
        } else {
            vec!["startAt".into()]
        },
        repeated_query_parameters: if command == LIST {
            vec![]
        } else {
            ["id", "type", "projectIds"].map(String::from).into()
        },
        response_max_bytes: MAX_RESPONSE_BYTES as u32,
    })
}
pub fn coverage(command: &str) -> Result<FieldCoverage, CraftError> {
    match command {
        LIST => Ok(FieldCoverage::VisibilityDependent),
        SEARCH => Ok(FieldCoverage::ClassicProjectFields),
        _ => Err(CraftError::field("/command", "expected_field_discovery")),
    }
}
pub fn response(command: &str) -> Result<Value, CraftError> {
    match command {
        LIST => response_schema::<FieldPageData<ListedField>>(),
        SEARCH => response_schema::<FieldPageData<SearchedField>>(),
        _ => Err(CraftError::field("/command", "expected_field_discovery")),
    }
}
pub struct JiraFields;
impl ProviderModule for JiraFields {
    fn manifest(&self) -> ProviderManifest {
        ProviderManifest {
            id: "jira".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            deployments: vec!["cloud".into()],
            contract_version: SCHEMA_VERSION.into(),
        }
    }
    fn register(&self, registry: &mut Registry) -> Result<(), CraftError> {
        for command in [LIST, SEARCH] {
            let descriptor = OperationDescriptor {
                id:command.into(),provider:"jira".into(),contract_version:SCHEMA_VERSION.into(),api_revision:"jira-cloud-v3".into(),
                description: if command == LIST {"Read the visibility-dependent native field list with buffered local paging."} else {"Search Classic-project fields with typed filters and immutable continuation."}.into(),
                effect:EffectClass::Read,input_schema:if command == LIST {schema_for::<FieldListInput>()?} else {schema_for::<FieldSearchInput>()?},response_schema:response(command)?,example:json!({}),
                response_example:json!(Envelope::success(command,"example".into(),json!({"target":{"provider":"jira","deployment":"cloud","tenant":"00000000-1111-2222-3333-444444444444","resource_type":"site","id":"00000000-1111-2222-3333-444444444444"},"items":[{"id":"summary","name":"Summary","schema":{"type":"string"}}],"coverage":coverage(command)?,"observation":null,"cached":false,"credential_cache_update":null}))),
                projection:Some(projection(command)),scope_alternatives:vec![vec!["read:jira-work".into()]],
                permissions:vec![if command == LIST {"No native permission required; returned fields still depend on visibility/configuration"} else {"Permission to access Jira"}.into()],
                tenant_conditions:vec!["Explicit site and verified actor; discovery does not establish field write applicability or a global catalog.".into()],
                routes:vec![route(command)?],retry_class:RetryClass::SafeRead,reconciliation:ReconciliationClass::NotApplicable,
            };
            if command == LIST {
                registry.register_typed_handler::<FieldListInput>(descriptor, Arc::new(Handler))?;
            } else {
                registry
                    .register_typed_handler::<FieldSearchInput>(descriptor, Arc::new(Handler))?;
            }
        }
        Ok(())
    }
}
struct Handler;
impl OperationHandler for Handler {
    fn validate(&self, request: &CanonicalRequest) -> Result<(), CraftError> {
        crate::projects::validate_target(&request.target)?;
        if request.preconditions.is_some() {
            return Err(CraftError::field(
                "/preconditions",
                "field_discovery_preconditions_unsupported",
            ));
        }
        if request.command == SEARCH {
            serde_json::from_value::<FieldSearchInput>(request.input.clone())
                .map_err(|_| CraftError::field("/input", "invalid_field_search_input"))?
                .validate()?;
        }
        Ok(())
    }
    fn execute(
        &self,
        _: &PreparedOperation,
        _: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        Err(CraftError::new(
            ErrorCode::UnsupportedCapability,
            "Field discovery requires the immutable-cursor host.",
        ))
    }
}
fn malformed() -> CraftError {
    CraftError::new(
        ErrorCode::RemoteUnavailable,
        "Field metadata did not establish valid records and continuation; no completeness or schema authority was inferred.",
    )
}
fn invoke(context: &mut dyn OperationContext, call: &NativeCall) -> Result<Value, CraftError> {
    context.checkpoint()?;
    let response = context.invoke(call)?;
    if response.status != 200 {
        let mut error = CraftError::new(
            match response.status {
                400 | 422 => ErrorCode::InputInvalid,
                401 => ErrorCode::AuthRequired,
                403 => ErrorCode::PermissionDenied,
                429 => ErrorCode::RateLimited,
                _ => ErrorCode::RemoteUnavailable,
            },
            "Field discovery failed; no automatic retry was performed.",
        );
        error.details.write_state = craft_core::contracts::RequestState::Failed;
        return Err(error);
    }
    craft_core::input::parse_json(
        &serde_json::to_vec(&response.body).map_err(|_| malformed())?,
        MAX_RESPONSE_BYTES,
    )
    .map_err(|_| malformed())
}
pub fn read_page(
    operation: &PreparedOperation,
    context: &mut dyn OperationContext,
    plan: &FieldPlan,
    position: Option<&NativePosition>,
) -> Result<NativePage, CraftError> {
    context.verify_operation(operation)?;
    plan.verify(operation)?;
    crate::projects::validate_target(&operation.request().target)?;
    if operation.request().command == SEARCH {
        return search::read(operation, context, plan, position);
    }
    if operation.request().command != LIST || position.is_some() {
        return Err(CraftError::field(
            "/cursor",
            "expected_initial_unpaginated_field_list",
        ));
    }
    let raw = invoke(
        context,
        &NativeCall {
            route: LIST_ROUTE.into(),
            path_parameters: BTreeMap::new(),
            query: BTreeMap::new(),
            repeated_query: BTreeMap::new(),
            body: None,
        },
    )?;
    let values = raw.as_array().ok_or_else(malformed)?;
    Ok(NativePage {
        context: None,
        items: normalize(values, plan, context)?,
        next: None,
    })
}
fn normalize(
    values: &[Value],
    plan: &FieldPlan,
    context: &dyn OperationContext,
) -> Result<Vec<Value>, CraftError> {
    if values.len() > 1000 {
        return Err(CraftError::new(
            ErrorCode::CursorCapacity,
            "Field records exceed the bounded native page capacity; no partial list was returned.",
        ));
    }
    let mut ids = BTreeSet::new();
    let mut items = Vec::new();
    for value in values {
        context.checkpoint()?;
        let item = plan.normalize(value)?;
        if !ids.insert(item["id"].as_str().ok_or_else(malformed)?.to_owned()) {
            return Err(malformed());
        }
        items.push(item);
    }
    Ok(items)
}
