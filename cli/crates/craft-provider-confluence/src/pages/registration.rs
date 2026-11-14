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
    use PageWriteKind as K;
    if matches!(kind, K::ContentStateSet | K::ContentStateClear) {
        return NativeRoute {
            id: kind.route().into(),
            method: if kind == K::ContentStateSet {
                HttpMethod::Put
            } else {
                HttpMethod::Delete
            },
            path: ["wiki", "rest", "api", "content"]
                .map(|p| RouteSegment::Literal(p.into()))
                .into_iter()
                .chain([
                    RouteSegment::Parameter("id".into()),
                    RouteSegment::Literal("state".into()),
                ])
                .collect(),
            query_parameters: vec!["status".into()],
            continuation_parameters: vec![],
            repeated_query_parameters: vec![],
            response_max_bytes: 262_144,
        };
    }
    let collection = if kind.resource_type() == "page" {
        "pages"
    } else {
        "footer-comments"
    };
    let mut path: Vec<_> = ["wiki", "api", "v2", collection]
        .into_iter()
        .map(|p| RouteSegment::Literal(p.into()))
        .collect();
    match kind {
        K::Create | K::CommentCreate => {}
        K::Update | K::Trash => path.push(RouteSegment::Parameter("id".into())),
        K::CommentUpdate | K::CommentDelete => {
            path.push(RouteSegment::Parameter("comment-id".into()))
        }
        // Badge routes return above with their v1 shape.
        K::ContentStateSet | K::ContentStateClear => {}
    }
    NativeRoute {
        id: kind.route().into(),
        method: match kind {
            K::Create | K::CommentCreate => HttpMethod::Post,
            K::Update | K::CommentUpdate => HttpMethod::Put,
            K::Trash | K::CommentDelete | K::ContentStateClear => HttpMethod::Delete,
            K::ContentStateSet => HttpMethod::Put,
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
        let comment_revision = json!({"version":1,"digest":digest,"representation":"storage"});
        let comment_body = json!({"representation":"storage","value":"<p>Looks good to me.</p>"});
        for kind in PageWriteKind::ALL {
            let command = kind.command();
            let revision = json!({"version":1,"digest":digest,"representation":"storage"});
            let effect = kind.effect();
            let (input_schema, example, scopes) = match kind {
                PageWriteKind::Create => (
                    schema_for::<PageCreateInput>()?,
                    json!({"space_id":"100","title":"Plan draft","placement":{"kind":"root"},"body":body}),
                    "write:page:confluence",
                ),
                PageWriteKind::Update => (
                    schema_for::<PageUpdateInput>()?,
                    json!({"id":"101","space_id":"100","expected":revision,"editor_draft_policy":"native_reconciliation_may_overwrite","title":"Reviewed plan"}),
                    "write:page:confluence",
                ),
                PageWriteKind::Trash => (
                    schema_for::<PageTrashInput>()?,
                    json!({"id":"101","space_id":"100","expected":revision}),
                    "delete:page:confluence",
                ),
                PageWriteKind::CommentCreate => (
                    schema_for::<CommentCreateInput>()?,
                    json!({"space_id":"100","page_id":"101","body":comment_body}),
                    "write:comment:confluence",
                ),
                PageWriteKind::CommentUpdate => (
                    schema_for::<CommentUpdateInput>()?,
                    json!({"space_id":"100","page_id":"101","id":"201","expected":comment_revision,"body":comment_body}),
                    "write:comment:confluence",
                ),
                PageWriteKind::CommentDelete => (
                    schema_for::<CommentDeleteInput>()?,
                    json!({"space_id":"100","page_id":"101","id":"201","expected":comment_revision}),
                    "delete:comment:confluence",
                ),
                PageWriteKind::ContentStateSet => (
                    schema_for::<ContentStateSetInput>()?,
                    json!({"space_id":"100","page_id":"101","state_id":1}),
                    "write:confluence-content",
                ),
                PageWriteKind::ContentStateClear => (
                    schema_for::<ContentStateClearInput>()?,
                    json!({"space_id":"100","page_id":"101"}),
                    "write:confluence-content",
                ),
            };
            let mut scope_alternatives = vec![vec![scopes.to_string()]];
            if scopes == "write:confluence-content" {
                // Atlassian also accepts the granular pair for these v1 badge routes.
                scope_alternatives.push(vec![
                    "write:content:confluence".into(),
                    "read:content-details:confluence".into(),
                ]);
            }
            let descriptor = OperationDescriptor {
                id:command.into(), provider:"confluence".into(), contract_version:SCHEMA_VERSION.into(), api_revision:"confluence-cloud-v2".into(),
                description:match kind {
                    PageWriteKind::Trash => "Move a reviewed current page to the space trash through verified context and a journaled host; never purges.",
                    PageWriteKind::CommentCreate => "Add a footer comment or reply to a current page in an allowed space; comment text is not approval.",
                    PageWriteKind::CommentUpdate => "Replace the body of a reviewed footer comment revision; edits never establish approval.",
                    PageWriteKind::CommentDelete => "Permanently delete a reviewed footer comment revision; Confluence offers no trash for comments.",
                    PageWriteKind::ContentStateSet => "Set the published page's content-state badge to a state the space offers; a badge is not approval or lifecycle.",
                    PageWriteKind::ContentStateClear => "Remove the published page's content-state badge; this never changes review or ticket authority.",
                    _ => "Prepare normal shared-page publication using verified context and a journaled host; publication is not approval.",
                }.into(),
                effect,
                input_schema,
                response_schema:response_schema::<PageWriteData>()?,
                example,
                response_example:json!(Envelope::success(command,"example".into(),json!({"target":target,"space_id":"100","page_id":"101","version":if matches!(kind, PageWriteKind::Update | PageWriteKind::CommentUpdate) { 2 } else { 1 }}))),
                projection:None, scope_alternatives,
                permissions:vec![match kind {
                    PageWriteKind::Trash => "Native Confluence space/content visibility and page delete permission.",
                    PageWriteKind::CommentCreate => "View the page and add comments in its space.",
                    PageWriteKind::CommentUpdate | PageWriteKind::CommentDelete => "View the page; edit or delete own comments, or space administration for others' comments.",
                    PageWriteKind::ContentStateSet | PageWriteKind::ContentStateClear => "Edit the page; space settings must allow the selected suggested or custom state.",
                    _ => "Native Confluence space/content visibility and page creation or editing permissions.",
                }.into()],
                tenant_conditions:vec![match kind {
                    PageWriteKind::Create => "Exact site/space/grant/actor and fresh qualified current evidence.",
                    PageWriteKind::Update => "Exact site/space/grant/actor and fresh qualified current evidence; explicit reviewed revision. Updates require the explicit native_reconciliation_may_overwrite editor-draft policy: publication can overwrite divergent unpublished editor drafts and draft preservation is never verified.",
                    PageWriteKind::Trash => "Exact site/space/grant/actor and a fresh current page matching the reviewed revision. Trash is recoverable by space administrators; purge is not offered.",
                    PageWriteKind::CommentCreate => "Exact site/space/grant/actor; the page must be current in that space and a reply parent must be a current comment on the same page.",
                    PageWriteKind::CommentUpdate | PageWriteKind::CommentDelete => "Exact site/space/grant/actor; the page must be current in that space and the comment must match the reviewed revision.",
                    PageWriteKind::ContentStateSet => "Exact site/space/grant/actor; the page must be current in that space and the state ID must be offered now. Always targets status=current.",
                    PageWriteKind::ContentStateClear => "Exact site/space/grant/actor; the page must be current in that space. Always targets status=current.",
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
                PageWriteKind::CommentCreate => registry
                    .register_typed_handler::<CommentCreateInput>(descriptor, Arc::new(Handler))?,
                PageWriteKind::CommentUpdate => registry
                    .register_typed_handler::<CommentUpdateInput>(descriptor, Arc::new(Handler))?,
                PageWriteKind::CommentDelete => registry
                    .register_typed_handler::<CommentDeleteInput>(descriptor, Arc::new(Handler))?,
                PageWriteKind::ContentStateSet => registry
                    .register_typed_handler::<ContentStateSetInput>(
                        descriptor,
                        Arc::new(Handler),
                    )?,
                PageWriteKind::ContentStateClear => registry
                    .register_typed_handler::<ContentStateClearInput>(
                        descriptor,
                        Arc::new(Handler),
                    )?,
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
