//! Typed project-search interpretation. Returned links are normalized by the bound host context.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use craft_core::contracts::{
    CanonicalRequest, CredentialCacheUpdate, EffectClass, Envelope, ResourceRef, SCHEMA_VERSION,
    validate_identity,
};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{
    HttpMethod, NativeCall, NativeRoute, OperationContext, OperationHandler, PreparedOperation,
    RouteSegment,
};
use craft_core::pagination::{NativePage, NativePosition, PageObservation};
use craft_core::projection::{ProjectionContract, ProjectionMode};
use craft_core::registry::{
    OperationDescriptor, ProviderManifest, ProviderModule, ReconciliationClass, Registry,
    RetryClass, response_schema, schema_for,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const COMMAND: &str = "jira.project.list";
pub const ROUTE: &str = "jira-project-search";

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectListInput {
    #[schemars(length(min = 1, max = 256))]
    pub query: Option<String>,
    #[serde(default = "page_size")]
    #[schemars(range(min = 1, max = 100))]
    pub page_size: u16,
}
fn page_size() -> u16 {
    50
}
impl ProjectListInput {
    pub fn validate(&self) -> Result<(), CraftError> {
        if !(1..=100).contains(&self.page_size) {
            return Err(CraftError::field(
                "/page_size",
                "expected_integer_between_1_and_100",
            ));
        }
        if self.query.as_ref().is_some_and(|value| {
            value.is_empty()
                || value.chars().count() > 256
                || value.len() > 1024
                || value.chars().any(char::is_control)
        }) {
            return Err(CraftError::field(
                "/query",
                "expected_bounded_literal_query",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectSummary {
    pub id: String,
    pub key: String,
    pub name: String,
    pub project_type: Option<String>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectPageData {
    pub target: ResourceRef,
    pub items: Vec<ProjectSummary>,
    pub observation: Option<PageObservation>,
    pub cached: bool,
    pub credential_cache_update: Option<CredentialCacheUpdate>,
}

pub struct JiraProjects;
pub fn route() -> NativeRoute {
    NativeRoute {
        repeated_query_parameters: Vec::new(),
        id: ROUTE.into(),
        method: HttpMethod::Get,
        path: ["rest", "api", "3", "project", "search"]
            .map(|part| RouteSegment::Literal(part.into()))
            .into(),
        query_parameters: ["startAt", "maxResults", "orderBy", "action", "query"]
            .map(String::from)
            .into(),
        continuation_parameters: vec!["startAt".into()],
        response_max_bytes: 1_048_576,
    }
}
pub fn projection() -> ProjectionContract {
    ProjectionContract {
        item_pointer: "/items".into(),
        always_returned: vec!["id".into()],
        default_selected: ["id", "key", "name"].map(String::from).into(),
        mode: ProjectionMode::Local,
    }
}
impl ProviderModule for JiraProjects {
    fn manifest(&self) -> ProviderManifest {
        ProviderManifest {
            id: "jira".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            deployments: vec!["cloud".into()],
            contract_version: SCHEMA_VERSION.into(),
        }
    }
    fn register(&self, registry: &mut Registry) -> Result<(), CraftError> {
        registry.register_typed_handler::<ProjectListInput>(OperationDescriptor {
            id:COMMAND.into(),provider:"jira".into(),contract_version:SCHEMA_VERSION.into(),api_revision:"jira-cloud-v3".into(),
            description:"List one bounded native project page through the immutable-cursor host.".into(),effect:EffectClass::Read,
            input_schema:schema_for::<ProjectListInput>()?,response_schema:response_schema::<ProjectPageData>()?,example:json!({"page_size":50}),
            response_example:json!(Envelope::success(COMMAND,"example".into(),json!({"target":{"provider":"jira","deployment":"cloud","tenant":"00000000-1111-2222-3333-444444444444","resource_type":"site","id":"00000000-1111-2222-3333-444444444444"},"items":[{"id":"10000","key":"ED","name":"Example","project_type":"software"}],"observation":{"observed_at":0,"page_digest":format!("sha256:{}","0".repeat(64))},"cached":false,"credential_cache_update":null}))),
            projection:Some(projection()),scope_alternatives:vec![vec!["read:jira-work".into()]],permissions:vec!["Project view permission evaluated by Jira".into()],
            tenant_conditions:vec!["Explicit site and verified actor; visibility does not establish mutation permission.".into()],
            routes:vec![route()],retry_class:RetryClass::SafeRead,reconciliation:ReconciliationClass::NotApplicable,
        },Arc::new(Handler))
    }
}
pub fn validate_target(target: &ResourceRef) -> Result<(), CraftError> {
    target.validate()?;
    let uuid = target.tenant.len() == 36
        && target.tenant.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        });
    if target.provider != "jira"
        || target.deployment != "cloud"
        || target.resource_type != "site"
        || target.id != target.tenant
        || !uuid
    {
        return Err(CraftError::field(
            "/target",
            "expected_exact_jira_cloud_site",
        ));
    }
    Ok(())
}
struct Handler;
impl OperationHandler for Handler {
    fn validate(&self, request: &CanonicalRequest) -> Result<(), CraftError> {
        validate_target(&request.target)?;
        if request.preconditions.is_some() {
            return Err(CraftError::field(
                "/preconditions",
                "project_list_preconditions_unsupported",
            ));
        }
        serde_json::from_value::<ProjectListInput>(request.input.clone())
            .map_err(|_| CraftError::field("/input", "invalid_project_list_input"))?
            .validate()
    }
    fn execute(
        &self,
        _: &PreparedOperation,
        _: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        Err(CraftError::new(
            ErrorCode::UnsupportedCapability,
            "Project listing requires the immutable-cursor host.",
        ))
    }
}
fn malformed() -> CraftError {
    CraftError::new(
        ErrorCode::RemoteUnavailable,
        "Project page did not establish valid records and continuation; no offset was guessed.",
    )
}
fn offset(value: &str) -> Result<u64, CraftError> {
    value
        .parse::<u64>()
        .ok()
        .filter(|offset| *offset <= i64::MAX as u64 && offset.to_string() == value)
        .ok_or_else(malformed)
}
fn optional<'a>(body: &'a Value, name: &str) -> Result<Option<&'a Value>, CraftError> {
    match body.get(name) {
        Some(Value::Null) => Err(malformed()),
        value => Ok(value),
    }
}

pub fn read_page(
    operation: &PreparedOperation,
    context: &mut dyn OperationContext,
    position: Option<&NativePosition>,
) -> Result<NativePage, CraftError> {
    context.verify_operation(operation)?;
    if operation.request().command != COMMAND {
        return Err(CraftError::field("/command", "expected_project_list"));
    }
    validate_target(&operation.request().target)?;
    let input: ProjectListInput =
        serde_json::from_value(operation.request().input.clone()).map_err(|_| malformed())?;
    input.validate()?;
    let start = match position {
        None => 0,
        Some(position) => {
            position.validate()?;
            if position.0.len() != 1 {
                return Err(CraftError::field(
                    "/cursor",
                    "unexpected_project_continuation",
                ));
            }
            offset(position.0.get("startAt").ok_or_else(malformed)?)?
        }
    };
    let mut query = BTreeMap::from([
        ("startAt".into(), start.to_string()),
        ("maxResults".into(), input.page_size.to_string()),
        ("orderBy".into(), "key".into()),
        ("action".into(), "view".into()),
    ]);
    if let Some(value) = input.query {
        query.insert("query".into(), value);
    }
    let call = NativeCall {
        repeated_query: std::collections::BTreeMap::new(),
        route: ROUTE.into(),
        path_parameters: BTreeMap::new(),
        query,
        body: None,
    };
    context.checkpoint()?;
    let response = context.invoke(&call)?;
    if response.status != 200 {
        return Err(CraftError::new(
            match response.status {
                401 => ErrorCode::AuthRequired,
                403 => ErrorCode::PermissionDenied,
                429 => ErrorCode::RateLimited,
                _ => ErrorCode::RemoteUnavailable,
            },
            "Project read failed; no automatic retry was performed.",
        ));
    }
    let body = response.body;
    if serde_json::to_vec(&body).map_err(|_| malformed())?.len() > 1_048_576 || !body.is_object() {
        return Err(malformed());
    }
    let returned_start = body
        .get("startAt")
        .and_then(Value::as_u64)
        .ok_or_else(malformed)?;
    let maximum = body
        .get("maxResults")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0 && *value <= u64::from(input.page_size))
        .ok_or_else(malformed)?;
    let values = body
        .get("values")
        .and_then(Value::as_array)
        .ok_or_else(malformed)?;
    if returned_start != start || values.len() as u64 > maximum {
        return Err(malformed());
    }
    if let Some(total) = optional(&body, "total")?
        && total.as_u64().is_none_or(|value| value > i64::MAX as u64)
    {
        return Err(malformed());
    }
    let last = optional(&body, "isLast")?
        .map(|value| value.as_bool().ok_or_else(malformed))
        .transpose()?;
    let link = optional(&body, "nextPage")?
        .map(|value| {
            value
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or_else(malformed)
        })
        .transpose()?;
    if (last == Some(true) && link.is_some()) || (last != Some(true) && link.is_none()) {
        return Err(malformed());
    }
    let next = if let Some(link) = link {
        // Some deployments clamp the advertised native maximum. It constrains link validation,
        // while subsequent requests keep the originally bound requested page size.
        let mut link_call = call.clone();
        link_call
            .query
            .insert("maxResults".into(), maximum.to_string());
        let normalized = context.normalize_next(&link_call, link)?;
        if normalized.route != call.route
            || normalized.path_parameters != call.path_parameters
            || normalized.body.is_some()
        {
            return Err(malformed());
        }
        let mut stable = normalized.query.clone();
        let next = offset(stable.remove("startAt").as_deref().ok_or_else(malformed)?)?;
        let mut expected = link_call.query.clone();
        expected.remove("startAt");
        if stable != expected || next <= start {
            return Err(CraftError::new(
                ErrorCode::PaginationStalled,
                "Project continuation did not preserve filters and advance its offset.",
            ));
        }
        Some(NativePosition(BTreeMap::from([(
            "startAt".into(),
            next.to_string(),
        )])))
    } else {
        None
    };
    let mut ids = BTreeSet::new();
    let mut keys = BTreeSet::new();
    let mut items = Vec::new();
    for value in values {
        context.checkpoint()?;
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| {
                !id.is_empty() && id.len() <= 512 && id.bytes().all(|byte| byte.is_ascii_digit())
            })
            .ok_or_else(malformed)?;
        let key = value
            .get("key")
            .and_then(Value::as_str)
            .ok_or_else(malformed)?;
        validate_identity("/project/key", key).map_err(|_| malformed())?;
        let name = value
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| {
                !name.is_empty() && name.len() <= 4096 && !name.chars().any(char::is_control)
            })
            .ok_or_else(malformed)?;
        let project_type = optional(value, "projectTypeKey")?
            .map(|value| {
                value
                    .as_str()
                    .filter(|value| {
                        !value.is_empty()
                            && value.len() <= 128
                            && !value.chars().any(char::is_control)
                    })
                    .ok_or_else(malformed)
            })
            .transpose()?;
        if !ids.insert(id) || !keys.insert(key) {
            return Err(malformed());
        }
        items.push(
            serde_json::to_value(ProjectSummary {
                id: id.into(),
                key: key.into(),
                name: name.into(),
                project_type: project_type.map(String::from),
            })
            .map_err(|_| malformed())?,
        );
    }
    Ok(NativePage {
        context: None,
        items,
        next,
    })
}
