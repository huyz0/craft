//! Link IDs and endpoint roles come from native records; URLs and unrelated issue fields are inert.
use super::*;
use craft_core::contracts::{CredentialCacheUpdate, ResourceRef};
use craft_core::pagination::{NativePage, NativePosition, PageObservation};
use craft_core::projection::{ProjectionContract, ProjectionMode};
pub const GET: &str = "jira.link.get";
pub const LIST: &str = "jira.link.list";
pub const LINK_ROUTE: &str = "get-issue-link";
pub const ISSUE_ROUTE: &str = "issue-links-field";
pub mod inspection;
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LinkGetInput {
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub link_id: String,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LinkListInput {
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub issue_id: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LinkRecord {
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub id: String,
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub inward_issue_id: String,
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub outward_issue_id: String,
    pub link_type: LinkType,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LinkGetData {
    pub target: ResourceRef,
    pub link: Option<LinkRecord>,
    pub observation: Option<PageObservation>,
    pub credential_cache_update: Option<CredentialCacheUpdate>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LinkListData {
    pub target: ResourceRef,
    pub items: Vec<LinkRecord>,
    pub source_issue_id: String,
    pub observation: Option<PageObservation>,
    pub cached: bool,
    pub credential_cache_update: Option<CredentialCacheUpdate>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TypesData {
    pub target: ResourceRef,
    pub items: Vec<LinkType>,
    pub observation: Option<PageObservation>,
    pub cached: bool,
    pub credential_cache_update: Option<CredentialCacheUpdate>,
}
pub fn numeric(value: &str) -> bool {
    value
        .parse::<i64>()
        .is_ok_and(|n| n > 0 && n.to_string() == value)
}
pub fn parse_record(raw: &Value, source: Option<&str>) -> Result<LinkRecord, CraftError> {
    if ["errors", "errorMessages", "warningMessages"]
        .iter()
        .any(|key| raw.get(key).is_some())
    {
        return Err(malformed());
    }
    let id = raw
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| numeric(id))
        .ok_or_else(malformed)?;
    let endpoint = |name: &str| -> Result<Option<String>, CraftError> {
        raw.get(name)
            .map(|v| {
                v.get("id")
                    .and_then(Value::as_str)
                    .filter(|id| numeric(id))
                    .map(String::from)
                    .ok_or_else(malformed)
            })
            .transpose()
    };
    let (inward, outward) = match (endpoint("inwardIssue")?, endpoint("outwardIssue")?, source) {
        (Some(i), Some(o), source) if i != o && source.is_none_or(|s| s == i || s == o) => (i, o),
        (Some(i), None, Some(s)) if i != s => (i, s.into()),
        (None, Some(o), Some(s)) if o != s => (s.into(), o),
        _ => return Err(malformed()),
    };
    let mut types =
        parse_types(&json!({"issueLinkTypes":[raw.get("type").ok_or_else(malformed)?]}))?.items;
    Ok(LinkRecord {
        id: id.into(),
        inward_issue_id: inward,
        outward_issue_id: outward,
        link_type: types.pop().ok_or_else(malformed)?,
    })
}
pub fn parse_issue_links(body: &Value, source: &str) -> Result<Vec<LinkRecord>, CraftError> {
    if body["id"] != source
        || [
            "errors",
            "errorMessages",
            "warningMessages",
            "nextPage",
            "nextPageToken",
        ]
        .iter()
        .any(|k| body.get(k).is_some())
    {
        return Err(malformed());
    }
    let rows = body["fields"]["issuelinks"]
        .as_array()
        .filter(|rows| rows.len() <= 1024)
        .ok_or_else(malformed)?;
    let mut ids = BTreeSet::new();
    let mut links = Vec::new();
    for row in rows {
        let link = parse_record(row, Some(source))?;
        if !ids.insert(link.id.clone()) {
            return Err(malformed());
        }
        links.push(link);
    }
    Ok(links)
}
pub fn projection(command: &str) -> ProjectionContract {
    let types = command == TYPES;
    ProjectionContract {
        item_pointer: if command == GET { "/link" } else { "/items" }.into(),
        always_returned: if types {
            vec!["id".into()]
        } else {
            ["id", "inward_issue_id", "outward_issue_id", "link_type.id"]
                .map(String::from)
                .into()
        },
        default_selected: if types {
            ["id", "name", "inward", "outward"].map(String::from).into()
        } else {
            ["id", "inward_issue_id", "outward_issue_id", "link_type"]
                .map(String::from)
                .into()
        },
        mode: ProjectionMode::Local,
    }
}
fn validate(request: &CanonicalRequest) -> Result<(), CraftError> {
    crate::projects::validate_target(&request.target)?;
    if request.preconditions.is_some()
        || (request.command == GET
            && (request.selection.cursor.is_some() || request.selection.limit != 20))
        || (request.command != GET && !(1..=100).contains(&request.selection.limit))
    {
        return Err(CraftError::field(
            "/request",
            "invalid_link_read_paging_or_preconditions",
        ));
    }
    let id = match request.command.as_str() {
        GET => {
            serde_json::from_value::<LinkGetInput>(request.input.clone())
                .map_err(|_| malformed())?
                .link_id
        }
        LIST => {
            serde_json::from_value::<LinkListInput>(request.input.clone())
                .map_err(|_| malformed())?
                .issue_id
        }
        TYPES => {
            serde_json::from_value::<TypesInput>(request.input.clone()).map_err(|_| malformed())?;
            return Ok(());
        }
        _ => return Err(CraftError::field("/command", "expected_link_read")),
    };
    if !numeric(&id) {
        return Err(CraftError::field(
            "/id",
            "expected_numeric_link_or_issue_id",
        ));
    }
    Ok(())
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
            "Link reads require the verified buffered host.",
        ))
    }
}
pub fn routes(command: &str) -> Vec<NativeRoute> {
    let (id, parts, parameter, queries) = match command {
        GET => (
            LINK_ROUTE,
            vec!["rest", "api", "3", "issueLink"],
            Some("linkId"),
            vec![],
        ),
        LIST => (
            ISSUE_ROUTE,
            vec!["rest", "api", "3", "issue"],
            Some("issueIdOrKey"),
            vec!["fields", "updateHistory", "failFast"],
        ),
        _ => (
            super::ROUTE,
            vec!["rest", "api", "3", "issueLinkType"],
            None,
            vec![],
        ),
    };
    let mut path: Vec<_> = parts
        .into_iter()
        .map(|s| RouteSegment::Literal(s.into()))
        .collect();
    if let Some(p) = parameter {
        path.push(RouteSegment::Parameter(p.into()));
    }
    vec![NativeRoute {
        id: id.into(),
        method: HttpMethod::Get,
        path,
        query_parameters: queries.into_iter().map(String::from).collect(),
        continuation_parameters: vec![],
        repeated_query_parameters: vec![],
        response_max_bytes: 1_048_576,
    }]
}
pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    for command in [GET, LIST, TYPES] {
        let (input_schema, response_schema, example) = match command {
            GET => (
                schema_for::<LinkGetInput>()?,
                response_schema::<LinkGetData>()?,
                json!({"link_id":"10001"}),
            ),
            LIST => (
                schema_for::<LinkListInput>()?,
                response_schema::<LinkListData>()?,
                json!({"issue_id":"10000"}),
            ),
            _ => (
                schema_for::<TypesInput>()?,
                response_schema::<TypesData>()?,
                json!({}),
            ),
        };
        let target = json!({"provider":"jira","deployment":"cloud","tenant":"00000000-1111-2222-3333-444444444444","resource_type":"site","id":"00000000-1111-2222-3333-444444444444"});
        let mut data = if command == GET {
            json!({"target":target,"link":null,"observation":null,"credential_cache_update":null})
        } else {
            json!({"target":target,"items":[],"observation":null,"cached":false,"credential_cache_update":null})
        };
        if command == LIST {
            data["source_issue_id"] = json!("10000");
        }
        let descriptor=OperationDescriptor {id:command.into(),provider:"jira".into(),contract_version:SCHEMA_VERSION.into(),api_revision:"jira-cloud-v3".into(),description:match command {GET=>"Read one current native link and exact endpoint roles.",LIST=>"List observed issue links with field selection and local opaque continuation.",_=>"List native link types with bounded local continuation."}.into(),effect:EffectClass::Read,input_schema,response_schema,example,response_example:json!(Envelope::success(command,"example".into(),data)),projection:Some(projection(command)),scope_alternatives:vec![vec!["read:jira-work".into()]],permissions:vec!["Native Browse projects and issue visibility; no mutation or attribution authority.".into()],tenant_conditions:vec!["Lists describe observed native fields, not hidden relations or an atomic snapshot.".into()],routes:routes(command),retry_class:RetryClass::SafeRead,reconciliation:ReconciliationClass::NotApplicable};
        match command {
            GET => {
                registry.register_typed_handler::<LinkGetInput>(descriptor, Arc::new(Handler))?
            }
            LIST => {
                registry.register_typed_handler::<LinkListInput>(descriptor, Arc::new(Handler))?
            }
            _ => registry.register_typed_handler::<TypesInput>(descriptor, Arc::new(Handler))?,
        };
    }
    Ok(())
}
pub fn call(operation: &PreparedOperation) -> Result<NativeCall, CraftError> {
    validate(operation.request())?;
    let command = operation.request().command.as_str();
    let (route, path_parameters, query) = match command {
        GET => (
            LINK_ROUTE,
            BTreeMap::from([(
                "linkId".into(),
                operation.request().input["link_id"]
                    .as_str()
                    .ok_or_else(malformed)?
                    .into(),
            )]),
            BTreeMap::new(),
        ),
        LIST => (
            ISSUE_ROUTE,
            BTreeMap::from([(
                "issueIdOrKey".into(),
                operation.request().input["issue_id"]
                    .as_str()
                    .ok_or_else(malformed)?
                    .into(),
            )]),
            BTreeMap::from([
                ("fields".into(), "issuelinks".into()),
                ("updateHistory".into(), "false".into()),
                ("failFast".into(), "true".into()),
            ]),
        ),
        _ => (super::ROUTE, BTreeMap::new(), BTreeMap::new()),
    };
    Ok(NativeCall {
        route: route.into(),
        path_parameters,
        query,
        repeated_query: BTreeMap::new(),
        body: None,
    })
}
pub fn read_page(
    operation: &PreparedOperation,
    context: &mut dyn OperationContext,
    position: Option<&NativePosition>,
) -> Result<NativePage, CraftError> {
    if position.is_some() {
        return Err(CraftError::field(
            "/continuation",
            "link_field_has_no_native_continuation",
        ));
    }
    context.verify_operation(operation)?;
    context.checkpoint()?;
    let response = context.invoke(&call(operation)?)?;
    context.checkpoint()?;
    if response.status != 200 {
        return Err(CraftError::new(
            match response.status {
                401 => ErrorCode::AuthRequired,
                403 => ErrorCode::PermissionDenied,
                429 => ErrorCode::RateLimited,
                _ => ErrorCode::RemoteUnavailable,
            },
            "Link read did not establish visible current evidence; no retry was performed.",
        ));
    }
    if serde_json::to_vec(&response.body)
        .map_err(|_| malformed())?
        .len()
        > 1_048_576
    {
        return Err(malformed());
    }
    let command = operation.request().command.as_str();
    let items = match command {
        GET => {
            let link = parse_record(&response.body, None)?;
            if response.body["id"] != operation.request().input["link_id"] {
                return Err(malformed());
            }
            vec![serde_json::to_value(link).map_err(|_| malformed())?]
        }
        LIST => parse_issue_links(
            &response.body,
            operation.request().input["issue_id"]
                .as_str()
                .ok_or_else(malformed)?,
        )?
        .into_iter()
        .map(serde_json::to_value)
        .collect::<Result<_, _>>()
        .map_err(|_| malformed())?,
        _ => parse_types(&response.body)?
            .items
            .into_iter()
            .map(serde_json::to_value)
            .collect::<Result<_, _>>()
            .map_err(|_| malformed())?,
    };
    Ok(NativePage {
        context: if command == LIST {
            Some(
                json!({"source_issue_id":operation.request().input["issue_id"],"coverage":"observed_issue_field"}),
            )
        } else {
            None
        },
        items,
        next: None,
    })
}
