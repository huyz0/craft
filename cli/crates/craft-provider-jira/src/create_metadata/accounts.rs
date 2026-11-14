use craft_core::contracts::{CanonicalRequest, EffectClass, Envelope, SCHEMA_VERSION};
use craft_core::execution::{HttpMethod, NativeRoute, OperationHandler, RouteSegment};
use craft_core::projection::{ProjectionContract, ProjectionMode};
use craft_core::registry::{
    OperationDescriptor, ReconciliationClass, Registry, RetryClass, response_schema, schema_for,
};

use super::*;

pub const COMMAND: &str = "jira.metadata.assignee";

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AccountRequest {
    #[schemars(length(min = 1, max = 128))]
    account_id: String,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssigneeSummary {
    pub target: ResourceRef,
    pub account_id: String,
    pub observed_at: u64,
    #[schemars(extend("const"=true))]
    pub active: bool,
    #[schemars(extend("const"=true))]
    pub assignable: bool,
    #[schemars(extend("const"=false))]
    pub mutation_authorized: bool,
}

pub struct AssigneeContext {
    summary: AssigneeSummary,
    principal: MetadataPrincipal,
    pub(super) freshness: Freshness,
}
impl AssigneeContext {
    pub fn summary(&self) -> &AssigneeSummary {
        &self.summary
    }
    pub(super) fn verify(
        &self,
        target: &ResourceRef,
        principal: &MetadataPrincipal,
        account: &str,
    ) -> Result<(), CraftError> {
        if &self.summary.target != target
            || &self.principal != principal
            || self.summary.account_id != account
        {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Assignable account evidence does not match the target, principal and account.",
            ));
        }
        self.freshness.check()
    }
}

pub(super) fn valid_account(id: &str) -> bool {
    id.len() <= 128
        && !matches!(id, "-1" | "null" | "unassigned" | "unknown")
        && craft_core::contracts::validate_identity("/account_id", id).is_ok()
        && !id.chars().any(char::is_whitespace)
}

fn input(operation: &PreparedOperation) -> Result<AccountRequest, CraftError> {
    if operation.request().command != COMMAND || !matches!(operation.effect(), EffectClass::Read) {
        return Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "Expected an assignable-account read operation.",
        ));
    }
    crate::created_issue::validate_target(&operation.request().target)?;
    let input: AccountRequest = serde_json::from_value(operation.request().input.clone())
        .map_err(|_| CraftError::field("/account_id", "invalid_account_lookup"))?;
    if !valid_account(&input.account_id) {
        return Err(CraftError::field(
            "/account_id",
            "expected_exact_account_id",
        ));
    }
    Ok(input)
}

fn fetch(
    operation: &PreparedOperation,
    context: &mut dyn OperationContext,
    observed_at: u64,
) -> Result<AssigneeSummary, CraftError> {
    let input = input(operation)?;
    context.verify_operation(operation)?;
    context.checkpoint()?;
    let response = context.invoke(&NativeCall {
        repeated_query: std::collections::BTreeMap::new(),
        route: "metadata-assignee".into(),
        path_parameters: BTreeMap::new(),
        query: BTreeMap::from([
            ("project".into(), operation.request().target.id.clone()),
            ("accountId".into(), input.account_id.clone()),
            ("startAt".into(), "0".into()),
            ("maxResults".into(), "2".into()),
        ]),
        body: None,
    })?;
    if response.status != 200 {
        let code = match response.status {
            401 => ErrorCode::AuthRequired,
            403 => ErrorCode::PermissionDenied,
            429 => ErrorCode::RateLimited,
            _ => ErrorCode::RemoteUnavailable,
        };
        return Err(CraftError::new(
            code,
            "Assignable-account lookup did not return qualified evidence; no retry was performed.",
        ));
    }
    let values = response
        .body
        .as_array()
        .filter(|values| values.len() <= 2)
        .ok_or_else(malformed)?;
    if values.len() != 1
        || values[0].get("accountId").and_then(Value::as_str) != Some(input.account_id.as_str())
        || values[0].get("active").and_then(Value::as_bool) != Some(true)
    {
        return Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "The exact active account was not positively qualified as assignable in this project.",
        ));
    }
    context.checkpoint()?;
    Ok(AssigneeSummary {
        target: operation.request().target.clone(),
        account_id: input.account_id,
        observed_at,
        active: true,
        assignable: true,
        mutation_authorized: false,
    })
}

/// Positive evidence only; missing results do not prove account absence or project permission.
pub fn read_assignable(
    operation: &PreparedOperation,
    context: &mut dyn OperationContext,
    principal: MetadataPrincipal,
    clock: Arc<dyn Clock>,
) -> Result<AssigneeContext, CraftError> {
    principal.validate()?;
    input(operation)?;
    let freshness = Freshness {
        wall: clock.unix_millis(),
        monotonic: clock.monotonic_millis(),
        clock,
    };
    let summary = fetch(operation, context, freshness.wall)?;
    freshness.check()?;
    Ok(AssigneeContext {
        summary,
        principal,
        freshness,
    })
}

pub(super) fn register(registry: &mut Registry, clock: Arc<dyn Clock>) -> Result<(), CraftError> {
    registry.register_typed_handler::<AccountRequest>(OperationDescriptor{
        id:COMMAND.into(),provider:"jira".into(),contract_version:SCHEMA_VERSION.into(),api_revision:"jira-cloud-v3".into(),description:"Qualify one exact active assignable account in a project; no mutation authority.".into(),effect:EffectClass::Read,
        input_schema:schema_for::<AccountRequest>()?,response_schema:response_schema::<AssigneeSummary>()?,example:json!({"account_id":"account-one"}),response_example:json!(Envelope::success(COMMAND,"example".into(),json!({"target":{"provider":"jira","deployment":"cloud","tenant":"00000000-1111-2222-3333-444444444444","resource_type":"project","id":"10000"},"account_id":"account-one","observed_at":0,"active":true,"assignable":true,"mutation_authorized":false}))),
        projection:Some(ProjectionContract{item_pointer:"".into(),always_returned:["target","account_id","observed_at","active","assignable","mutation_authorized"].map(String::from).into(),default_selected:["target","account_id","observed_at","active","assignable","mutation_authorized"].map(String::from).into(),mode:ProjectionMode::Local}),
        scope_alternatives:vec![vec!["read:jira-user".into()]],permissions:vec!["Browse users and groups OR Assign issues".into()],tenant_conditions:vec!["Exact project/account positive result; no inferred absence or unassignment policy.".into()],
        routes:vec![NativeRoute{
repeated_query_parameters: Vec::new(),id:"metadata-assignee".into(),method:HttpMethod::Get,path:["rest","api","3","user","assignable","search"].map(|value|RouteSegment::Literal(value.into())).into(),query_parameters:["project","accountId","startAt","maxResults"].map(String::from).into(),continuation_parameters:vec![],response_max_bytes:65_536}],retry_class:RetryClass::SafeRead,reconciliation:ReconciliationClass::NotApplicable,
    },Arc::new(Handler{clock}))
}
struct Handler {
    clock: Arc<dyn Clock>,
}
impl OperationHandler for Handler {
    fn validate(&self, request: &CanonicalRequest) -> Result<(), CraftError> {
        crate::created_issue::validate_target(&request.target)?;
        let input: AccountRequest = serde_json::from_value(request.input.clone())
            .map_err(|_| CraftError::field("/account_id", "invalid_account_lookup"))?;
        if !valid_account(&input.account_id) {
            return Err(CraftError::field(
                "/account_id",
                "expected_exact_account_id",
            ));
        }
        Ok(())
    }
    fn execute(
        &self,
        operation: &PreparedOperation,
        context: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        serde_json::to_value(fetch(operation, context, self.clock.unix_millis())?)
            .map_err(|_| malformed())
    }
}
