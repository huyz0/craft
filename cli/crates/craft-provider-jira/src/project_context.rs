//! Project context is read-only metadata; status membership is not issue-transition authority.

mod fields;
use craft_core::contracts::{
    CanonicalRequest, CredentialCacheUpdate, EffectClass, Envelope, ResourceRef, SCHEMA_VERSION,
};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{
    HttpMethod, NativeCall, NativeResponse, NativeRoute, OperationContext, OperationHandler,
    PreparedOperation, RouteSegment,
};
use craft_core::pagination::{NativePage, NativePosition, PageObservation};
use craft_core::registry::{
    OperationDescriptor, ProviderManifest, ProviderModule, ReconciliationClass, Registry,
    RetryClass, response_schema, schema_for,
};
pub use fields::{FieldPlan, ProjectDetails, StatusGroup, projection};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

pub const GET: &str = "jira.project.get";
pub const STATUSES: &str = "jira.project.statuses";
pub const GET_ROUTE: &str = "jira-project-get";
pub const STATUSES_ROUTE: &str = "jira-project-statuses";
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectContextInput {}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum IdentityResolution {
    ExactId,
    ExactKey,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectGetData {
    pub target: ResourceRef,
    pub project: Option<ProjectDetails>,
    pub identity_resolution: Option<IdentityResolution>,
    pub observed_at: Option<u64>,
    pub credential_cache_update: Option<CredentialCacheUpdate>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectStatusesData {
    pub target: ResourceRef,
    pub items: Vec<StatusGroup>,
    pub observation: Option<PageObservation>,
    pub cached: bool,
    pub credential_cache_update: Option<CredentialCacheUpdate>,
}
pub struct JiraProjectContext;
fn route(command: &str) -> NativeRoute {
    let mut path: Vec<_> = ["rest", "api", "3", "project"]
        .map(|part| RouteSegment::Literal(part.into()))
        .into();
    path.push(RouteSegment::Parameter("projectIdOrKey".into()));
    if command == STATUSES {
        path.push(RouteSegment::Literal("statuses".into()));
    }
    NativeRoute {
        repeated_query_parameters: Vec::new(),
        id: if command == GET {
            GET_ROUTE
        } else {
            STATUSES_ROUTE
        }
        .into(),
        method: HttpMethod::Get,
        path,
        query_parameters: vec![],
        continuation_parameters: vec![],
        response_max_bytes: 1_048_576,
    }
}
impl ProviderModule for JiraProjectContext {
    fn manifest(&self) -> ProviderManifest {
        ProviderManifest {
            id: "jira".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            deployments: vec!["cloud".into()],
            contract_version: SCHEMA_VERSION.into(),
        }
    }
    fn register(&self, registry: &mut Registry) -> Result<(), CraftError> {
        for command in [GET, STATUSES] {
            let target = json!({"provider":"jira","deployment":"cloud","tenant":"00000000-1111-2222-3333-444444444444","resource_type":"project","id":"10000"});
            let data = if command == GET {
                json!({"target":target,"project":{"id":"10000","key":"ED","name":"Example","project_type":"software"},"identity_resolution":"exact_id","observed_at":0,"credential_cache_update":null})
            } else {
                json!({"target":target,"items":[{"id":"3","name":"Task","subtask":false,"statuses":[{"id":"1","name":"Open"}]}],"observation":{"observed_at":0,"page_digest":format!("sha256:{}","0".repeat(64))},"cached":false,"credential_cache_update":null})
            };
            let descriptor=OperationDescriptor {
                id:command.into(),provider:"jira".into(),contract_version:SCHEMA_VERSION.into(),api_revision:"jira-cloud-v3".into(),
                description:"Read selected project details or whole issue-type status groups through verified host execution.".into(),effect:EffectClass::Read,
                input_schema:schema_for::<ProjectContextInput>()?,response_schema:if command==GET{response_schema::<ProjectGetData>()?}else{response_schema::<ProjectStatusesData>()?},example:json!({}),response_example:json!(Envelope::success(command,"example".into(),data)),
                projection:Some(projection(command)),scope_alternatives:vec![vec!["read:jira-work".into()]],permissions:vec!["Browse projects evaluated by Jira".into()],
                tenant_conditions:vec!["Locally projected native data; project status membership does not prove issue-transition availability or mutation permission.".into()],
                routes:vec![route(command)],retry_class:RetryClass::SafeRead,reconciliation:ReconciliationClass::NotApplicable,
            };
            registry
                .register_typed_handler::<ProjectContextInput>(descriptor, Arc::new(Handler))?;
        }
        Ok(())
    }
}
pub fn numeric(value: &str) -> bool {
    !value.is_empty() && value.len() <= 512 && value.bytes().all(|byte| byte.is_ascii_digit())
}
pub fn key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value.as_bytes()[0].is_ascii_alphabetic()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}
pub fn validate_target(target: &ResourceRef) -> Result<(), CraftError> {
    target.validate()?;
    crate::projects::validate_target(&ResourceRef {
        resource_type: "site".into(),
        id: target.tenant.clone(),
        ..target.clone()
    })?;
    if target.resource_type != "project" || !(numeric(&target.id) || key(&target.id)) {
        return Err(CraftError::field(
            "/target",
            "expected_exact_jira_project_id_or_case_sensitive_key",
        ));
    }
    Ok(())
}
struct Handler;
impl OperationHandler for Handler {
    fn validate(&self, request: &CanonicalRequest) -> Result<(), CraftError> {
        validate_target(&request.target)?;
        if request.preconditions.is_some()
            || (request.command == GET && request.selection.cursor.is_some())
        {
            return Err(CraftError::field(
                "/selection",
                "unsupported_project_read_precondition_or_get_cursor",
            ));
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
            "Project context requires the verified bounded host.",
        ))
    }
}
fn malformed() -> CraftError {
    let mut error = CraftError::new(
        ErrorCode::RemoteUnavailable,
        "Project context did not establish valid selected metadata and identity.",
    );
    error.details.write_state = craft_core::contracts::RequestState::Failed;
    error
}
fn invoke(
    operation: &PreparedOperation,
    context: &mut dyn OperationContext,
    route: &str,
) -> Result<Value, CraftError> {
    context.checkpoint()?;
    let NativeResponse { status, body, .. } = context.invoke(&NativeCall {
        repeated_query: std::collections::BTreeMap::new(),
        route: route.into(),
        path_parameters: BTreeMap::from([(
            "projectIdOrKey".into(),
            operation.request().target.id.clone(),
        )]),
        query: BTreeMap::new(),
        body: None,
    })?;
    if status != 200 {
        let mut error = CraftError::new(
            match status {
                400 | 422 => ErrorCode::InputInvalid,
                401 => ErrorCode::AuthRequired,
                403 => ErrorCode::PermissionDenied,
                429 => ErrorCode::RateLimited,
                _ => ErrorCode::RemoteUnavailable,
            },
            if status == 404 {
                "Project is absent or invisible; this does not establish absence."
            } else {
                "Project context read failed; no further automatic retry is made."
            },
        );
        error.details.write_state = craft_core::contracts::RequestState::Failed;
        return Err(error);
    }
    craft_core::input::parse_json(
        &serde_json::to_vec(&body).map_err(|_| malformed())?,
        1_048_576,
    )
    .map_err(|_| malformed())
}
/// Reads one project through declared routes and exact selector/field-plan binding, without retries.
pub fn read_get(
    operation: &PreparedOperation,
    context: &mut dyn OperationContext,
    plan: &FieldPlan,
) -> Result<(Value, IdentityResolution), CraftError> {
    context.verify_operation(operation)?;
    plan.verify(operation)?;
    if operation.request().command != GET {
        return Err(CraftError::field("/command", "expected_project_get"));
    }
    let raw = invoke(operation, context, GET_ROUTE)?;
    let id = raw
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| numeric(id))
        .ok_or_else(malformed)?;
    let actual_key = raw
        .get("key")
        .and_then(Value::as_str)
        .filter(|value| key(value))
        .ok_or_else(malformed)?;
    let selected = &operation.request().target.id;
    let resolution = if numeric(selected) {
        if id != selected {
            return Err(malformed());
        }
        IdentityResolution::ExactId
    } else {
        if actual_key != selected {
            return Err(malformed());
        }
        IdentityResolution::ExactKey
    };
    Ok((plan.normalize_project(&raw)?, resolution))
}
/// The API is unpaginated. Returns one terminal native page of complete issue-type groups.
pub fn read_statuses(
    operation: &PreparedOperation,
    context: &mut dyn OperationContext,
    plan: &FieldPlan,
    position: Option<&NativePosition>,
) -> Result<NativePage, CraftError> {
    context.verify_operation(operation)?;
    plan.verify(operation)?;
    if operation.request().command != STATUSES || position.is_some() {
        return Err(CraftError::field(
            "/cursor",
            "expected_initial_unpaginated_project_statuses",
        ));
    }
    let raw = invoke(operation, context, STATUSES_ROUTE)?;
    let groups = raw
        .as_array()
        .filter(|groups| groups.len() <= 1000)
        .ok_or_else(malformed)?;
    let mut ids = std::collections::BTreeSet::new();
    let mut items = Vec::new();
    for group in groups {
        context.checkpoint()?;
        let group = plan.normalize_group(group)?;
        if !ids.insert(group["id"].as_str().ok_or_else(malformed)?.to_owned()) {
            return Err(malformed());
        }
        items.push(group);
    }
    Ok(NativePage {
        context: None,
        items,
        next: None,
    })
}
