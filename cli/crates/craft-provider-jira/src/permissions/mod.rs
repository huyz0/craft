//! Current-user permission observations retain context semantics and never confer mutation authority.
mod models;
mod selection;
pub use models::*;
pub use selection::{FieldPlan, projection};

use crate::create_metadata::MetadataPrincipal;
use craft_core::contracts::{
    CanonicalRequest, EffectClass, Envelope, SCHEMA_VERSION, validate_identity,
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
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub const COMMAND: &str = "jira.permission.check";
pub const ROUTE: &str = "jira-current-user-permissions";
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PermissionInput {
    #[schemars(length(min = 1, max = 32))]
    pub permissions: Vec<String>,
    pub project: Option<String>,
    pub issue: Option<String>,
    pub comment_id: Option<String>,
}
impl PermissionInput {
    pub fn validate(&self) -> Result<(), CraftError> {
        let mut unique = BTreeSet::new();
        if self.permissions.is_empty()
            || self.permissions.len() > 32
            || self.permissions.iter().any(|key| {
                key.is_empty()
                    || key.len() > 256
                    || !key.bytes().all(|b| {
                        b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-')
                    })
                    || !unique.insert(key)
            })
        {
            return Err(CraftError::field(
                "/permissions",
                "expected_one_to_thirty_two_unique_native_permission_keys",
            ));
        }
        if [
            self.project.is_some(),
            self.issue.is_some(),
            self.comment_id.is_some(),
        ]
        .into_iter()
        .filter(|set| *set)
        .count()
            > 1
        {
            return Err(CraftError::field(
                "/context",
                "expected_at_most_one_permission_context",
            ));
        }
        if let Some(project) = &self.project
            && !crate::project_context::numeric(project)
            && !crate::project_context::key(project)
        {
            return Err(CraftError::field("/project", "expected_project_id_or_key"));
        }
        if let Some(issue) = &self.issue {
            crate::issue_reads::IssueGetInput {
                issue: issue.clone(),
            }
            .validate()?;
        }
        if let Some(id) = &self.comment_id {
            if !crate::issue_reads::numeric_id(id) {
                return Err(CraftError::field(
                    "/comment_id",
                    "expected_numeric_comment_id",
                ));
            }
            if self.permissions != ["BROWSE_PROJECTS"] {
                return Err(CraftError::field(
                    "/permissions",
                    "comment_context_requires_only_browse_projects",
                ));
            }
        }
        Ok(())
    }
    pub fn context(&self) -> Result<PermissionContext, CraftError> {
        self.validate()?;
        let (kind, selector, parameter, evaluation) = if let Some(project) = &self.project {
            (
                ContextKind::Project,
                Some(project.clone()),
                Some(
                    if crate::project_context::numeric(project) {
                        "projectId"
                    } else {
                        "projectKey"
                    }
                    .into(),
                ),
                ProjectPermissionEvaluation::AnyQualifyingIssueOrProject,
            )
        } else if let Some(issue) = &self.issue {
            (
                ContextKind::Issue,
                Some(issue.clone()),
                Some(
                    if crate::issue_reads::numeric_id(issue) {
                        "issueId"
                    } else {
                        "issueKey"
                    }
                    .into(),
                ),
                ProjectPermissionEvaluation::RequestedIssueOrContainingProject,
            )
        } else if let Some(id) = &self.comment_id {
            (
                ContextKind::Comment,
                Some(id.clone()),
                Some("commentId".into()),
                ProjectPermissionEvaluation::CommentVisibilityAndParentIssue,
            )
        } else {
            (
                ContextKind::Global,
                None,
                None,
                ProjectPermissionEvaluation::AnyProject,
            )
        };
        Ok(PermissionContext {
            kind,
            requested_selector: selector,
            native_parameter: parameter,
            project_permission_evaluation: evaluation,
        })
    }
    fn query(&self) -> Result<BTreeMap<String, String>, CraftError> {
        let context = self.context()?;
        let mut query = BTreeMap::from([("permissions".into(), self.permissions.join(","))]);
        if let (Some(name), Some(value)) = (context.native_parameter, context.requested_selector) {
            query.insert(name, value);
        }
        Ok(query)
    }
}
pub fn input(request: &CanonicalRequest) -> Result<PermissionInput, CraftError> {
    if request.command != COMMAND {
        return Err(CraftError::field("/command", "expected_permission_check"));
    }
    crate::projects::validate_target(&request.target)?;
    let input: PermissionInput = serde_json::from_value(request.input.clone())
        .map_err(|_| CraftError::field("/input", "invalid_permission_input"))?;
    input.validate()?;
    Ok(input)
}
pub fn response() -> Result<Value, CraftError> {
    response_schema::<PermissionData>()
}
pub fn route() -> NativeRoute {
    NativeRoute {
        id: ROUTE.into(),
        method: HttpMethod::Get,
        path: ["rest", "api", "3", "mypermissions"]
            .map(|value| RouteSegment::Literal(value.into()))
            .into(),
        query_parameters: [
            "permissions",
            "projectId",
            "projectKey",
            "issueId",
            "issueKey",
            "commentId",
        ]
        .map(String::from)
        .into(),
        continuation_parameters: vec![],
        repeated_query_parameters: vec![],
        response_max_bytes: 1_048_576,
    }
}
pub struct JiraPermissions;
impl ProviderModule for JiraPermissions {
    fn manifest(&self) -> ProviderManifest {
        ProviderManifest {
            id: "jira".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            deployments: vec!["cloud".into()],
            contract_version: SCHEMA_VERSION.into(),
        }
    }
    fn register(&self, registry: &mut Registry) -> Result<(), CraftError> {
        let input = PermissionInput {
            permissions: vec!["BROWSE_PROJECTS".into()],
            project: None,
            issue: Some("ED-1".into()),
            comment_id: None,
        };
        registry.register_typed_handler::<PermissionInput>(OperationDescriptor {
            id:COMMAND.into(),provider:"jira".into(),contract_version:SCHEMA_VERSION.into(),api_revision:"jira-cloud-v3".into(),
            description:"Observe current-user permission flags in an explicit native context; no mutation authority.".into(),
            effect:EffectClass::Read,input_schema:schema_for::<PermissionInput>()?,response_schema:response()?,example:json!(input),
            response_example:json!(Envelope::success(COMMAND,"example".into(),json!({"target":{"provider":"jira","deployment":"cloud","tenant":"00000000-1111-2222-3333-444444444444","resource_type":"site","id":"00000000-1111-2222-3333-444444444444"},"context":input.context()?,"requested_permissions":input.permissions,"items":[{"key":"BROWSE_PROJECTS","returned":true,"have_permission":true,"kind":"PROJECT","name":"Browse projects"}],"observation":null,"cached":false,"credential_cache_update":null,"atomic_snapshot":false,"mutation_authorized":false}))),
            projection:Some(projection()),scope_alternatives:vec![vec!["read:jira-work".into()]],
            permissions:vec!["No native permission required; results retain global/project/issue/comment evaluation semantics and do not establish resource visibility or applicability.".into()],
            tenant_conditions:vec!["Exact site and verified current actor; no other-user lookup, bulk POST or anonymous fallback.".into()],
            routes:vec![route()],retry_class:RetryClass::SafeRead,reconciliation:ReconciliationClass::NotApplicable,
        },Arc::new(Handler))
    }
}
struct Handler;
impl OperationHandler for Handler {
    fn validate(&self, request: &CanonicalRequest) -> Result<(), CraftError> {
        input(request)?;
        if request.preconditions.is_some() {
            return Err(CraftError::field(
                "/preconditions",
                "permission_read_preconditions_unsupported",
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
            "Permission reads require the verified buffered-read host.",
        ))
    }
}
fn malformed() -> CraftError {
    CraftError::new(
        ErrorCode::RemoteUnavailable,
        "Permission response did not establish bounded requested-key observations.",
    )
}
fn expired() -> CraftError {
    CraftError::new(
        ErrorCode::VersionConflict,
        "Permission observation is stale or its clock moved backward; read fresh context before preparing another action.",
    )
}
/// Non-deserializable observation only; even a fresh instance grants no mutation authority.
pub struct LivePermissionObservation {
    page: NativePage,
    operation: String,
    principal: MetadataPrincipal,
    wall: u64,
    monotonic: u64,
}
impl LivePermissionObservation {
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
                "Permission observation differs from the operation or principal.",
            ));
        }
        let wall = clock.unix_millis();
        let monotonic = clock.monotonic_millis();
        if wall < self.wall
            || monotonic < self.monotonic
            || wall - self.wall >= 60_000
            || monotonic - self.monotonic >= 60_000
        {
            return Err(expired());
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
) -> Result<LivePermissionObservation, CraftError> {
    context.verify_operation(operation)?;
    plan.verify(operation)?;
    let input = input(operation.request())?;
    principal.grant.validate()?;
    validate_identity("/actor", &principal.actor)?;
    if matches!(principal.actor.as_str(), "unknown" | "anonymous") {
        return Err(CraftError::field("/actor", "expected_verified_actor"));
    }
    if position.is_some() {
        return Err(CraftError::field(
            "/cursor",
            "expected_initial_unpaginated_permission_read",
        ));
    }
    let wall = clock.unix_millis();
    let monotonic = clock.monotonic_millis();
    context.checkpoint()?;
    let response = context.invoke(&NativeCall {
        route: ROUTE.into(),
        path_parameters: BTreeMap::new(),
        query: input.query()?,
        repeated_query: BTreeMap::new(),
        body: None,
    })?;
    if response.status != 200 {
        let mut error = CraftError::new(
            match response.status {
                400 | 422 => ErrorCode::InputInvalid,
                401 => ErrorCode::AuthRequired,
                403 => ErrorCode::PermissionDenied,
                429 => ErrorCode::RateLimited,
                _ => ErrorCode::RemoteUnavailable,
            },
            "Permission read failed; no permission or resource absence was inferred and no automatic retry was performed.",
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
            .is_some_and(|value| value.as_object().is_none_or(|values| !values.is_empty()))
    {
        return Err(malformed());
    }
    let items = plan.normalize(
        body.get("permissions").ok_or_else(malformed)?,
        &input.permissions,
        context,
    )?;
    let result = LivePermissionObservation {
        page: NativePage {
            items,
            next: None,
            context: None,
        },
        operation: operation.digest().into(),
        principal: principal.clone(),
        wall,
        monotonic,
    };
    context.checkpoint()?;
    result.verify(operation, principal, clock)?;
    Ok(result)
}
