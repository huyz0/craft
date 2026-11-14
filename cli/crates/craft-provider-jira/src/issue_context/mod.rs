//! Issue-bound observations bracket native metadata; neither public data nor cached pages authorize writes.
mod models;
pub mod mutation;
mod selection;

use crate::create_metadata::MetadataPrincipal;
use crate::issue_reads::{IdentityResolution, IssueGetInput, key, numeric_id};
use craft_core::contracts::{
    CanonicalRequest, EffectClass, ResourceRef, SCHEMA_VERSION, validate_identity,
};
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
use craft_core::storage::Clock;
pub use models::*;
use schemars::JsonSchema;
pub use selection::{FieldPlan, projection};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub const EDIT: &str = "jira.metadata.edit";
pub const TRANSITIONS: &str = "jira.transition.list";
pub const IDENTITY_ROUTE: &str = "jira-context-issue";
pub const EDIT_ROUTE: &str = "jira-edit-metadata";
pub const TRANSITIONS_ROUTE: &str = "jira-issue-transitions";
const FRESHNESS_MS: u64 = 60_000;

impl IssueObservation {
    /// Checks inert cached evidence against the original target, without recreating live authority.
    pub fn validate_for_target(&self, target: &ResourceRef) -> Result<(), CraftError> {
        validate_target(target, &self.requested_issue)?;
        let expected = ResourceRef {
            id: self.state.id.clone(),
            ..target.clone()
        };
        let resolution = if numeric_id(&self.requested_issue) {
            if self.state.id != self.requested_issue {
                return Err(malformed());
            }
            IdentityResolution::ExactId
        } else if self.state.key.eq_ignore_ascii_case(&self.requested_issue) {
            IdentityResolution::MatchingKey
        } else {
            IdentityResolution::ResolvedKey
        };
        if self.resolved_issue != expected
            || self.resolution != resolution
            || ![
                &self.state.id,
                &self.state.project_id,
                &self.state.issue_type_id,
                &self.state.status_id,
            ]
            .iter()
            .all(|id| numeric_id(id))
            || key(&self.state.key).is_err()
            || self.state.updated.is_empty()
            || self.state.updated.len() > 128
            || self.state.updated.chars().any(char::is_control)
        {
            return Err(malformed());
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditInput {
    pub issue: String,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransitionInput {
    pub issue: String,
    pub transition_id: Option<String>,
    #[serde(default)]
    pub include_unavailable: bool,
    #[serde(default)]
    pub sort_by_status: bool,
}
impl TransitionInput {
    pub fn validate(&self) -> Result<(), CraftError> {
        IssueGetInput {
            issue: self.issue.clone(),
        }
        .validate()?;
        if let Some(id) = &self.transition_id {
            validate_identity("/transition_id", id)?;
        }
        Ok(())
    }
}
pub fn response(command: &str) -> Result<Value, CraftError> {
    match command {
        EDIT => response_schema::<ContextData<EditableField>>(),
        TRANSITIONS => response_schema::<ContextData<Transition>>(),
        _ => Err(CraftError::field(
            "/command",
            "expected_issue_metadata_read",
        )),
    }
}
pub fn routes(command: &str) -> Result<Vec<NativeRoute>, CraftError> {
    if !matches!(command, EDIT | TRANSITIONS) {
        return Err(CraftError::field(
            "/command",
            "expected_issue_metadata_read",
        ));
    }
    let base = || {
        ["rest", "api", "3", "issue"]
            .map(|value| RouteSegment::Literal(value.into()))
            .into_iter()
            .chain([RouteSegment::Parameter("issueIdOrKey".into())])
            .collect::<Vec<_>>()
    };
    let mut metadata_path = base();
    metadata_path.push(RouteSegment::Literal(
        if command == EDIT {
            "editmeta"
        } else {
            "transitions"
        }
        .into(),
    ));
    Ok(vec![
        NativeRoute {
            id: IDENTITY_ROUTE.into(),
            method: HttpMethod::Get,
            path: base(),
            query_parameters: ["fields", "fieldsByKeys", "updateHistory", "failFast"]
                .map(String::from)
                .into(),
            continuation_parameters: vec![],
            repeated_query_parameters: vec![],
            response_max_bytes: 1_048_576,
        },
        NativeRoute {
            id: if command == EDIT {
                EDIT_ROUTE
            } else {
                TRANSITIONS_ROUTE
            }
            .into(),
            method: HttpMethod::Get,
            path: metadata_path,
            query_parameters: if command == EDIT {
                vec![]
            } else {
                [
                    "expand",
                    "transitionId",
                    "includeUnavailableTransitions",
                    "sortByOpsBarAndStatus",
                ]
                .map(String::from)
                .into()
            },
            continuation_parameters: vec![],
            repeated_query_parameters: vec![],
            response_max_bytes: 1_048_576,
        },
    ])
}
pub struct JiraIssueContext;
impl ProviderModule for JiraIssueContext {
    fn manifest(&self) -> ProviderManifest {
        ProviderManifest {
            id: "jira".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            deployments: vec!["cloud".into()],
            contract_version: SCHEMA_VERSION.into(),
        }
    }
    fn register(&self, registry: &mut Registry) -> Result<(), CraftError> {
        for command in [EDIT, TRANSITIONS] {
            let example = if command == EDIT {
                json!({"id":"summary","name":"Summary","required":true,"operations":["set"],"schema":{"type":"string"}})
            } else {
                json!({"id":"31","name":"Done","to":{"id":"3","name":"Done"},"has_screen":false,"is_available":true})
            };
            let descriptor = OperationDescriptor {
                id: command.into(),
                provider: "jira".into(),
                contract_version: SCHEMA_VERSION.into(),
                api_revision: "jira-cloud-v3".into(),
                description: "Read bounded issue-bound metadata with bracketing identity/state checks; no mutation authority.".into(),
                effect: EffectClass::Read,
                input_schema: if command == EDIT {
                    schema_for::<EditInput>()?
                } else {
                    schema_for::<TransitionInput>()?
                },
                response_schema: response(command)?,
                example: json!({"issue":"10000"}),
                response_example: json!(craft_core::contracts::Envelope::success(
                    command,
                    "example".into(),
                    json!({
                        "target": {
                            "provider":"jira", "deployment":"cloud",
                            "tenant":"00000000-1111-2222-3333-444444444444",
                            "resource_type":"issue", "id":"10000"
                        },
                        "items":[example], "issue_observation":null, "observation":null,
                        "cached":false, "credential_cache_update":null,
                        "atomic_snapshot":false, "mutation_authorized":false
                    })
                )),
                projection: Some(projection(command)),
                scope_alternatives: vec![vec!["read:jira-work".into()]],
                permissions: vec!["Browse projects and issue-level visibility; editable fields require Edit issues and transitions depend on Transition issues.".into()],
                tenant_conditions: vec!["Exact issue/site/grant/actor; no administrative override or status substitution.".into()],
                routes: routes(command)?,
                retry_class: RetryClass::SafeRead,
                reconciliation: ReconciliationClass::NotApplicable,
            };
            if command == EDIT {
                registry.register_typed_handler::<EditInput>(descriptor, Arc::new(Handler))?;
            } else {
                registry
                    .register_typed_handler::<TransitionInput>(descriptor, Arc::new(Handler))?;
            }
        }
        Ok(())
    }
}
pub fn validate_target(target: &ResourceRef, issue: &str) -> Result<(), CraftError> {
    target.validate()?;
    IssueGetInput {
        issue: issue.into(),
    }
    .validate()?;
    let site = ResourceRef {
        resource_type: "site".into(),
        id: target.tenant.clone(),
        ..target.clone()
    };
    crate::projects::validate_target(&site)?;
    if target.resource_type != "issue" || target.id != issue {
        return Err(CraftError::field(
            "/target",
            "expected_exact_requested_issue",
        ));
    }
    Ok(())
}
pub fn effective_input(request: &CanonicalRequest) -> Result<Value, CraftError> {
    match request.command.as_str() {
        EDIT => {
            let input: EditInput = serde_json::from_value(request.input.clone())
                .map_err(|_| CraftError::field("/input", "invalid_edit_metadata_input"))?;
            validate_target(&request.target, &input.issue)?;
            Ok(json!(input))
        }
        TRANSITIONS => {
            let input: TransitionInput = serde_json::from_value(request.input.clone())
                .map_err(|_| CraftError::field("/input", "invalid_transition_list_input"))?;
            input.validate()?;
            validate_target(&request.target, &input.issue)?;
            Ok(json!(input))
        }
        _ => Err(CraftError::field(
            "/command",
            "expected_issue_metadata_read",
        )),
    }
}
struct Handler;
impl OperationHandler for Handler {
    fn validate(&self, request: &CanonicalRequest) -> Result<(), CraftError> {
        if request.preconditions.is_some() {
            return Err(CraftError::field(
                "/preconditions",
                "metadata_read_preconditions_unsupported",
            ));
        }
        effective_input(request)?;
        Ok(())
    }
    fn execute(
        &self,
        _: &PreparedOperation,
        _: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        Err(CraftError::new(
            ErrorCode::UnsupportedCapability,
            "Issue metadata requires the verified immutable-cursor host.",
        ))
    }
}
fn malformed() -> CraftError {
    CraftError::new(
        ErrorCode::RemoteUnavailable,
        "Issue metadata did not establish valid bounded identity and field/transition evidence.",
    )
}
fn changed() -> CraftError {
    CraftError::new(
        ErrorCode::VersionConflict,
        "Issue context changed or exceeded its freshness window; read fresh metadata before preparing a write.",
    )
}
fn invoke(context: &mut dyn OperationContext, call: NativeCall) -> Result<Value, CraftError> {
    context.checkpoint()?;
    let response = context.invoke(&call)?;
    if response.status != 200 {
        let mut error = CraftError::new(
            match response.status {
                400 | 422 => ErrorCode::InputInvalid,
                401 => ErrorCode::AuthRequired,
                403 => ErrorCode::PermissionDenied,
                429 => ErrorCode::RateLimited,
                _ => ErrorCode::RemoteUnavailable,
            },
            if response.status == 404 {
                "Issue is absent or invisible; absence was not established."
            } else {
                "Issue metadata read failed; no further automatic retry is made."
            },
        );
        error.details.write_state = craft_core::contracts::RequestState::Failed;
        return Err(error);
    }
    let body = craft_core::input::parse_json(
        &serde_json::to_vec(&response.body).map_err(|_| malformed())?,
        1_048_576,
    )
    .map_err(|_| malformed())?;
    if !body.is_object()
        || ["warningMessages", "errorMessages"].iter().any(|name| {
            body.get(name)
                .is_some_and(|value| value.as_array().is_none_or(|values| !values.is_empty()))
        })
        || body
            .get("errors")
            .is_some_and(|value| value.as_object().is_none_or(|errors| !errors.is_empty()))
    {
        return Err(malformed());
    }
    Ok(body)
}
fn state(context: &mut dyn OperationContext, selector: &str) -> Result<IssueState, CraftError> {
    let raw = invoke(
        context,
        NativeCall {
            route: IDENTITY_ROUTE.into(),
            path_parameters: BTreeMap::from([("issueIdOrKey".into(), selector.into())]),
            query: BTreeMap::from([
                ("fields".into(), "project,issuetype,status,updated".into()),
                ("fieldsByKeys".into(), "false".into()),
                ("updateHistory".into(), "false".into()),
                ("failFast".into(), "true".into()),
            ]),
            repeated_query: BTreeMap::new(),
            body: None,
        },
    )?;
    parse_state(&raw, selector)
}
pub(crate) fn parse_state(raw: &Value, selector: &str) -> Result<IssueState, CraftError> {
    let id = raw
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| numeric_id(id))
        .ok_or_else(malformed)?;
    if numeric_id(selector) && id != selector {
        return Err(malformed());
    }
    let issue_key = raw
        .get("key")
        .and_then(Value::as_str)
        .ok_or_else(malformed)?;
    key(issue_key).map_err(|_| malformed())?;
    let fields = raw.get("fields").ok_or_else(malformed)?;
    let number = |name: &str| {
        fields
            .get(name)
            .and_then(|value| value.get("id"))
            .and_then(Value::as_str)
            .filter(|id| numeric_id(id))
            .map(String::from)
            .ok_or_else(malformed)
    };
    let updated = fields
        .get("updated")
        .and_then(Value::as_str)
        .filter(|value| {
            !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
        })
        .ok_or_else(malformed)?;
    Ok(IssueState {
        id: id.into(),
        key: issue_key.into(),
        project_id: number("project")?,
        issue_type_id: number("issuetype")?,
        status_id: number("status")?,
        updated: updated.into(),
    })
}
/// In-process evidence only. Serialization cannot recreate this operation/principal/freshness proof.
pub struct LiveIssueContext {
    page: NativePage,
    native: Value,
    observation: IssueObservation,
    operation: String,
    principal: MetadataPrincipal,
    wall: u64,
    monotonic: u64,
}
impl LiveIssueContext {
    pub fn page(&self) -> &NativePage {
        &self.page
    }
    pub fn verify(
        &self,
        operation: &PreparedOperation,
        principal: &MetadataPrincipal,
        clock: &dyn Clock,
    ) -> Result<(), CraftError> {
        if operation.digest() != self.operation || principal != &self.principal {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Live issue context differs from the operation or principal.",
            ));
        }
        let wall = clock.unix_millis();
        let monotonic = clock.monotonic_millis();
        if wall < self.wall
            || monotonic < self.monotonic
            || wall - self.wall >= FRESHNESS_MS
            || monotonic - self.monotonic >= FRESHNESS_MS
        {
            return Err(changed());
        }
        Ok(())
    }
}
pub fn read_context(
    operation: &PreparedOperation,
    context: &mut dyn OperationContext,
    plan: &FieldPlan,
    position: Option<&NativePosition>,
    principal: &MetadataPrincipal,
    clock: &dyn Clock,
) -> Result<LiveIssueContext, CraftError> {
    context.verify_operation(operation)?;
    plan.verify(operation)?;
    effective_input(operation.request())?;
    principal.grant.validate()?;
    validate_identity("/actor", &principal.actor)?;
    if position.is_some() {
        return Err(CraftError::field(
            "/cursor",
            "expected_initial_unpaginated_issue_metadata",
        ));
    }
    let wall = clock.unix_millis();
    let monotonic = clock.monotonic_millis();
    let before = state(context, &operation.request().target.id)?;
    let command = operation.request().command.as_str();
    let mut query = BTreeMap::new();
    let mut filter = None;
    if command == TRANSITIONS {
        let input: TransitionInput =
            serde_json::from_value(operation.request().input.clone()).map_err(|_| malformed())?;
        query.insert(
            "includeUnavailableTransitions".into(),
            input.include_unavailable.to_string(),
        );
        query.insert(
            "sortByOpsBarAndStatus".into(),
            input.sort_by_status.to_string(),
        );
        if let Some(id) = input.transition_id {
            query.insert("transitionId".into(), id.clone());
            filter = Some(id);
        }
        if plan.transition_fields() {
            query.insert("expand".into(), "transitions.fields".into());
        }
    }
    let raw = invoke(
        context,
        NativeCall {
            route: if command == EDIT {
                EDIT_ROUTE
            } else {
                TRANSITIONS_ROUTE
            }
            .into(),
            path_parameters: BTreeMap::from([("issueIdOrKey".into(), before.id.clone())]),
            query,
            repeated_query: BTreeMap::new(),
            body: None,
        },
    )?;
    let items = if command == EDIT {
        plan.fields(raw.get("fields").ok_or_else(malformed)?, "", context)?
    } else {
        let values = raw
            .get("transitions")
            .and_then(Value::as_array)
            .filter(|values| values.len() <= 1000)
            .ok_or_else(malformed)?;
        let mut ids = BTreeSet::new();
        let mut items = Vec::new();
        for value in values {
            context.checkpoint()?;
            let item = plan.transition(value, context)?;
            let id = item["id"].as_str().ok_or_else(malformed)?;
            if !ids.insert(id.to_owned()) || filter.as_ref().is_some_and(|expected| expected != id)
            {
                return Err(malformed());
            }
            items.push(item);
        }
        items
    };
    let after = state(context, &before.id)?;
    if before != after {
        return Err(changed());
    }
    let resolution = if numeric_id(&operation.request().target.id) {
        IdentityResolution::ExactId
    } else if before
        .key
        .eq_ignore_ascii_case(&operation.request().target.id)
    {
        IdentityResolution::MatchingKey
    } else {
        IdentityResolution::ResolvedKey
    };
    let observation = IssueObservation {
        requested_issue: operation.request().target.id.clone(),
        resolved_issue: ResourceRef {
            id: before.id.clone(),
            ..operation.request().target.clone()
        },
        resolution,
        state: before,
    };
    let result = LiveIssueContext {
        page: NativePage {
            items,
            next: None,
            context: Some(json!(&observation)),
        },
        native: raw,
        observation,
        operation: operation.digest().into(),
        principal: principal.clone(),
        wall,
        monotonic,
    };
    context.checkpoint()?;
    result.verify(operation, principal, clock)?;
    Ok(result)
}
