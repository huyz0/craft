//! Complete attachment bytes use a fixed content route and a verified streaming host.
use craft_core::contracts::{
    CanonicalRequest, CredentialCacheUpdate, EffectClass, Envelope, RequestState, ResourceRef,
    SCHEMA_VERSION,
};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{
    HttpMethod, NativeCall, NativeRoute, OperationContext, OperationHandler, PreparedOperation,
    RouteSegment,
};
use craft_core::registry::{
    OperationDescriptor, ReconciliationClass, Registry, RetryClass, response_schema, schema_for,
};
use craft_core::storage::ArtifactReceipt;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

pub const DOWNLOAD: &str = "jira.attachment.download";
const ROUTE: &str = "jira-attachment-content";
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DownloadInput {
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub id: String,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DownloadData {
    pub target: ResourceRef,
    pub attachment_id: String,
    pub artifact_name: String,
    pub artifact: Option<ArtifactReceipt>,
    pub artifact_write_state: RequestState,
    pub exported: Option<ArtifactReceipt>,
    pub export_write_state: RequestState,
    pub received_bytes: u64,
    pub response_media_type: Option<String>,
    pub credential_cache_update: Option<CredentialCacheUpdate>,
}
pub fn validate(request: &CanonicalRequest) -> Result<(), CraftError> {
    crate::projects::validate_target(&request.target)?;
    let input: DownloadInput = serde_json::from_value(request.input.clone())
        .map_err(|_| CraftError::field("/input", "expected_attachment_download_id"))?;
    if request.command != DOWNLOAD
        || !crate::attachments::numeric(&input.id)
        || request.preconditions.is_some()
        || request.selection.cursor.is_some()
        || !request.selection.fields.is_empty()
    {
        return Err(CraftError::field(
            "/request",
            "expected_numeric_attachment_download_without_cursor_projection_or_preconditions",
        ));
    }
    Ok(())
}
pub fn call(operation: &PreparedOperation) -> Result<NativeCall, CraftError> {
    validate(operation.request())?;
    Ok(NativeCall {
        route: ROUTE.into(),
        path_parameters: BTreeMap::from([(
            "id".into(),
            operation.request().input["id"]
                .as_str()
                .ok_or_else(|| CraftError::field("/id", "expected_attachment_id"))?
                .into(),
        )]),
        query: BTreeMap::from([("redirect".into(), "false".into())]),
        repeated_query: BTreeMap::new(),
        body: None,
    })
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
            "Attachment bytes require the verified streaming host.",
        ))
    }
}
pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    let target = ResourceRef {
        provider: "jira".into(),
        deployment: "cloud".into(),
        tenant: "00000000-1111-2222-3333-444444444444".into(),
        resource_type: "site".into(),
        id: "00000000-1111-2222-3333-444444444444".into(),
    };
    let data = DownloadData {
        target,
        attachment_id: "10001".into(),
        artifact_name: "attachment.bin".into(),
        artifact: None,
        artifact_write_state: RequestState::NotSent,
        exported: None,
        export_write_state: RequestState::NotSent,
        received_bytes: 0,
        response_media_type: None,
        credential_cache_update: None,
    };
    registry.register_typed_handler::<DownloadInput>(OperationDescriptor {
        id:DOWNLOAD.into(), provider:"jira".into(), contract_version:SCHEMA_VERSION.into(), api_revision:"jira-v3-attachment-content-v1".into(),
        description:"Stream complete attachment bytes into an immutable artifact and explicit guarded output; no redirects or automatic retries.".into(),effect:EffectClass::Read,
        input_schema:schema_for::<DownloadInput>()?, response_schema:response_schema::<DownloadData>()?,example:json!({"id":"10001"}),response_example:json!(Envelope::success(DOWNLOAD,"example".into(),json!(data))),projection:None,
        scope_alternatives:vec![vec!["read:jira-work".into()]],permissions:vec!["Native Browse projects, issue security and private-comment restrictions apply.".into()],
        tenant_conditions:vec!["Explicit download allowance required; HTTP 206/303 do not publish. Local artifact and export receipts have independent dispositions.".into()],
        routes:vec![NativeRoute {id:ROUTE.into(),method:HttpMethod::Get,path:["rest","api","3","attachment","content"].into_iter().map(|v|RouteSegment::Literal(v.into())).chain([RouteSegment::Parameter("id".into())]).collect(),query_parameters:vec!["redirect".into()],continuation_parameters:vec![],repeated_query_parameters:vec![],response_max_bytes:512}],
        retry_class:RetryClass::SafeRead,reconciliation:ReconciliationClass::NotApplicable,
    },Arc::new(Handler))
}
#[cfg(test)]
mod tests {
    use super::*;
    use craft_core::contracts::Selection;
    #[test]
    fn compiled_download_route_keeps_redirect_disabled_and_refuses_metadata_or_projection()
    -> Result<(), CraftError> {
        let mut registry = Registry::default();
        register(&mut registry)?;
        let mut request = CanonicalRequest {
            command: DOWNLOAD.into(),
            target: ResourceRef {
                provider: "jira".into(),
                deployment: "cloud".into(),
                tenant: "00000000-1111-2222-3333-444444444444".into(),
                resource_type: "site".into(),
                id: "00000000-1111-2222-3333-444444444444".into(),
            },
            input: json!({"id":"10001"}),
            selection: Selection {
                fields: vec![],
                limit: 20,
                cursor: None,
                max_bytes: 262144,
            },
            preconditions: None,
        };
        let operation = registry.prepare(request.clone())?;
        let call = call(&operation)?;
        assert_eq!(
            call.query,
            BTreeMap::from([("redirect".into(), "false".into())])
        );
        assert_eq!(call.path_parameters["id"], "10001");
        assert!(call.body.is_none());
        request.selection.fields = vec!["artifact".into()];
        assert!(registry.prepare(request.clone()).is_err());
        request.selection.fields.clear();
        request.input = json!({"id":"10001","url":"https://invalid.example"});
        assert!(registry.prepare(request.clone()).is_err());
        request.input = json!({"id":"01"});
        assert!(registry.prepare(request).is_err());
        Ok(())
    }
}
