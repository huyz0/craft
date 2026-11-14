//! Issue-bound comment data remains inert; native visibility never becomes workflow authority.
use craft_core::contracts::{
    CanonicalRequest, CredentialCacheUpdate, EffectClass, Envelope, ResourceRef, SCHEMA_VERSION,
};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{
    HttpMethod, NativeCall, NativeRoute, OperationContext, OperationHandler, PreparedOperation,
    RouteSegment,
};
use craft_core::input::{canonical_digest, parse_document_json};
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

pub mod adf;
pub mod mutation;

pub const GET: &str = "jira.comment.get";
pub const LIST: &str = "jira.comment.list";
pub const GET_ROUTE: &str = "jira-comment-get";
pub const LIST_ROUTE: &str = "jira-comment-list";
const RESPONSE_MAX: usize = 4_194_304;
const BODY_MAX: usize = 262_144;
#[cfg(test)]
mod tests;

pub fn numeric(value: &str) -> bool {
    value
        .parse::<i64>()
        .is_ok_and(|v| v > 0 && v.to_string() == value)
}
fn malformed() -> CraftError {
    CraftError::new(
        ErrorCode::RemoteUnavailable,
        "Native comment evidence is malformed or inconsistent; no retry was performed.",
    )
}
fn page_size() -> u16 {
    25
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetInput {
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub issue_id: String,
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub comment_id: String,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListInput {
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub issue_id: String,
    #[serde(default = "page_size")]
    #[schemars(range(min = 1, max = 100))]
    pub page_size: u16,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VisibilityType {
    Group,
    Role,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Visibility {
    #[serde(rename = "type")]
    pub kind: VisibilityType,
    pub identifier: Option<String>,
    pub value: Option<String>,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BodyState {
    NotRequested,
    NotReturned,
    Returned,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentRecord {
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub id: String,
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub issue_id: String,
    pub created: Option<String>,
    pub updated: Option<String>,
    pub author_account_id: Option<String>,
    pub update_author_account_id: Option<String>,
    pub visibility: Option<Visibility>,
    pub jsd_public: Option<bool>,
    pub jsd_author_can_see_request: Option<bool>,
    pub body_state: BodyState,
    pub body: Option<Value>,
    #[schemars(length(min = 71, max = 71), regex(pattern = "^sha256:[0-9a-f]{64}$"))]
    pub revision: Option<String>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetData {
    pub target: ResourceRef,
    pub comment: Option<CommentRecord>,
    pub observation: Option<PageObservation>,
    pub credential_cache_update: Option<CredentialCacheUpdate>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListData {
    pub target: ResourceRef,
    pub items: Vec<CommentRecord>,
    pub source_issue_id: String,
    pub observation: Option<PageObservation>,
    pub cached: bool,
    pub credential_cache_update: Option<CredentialCacheUpdate>,
}
fn optional_text(raw: &Value, path: &str, max: usize) -> Result<Option<String>, CraftError> {
    raw.pointer(path)
        .filter(|v| !v.is_null())
        .map(|v| {
            v.as_str()
                .filter(|s| !s.is_empty() && s.len() <= max && !s.chars().any(char::is_control))
                .map(str::to_owned)
                .ok_or_else(malformed)
        })
        .transpose()
}
fn optional_bool(raw: &Value, key: &str) -> Result<Option<bool>, CraftError> {
    raw.get(key)
        .filter(|v| !v.is_null())
        .map(|v| v.as_bool().ok_or_else(malformed))
        .transpose()
}
pub fn parse_record(
    raw: &Value,
    issue: &str,
    body_selected: bool,
) -> Result<CommentRecord, CraftError> {
    if !raw.is_object()
        || ["errors", "errorMessages", "warningMessages"]
            .iter()
            .any(|k| raw.get(k).is_some())
    {
        return Err(malformed());
    }
    let id = raw["id"]
        .as_str()
        .filter(|s| numeric(s))
        .ok_or_else(malformed)?
        .to_owned();
    if raw
        .get("issueId")
        .is_some_and(|v| v.as_str() != Some(issue))
    {
        return Err(malformed());
    }
    let visibility: Option<Visibility> = raw
        .get("visibility")
        .filter(|v| !v.is_null())
        .map(|v| serde_json::from_value(v.clone()).map_err(|_| malformed()))
        .transpose()?;
    if let Some(v) = &visibility
        && (v.identifier.is_none() && v.value.is_none()
            || [v.identifier.as_ref(), v.value.as_ref()]
                .iter()
                .flatten()
                .any(|s| s.is_empty() || s.len() > 512 || s.chars().any(char::is_control)))
    {
        return Err(malformed());
    }
    let body = raw.get("body").filter(|v| !v.is_null());
    if let Some(body) = body {
        let bytes = serde_json::to_vec(body).map_err(|_| malformed())?;
        parse_document_json(&bytes, BODY_MAX).map_err(|_| malformed())?;
        if body["type"] != "doc" || body["version"] != 1 || !body["content"].is_array() {
            return Err(malformed());
        }
    }
    let mut record = CommentRecord {
        id,
        issue_id: issue.into(),
        created: optional_text(raw, "/created", 128)?,
        updated: optional_text(raw, "/updated", 128)?,
        author_account_id: optional_text(raw, "/author/accountId", 512)?,
        update_author_account_id: optional_text(raw, "/updateAuthor/accountId", 512)?,
        visibility,
        jsd_public: optional_bool(raw, "jsdPublic")?,
        jsd_author_can_see_request: optional_bool(raw, "jsdAuthorCanSeeRequest")?,
        body_state: if !body_selected {
            BodyState::NotRequested
        } else if body.is_none() {
            BodyState::NotReturned
        } else {
            BodyState::Returned
        },
        body: if body_selected { body.cloned() } else { None },
        revision: None,
    };
    if let Some(body) = body {
        record.revision = Some(canonical_digest(
            &json!({"id":record.id,"issue_id":issue,"body":body,"visibility":record.visibility,"jsd_public":record.jsd_public,"jsd_author_can_see_request":record.jsd_author_can_see_request,"author":record.author_account_id,"update_author":record.update_author_account_id,"created":record.created,"updated":record.updated}),
        )?);
    }
    Ok(record)
}

pub fn validate(request: &CanonicalRequest) -> Result<(), CraftError> {
    crate::projects::validate_target(&request.target)?;
    if request.preconditions.is_some() {
        return Err(CraftError::field(
            "/preconditions",
            "unsupported_comment_read_preconditions",
        ));
    }
    let issue = match request.command.as_str() {
        GET => {
            let v: GetInput = serde_json::from_value(request.input.clone())
                .map_err(|_| CraftError::field("/input", "invalid_comment_get"))?;
            if !numeric(&v.comment_id) || request.selection.cursor.is_some() {
                return Err(CraftError::field(
                    "/comment_id",
                    "expected_numeric_comment_without_cursor",
                ));
            }
            v.issue_id
        }
        LIST => {
            let v: ListInput = serde_json::from_value(request.input.clone())
                .map_err(|_| CraftError::field("/input", "invalid_comment_list"))?;
            if !(1..=100).contains(&v.page_size) {
                return Err(CraftError::field(
                    "/page_size",
                    "expected_one_to_one_hundred",
                ));
            }
            v.issue_id
        }
        _ => return Err(CraftError::field("/command", "expected_comment_read")),
    };
    if !numeric(&issue) {
        return Err(CraftError::field(
            "/issue_id",
            "expected_canonical_positive_issue_id",
        ));
    }
    Ok(())
}
pub fn projection(command: &str) -> ProjectionContract {
    ProjectionContract {
        item_pointer: if command == GET { "/comment" } else { "/items" }.into(),
        always_returned: [
            "id",
            "issue_id",
            "visibility",
            "jsd_public",
            "jsd_author_can_see_request",
            "body_state",
            "revision",
        ]
        .map(String::from)
        .into(),
        default_selected: [
            "id",
            "issue_id",
            "created",
            "updated",
            "visibility",
            "jsd_public",
            "jsd_author_can_see_request",
            "body_state",
            "revision",
        ]
        .map(String::from)
        .into(),
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
            "Comment reads require the verified buffered host.",
        ))
    }
}
pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    for command in [GET, LIST] {
        let mut path = ["rest", "api", "3", "issue"]
            .map(|s| RouteSegment::Literal(s.into()))
            .to_vec();
        path.extend([
            RouteSegment::Parameter("issueIdOrKey".into()),
            RouteSegment::Literal("comment".into()),
        ]);
        if command == GET {
            path.push(RouteSegment::Parameter("id".into()));
        }
        let route = NativeRoute {
            id: if command == GET {
                GET_ROUTE
            } else {
                LIST_ROUTE
            }
            .into(),
            method: HttpMethod::Get,
            path,
            query_parameters: if command == LIST {
                vec!["startAt".into(), "maxResults".into(), "orderBy".into()]
            } else {
                vec![]
            },
            continuation_parameters: if command == LIST {
                vec!["startAt".into()]
            } else {
                vec![]
            },
            repeated_query_parameters: vec![],
            response_max_bytes: RESPONSE_MAX as u32,
        };
        let target = json!({"provider":"jira","deployment":"cloud","tenant":"00000000-1111-2222-3333-444444444444","resource_type":"site","id":"00000000-1111-2222-3333-444444444444"});
        let descriptor = OperationDescriptor {
            id:command.into(),provider:"jira".into(),contract_version:SCHEMA_VERSION.into(),api_revision:"jira-cloud-v3-comment-read-v1".into(),
            description: if command==GET {"Read one issue-bound comment with native visibility and selected inert ADF."} else {"List visible issue comments with opaque paging, native privacy evidence and optional inert ADF."}.into(), effect:EffectClass::Read,
            input_schema: if command==GET {schema_for::<GetInput>()?} else {schema_for::<ListInput>()?}, response_schema: if command==GET {response_schema::<GetData>()?} else {response_schema::<ListData>()?},
            example: if command==GET {json!({"issue_id":"10000","comment_id":"10001"})} else {json!({"issue_id":"10000","page_size":25})},
            response_example:json!(Envelope::success(command,"example".into(),if command==GET {json!({"target":target,"comment":null,"observation":null,"credential_cache_update":null})} else {json!({"target":target,"items":[],"source_issue_id":"10000","observation":null,"cached":false,"credential_cache_update":null})})),
            projection:Some(projection(command)),scope_alternatives:vec![vec!["read:jira-work".into()]],permissions:vec!["Native Browse projects, issue security and comment visibility membership.".into()],
            tenant_conditions:vec!["Visible comments are not a complete access inventory or human approval; ordering is live, not a snapshot.".into()],routes:vec![route],retry_class:RetryClass::SafeRead,reconciliation:ReconciliationClass::NotApplicable,
        };
        if command == GET {
            registry.register_typed_handler::<GetInput>(descriptor, Arc::new(Handler))?;
        } else {
            registry.register_typed_handler::<ListInput>(descriptor, Arc::new(Handler))?;
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
    let get = operation.request().command == GET;
    let issue = operation.request().input["issue_id"]
        .as_str()
        .ok_or_else(malformed)?;
    let mut params = BTreeMap::from([("issueIdOrKey".into(), issue.into())]);
    let mut query = BTreeMap::new();
    let start = if let Some(position) = position {
        if get || position.0.len() != 1 {
            return Err(malformed());
        }
        position
            .0
            .get("start_at")
            .and_then(|s| {
                s.parse::<u64>()
                    .ok()
                    .filter(|v| *v <= i64::MAX as u64 && v.to_string() == *s)
            })
            .ok_or_else(malformed)?
    } else {
        0
    };
    let size = operation
        .request()
        .input
        .get("page_size")
        .and_then(Value::as_u64)
        .unwrap_or(25);
    if get {
        params.insert(
            "id".into(),
            operation.request().input["comment_id"]
                .as_str()
                .ok_or_else(malformed)?
                .into(),
        );
    } else {
        query.extend([
            ("startAt".into(), start.to_string()),
            ("maxResults".into(), size.to_string()),
            ("orderBy".into(), "created".into()),
        ]);
    }
    context.checkpoint()?;
    let response = context.invoke(&NativeCall {
        route: if get { GET_ROUTE } else { LIST_ROUTE }.into(),
        path_parameters: params,
        query,
        repeated_query: BTreeMap::new(),
        body: None,
    })?;
    context.checkpoint()?;
    if response.status != 200 {
        return Err(CraftError::new(
            match response.status {
                401 => ErrorCode::AuthRequired,
                403 => ErrorCode::PermissionDenied,
                429 => ErrorCode::RateLimited,
                _ => ErrorCode::RemoteUnavailable,
            },
            "Comment read did not establish visible evidence; no retry was performed.",
        ));
    }
    if serde_json::to_vec(&response.body)
        .map_err(|_| malformed())?
        .len()
        > RESPONSE_MAX
    {
        return Err(malformed());
    }
    let body_selected = operation
        .request()
        .selection
        .fields
        .iter()
        .any(|f| f == "body" || f.starts_with("body."));
    if get {
        let record = parse_record(&response.body, issue, body_selected)?;
        if record.id != operation.request().input["comment_id"] {
            return Err(malformed());
        }
        return Ok(NativePage {
            items: vec![json!(record)],
            context: None,
            next: None,
        });
    }
    let raw = &response.body;
    if [
        "errors",
        "errorMessages",
        "warningMessages",
        "nextPage",
        "nextPageToken",
        "isLast",
    ]
    .iter()
    .any(|k| raw.get(k).is_some())
    {
        return Err(malformed());
    }
    let offset = raw["startAt"].as_u64().ok_or_else(malformed)?;
    let advertised = raw["maxResults"]
        .as_u64()
        .filter(|v| *v > 0 && *v <= size)
        .ok_or_else(malformed)?;
    let total = raw["total"]
        .as_u64()
        .filter(|v| *v <= i64::MAX as u64)
        .ok_or_else(malformed)?;
    let comments = raw["comments"]
        .as_array()
        .filter(|v| v.len() <= advertised as usize)
        .ok_or_else(malformed)?;
    if offset != start
        || !comments.is_empty()
            && offset
                .checked_add(comments.len() as u64)
                .is_none_or(|end| end > total)
    {
        return Err(malformed());
    }
    let mut seen = BTreeSet::new();
    let mut items = vec![];
    for raw in comments {
        let record = parse_record(raw, issue, body_selected)?;
        if !seen.insert(record.id.clone()) {
            return Err(malformed());
        }
        items.push(json!(record));
    }
    let next = offset
        .checked_add(advertised)
        .filter(|next| *next <= i64::MAX as u64)
        .ok_or_else(malformed)?;
    Ok(NativePage {
        items,
        context: Some(json!({"source_issue_id":issue,"coverage":"visible_comment_page"})),
        next: if next < total {
            Some(NativePosition(BTreeMap::from([(
                "start_at".into(),
                next.to_string(),
            )])))
        } else {
            None
        },
    })
}
