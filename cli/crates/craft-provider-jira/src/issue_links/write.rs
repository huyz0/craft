//! Bracketed native link preparation and immutable comment-free request payloads.
use super::*;
use crate::create_metadata::MetadataPrincipal;
use craft_core::contracts::ResourceRef;
use craft_core::storage::{Clock, SubmissionState};
use std::cell::Cell;

#[cfg(test)]
mod tests;

pub const CREATE: &str = "jira.link.create";
pub const CONTEXT: &str = "jira.link.context";
pub const CREATE_ROUTE: &str = "create-issue-link";
pub const DELETE: &str = "jira.link.delete";
pub const DELETE_CONTEXT: &str = "jira.link.delete-context";
pub const DELETE_ROUTE: &str = "delete-issue-link";
const ISSUE: &str = "link-endpoint";
const PERMISSIONS: &str = "link-permissions";
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LinkInput {
    pub inward_issue_id: String,
    pub outward_issue_id: String,
    pub inward_project_id: String,
    pub link_type: LinkType,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LinkDeleteInput {
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub link_id: String,
    pub expected: LinkInput,
}
impl LinkDeleteInput {
    pub fn validate(&self) -> Result<(), CraftError> {
        self.expected.validate()?;
        if !numeric(&self.link_id) {
            return Err(CraftError::field("/link_id", "expected_numeric_link_id"));
        }
        Ok(())
    }
}
fn delete_input(request: &CanonicalRequest) -> Result<LinkDeleteInput, CraftError> {
    crate::created_issue::validate_target(&request.target)?;
    if !numeric(&request.target.id)
        || request.preconditions.is_some()
        || request.selection.cursor.is_some()
        || !request.selection.fields.is_empty()
    {
        return Err(CraftError::field(
            "/request",
            "exact_complete_link_deletion_required",
        ));
    }
    let input: LinkDeleteInput = serde_json::from_value(request.input.clone())
        .map_err(|_| CraftError::field("/input", "invalid_link_deletion"))?;
    input.validate()?;
    Ok(input)
}
fn numeric(value: &str) -> bool {
    value
        .parse::<i64>()
        .is_ok_and(|n| n > 0 && n.to_string() == value)
}
impl LinkInput {
    pub fn validate(&self) -> Result<(), CraftError> {
        if !numeric(&self.inward_issue_id)
            || !numeric(&self.outward_issue_id)
            || !numeric(&self.inward_project_id)
            || self.inward_issue_id == self.outward_issue_id
        {
            return Err(CraftError::field(
                "/input",
                "expected_distinct_numeric_link_endpoints_and_project",
            ));
        }
        parse_types(&json!({"issueLinkTypes":[self.link_type]})).map_err(|_| {
            CraftError::field("/link_type", "expected_complete_numeric_native_link_type")
        })?;
        Ok(())
    }
}
fn input(request: &CanonicalRequest) -> Result<LinkInput, CraftError> {
    crate::created_issue::validate_target(&request.target)?;
    if !numeric(&request.target.id)
        || request.preconditions.is_some()
        || request.selection.cursor.is_some()
        || !request.selection.fields.is_empty()
    {
        return Err(CraftError::field(
            "/request",
            "exact_complete_link_operation_required",
        ));
    }
    let input: LinkInput = serde_json::from_value(request.input.clone())
        .map_err(|_| CraftError::field("/input", "invalid_link_input"))?;
    input.validate()?;
    Ok(input)
}
struct Handler;
impl OperationHandler for Handler {
    fn validate(&self, request: &CanonicalRequest) -> Result<(), CraftError> {
        if matches!(request.command.as_str(), DELETE | DELETE_CONTEXT) {
            delete_input(request).map(|_| ())
        } else {
            input(request).map(|_| ())
        }
    }
    fn execute(
        &self,
        _: &PreparedOperation,
        _: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "Link operations require native preparation and the journaled host.",
        ))
    }
}
fn route(id: &str, method: HttpMethod, parts: &[&str], query: &[&str]) -> NativeRoute {
    NativeRoute {
        id: id.into(),
        method,
        path: parts
            .iter()
            .map(|s| {
                if matches!(*s, "issueIdOrKey" | "linkId") {
                    RouteSegment::Parameter((*s).into())
                } else {
                    RouteSegment::Literal((*s).into())
                }
            })
            .collect(),
        query_parameters: query.iter().map(|s| (*s).into()).collect(),
        continuation_parameters: vec![],
        repeated_query_parameters: vec![],
        response_max_bytes: 1_048_576,
    }
}
pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    let example = json!({"inward_issue_id":"10001","outward_issue_id":"10002","inward_project_id":"10000","link_type":{"id":"10000","name":"Blocks","inward":"is blocked by","outward":"blocks"}});
    for command in [CONTEXT, CREATE] {
        let write = command == CREATE;
        let routes = if write {
            vec![route(
                CREATE_ROUTE,
                HttpMethod::Post,
                &["rest", "api", "3", "issueLink"],
                &[],
            )]
        } else {
            vec![
                route(
                    super::ROUTE,
                    HttpMethod::Get,
                    &["rest", "api", "3", "issueLinkType"],
                    &[],
                ),
                route(
                    ISSUE,
                    HttpMethod::Get,
                    &["rest", "api", "3", "issue", "issueIdOrKey"],
                    &["fields", "updateHistory", "failFast"],
                ),
                route(
                    PERMISSIONS,
                    HttpMethod::Get,
                    &["rest", "api", "3", "mypermissions"],
                    &["issueId", "permissions"],
                ),
            ]
        };
        registry.register_typed_handler::<LinkInput>(OperationDescriptor {id:command.into(),provider:"jira".into(),contract_version:SCHEMA_VERSION.into(),api_revision:"jira-cloud-v3".into(),description:if write {"Create one exact comment-free directed issue relation through the journaled host."} else {"Qualify link type, current endpoint projects and issue-specific permissions."}.into(),effect:if write {EffectClass::Write} else {EffectClass::Read},input_schema:schema_for::<LinkInput>()?,response_schema:json!({"type":"object"}),example:example.clone(),response_example:json!({}),projection:None,scope_alternatives:vec![vec![if write {"write:jira-work"} else {"read:jira-work"}.into()]],permissions:vec!["Browse projects on both endpoints; Link issues on outward endpoint.".into()],tenant_conditions:vec!["Issue linking enabled, exact numeric endpoint identities and native labels.".into()],routes,retry_class:if write {RetryClass::ReconcileBeforeReplay} else {RetryClass::SafeRead},reconciliation:if write {ReconciliationClass::KnownResource} else {ReconciliationClass::NotApplicable}},Arc::new(Handler))?;
    }
    for command in [DELETE_CONTEXT, DELETE] {
        let deletion = command == DELETE;
        let mut descriptor = registry
            .descriptor(if deletion { CREATE } else { CONTEXT })?
            .clone();
        descriptor.id = command.into();
        descriptor.input_schema = schema_for::<LinkDeleteInput>()?;
        descriptor.effect = if deletion {
            EffectClass::Permanent
        } else {
            EffectClass::Read
        };
        descriptor.description = if deletion {
            "Delete one exact expected native issue link through the journaled host."
        } else {
            "Qualify exact native link deletion context and permissions."
        }
        .into();
        descriptor.example = json!({"link_id":"10003","expected":example});
        descriptor.permissions =
            vec!["Browse projects on both endpoints; Link issues on at least one endpoint.".into()];
        descriptor.routes = if deletion {
            vec![route(
                DELETE_ROUTE,
                HttpMethod::Delete,
                &["rest", "api", "3", "issueLink", "linkId"],
                &[],
            )]
        } else {
            let mut routes = descriptor.routes;
            routes.extend(super::reads::routes(super::reads::GET));
            routes
        };
        registry.register_typed_handler::<LinkDeleteInput>(descriptor, Arc::new(Handler))?;
    }
    Ok(())
}
struct Freshness {
    clock: Arc<dyn Clock>,
    wall: u64,
    mono: u64,
    last_wall: Cell<u64>,
    last_mono: Cell<u64>,
}
impl Freshness {
    fn new(clock: Arc<dyn Clock>) -> Self {
        let wall = clock.unix_millis();
        let mono = clock.monotonic_millis();
        Self {
            clock,
            wall,
            mono,
            last_wall: Cell::new(wall),
            last_mono: Cell::new(mono),
        }
    }
    fn check(&self) -> Result<(), CraftError> {
        let wall = self.clock.unix_millis();
        let mono = self.clock.monotonic_millis();
        if wall < self.last_wall.get()
            || mono < self.last_mono.get()
            || wall.saturating_sub(self.wall) >= 30_000
            || mono.saturating_sub(self.mono) >= 30_000
        {
            return Err(CraftError::new(
                ErrorCode::VersionConflict,
                "Link metadata expired or its clock moved backward.",
            ));
        }
        self.last_wall.set(wall);
        self.last_mono.set(mono);
        Ok(())
    }
}
pub struct FrozenLink {
    call: NativeCall,
    digest: String,
    principal: MetadataPrincipal,
    freshness: Freshness,
}
impl FrozenLink {
    pub fn call(&self) -> &NativeCall {
        &self.call
    }
    pub fn verify(
        &self,
        operation: &PreparedOperation,
        principal: &MetadataPrincipal,
    ) -> Result<(), CraftError> {
        if !matches!(operation.request().command.as_str(), CREATE | DELETE)
            || self.digest != operation.digest()
            || &self.principal != principal
        {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Frozen link does not match its operation and authenticated principal.",
            ));
        }
        self.freshness.check()
    }
}
fn call(route: &str, issue: Option<&str>, query: BTreeMap<String, String>) -> NativeCall {
    NativeCall {
        route: route.into(),
        path_parameters: issue
            .map(|id| BTreeMap::from([("issueIdOrKey".into(), id.into())]))
            .unwrap_or_default(),
        query,
        repeated_query: BTreeMap::new(),
        body: None,
    }
}
fn read(
    context: &mut dyn OperationContext,
    call: &NativeCall,
    freshness: &Freshness,
) -> Result<Value, CraftError> {
    context.checkpoint()?;
    freshness.check()?;
    let response = context.invoke(call)?;
    context.checkpoint()?;
    freshness.check()?;
    if response.status != 200 {
        return Err(CraftError::new(
            match response.status {
                401 => ErrorCode::AuthRequired,
                403 => ErrorCode::PermissionDenied,
                429 => ErrorCode::RateLimited,
                _ => ErrorCode::RemoteUnavailable,
            },
            "Link metadata could not be qualified; no automatic retry was performed.",
        ));
    }
    if serde_json::to_vec(&response.body)
        .map_err(|_| malformed())?
        .len()
        > 1_048_576
    {
        return Err(malformed());
    }
    if ["errorMessages", "errors", "warningMessages"]
        .iter()
        .any(|key| response.body.get(key).is_some())
    {
        return Err(malformed());
    }
    Ok(response.body)
}
fn endpoint(
    context: &mut dyn OperationContext,
    id: &str,
    project: &str,
    freshness: &Freshness,
) -> Result<(), CraftError> {
    let body = read(
        context,
        &call(
            ISSUE,
            Some(id),
            BTreeMap::from([
                ("fields".into(), "project".into()),
                ("updateHistory".into(), "false".into()),
                ("failFast".into(), "true".into()),
            ]),
        ),
        freshness,
    )?;
    if body["id"] != id || body["fields"]["project"]["id"] != project {
        return Err(CraftError::new(
            ErrorCode::VersionConflict,
            "Link endpoint identity or project differs from its explicit expected target.",
        ));
    }
    Ok(())
}
fn permission(
    context: &mut dyn OperationContext,
    id: &str,
    outward: bool,
    freshness: &Freshness,
) -> Result<(), CraftError> {
    let link = observed_permissions(context, id, outward, freshness)?;
    if outward && !link {
        return Err(CraftError::new(
            ErrorCode::PermissionDenied,
            "Outward Link issues permission is not established.",
        ));
    }
    Ok(())
}
fn observed_permissions(
    context: &mut dyn OperationContext,
    id: &str,
    include_link: bool,
    freshness: &Freshness,
) -> Result<bool, CraftError> {
    let body = read(
        context,
        &call(
            PERMISSIONS,
            None,
            BTreeMap::from([
                ("issueId".into(), id.into()),
                (
                    "permissions".into(),
                    if include_link {
                        "BROWSE_PROJECTS,LINK_ISSUES"
                    } else {
                        "BROWSE_PROJECTS"
                    }
                    .into(),
                ),
            ]),
        ),
        freshness,
    )?;
    if body["permissions"]["BROWSE_PROJECTS"]["havePermission"] != true
        || body["permissions"]["BROWSE_PROJECTS"]["key"] != "BROWSE_PROJECTS"
    {
        return Err(CraftError::new(
            ErrorCode::PermissionDenied,
            "Required issue-scoped linking permission is missing or unestablished.",
        ));
    }
    if !include_link {
        return Ok(false);
    }
    if body["permissions"]["LINK_ISSUES"]["key"] != "LINK_ISSUES" {
        return Err(malformed());
    }
    body["permissions"]["LINK_ISSUES"]["havePermission"]
        .as_bool()
        .ok_or_else(malformed)
}

pub fn prepare_delete(
    read_op: &PreparedOperation,
    write_op: &PreparedOperation,
    context: &mut dyn OperationContext,
    principal: MetadataPrincipal,
    clock: Arc<dyn Clock>,
) -> Result<FrozenLink, CraftError> {
    if read_op.request().command != DELETE_CONTEXT
        || write_op.request().command != DELETE
        || !matches!(read_op.effect(), EffectClass::Read)
        || !matches!(write_op.effect(), EffectClass::Permanent)
        || read_op.request().target != write_op.request().target
        || read_op.request().input != write_op.request().input
    {
        return Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "Expected matching sealed link deletion operations.",
        ));
    }
    let input = delete_input(write_op.request())?;
    principal.grant.validate()?;
    craft_core::contracts::validate_identity("/actor", &principal.actor)?;
    context.verify_operation(read_op)?;
    let freshness = Freshness::new(clock);
    let check_type = |context: &mut dyn OperationContext| -> Result<(), CraftError> {
        let types = parse_types(&read(
            context,
            &call(super::ROUTE, None, BTreeMap::new()),
            &freshness,
        )?)?;
        if !types.items.contains(&input.expected.link_type) {
            return Err(CraftError::new(
                ErrorCode::VersionConflict,
                "Expected deletion link type changed.",
            ));
        }
        Ok(())
    };
    let check_link = |context: &mut dyn OperationContext| -> Result<(), CraftError> {
        let request = NativeCall {
            route: super::reads::LINK_ROUTE.into(),
            path_parameters: BTreeMap::from([("linkId".into(), input.link_id.clone())]),
            query: BTreeMap::new(),
            repeated_query: BTreeMap::new(),
            body: None,
        };
        let row = super::reads::parse_record(&read(context, &request, &freshness)?, None)?;
        if row.id != input.link_id
            || row.inward_issue_id != input.expected.inward_issue_id
            || row.outward_issue_id != input.expected.outward_issue_id
            || row.link_type != input.expected.link_type
        {
            return Err(CraftError::new(
                ErrorCode::VersionConflict,
                "Native link differs from expected deletion identity, type or endpoints.",
            ));
        }
        Ok(())
    };
    check_type(context)?;
    check_link(context)?;
    let expected = &input.expected;
    let project = &write_op.request().target.id;
    endpoint(context, &expected.outward_issue_id, project, &freshness)?;
    endpoint(
        context,
        &expected.inward_issue_id,
        &expected.inward_project_id,
        &freshness,
    )?;
    let outward = observed_permissions(context, &expected.outward_issue_id, true, &freshness)?;
    let inward = observed_permissions(context, &expected.inward_issue_id, true, &freshness)?;
    if !outward && !inward {
        return Err(CraftError::new(
            ErrorCode::PermissionDenied,
            "Link issues permission is required on at least one endpoint.",
        ));
    }
    endpoint(context, &expected.outward_issue_id, project, &freshness)?;
    endpoint(
        context,
        &expected.inward_issue_id,
        &expected.inward_project_id,
        &freshness,
    )?;
    check_link(context)?;
    check_type(context)?;
    context.checkpoint()?;
    freshness.check()?;
    Ok(FrozenLink {
        call: NativeCall {
            route: DELETE_ROUTE.into(),
            path_parameters: BTreeMap::from([("linkId".into(), input.link_id)]),
            query: BTreeMap::new(),
            repeated_query: BTreeMap::new(),
            body: None,
        },
        digest: write_op.digest().into(),
        principal,
        freshness,
    })
}

pub fn interpret_delete(
    target: &ResourceRef,
    input: &LinkDeleteInput,
    response: &craft_core::execution::NativeResponse,
) -> LinkObservation {
    if matches!(response.status, 200 | 204) {
        let mut resources = vec![ResourceRef {
            resource_type: "issue_link".into(),
            id: input.link_id.clone(),
            ..target.clone()
        }];
        resources.extend(
            [
                &input.expected.outward_issue_id,
                &input.expected.inward_issue_id,
            ]
            .map(|id| ResourceRef {
                resource_type: "issue".into(),
                id: id.clone(),
                ..target.clone()
            }),
        );
        return LinkObservation {
            state: SubmissionState::Confirmed,
            resources,
            error: None,
        };
    }
    let failed = matches!(response.status, 400 | 401 | 404);
    let mut error = CraftError::new(
        match response.status {
            401 => ErrorCode::AuthRequired,
            400 => ErrorCode::InputInvalid,
            404 => ErrorCode::RemoteUnavailable,
            429 => ErrorCode::RateLimited,
            _ => ErrorCode::WriteOutcomeUnknown,
        },
        "Link DELETE did not return its documented acknowledgment; no retry was performed.",
    );
    error.details.write_state = if failed {
        craft_core::contracts::RequestState::Failed
    } else {
        craft_core::contracts::RequestState::SentUnknown
    };
    LinkObservation {
        state: if failed {
            SubmissionState::Failed
        } else {
            SubmissionState::Unknown
        },
        resources: vec![],
        error: Some(error),
    }
}
pub fn prepare_link(
    read_op: &PreparedOperation,
    write_op: &PreparedOperation,
    context: &mut dyn OperationContext,
    principal: MetadataPrincipal,
    clock: Arc<dyn Clock>,
) -> Result<FrozenLink, CraftError> {
    if read_op.request().command != CONTEXT
        || write_op.request().command != CREATE
        || !matches!(read_op.effect(), EffectClass::Read)
        || !matches!(write_op.effect(), EffectClass::Write)
        || read_op.request().target != write_op.request().target
        || read_op.request().input != write_op.request().input
    {
        return Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "Expected matching sealed link metadata and write operations.",
        ));
    }
    let input = input(write_op.request())?;
    principal.grant.validate()?;
    craft_core::contracts::validate_identity("/actor", &principal.actor)?;
    context.verify_operation(read_op)?;
    let freshness = Freshness::new(clock);
    let check_type = |context: &mut dyn OperationContext| -> Result<(), CraftError> {
        let types = parse_types(&read(
            context,
            &call(super::ROUTE, None, BTreeMap::new()),
            &freshness,
        )?)?;
        if !types.items.contains(&input.link_type) {
            return Err(CraftError::new(
                ErrorCode::VersionConflict,
                "Expected native link type or directional labels changed.",
            ));
        }
        Ok(())
    };
    check_type(context)?;
    let outward_project = &write_op.request().target.id;
    endpoint(
        context,
        &input.outward_issue_id,
        outward_project,
        &freshness,
    )?;
    endpoint(
        context,
        &input.inward_issue_id,
        &input.inward_project_id,
        &freshness,
    )?;
    permission(context, &input.outward_issue_id, true, &freshness)?;
    permission(context, &input.inward_issue_id, false, &freshness)?;
    endpoint(
        context,
        &input.outward_issue_id,
        outward_project,
        &freshness,
    )?;
    endpoint(
        context,
        &input.inward_issue_id,
        &input.inward_project_id,
        &freshness,
    )?;
    check_type(context)?;
    context.checkpoint()?;
    freshness.check()?;
    Ok(FrozenLink {
        call: NativeCall {
            route: CREATE_ROUTE.into(),
            path_parameters: BTreeMap::new(),
            query: BTreeMap::new(),
            repeated_query: BTreeMap::new(),
            body: Some(
                json!({"type":{"id":input.link_type.id},"inwardIssue":{"id":input.inward_issue_id},"outwardIssue":{"id":input.outward_issue_id}}),
            ),
        },
        digest: write_op.digest().into(),
        principal,
        freshness,
    })
}
pub struct LinkObservation {
    pub state: SubmissionState,
    pub resources: Vec<ResourceRef>,
    pub error: Option<CraftError>,
}
pub fn interpret(
    target: &ResourceRef,
    input: &LinkInput,
    response: &craft_core::execution::NativeResponse,
) -> LinkObservation {
    if response.status == 201 {
        return LinkObservation {
            state: SubmissionState::Confirmed,
            resources: [&input.outward_issue_id, &input.inward_issue_id]
                .map(|id| ResourceRef {
                    resource_type: "issue".into(),
                    id: id.clone(),
                    ..target.clone()
                })
                .into(),
            error: None,
        };
    }
    let failed = matches!(response.status, 400 | 401 | 404 | 413);
    let state = if failed {
        SubmissionState::Failed
    } else {
        SubmissionState::Unknown
    };
    let mut error = CraftError::new(
        match response.status {
            401 => ErrorCode::AuthRequired,
            400 | 413 => ErrorCode::InputInvalid,
            404 => ErrorCode::RemoteUnavailable,
            429 => ErrorCode::RateLimited,
            _ => ErrorCode::WriteOutcomeUnknown,
        },
        "Issue-link POST did not return its documented success; no retry was performed.",
    );
    error.details.write_state = if failed {
        craft_core::contracts::RequestState::Failed
    } else {
        craft_core::contracts::RequestState::SentUnknown
    };
    LinkObservation {
        state,
        resources: vec![],
        error: Some(error),
    }
}
