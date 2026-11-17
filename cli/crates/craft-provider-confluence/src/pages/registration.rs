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
    let collection = match kind.resource_type() {
        "page" => "pages",
        "attachment" => "attachments",
        "space" => "spaces",
        "folder" => "folders",
        "whiteboard" => "whiteboards",
        _ => "footer-comments",
    };
    let mut path: Vec<_> = ["wiki", "api", "v2", collection]
        .into_iter()
        .map(|p| RouteSegment::Literal(p.into()))
        .collect();
    match kind {
        K::Create | K::CommentCreate | K::SpaceCreate | K::FolderCreate | K::WhiteboardCreate => {}
        K::Update | K::Trash | K::AttachmentTrash | K::FolderTrash | K::WhiteboardTrash => {
            path.push(RouteSegment::Parameter("id".into()))
        }
        K::CommentUpdate | K::CommentDelete => {
            path.push(RouteSegment::Parameter("comment-id".into()))
        }
        // Badge routes return above with their v1 shape; width declares its own property routes.
        K::ContentStateSet | K::ContentStateClear | K::WidthSet => {}
    }
    NativeRoute {
        id: kind.route().into(),
        method: match kind {
            K::Create
            | K::CommentCreate
            | K::SpaceCreate
            | K::FolderCreate
            | K::WhiteboardCreate => HttpMethod::Post,
            K::Update | K::CommentUpdate | K::WidthSet => HttpMethod::Put,
            K::Trash
            | K::CommentDelete
            | K::ContentStateClear
            | K::AttachmentTrash
            | K::FolderTrash
            | K::WhiteboardTrash => HttpMethod::Delete,
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
                PageWriteKind::SpaceCreate => (
                    schema_for::<SpaceCreateInput>()?,
                    json!({"key":"CRAFT","name":"Craft plans","visibility":"default_roles"}),
                    "write:space:confluence",
                ),
                PageWriteKind::FolderCreate | PageWriteKind::WhiteboardCreate => (
                    schema_for::<ContentCreateInput>()?,
                    json!({"space_id":"100","title":"Plans","placement":{"kind":"root"}}),
                    if kind == PageWriteKind::FolderCreate {
                        "write:folder:confluence"
                    } else {
                        "write:whiteboard:confluence"
                    },
                ),
                PageWriteKind::FolderTrash | PageWriteKind::WhiteboardTrash => (
                    schema_for::<ContentTrashInput>()?,
                    json!({"space_id":"100","id":"401","expected":{"version":1}}),
                    if kind == PageWriteKind::FolderTrash {
                        "delete:folder:confluence"
                    } else {
                        "delete:whiteboard:confluence"
                    },
                ),
                PageWriteKind::WidthSet => (
                    schema_for::<PageWidthSetInput>()?,
                    json!({"space_id":"100","page_id":"101","width":"full_width"}),
                    "write:page:confluence",
                ),
                PageWriteKind::AttachmentTrash => (
                    schema_for::<AttachmentTrashInput>()?,
                    json!({"space_id":"100","page_id":"101","id":"att301","expected":{"version":1,"file_id":"0b6f6a1c-file"}}),
                    "delete:attachment:confluence",
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
                    PageWriteKind::Trash => "Move a reviewed page to the space trash; never purges.",
                    PageWriteKind::CommentCreate => "Add a footer comment or reply; comment text is not approval.",
                    PageWriteKind::CommentUpdate => "Replace a reviewed footer comment's body.",
                    PageWriteKind::CommentDelete => "Permanently delete a reviewed footer comment.",
                    PageWriteKind::ContentStateSet => "Set the page badge to a state the space offers; never approval.",
                    PageWriteKind::ContentStateClear => "Remove the page badge; never changes review authority.",
                    PageWriteKind::AttachmentTrash => "Move a reviewed attachment to the trash; never purges.",
                    PageWriteKind::SpaceCreate => "Create a space with an unused key and explicit visibility (v2 only).",
                    PageWriteKind::FolderCreate => "Create a folder in a space.",
                    PageWriteKind::WhiteboardCreate => "Create a blank whiteboard in a space.",
                    PageWriteKind::FolderTrash => "Move a reviewed folder to the trash.",
                    PageWriteKind::WhiteboardTrash => "Move a reviewed whiteboard to the trash.",
                    PageWriteKind::WidthSet => "Set the page display width for viewers and the editor.",
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
                    PageWriteKind::AttachmentTrash => "View the page and delete attachments in its space.",
                    PageWriteKind::SpaceCreate => "Global Create space permission; the v2 route needs role-based access control on the site.",
                    PageWriteKind::FolderCreate | PageWriteKind::WhiteboardCreate => "View the space and create content of this type in it.",
                    PageWriteKind::FolderTrash | PageWriteKind::WhiteboardTrash => "View the item and delete content of this type in its space.",
                    PageWriteKind::WidthSet => "View and edit the page.",
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
                    PageWriteKind::AttachmentTrash => "Exact site/space/grant/actor; the attachment must be current on that page with the reviewed version and file ID.",
                    PageWriteKind::SpaceCreate => "Exact site/grant/actor under a site-level Confluence administration allowance; a fresh exact-key lookup must be empty.",
                    PageWriteKind::FolderCreate | PageWriteKind::WhiteboardCreate => "Exact site/space/grant/actor; the space, and a named parent page or folder, must be current in that space.",
                    PageWriteKind::FolderTrash | PageWriteKind::WhiteboardTrash => "Exact site/space/grant/actor; the item must be current in that space at the reviewed version. Trash is recoverable; purge is not offered.",
                    PageWriteKind::WidthSet => "Exact site/space/grant/actor; the page must be current in that space. Writes content-appearance-published, then content-appearance-draft, each as a separately journaled step; the draft write is sent only after the published write is confirmed.",
                }.into()],
                routes:if kind == PageWriteKind::WidthSet { width_writes::routes().to_vec() } else { vec![route(kind)] }, retry_class:RetryClass::ReconcileBeforeReplay, reconciliation:ReconciliationClass::KnownResource,
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
                PageWriteKind::AttachmentTrash => registry
                    .register_typed_handler::<AttachmentTrashInput>(
                        descriptor,
                        Arc::new(Handler),
                    )?,
                PageWriteKind::SpaceCreate => registry
                    .register_typed_handler::<SpaceCreateInput>(descriptor, Arc::new(Handler))?,
                PageWriteKind::FolderCreate | PageWriteKind::WhiteboardCreate => {
                    registry.register_typed_handler::<ContentCreateInput>(
                        descriptor,
                        Arc::new(Handler),
                    )?
                }
                PageWriteKind::FolderTrash | PageWriteKind::WhiteboardTrash => registry
                    .register_typed_handler::<ContentTrashInput>(
                    descriptor,
                    Arc::new(Handler),
                )?,
                PageWriteKind::WidthSet => registry
                    .register_typed_handler::<PageWidthSetInput>(descriptor, Arc::new(Handler))?,
            }
        }
        width_writes::register_observe(registry)
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
