//! Field-selected issue reads and opaque-token enhanced search. Native content is inert data.

mod fields;
use craft_core::contracts::{
    CanonicalRequest, CredentialCacheUpdate, EffectClass, Envelope, RequestState, ResourceRef,
    SCHEMA_VERSION,
};
use craft_core::error::{CraftError, ErrorCode, FieldIssue};
use craft_core::execution::{
    HttpMethod, NativeCall, NativeResponse, NativeRoute, OperationContext, OperationHandler,
    PreparedOperation, RouteSegment,
};
use craft_core::pagination::{NativePage, NativePosition, PageObservation};
use craft_core::registry::{
    OperationDescriptor, ProviderManifest, ProviderModule, ReconciliationClass, Registry,
    RetryClass, response_schema, schema_for,
};
pub use fields::{FieldPlan, IssueSummary, projection};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub const GET: &str = "jira.issue.get";
pub const SEARCH: &str = "jira.issue.search";
pub const GET_ROUTE: &str = "jira-issue-get";
pub const SEARCH_ROUTE: &str = "jira-issue-search";

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueGetInput {
    #[schemars(length(min = 1, max = 512))]
    pub issue: String,
}
impl IssueGetInput {
    pub fn validate(&self) -> Result<(), CraftError> {
        if numeric_id(&self.issue) || key(&self.issue).is_ok() {
            Ok(())
        } else {
            Err(CraftError::field(
                "/issue",
                "expected_numeric_issue_id_or_key",
            ))
        }
    }
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueSearchInput {
    #[schemars(length(min = 1, max = 16_384))]
    pub jql: String,
    #[serde(default = "page_size")]
    #[schemars(range(min = 1, max = 100))]
    pub page_size: u16,
    #[serde(default)]
    #[schemars(length(max = 50))]
    pub reconcile_issue_ids: Vec<String>,
}
fn page_size() -> u16 {
    50
}
impl IssueSearchInput {
    pub fn validate(&self) -> Result<(), CraftError> {
        if self.jql.trim().is_empty()
            || self.jql.len() > 16_384
            || self
                .jql
                .chars()
                .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
        {
            return Err(CraftError::field("/jql", "expected_bounded_literal_jql"));
        }
        if !(1..=100).contains(&self.page_size) {
            return Err(CraftError::field(
                "/page_size",
                "expected_integer_between_1_and_100",
            ));
        }
        let mut ids = BTreeSet::new();
        if self.reconcile_issue_ids.len() > 50
            || self.reconcile_issue_ids.iter().any(|id| {
                !id.parse::<i64>()
                    .is_ok_and(|value| value > 0 && value.to_string() == *id)
                    || !ids.insert(id)
            })
        {
            return Err(CraftError::field(
                "/reconcile_issue_ids",
                "expected_at_most_fifty_unique_canonical_positive_int64_ids",
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Serialize, Deserialize, JsonSchema, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IdentityResolution {
    ExactId,
    MatchingKey,
    ResolvedKey,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueGetData {
    pub target: ResourceRef,
    pub requested_issue: String,
    pub issue: Option<IssueSummary>,
    pub identity_resolution: Option<IdentityResolution>,
    pub observed_at: Option<u64>,
    pub credential_cache_update: Option<CredentialCacheUpdate>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueSearchData {
    pub target: ResourceRef,
    pub items: Vec<IssueSummary>,
    pub observation: Option<PageObservation>,
    pub cached: bool,
    pub reconcile_issue_ids: Vec<String>,
    pub credential_cache_update: Option<CredentialCacheUpdate>,
}
pub struct JiraIssues;
pub fn route(command: &str) -> NativeRoute {
    let (id, method, path, query) = if command == GET {
        (
            GET_ROUTE,
            HttpMethod::Get,
            vec![
                RouteSegment::Literal("issue".into()),
                RouteSegment::Parameter("issueIdOrKey".into()),
            ],
            vec!["fields", "fieldsByKeys", "updateHistory", "failFast"],
        )
    } else {
        (
            SEARCH_ROUTE,
            HttpMethod::Post,
            vec![
                RouteSegment::Literal("search".into()),
                RouteSegment::Literal("jql".into()),
            ],
            vec![],
        )
    };
    NativeRoute {
        repeated_query_parameters: Vec::new(),
        id: id.into(),
        method,
        path: ["rest", "api", "3"]
            .map(|part| RouteSegment::Literal(part.into()))
            .into_iter()
            .chain(path)
            .collect(),
        query_parameters: query.into_iter().map(String::from).collect(),
        continuation_parameters: vec![],
        response_max_bytes: 1_048_576,
    }
}
impl ProviderModule for JiraIssues {
    fn manifest(&self) -> ProviderManifest {
        ProviderManifest {
            id: "jira".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            deployments: vec!["cloud".into()],
            contract_version: SCHEMA_VERSION.into(),
        }
    }
    fn register(&self, registry: &mut Registry) -> Result<(), CraftError> {
        for command in [GET, SEARCH] {
            let target = json!({"provider":"jira","deployment":"cloud","tenant":"00000000-1111-2222-3333-444444444444","resource_type":"site","id":"00000000-1111-2222-3333-444444444444"});
            let issue = json!({"id":"10000","key":"ED-1","summary":"Example","project":{"id":"10001","key":"ED"},"status":{"id":"1","name":"Open"}});
            let data = if command == GET {
                json!({"target":target,"requested_issue":"10000","issue":issue,"identity_resolution":"exact_id","observed_at":0,"credential_cache_update":null})
            } else {
                json!({"target":target,"items":[issue],"observation":{"observed_at":0,"page_digest":format!("sha256:{}","0".repeat(64))},"cached":false,"reconcile_issue_ids":[],"credential_cache_update":null})
            };
            let descriptor = OperationDescriptor {
                id: command.into(), provider: "jira".into(), contract_version: SCHEMA_VERSION.into(),
                api_revision: "jira-cloud-v3".into(),
                description: "Read selected issue metadata through verified bounded host execution.".into(),
                effect: EffectClass::Read,
                input_schema: if command == GET { schema_for::<IssueGetInput>()? } else { schema_for::<IssueSearchInput>()? },
                response_schema: if command == GET { response_schema::<IssueGetData>()? } else { response_schema::<IssueSearchData>()? },
                example: if command == GET { json!({"issue":"10000"}) } else { json!({"jql":"project = ED ORDER BY key","page_size":50,"reconcile_issue_ids":[]}) },
                response_example: json!(Envelope::success(command,"example".into(),data)),
                projection: Some(projection(command)),
                scope_alternatives: vec![vec!["read:jira-work".into()]],
                permissions: vec!["Browse projects and issue-level security visibility evaluated by Jira".into()],
                tenant_conditions: vec!["Live visibility; search does not prove absence or global read-after-write consistency.".into()],
                routes: vec![route(command)], retry_class: RetryClass::SafeRead,
                reconciliation: ReconciliationClass::NotApplicable,
            };
            if command == GET {
                registry.register_typed_handler::<IssueGetInput>(descriptor, Arc::new(Handler))?;
            } else {
                registry
                    .register_typed_handler::<IssueSearchInput>(descriptor, Arc::new(Handler))?;
            }
        }
        Ok(())
    }
}
struct Handler;
impl OperationHandler for Handler {
    fn validate(&self, request: &CanonicalRequest) -> Result<(), CraftError> {
        crate::projects::validate_target(&request.target)?;
        if request.preconditions.is_some()
            || (request.command == GET && request.selection.cursor.is_some())
        {
            return Err(CraftError::field(
                "/selection",
                "unsupported_issue_read_precondition_or_get_cursor",
            ));
        }
        if request.command == GET {
            serde_json::from_value::<IssueGetInput>(request.input.clone())
                .map_err(|_| malformed())?
                .validate()
        } else {
            serde_json::from_value::<IssueSearchInput>(request.input.clone())
                .map_err(|_| malformed())?
                .validate()
        }
    }
    fn execute(
        &self,
        _: &PreparedOperation,
        _: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        Err(CraftError::new(
            ErrorCode::UnsupportedCapability,
            "Issue reads require the verified bounded host.",
        ))
    }
}
pub fn numeric_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 512 && id.bytes().all(|byte| byte.is_ascii_digit())
}
pub fn key(value: &str) -> Result<&str, CraftError> {
    let Some((prefix, number)) = value.split_once('-') else {
        return Err(malformed());
    };
    if value.len() > 512
        || prefix.is_empty()
        || !prefix.as_bytes()[0].is_ascii_alphabetic()
        || !prefix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        || number.is_empty()
        || !number.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(malformed());
    }
    Ok(value)
}
fn malformed() -> CraftError {
    let mut error = CraftError::new(
        ErrorCode::RemoteUnavailable,
        "Issue response did not establish valid selected data and identity/continuation.",
    );
    error.details.write_state = RequestState::Failed;
    error
}
fn invoke(context: &mut dyn OperationContext, call: &NativeCall) -> Result<Value, CraftError> {
    context.checkpoint()?;
    let NativeResponse { status, body, .. } = context.invoke(call)?;
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
                "Issue is absent or not visible; this response does not establish absence."
            } else {
                "Issue read failed; no automatic retry was performed."
            },
        );
        error.details.write_state = RequestState::Failed;
        return Err(error);
    }
    let encoded = serde_json::to_vec(&body).map_err(|_| malformed())?;
    if !body.is_object() {
        return Err(malformed());
    }
    craft_core::input::parse_json(&encoded, 1_048_576).map_err(|_| malformed())
}
/// Performs one declared native get after context and field-plan binding checks.
/// Returns normalized data and exact-ID/key-resolution evidence; no retry or URL following.
pub fn read_get(
    operation: &PreparedOperation,
    context: &mut dyn OperationContext,
    plan: &FieldPlan,
) -> Result<(Value, IdentityResolution), CraftError> {
    context.verify_operation(operation)?;
    if operation.request().command != GET {
        return Err(CraftError::field("/command", "expected_issue_get"));
    }
    plan.verify(operation)?;
    let input: IssueGetInput =
        serde_json::from_value(operation.request().input.clone()).map_err(|_| malformed())?;
    input.validate()?;
    let raw = invoke(
        context,
        &NativeCall {
            repeated_query: std::collections::BTreeMap::new(),
            route: GET_ROUTE.into(),
            path_parameters: BTreeMap::from([("issueIdOrKey".into(), input.issue.clone())]),
            query: BTreeMap::from([
                ("fields".into(), plan.native_fields().join(",")),
                ("fieldsByKeys".into(), "false".into()),
                ("updateHistory".into(), "false".into()),
                ("failFast".into(), "true".into()),
            ]),
            body: None,
        },
    )?;
    let id = raw
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| numeric_id(id))
        .ok_or_else(malformed)?;
    let resolution = if numeric_id(&input.issue) {
        if id != input.issue {
            return Err(malformed());
        }
        IdentityResolution::ExactId
    } else {
        let actual = key(raw
            .get("key")
            .and_then(Value::as_str)
            .ok_or_else(malformed)?)?;
        if actual.eq_ignore_ascii_case(&input.issue) {
            IdentityResolution::MatchingKey
        } else {
            IdentityResolution::ResolvedKey
        }
    };
    Ok((plan.normalize(&raw)?, resolution))
}
/// Performs one enhanced-search POST using only the supplied opaque native position.
/// Returns a qualified normalized page or an error, without publishing cursor state.
pub fn read_search_page(
    operation: &PreparedOperation,
    context: &mut dyn OperationContext,
    plan: &FieldPlan,
    position: Option<&NativePosition>,
) -> Result<NativePage, CraftError> {
    context.verify_operation(operation)?;
    if operation.request().command != SEARCH {
        return Err(CraftError::field("/command", "expected_issue_search"));
    }
    plan.verify(operation)?;
    let input: IssueSearchInput =
        serde_json::from_value(operation.request().input.clone()).map_err(|_| malformed())?;
    input.validate()?;
    let mut body = json!({"jql":input.jql,"maxResults":input.page_size,"fields":plan.native_fields(),"fieldsByKeys":false,"includeArchivedProjects":false,"reconcileIssues":input.reconcile_issue_ids.iter().map(|id|id.parse::<i64>().map_err(|_|malformed())).collect::<Result<Vec<_>,_>>()?});
    if let Some(position) = position {
        position.validate()?;
        if position.0.len() != 1 {
            return Err(CraftError::field("/cursor", "invalid_search_position"));
        }
        body["nextPageToken"] = json!(position.0.get("nextPageToken").ok_or_else(malformed)?);
    }
    let raw = invoke(
        context,
        &NativeCall {
            repeated_query: std::collections::BTreeMap::new(),
            route: SEARCH_ROUTE.into(),
            path_parameters: BTreeMap::new(),
            query: BTreeMap::new(),
            body: Some(body),
        },
    )?;
    if let Some(warnings) = raw.get("warnings")
        && warnings
            .as_array()
            .is_none_or(|warnings| !warnings.is_empty())
    {
        let mut error = malformed();
        error.details.issues.push(FieldIssue::new(
            "/response/warnings",
            "provider_search_warning",
        ));
        error.next_action="Refine the JQL or inspect the provider warning through an authorized diagnostic path; no successful page or cursor was released.".into();
        return Err(error);
    }
    let values = raw
        .get("issues")
        .and_then(Value::as_array)
        .filter(|values| values.len() <= usize::from(input.page_size))
        .ok_or_else(malformed)?;
    let token = match raw.get("nextPageToken") {
        None | Some(Value::Null) => None,
        Some(Value::String(token)) => Some(token),
        _ => return Err(malformed()),
    };
    let last = raw
        .get("isLast")
        .map(|value| value.as_bool().ok_or_else(malformed))
        .transpose()?;
    if (last == Some(true) && token.is_some()) || (last == Some(false) && token.is_none()) {
        return Err(malformed());
    }
    let next = token
        .map(|token| {
            let position =
                NativePosition(BTreeMap::from([("nextPageToken".into(), token.clone())]));
            position.validate().map_err(|_| malformed())?;
            Ok(position)
        })
        .transpose()?;
    let mut ids = BTreeSet::new();
    let mut items = Vec::new();
    for raw in values {
        context.checkpoint()?;
        let item = plan.normalize(raw)?;
        if !ids.insert(item["id"].as_str().ok_or_else(malformed)?.to_owned()) {
            return Err(malformed());
        }
        items.push(item);
    }
    Ok(NativePage {
        context: None,
        items,
        next,
    })
}
