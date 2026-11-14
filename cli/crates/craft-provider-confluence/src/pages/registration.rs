use super::*;
use craft_core::contracts::{Envelope, SCHEMA_VERSION};
use craft_core::execution::{
    HttpMethod, NativeRoute, OperationContext, OperationHandler, RouteSegment,
};
use craft_core::registry::{
    OperationDescriptor, ProviderManifest, ProviderModule, ReconciliationClass, Registry,
    RetryClass, response_schema, schema_for,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageWriteData {
    pub target: ResourceRef,
    pub space_id: String,
    pub page_id: String,
    pub version: u32,
}
pub struct ConfluencePages;
pub(super) fn route(update: bool) -> NativeRoute {
    let mut path: Vec<_> = ["wiki", "api", "v2", "pages"]
        .into_iter()
        .map(|p| RouteSegment::Literal(p.into()))
        .collect();
    if update {
        path.push(RouteSegment::Parameter("id".into()));
    }
    NativeRoute {
        id: if update { UPDATE_ROUTE } else { CREATE_ROUTE }.into(),
        method: if update {
            HttpMethod::Put
        } else {
            HttpMethod::Post
        },
        path,
        query_parameters: if update {
            vec![]
        } else {
            vec!["root-level".into()]
        },
        continuation_parameters: vec![],
        repeated_query_parameters: vec![],
        response_max_bytes: 1_048_576,
    }
}
impl ProviderModule for ConfluencePages {
    fn manifest(&self) -> ProviderManifest {
        crate::ConfluenceContext.manifest()
    }
    fn register(&self, registry: &mut Registry) -> Result<(), CraftError> {
        let target = json!({"provider":"confluence","deployment":"cloud","tenant":"00000000-1111-2222-3333-444444444444","resource_type":"site","id":"00000000-1111-2222-3333-444444444444"});
        let body = json!({"representation":"storage","value":"<p>Plan for human review</p>"});
        let digest = canonical_digest(
            &json!({"id":"101","space_id":"100","title":"Plan draft","version":1,"body":body}),
        )?;
        for command in [PAGE_CREATE, PAGE_UPDATE] {
            let update = command == PAGE_UPDATE;
            let descriptor = OperationDescriptor {
                id:command.into(), provider:"confluence".into(), contract_version:SCHEMA_VERSION.into(), api_revision:"confluence-cloud-v2".into(),
                description:"Prepare normal shared-page publication using verified context and a journaled host; publication is not approval.".into(),
                effect:EffectClass::Write,
                input_schema:if update { schema_for::<PageUpdateInput>()? } else { schema_for::<PageCreateInput>()? },
                response_schema:response_schema::<PageWriteData>()?,
                example:if update { json!({"id":"101","space_id":"100","expected":{"version":1,"digest":digest,"representation":"storage"},"editor_draft_policy":"native_reconciliation_may_overwrite","title":"Reviewed plan"}) } else { json!({"space_id":"100","title":"Plan draft","placement":{"kind":"root"},"body":body}) },
                response_example:json!(Envelope::success(command,"example".into(),json!({"target":target,"space_id":"100","page_id":"101","version":if update { 2 } else { 1 }}))),
                projection:None, scope_alternatives:vec![vec!["write:page:confluence".into()]],
                permissions:vec!["Native Confluence space/content visibility and page creation or editing permissions.".into()],
                tenant_conditions:vec!["Exact site/space/grant/actor and fresh qualified current evidence; explicit reviewed revision for updates. Updates require the explicit native_reconciliation_may_overwrite editor-draft policy: publication can overwrite divergent unpublished editor drafts and draft preservation is never verified.".into()],
                routes:vec![route(update)], retry_class:RetryClass::ReconcileBeforeReplay, reconciliation:ReconciliationClass::KnownResource,
            };
            if update {
                registry
                    .register_typed_handler::<PageUpdateInput>(descriptor, Arc::new(Handler))?;
            } else {
                registry
                    .register_typed_handler::<PageCreateInput>(descriptor, Arc::new(Handler))?;
            }
        }
        Ok(())
    }
}
struct Handler;
impl OperationHandler for Handler {
    fn validate(&self, request: &CanonicalRequest) -> Result<(), CraftError> {
        input::validate_request(request)
    }
    fn execute(
        &self,
        _: &PreparedOperation,
        _: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "Page publication requires the verified journaled host.",
        ))
    }
}
