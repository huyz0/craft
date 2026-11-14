//! Native link-type metadata. Labels are observed data, never an implicit relation choice.
pub mod reads;
pub mod write;
use craft_core::contracts::{CanonicalRequest, EffectClass, Envelope, SCHEMA_VERSION};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{
    HttpMethod, NativeCall, NativeRoute, OperationContext, OperationHandler, PreparedOperation,
    RouteSegment,
};
use craft_core::registry::{
    OperationDescriptor, ReconciliationClass, Registry, RetryClass, response_schema, schema_for,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub const TYPES: &str = "jira.link.types";
const ROUTE: &str = "issue-link-types";
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TypesInput {}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LinkType {
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub id: String,
    #[schemars(length(min = 1, max = 256))]
    pub name: String,
    #[schemars(length(min = 1, max = 256))]
    pub inward: String,
    #[schemars(length(min = 1, max = 256))]
    pub outward: String,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LinkTypes {
    pub items: Vec<LinkType>,
}
fn malformed() -> CraftError {
    CraftError::new(
        ErrorCode::RemoteUnavailable,
        "Jira link-type metadata is incomplete, inconsistent or exceeds its bound.",
    )
}
fn text(value: &Value, key: &str) -> Result<String, CraftError> {
    let value = value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(malformed)?;
    if value.trim().is_empty()
        || value.trim() != value
        || value.len() > 256
        || value.chars().any(char::is_control)
    {
        return Err(malformed());
    }
    Ok(value.into())
}
pub fn parse_types(body: &Value) -> Result<LinkTypes, CraftError> {
    if !body.is_object()
        || [
            "errors",
            "errorMessages",
            "warningMessages",
            "nextPage",
            "next",
            "startAt",
            "total",
            "isLast",
        ]
        .iter()
        .any(|key| body.get(key).is_some())
    {
        return Err(malformed());
    }
    let rows = body
        .get("issueLinkTypes")
        .and_then(Value::as_array)
        .filter(|rows| rows.len() <= 256)
        .ok_or_else(malformed)?;
    let mut ids = BTreeSet::new();
    let mut items = Vec::new();
    for row in rows {
        let id = text(row, "id")?;
        if !id
            .parse::<i64>()
            .is_ok_and(|n| n > 0 && n.to_string() == id)
            || !ids.insert(id.clone())
        {
            return Err(malformed());
        }
        items.push(LinkType {
            id,
            name: text(row, "name")?,
            inward: text(row, "inward")?,
            outward: text(row, "outward")?,
        });
    }
    items.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(LinkTypes { items })
}
pub fn read_types(
    operation: &PreparedOperation,
    context: &mut dyn OperationContext,
) -> Result<LinkTypes, CraftError> {
    if operation.request().command != TYPES || !matches!(operation.effect(), EffectClass::Read) {
        return Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "Expected a sealed Jira link-type read.",
        ));
    }
    context.verify_operation(operation)?;
    context.checkpoint()?;
    let response = context.invoke(&NativeCall {
        route: ROUTE.into(),
        path_parameters: BTreeMap::new(),
        query: BTreeMap::new(),
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
            "Jira link-type read failed; no automatic retry was performed.",
        ));
    }
    if serde_json::to_vec(&response.body)
        .map_err(|_| malformed())?
        .len()
        > 1_048_576
    {
        return Err(malformed());
    }
    parse_types(&response.body)
}
struct Handler;
impl OperationHandler for Handler {
    fn validate(&self, request: &CanonicalRequest) -> Result<(), CraftError> {
        crate::created_issue::validate_target(&request.target)?;
        if request.preconditions.is_some()
            || request.selection.cursor.is_some()
            || !request.selection.fields.is_empty()
        {
            return Err(CraftError::field(
                "/request",
                "complete_link_type_metadata_required",
            ));
        }
        serde_json::from_value::<TypesInput>(request.input.clone())
            .map_err(|_| CraftError::field("/input", "expected_empty_link_types_input"))?;
        Ok(())
    }
    fn execute(
        &self,
        operation: &PreparedOperation,
        context: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        serde_json::to_value(read_types(operation, context)?).map_err(|_| malformed())
    }
}
pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    registry.register_typed_handler::<TypesInput>(
        OperationDescriptor {
            id: TYPES.into(),
            provider: "jira".into(),
            contract_version: SCHEMA_VERSION.into(),
            api_revision: "jira-cloud-v3".into(),
            description: "Read complete native issue-link types for explicit relation mapping."
                .into(),
            effect: EffectClass::Read,
            input_schema: schema_for::<TypesInput>()?,
            response_schema: response_schema::<LinkTypes>()?,
            example: json!({}),
            response_example: json!(Envelope::success(
                TYPES,
                "example".into(),
                json!({"items":[]})
            )),
            projection: None,
            scope_alternatives: vec![
                vec!["read:jira-work".into()],
                vec!["read:issue-link-type:jira".into()],
            ],
            permissions: vec![
                "Link-type visibility does not establish Link issues permission.".into(),
            ],
            tenant_conditions: vec![
                "Issue linking must be enabled; labels and relation choice remain explicit.".into(),
            ],
            routes: vec![NativeRoute {
                id: ROUTE.into(),
                method: HttpMethod::Get,
                path: ["rest", "api", "3", "issueLinkType"]
                    .map(|s| RouteSegment::Literal(s.into()))
                    .into(),
                query_parameters: vec![],
                continuation_parameters: vec![],
                repeated_query_parameters: vec![],
                response_max_bytes: 1_048_576,
            }],
            retry_class: RetryClass::SafeRead,
            reconciliation: ReconciliationClass::NotApplicable,
        },
        Arc::new(Handler),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn link_catalog_requires_complete_unique_native_semantics() -> Result<(), CraftError> {
        let row = json!({"id":"10000","name":"Blocks","inward":"is blocked by","outward":"blocks","self":"https://example.invalid/ignored"});
        assert_eq!(
            parse_types(&json!({"issueLinkTypes":[row.clone()]}))?.items[0].outward,
            "blocks"
        );
        for body in [
            json!({}),
            json!({"issueLinkTypes":[row.clone(),row.clone()]}),
            json!({"issueLinkTypes":[],"nextPage":"more"}),
            json!({"issueLinkTypes":[],"warningMessages":[]}),
            json!({"issueLinkTypes":[{"id":"10000","name":"Blocks","outward":"blocks"}]}),
        ] {
            assert!(parse_types(&body).is_err(), "{body}");
        }
        let mut invalid = row;
        invalid["id"] = json!("010000");
        assert!(parse_types(&json!({"issueLinkTypes":[invalid]})).is_err());
        Ok(())
    }
}
