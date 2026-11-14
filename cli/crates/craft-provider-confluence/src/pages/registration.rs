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
pub(super) fn route(kind: PageWriteKind) -> NativeRoute {
    let mut path: Vec<_> = ["wiki", "api", "v2", "pages"]
        .into_iter()
        .map(|p| RouteSegment::Literal(p.into()))
        .collect();
    if kind != PageWriteKind::Create {
        path.push(RouteSegment::Parameter("id".into()));
    }
    NativeRoute {
        id: kind.route().into(),
        method: match kind {
            PageWriteKind::Create => HttpMethod::Post,
            PageWriteKind::Update => HttpMethod::Put,
            PageWriteKind::Trash => HttpMethod::Delete,
        },
        path,
        query_parameters: if kind == PageWriteKind::Create {
            vec!["root-level".into()]
        } else {
            vec![]
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
        for kind in [
            PageWriteKind::Create,
            PageWriteKind::Update,
            PageWriteKind::Trash,
        ] {
            let command = kind.command();
            let revision = json!({"version":1,"digest":digest,"representation":"storage"});
            let (effect, input_schema, example, scopes) = match kind {
                PageWriteKind::Create => (
                    EffectClass::Write,
                    schema_for::<PageCreateInput>()?,
                    json!({"space_id":"100","title":"Plan draft","placement":{"kind":"root"},"body":body}),
                    "write:page:confluence",
                ),
                PageWriteKind::Update => (
                    EffectClass::Write,
                    schema_for::<PageUpdateInput>()?,
                    json!({"id":"101","space_id":"100","expected":revision,"editor_draft_policy":"native_reconciliation_may_overwrite","title":"Reviewed plan"}),
                    "write:page:confluence",
                ),
                PageWriteKind::Trash => (
                    EffectClass::Trash,
                    schema_for::<PageTrashInput>()?,
                    json!({"id":"101","space_id":"100","expected":revision}),
                    "delete:page:confluence",
                ),
            };
            let descriptor = OperationDescriptor {
                id:command.into(), provider:"confluence".into(), contract_version:SCHEMA_VERSION.into(), api_revision:"confluence-cloud-v2".into(),
                description:match kind {
                    PageWriteKind::Trash => "Move a reviewed current page to the space trash through verified context and a journaled host; never purges.",
                    _ => "Prepare normal shared-page publication using verified context and a journaled host; publication is not approval.",
                }.into(),
                effect,
                input_schema,
                response_schema:response_schema::<PageWriteData>()?,
                example,
                response_example:json!(Envelope::success(command,"example".into(),json!({"target":target,"space_id":"100","page_id":"101","version":if kind == PageWriteKind::Update { 2 } else { 1 }}))),
                projection:None, scope_alternatives:vec![vec![scopes.into()]],
                permissions:vec![match kind {
                    PageWriteKind::Trash => "Native Confluence space/content visibility and page delete permission.",
                    _ => "Native Confluence space/content visibility and page creation or editing permissions.",
                }.into()],
                tenant_conditions:vec![match kind {
                    PageWriteKind::Create => "Exact site/space/grant/actor and fresh qualified current evidence.",
                    PageWriteKind::Update => "Exact site/space/grant/actor and fresh qualified current evidence; explicit reviewed revision. Updates require the explicit native_reconciliation_may_overwrite editor-draft policy: publication can overwrite divergent unpublished editor drafts and draft preservation is never verified.",
                    PageWriteKind::Trash => "Exact site/space/grant/actor and a fresh current page matching the reviewed revision. Trash is recoverable by space administrators; purge is not offered.",
                }.into()],
                routes:vec![route(kind)], retry_class:RetryClass::ReconcileBeforeReplay, reconciliation:ReconciliationClass::KnownResource,
            };
            match kind {
                PageWriteKind::Create => registry
                    .register_typed_handler::<PageCreateInput>(descriptor, Arc::new(Handler))?,
                PageWriteKind::Update => registry
                    .register_typed_handler::<PageUpdateInput>(descriptor, Arc::new(Handler))?,
                PageWriteKind::Trash => registry
                    .register_typed_handler::<PageTrashInput>(descriptor, Arc::new(Handler))?,
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
