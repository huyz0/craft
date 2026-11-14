//! Read-only postcondition observations never attribute a relation to an earlier request.
use super::*;
use crate::issue_links::write::LinkInput;
use craft_core::execution::{NativeResponse, RetryAfter};
use craft_core::storage::Clock;
pub const COMMAND: &str = "jira.link.inspect";
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InspectionInput {
    pub expected: LinkInput,
    pub link_id: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CurrentRelation {
    Uninspected,
    Observed,
    NotObserved,
    AbsentOrInvisible,
    Changed,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LinkObservation {
    pub link_id: String,
    pub link: Option<LinkRecord>,
    pub observed_at: u64,
}
pub struct InspectionBatch {
    pub observations: Vec<LinkObservation>,
    pub current: CurrentRelation,
    pub complete: bool,
    pub error: Option<CraftError>,
    pub http_status: Option<u16>,
    pub retry_after: Option<RetryAfter>,
}
fn input(request: &CanonicalRequest) -> Result<InspectionInput, CraftError> {
    crate::created_issue::validate_target(&request.target)?;
    if request.preconditions.is_some() || request.selection.cursor.is_some() {
        return Err(CraftError::field(
            "/request",
            "inspection_has_no_cursor_or_preconditions",
        ));
    }
    let input: InspectionInput = serde_json::from_value(request.input.clone())
        .map_err(|_| CraftError::field("/input", "invalid_link_inspection"))?;
    input.expected.validate()?;
    if input.link_id.as_ref().is_some_and(|id| !numeric(id)) {
        return Err(CraftError::field("/link_id", "expected_numeric_link_id"));
    }
    Ok(input)
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
            ErrorCode::PolicyDenied,
            "Inspection requires the journal-bound recovery host.",
        ))
    }
}
pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    let mut native_routes = routes(GET);
    native_routes.extend(routes(LIST));
    registry.register_typed_handler::<InspectionInput>(OperationDescriptor {id:COMMAND.into(),provider:"jira".into(),contract_version:SCHEMA_VERSION.into(),api_revision:"jira-cloud-v3".into(),description:"Observe an exact original link relation without attributing or replaying its write.".into(),effect:EffectClass::Read,input_schema:schema_for::<InspectionInput>()?,response_schema:json!({"type":"object"}),example:json!({"expected":{"inward_issue_id":"10001","outward_issue_id":"10002","inward_project_id":"200","link_type":{"id":"10000","name":"Blocks","inward":"is blocked by","outward":"blocks"}},"link_id":null}),response_example:json!({}),projection:None,scope_alternatives:vec![vec!["read:jira-work".into()]],permissions:vec!["Current visibility of expected endpoint projects; no write authority.".into()],tenant_conditions:vec!["Not observed or 404 is not proof of failed original mutation.".into()],routes:native_routes,retry_class:RetryClass::SafeRead,reconciliation:ReconciliationClass::NotApplicable},Arc::new(Handler))
}
fn response(
    context: &mut dyn OperationContext,
    call: &NativeCall,
    batch: &mut InspectionBatch,
) -> Result<NativeResponse, CraftError> {
    context.checkpoint()?;
    let response = context.invoke(call)?;
    context.checkpoint()?;
    batch.http_status = Some(response.status);
    batch.retry_after = response.retry_after;
    if !matches!(response.status, 200 | 404) {
        return Err(CraftError::new(
            match response.status {
                401 => ErrorCode::AuthRequired,
                403 => ErrorCode::PermissionDenied,
                429 => ErrorCode::RateLimited,
                _ => ErrorCode::RemoteUnavailable,
            },
            "Link investigation did not establish bounded current evidence; no retry was performed.",
        ));
    }
    if serde_json::to_vec(&response.body)
        .map_err(|_| malformed())?
        .len()
        > 1_048_576
    {
        return Err(malformed());
    }
    Ok(response)
}
fn endpoint(
    context: &mut dyn OperationContext,
    id: &str,
    project: &str,
    links: bool,
    batch: &mut InspectionBatch,
) -> Result<Option<Value>, CraftError> {
    let response = response(
        context,
        &NativeCall {
            route: ISSUE_ROUTE.into(),
            path_parameters: BTreeMap::from([("issueIdOrKey".into(), id.into())]),
            query: BTreeMap::from([
                (
                    "fields".into(),
                    if links {
                        "project,issuelinks"
                    } else {
                        "project"
                    }
                    .into(),
                ),
                ("updateHistory".into(), "false".into()),
                ("failFast".into(), "true".into()),
            ]),
            repeated_query: BTreeMap::new(),
            body: None,
        },
        batch,
    )?;
    if response.status == 404 {
        return Ok(None);
    }
    if ["errors", "errorMessages", "warningMessages"]
        .iter()
        .any(|key| response.body.get(key).is_some())
    {
        return Err(malformed());
    }
    if response.body["id"] != id || response.body["fields"]["project"]["id"] != project {
        return Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "Expected link endpoint is outside the authorized recovery project or identity.",
        ));
    }
    Ok(Some(response.body))
}
pub fn inspect(
    operation: &PreparedOperation,
    context: &mut dyn OperationContext,
    clock: &dyn Clock,
) -> InspectionBatch {
    let mut batch = InspectionBatch {
        observations: vec![],
        current: CurrentRelation::Uninspected,
        complete: false,
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
                "Expected a sealed read-only link inspection.",
            ));
        }
        context.verify_operation(operation)?;
        let input = input(operation.request())?;
        let outward = endpoint(
            context,
            &input.expected.outward_issue_id,
            &operation.request().target.id,
            input.link_id.is_none(),
            &mut batch,
        )?;
        let inward = endpoint(
            context,
            &input.expected.inward_issue_id,
            &input.expected.inward_project_id,
            false,
            &mut batch,
        )?;
        if outward.is_none() || inward.is_none() {
            batch.current = CurrentRelation::AbsentOrInvisible;
            batch.complete = true;
            return Ok(());
        }
        let candidates = if let Some(id) = input.link_id {
            vec![id]
        } else {
            let rows = parse_issue_links(
                &outward.ok_or_else(malformed)?,
                &input.expected.outward_issue_id,
            )?;
            rows.into_iter()
                .filter(|row| {
                    row.inward_issue_id == input.expected.inward_issue_id
                        && row.outward_issue_id == input.expected.outward_issue_id
                        && row.link_type.id == input.expected.link_type.id
                })
                .map(|row| row.id)
                .collect::<Vec<_>>()
        };
        if candidates.len() > 20 {
            return Err(CraftError::new(
                ErrorCode::RemoteUnavailable,
                "Matching native link candidates exceed the bounded inspection strategy.",
            ));
        }
        for id in candidates {
            let response = response(
                context,
                &NativeCall {
                    route: LINK_ROUTE.into(),
                    path_parameters: BTreeMap::from([("linkId".into(), id.clone())]),
                    query: BTreeMap::new(),
                    repeated_query: BTreeMap::new(),
                    body: None,
                },
                &mut batch,
            )?;
            let link = if response.status == 404 {
                None
            } else {
                let row = parse_record(&response.body, None)?;
                if row.id != id
                    || row.inward_issue_id != input.expected.inward_issue_id
                    || row.outward_issue_id != input.expected.outward_issue_id
                {
                    return Err(CraftError::new(
                        ErrorCode::VersionConflict,
                        "Observed native link is not the expected endpoint relation.",
                    ));
                }
                Some(row)
            };
            batch.observations.push(LinkObservation {
                link_id: id,
                link,
                observed_at: clock.unix_millis(),
            });
            context.checkpoint()?;
        }
        batch.current = if batch.observations.iter().any(|o| {
            o.link
                .as_ref()
                .is_some_and(|l| l.link_type == input.expected.link_type)
        }) {
            CurrentRelation::Observed
        } else if batch.observations.iter().any(|o| o.link.is_some()) {
            CurrentRelation::Changed
        } else if batch.observations.is_empty() {
            CurrentRelation::NotObserved
        } else {
            CurrentRelation::AbsentOrInvisible
        };
        batch.complete = true;
        Ok(())
    })();
    batch.error = result.err();
    batch
}
