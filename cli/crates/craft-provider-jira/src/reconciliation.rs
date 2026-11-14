//! Read-only current-state observations, never proof of which request caused an effect.
use crate::issue_context::{IssueState, JiraIssueContext, parse_state};
use crate::issue_reads::numeric_id;
use craft_core::contracts::{
    CanonicalRequest, EffectClass, Envelope, ResourceRef, SCHEMA_VERSION, validate_identity,
};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{
    HttpMethod, NativeCall, NativeRoute, OperationContext, OperationHandler, PreparedOperation,
    RetryAfter, RouteSegment,
};
use craft_core::presence::InputField;
use craft_core::registry::{
    OperationDescriptor, ProviderManifest, ProviderModule, ReconciliationClass, Registry,
    RetryClass, response_schema, schema_for,
};
use craft_core::storage::Clock;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub const COMMAND: &str = "jira.operation.inspect";
const ROUTE: &str = "jira-recovery-issue";
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InspectionInput {
    #[schemars(length(min=1,max=20),extend("uniqueItems"=true))]
    pub issue_ids: Vec<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    Visible,
    AbsentOrInvisible,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueInspection {
    pub issue_id: String,
    pub visibility: Visibility,
    pub state: Option<IssueState>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub assignee_account_id: InputField<String>,
    pub observed_at: u64,
}
pub struct InspectionBatch {
    pub observations: Vec<IssueInspection>,
    pub error: Option<CraftError>,
    pub http_status: Option<u16>,
    pub retry_after: Option<RetryAfter>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct InspectionData {
    target: ResourceRef,
    observations: Vec<IssueInspection>,
    #[schemars(extend("const"=false))]
    attribution_upgraded: bool,
}
fn malformed() -> CraftError {
    CraftError::new(
        ErrorCode::RemoteUnavailable,
        "Jira inspection did not establish bounded issue identity/state evidence.",
    )
}
fn input(request: &CanonicalRequest) -> Result<InspectionInput, CraftError> {
    crate::created_issue::validate_target(&request.target)?;
    if request.preconditions.is_some()
        || request.selection.cursor.is_some()
        || request.selection.limit != 20
    {
        return Err(CraftError::field(
            "/request",
            "inspection_preconditions_paging_unsupported",
        ));
    }
    let input: InspectionInput = serde_json::from_value(request.input.clone())
        .map_err(|_| CraftError::field("/input", "invalid_issue_inspection"))?;
    let mut ids = BTreeSet::new();
    if input.issue_ids.is_empty()
        || input.issue_ids.len() > 20
        || input
            .issue_ids
            .iter()
            .any(|id| !numeric_id(id) || !ids.insert(id))
    {
        return Err(CraftError::field(
            "/issue_ids",
            "expected_one_to_twenty_unique_numeric_ids",
        ));
    }
    Ok(input)
}
/// Inspects a fixed bounded set; partial observations survive failure without any attribution change.
pub fn inspect(
    operation: &PreparedOperation,
    context: &mut dyn OperationContext,
    clock: &dyn Clock,
) -> InspectionBatch {
    let mut batch = InspectionBatch {
        observations: vec![],
        error: None,
        http_status: None,
        retry_after: None,
    };
    let result = (|| {
        if operation.request().command != COMMAND
            || !matches!(operation.effect(), EffectClass::Read)
        {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Expected the read-only issue inspection operation.",
            ));
        }
        let input = input(operation.request())?;
        context.verify_operation(operation)?;
        for id in input.issue_ids {
            context.checkpoint()?;
            context.verify_operation(operation)?;
            let response = context.invoke(&NativeCall {
                route: ROUTE.into(),
                path_parameters: BTreeMap::from([("issueIdOrKey".into(), id.clone())]),
                query: BTreeMap::from([
                    (
                        "fields".into(),
                        "project,issuetype,status,updated,assignee".into(),
                    ),
                    ("fieldsByKeys".into(), "false".into()),
                    ("updateHistory".into(), "false".into()),
                    ("failFast".into(), "true".into()),
                ]),
                repeated_query: BTreeMap::new(),
                body: None,
            })?;
            batch.http_status = Some(response.status);
            batch.retry_after = response.retry_after;
            let mut observation = IssueInspection {
                issue_id: id.clone(),
                visibility: Visibility::AbsentOrInvisible,
                state: None,
                assignee_account_id: InputField::Omitted,
                observed_at: 0,
            };
            match response.status {
                404 => {}
                200 => {
                    let body = craft_core::input::parse_json(
                        &serde_json::to_vec(&response.body).map_err(|_| malformed())?,
                        131_072,
                    )
                    .map_err(|_| malformed())?;
                    if ["warningMessages", "errorMessages"].iter().any(|key| {
                        body.get(key)
                            .is_some_and(|v| v.as_array().is_none_or(|a| !a.is_empty()))
                    }) || body
                        .get("errors")
                        .is_some_and(|v| v.as_object().is_none_or(|a| !a.is_empty()))
                    {
                        return Err(malformed());
                    }
                    let state = parse_state(&body, &id)?;
                    if state.project_id != operation.request().target.id {
                        return Err(CraftError::new(
                            ErrorCode::PolicyDenied,
                            "The issue is outside the trusted recovery project.",
                        ));
                    }
                    let assignee = body
                        .get("fields")
                        .and_then(|v| v.get("assignee"))
                        .ok_or_else(malformed)?;
                    observation.assignee_account_id = if assignee.is_null() {
                        InputField::Null
                    } else {
                        let account = assignee
                            .get("accountId")
                            .and_then(Value::as_str)
                            .filter(|id| {
                                id.len() <= 128
                                    && !id.chars().any(char::is_whitespace)
                                    && !matches!(*id, "-1" | "null" | "unknown")
                                    && validate_identity("/account_id", id).is_ok()
                            })
                            .ok_or_else(malformed)?;
                        InputField::Value(account.into())
                    };
                    observation.visibility = Visibility::Visible;
                    observation.state = Some(state);
                }
                status => {
                    return Err(CraftError::new(
                        match status {
                            401 => ErrorCode::AuthRequired,
                            403 => ErrorCode::PermissionDenied,
                            429 => ErrorCode::RateLimited,
                            _ => ErrorCode::RemoteUnavailable,
                        },
                        "Jira inspection failed; no retry or mutation was attempted.",
                    ));
                }
            }
            observation.observed_at = clock.unix_millis();
            batch.observations.push(observation);
        }
        context.checkpoint()?;
        context.verify_operation(operation)
    })();
    batch.error = result.err();
    batch
}
pub struct JiraReconciliation;
impl ProviderModule for JiraReconciliation {
    fn manifest(&self) -> ProviderManifest {
        JiraIssueContext.manifest()
    }
    fn register(&self, registry: &mut Registry) -> Result<(), CraftError> {
        registry.register_typed_handler::<InspectionInput>(OperationDescriptor{
            id:COMMAND.into(),provider:"jira".into(),contract_version:SCHEMA_VERSION.into(),api_revision:"jira-cloud-v3".into(),
            description:"Inspect bounded issue identities in one project without attributing or replaying a write.".into(),effect:EffectClass::Read,
            input_schema:schema_for::<InspectionInput>()?,response_schema:response_schema::<InspectionData>()?,example:json!({"issue_ids":["10000"]}),
            response_example:json!(Envelope::success(COMMAND,"example".into(),json!({"target":{"provider":"jira","deployment":"cloud","tenant":"00000000-1111-2222-3333-444444444444","resource_type":"project","id":"100"},"observations":[],"attribution_upgraded":false}))),
            projection:None,scope_alternatives:vec![vec!["read:jira-work".into()]],permissions:vec!["Browse projects and issue-level visibility.".into()],tenant_conditions:vec!["Exact numeric issue/project; current state is not effect attribution.".into()],
            routes:vec![NativeRoute{id:ROUTE.into(),method:HttpMethod::Get,path:vec![RouteSegment::Literal("rest".into()),RouteSegment::Literal("api".into()),RouteSegment::Literal("3".into()),RouteSegment::Literal("issue".into()),RouteSegment::Parameter("issueIdOrKey".into())],query_parameters:["fields","fieldsByKeys","updateHistory","failFast"].map(String::from).into(),continuation_parameters:vec![],repeated_query_parameters:vec![],response_max_bytes:131_072}],
            retry_class:RetryClass::SafeRead,reconciliation:ReconciliationClass::NotApplicable,
        },Arc::new(Handler))
    }
}
struct Handler;
impl OperationHandler for Handler {
    fn validate(&self, request: &CanonicalRequest) -> Result<(), CraftError> {
        input(request).map(|_| ())
    }
    fn execute(
        &self,
        _: &PreparedOperation,
        _: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        Err(CraftError::new(
            ErrorCode::UnsupportedCapability,
            "Issue inspection requires the journal-bound recovery host.",
        ))
    }
}
