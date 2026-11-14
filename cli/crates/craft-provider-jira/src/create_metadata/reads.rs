use std::collections::{BTreeMap, BTreeSet};

use craft_core::contracts::{CanonicalRequest, EffectClass, Envelope, SCHEMA_VERSION};
use craft_core::execution::{HttpMethod, NativeRoute, OperationHandler, RouteSegment};
use craft_core::projection::{ProjectionContract, ProjectionMode};
use craft_core::registry::{
    OperationDescriptor, ProviderManifest, ProviderModule, ReconciliationClass, Registry,
    RetryClass, response_schema, schema_for,
};

use super::*;

pub struct JiraMetadata {
    clock: Arc<dyn Clock>,
}
impl JiraMetadata {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self { clock }
    }
}

pub fn routes() -> Vec<NativeRoute> {
    let literal = |value: &str| RouteSegment::Literal(value.into());
    let parameter = |value: &str| RouteSegment::Parameter(value.into());
    let project = vec![
        literal("rest"),
        literal("api"),
        literal("3"),
        literal("project"),
        parameter("projectIdOrKey"),
    ];
    let types = vec![
        literal("rest"),
        literal("api"),
        literal("3"),
        literal("issue"),
        literal("createmeta"),
        parameter("projectIdOrKey"),
        literal("issuetypes"),
    ];
    let mut fields = types.clone();
    fields.push(parameter("issueTypeId"));
    [
        ("metadata-project", project, false),
        ("metadata-types", types, true),
        ("metadata-fields", fields, true),
    ]
    .into_iter()
    .map(|(id, path, paged)| NativeRoute {
        repeated_query_parameters: Vec::new(),
        id: id.into(),
        method: HttpMethod::Get,
        path,
        query_parameters: if paged {
            vec!["startAt".into(), "maxResults".into()]
        } else {
            vec![]
        },
        continuation_parameters: vec![],
        response_max_bytes: 1_048_576,
    })
    .chain(std::iter::once(NativeRoute {
        repeated_query_parameters: Vec::new(),
        id: "metadata-parent".into(),
        method: HttpMethod::Get,
        path: vec![
            literal("rest"),
            literal("api"),
            literal("3"),
            literal("issue"),
            parameter("issueIdOrKey"),
        ],
        query_parameters: ["fields", "failFast", "updateHistory"]
            .map(String::from)
            .into(),
        continuation_parameters: vec![],
        response_max_bytes: 1_048_576,
    }))
    .collect()
}

impl ProviderModule for JiraMetadata {
    fn manifest(&self) -> ProviderManifest {
        ProviderManifest {
            id: "jira".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            deployments: vec!["cloud".into()],
            contract_version: SCHEMA_VERSION.into(),
        }
    }
    fn register(&self, registry: &mut Registry) -> Result<(), CraftError> {
        let output = json!({"target":{"provider":"jira","deployment":"cloud","tenant":"00000000-1111-2222-3333-444444444444","resource_type":"project","id":"10000"},"project_key":"ED","issue_type_id":"10001","issue_type_name":"Task","subtask":false,"observed_at":0,"fields":[],"complete":true,"mutation_authorized":false});
        registry.register_typed_handler::<MetadataRequest>(
            OperationDescriptor {
                id: COMMAND.into(),
                provider: "jira".into(),
                contract_version: SCHEMA_VERSION.into(),
                api_revision: "jira-cloud-v3".into(),
                description: "Read bounded project/type create metadata; no mutation authority."
                    .into(),
                effect: EffectClass::Read,
                input_schema: schema_for::<MetadataRequest>()?,
                response_schema: response_schema::<MetadataSummary>()?,
                example: json!({"issue_type_id":"10001","page_size":50}),
                response_example: json!(Envelope::success(COMMAND, "example".into(), output)),
                projection: Some(ProjectionContract {
                    item_pointer: "".into(),
                    always_returned: [
                        "target",
                        "issue_type_id",
                        "subtask",
                        "parent",
                        "observed_at",
                        "complete",
                        "mutation_authorized",
                    ]
                    .map(String::from)
                    .into(),
                    default_selected: [
                        "target",
                        "project_key",
                        "issue_type_id",
                        "issue_type_name",
                        "subtask",
                        "parent",
                        "observed_at",
                        "fields.id",
                        "fields.required",
                        "fields.has_default",
                        "fields.settable",
                        "fields.unsupported_reason",
                        "complete",
                        "mutation_authorized",
                    ]
                    .map(String::from)
                    .into(),
                    mode: ProjectionMode::Local,
                }),
                scope_alternatives: vec![vec!["read:jira-work".into()]],
                permissions: vec!["Browse projects".into(), "Create issues".into()],
                tenant_conditions: vec![
                    "Exact project/type context; pagination is not snapshot isolation.".into(),
                ],
                routes: routes(),
                retry_class: RetryClass::SafeRead,
                reconciliation: ReconciliationClass::NotApplicable,
            },
            Arc::new(Handler {
                clock: self.clock.clone(),
            }),
        )?;
        accounts::register(registry, self.clock.clone())
    }
}
struct Handler {
    clock: Arc<dyn Clock>,
}
impl OperationHandler for Handler {
    fn validate(&self, request: &CanonicalRequest) -> Result<(), CraftError> {
        crate::created_issue::validate_target(&request.target)?;
        serde_json::from_value::<MetadataRequest>(request.input.clone())
            .map_err(|_| CraftError::field("/input", "invalid_metadata_request"))?
            .validate()
    }
    fn execute(
        &self,
        operation: &PreparedOperation,
        context: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        let input =
            serde_json::from_value(operation.request().input.clone()).map_err(|_| malformed())?;
        let output = fetch(operation, context, &input, self.clock.unix_millis())?;
        serde_json::to_value(output).map_err(|_| malformed())
    }
}

fn invoke(
    context: &mut dyn OperationContext,
    call: &NativeCall,
    bytes: &mut usize,
) -> Result<Value, CraftError> {
    context.checkpoint()?;
    let response = context.invoke(call)?;
    if response.status != 200 {
        let code = match response.status {
            401 => ErrorCode::AuthRequired,
            403 => ErrorCode::PermissionDenied,
            429 => ErrorCode::RateLimited,
            _ => ErrorCode::RemoteUnavailable,
        };
        return Err(CraftError::new(
            code,
            "Create metadata read did not return a qualified response; no automatic retry was performed.",
        ));
    }
    let size = serde_json::to_vec(&response.body)
        .map_err(|_| malformed())?
        .len();
    *bytes = bytes
        .checked_add(size)
        .filter(|size| *size <= MAX_BYTES)
        .ok_or_else(malformed)?;
    Ok(response.body)
}

fn pages(
    context: &mut dyn OperationContext,
    base: NativeCall,
    key: &str,
    page_size: u16,
    bytes: &mut usize,
) -> Result<Vec<Value>, CraftError> {
    let mut values = Vec::new();
    let mut offset = 0u64;
    let mut total = None;
    for _ in 0..MAX_PAGES {
        let mut call = base.clone();
        call.query = BTreeMap::from([
            ("startAt".into(), offset.to_string()),
            ("maxResults".into(), page_size.to_string()),
        ]);
        let body = invoke(context, &call, bytes)?;
        let start = body
            .get("startAt")
            .and_then(Value::as_u64)
            .ok_or_else(malformed)?;
        let count = body
            .get("maxResults")
            .and_then(Value::as_u64)
            .filter(|count| *count > 0 && *count <= u64::from(page_size))
            .ok_or_else(malformed)?;
        let observed_total = body
            .get("total")
            .and_then(Value::as_u64)
            .filter(|total| *total <= MAX_ITEMS as u64)
            .ok_or_else(malformed)?;
        let entries = body
            .get(key)
            .or_else(|| {
                if key == "fields" {
                    body.get("results")
                } else {
                    None
                }
            })
            .and_then(Value::as_array)
            .ok_or_else(malformed)?;
        if key == "fields"
            && body
                .get("fields")
                .zip(body.get("results"))
                .is_some_and(|(a, b)| a != b)
        {
            return Err(malformed());
        }
        if start != offset
            || total.is_some_and(|total| total != observed_total)
            || entries.len() as u64 > count
        {
            return Err(malformed());
        }
        total = Some(observed_total);
        offset = offset
            .checked_add(entries.len() as u64)
            .filter(|next| *next <= observed_total)
            .ok_or_else(malformed)?;
        values.extend_from_slice(entries);
        if offset == observed_total {
            return Ok(values);
        }
        if entries.is_empty() {
            return Err(malformed());
        }
    }
    Err(malformed())
}

pub(super) fn fetch(
    operation: &PreparedOperation,
    context: &mut dyn OperationContext,
    input: &MetadataRequest,
    observed_at: u64,
) -> Result<MetadataSummary, CraftError> {
    let target = &operation.request().target;
    let parameters = BTreeMap::from([("projectIdOrKey".into(), target.id.clone())]);
    let mut bytes = 0;
    let project = invoke(
        context,
        &NativeCall {
            repeated_query: std::collections::BTreeMap::new(),
            route: "metadata-project".into(),
            path_parameters: parameters.clone(),
            query: BTreeMap::new(),
            body: None,
        },
        &mut bytes,
    )?;
    if project.get("id").and_then(Value::as_str) != Some(target.id.as_str()) {
        return Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "Project metadata does not match the selected project ID.",
        ));
    }
    let project_key = project
        .get("key")
        .and_then(Value::as_str)
        .filter(|key| !key.is_empty() && key.len() <= 512)
        .ok_or_else(malformed)?
        .to_owned();
    let types = pages(
        context,
        NativeCall {
            repeated_query: std::collections::BTreeMap::new(),
            route: "metadata-types".into(),
            path_parameters: parameters.clone(),
            query: BTreeMap::new(),
            body: None,
        },
        "issueTypes",
        input.page_size,
        &mut bytes,
    )?;
    let mut ids = BTreeSet::new();
    let mut selected = None;
    for kind in types {
        let id = kind
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| numeric(id))
            .ok_or_else(malformed)?;
        if !ids.insert(id.to_owned()) {
            return Err(malformed());
        }
        if id == input.issue_type_id {
            let name = kind
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty() && name.len() <= 512)
                .ok_or_else(malformed)?
                .to_owned();
            let subtask = kind
                .get("subtask")
                .and_then(Value::as_bool)
                .ok_or_else(malformed)?;
            selected = Some((name, subtask));
        }
    }
    let (issue_type_name, subtask) = selected.ok_or_else(|| {
        CraftError::field("/issue_type_id", "issue_type_not_returned_for_project")
    })?;
    let mut parameters = parameters;
    parameters.insert("issueTypeId".into(), input.issue_type_id.clone());
    let fields = pages(
        context,
        NativeCall {
            repeated_query: std::collections::BTreeMap::new(),
            route: "metadata-fields".into(),
            path_parameters: parameters,
            query: BTreeMap::new(),
            body: None,
        },
        "fields",
        input.page_size,
        &mut bytes,
    )?;
    let parent = if let Some(id) = &input.parent_issue_id {
        if !subtask {
            return Err(CraftError::new(
                ErrorCode::UnsupportedCapability,
                "Parent qualification currently supports subtasks only.",
            ));
        }
        let issue = invoke(
            context,
            &NativeCall {
                repeated_query: std::collections::BTreeMap::new(),
                route: "metadata-parent".into(),
                path_parameters: BTreeMap::from([("issueIdOrKey".into(), id.clone())]),
                query: BTreeMap::from([
                    ("fields".into(), "project,issuetype".into()),
                    ("failFast".into(), "true".into()),
                    ("updateHistory".into(), "false".into()),
                ]),
                body: None,
            },
            &mut bytes,
        )?;
        if issue.get("id").and_then(Value::as_str) != Some(id.as_str())
            || issue.pointer("/fields/project/id").and_then(Value::as_str)
                != Some(target.id.as_str())
            || issue
                .pointer("/fields/issuetype/subtask")
                .and_then(Value::as_bool)
                != Some(false)
            || !issue
                .pointer("/fields/issuetype/id")
                .and_then(Value::as_str)
                .is_some_and(numeric)
        {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Parent response does not qualify the exact visible non-subtask issue in the selected project.",
            ));
        }
        Some(ResourceRef {
            resource_type: "issue".into(),
            id: id.clone(),
            ..target.clone()
        })
    } else {
        None
    };
    let mut ids = BTreeSet::new();
    let mut normalized = Vec::new();
    for value in fields {
        let field = fields::parse(
            value,
            &target.id,
            &input.issue_type_id,
            parent.as_ref().map(|value| value.id.as_str()),
        )?;
        if !ids.insert(field.id.clone()) {
            return Err(malformed());
        }
        normalized.push(field);
    }
    context.checkpoint()?;
    Ok(MetadataSummary {
        target: target.clone(),
        project_key,
        issue_type_id: input.issue_type_id.clone(),
        issue_type_name,
        subtask,
        parent,
        observed_at,
        fields: normalized,
        complete: true,
        mutation_authorized: false,
    })
}
