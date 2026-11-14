use crate::{models::*, selection::FieldPlan};
use craft_core::contracts::{CanonicalRequest, EffectClass, Envelope, ResourceRef, SCHEMA_VERSION};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{
    HttpMethod, NativeRoute, OperationContext, OperationHandler, PreparedOperation, RouteSegment,
};
use craft_core::projection::{ProjectionContract, ProjectionMode};
use craft_core::registry::{
    OperationDescriptor, ProviderManifest, ProviderModule, ReconciliationClass, Registry,
    RetryClass, response_schema, schema_for,
};
use serde_json::{Value, json};
use std::sync::Arc;
pub const SPACE_GET: &str = "confluence.space.get";
pub const SPACE_LIST: &str = "confluence.space.list";
pub const PAGE_GET: &str = "confluence.page.get";
pub const PAGE_LIST: &str = "confluence.page.list";
pub const PAGE_SEARCH: &str = "confluence.page.search";
pub struct ConfluenceContext;
pub(crate) fn is_space(command: &str) -> bool {
    matches!(command, SPACE_GET | SPACE_LIST)
}
pub(crate) fn is_get(command: &str) -> bool {
    matches!(command, SPACE_GET | PAGE_GET | crate::comments::COMMENT_GET)
}
pub fn projection(command: &str) -> ProjectionContract {
    if matches!(
        command,
        crate::comments::paging::COMMENT_LIST | crate::comments::paging::COMMENT_REPLIES
    ) {
        return crate::comments::paging::projection();
    }
    if command == crate::comments::COMMENT_GET {
        return crate::comments::projection();
    }
    let (required, defaults) = if is_space(command) {
        (
            vec!["id", "key", "fields_not_returned"],
            vec!["id", "key", "name", "space_type", "status"],
        )
    } else {
        (
            vec![
                "id",
                "space_id",
                "status",
                "body_state",
                "fields_not_returned",
            ],
            vec!["id", "space_id", "title", "status", "version"],
        )
    };
    ProjectionContract {
        item_pointer: if is_get(command) { "/item" } else { "/items" }.into(),
        always_returned: required.into_iter().map(String::from).collect(),
        default_selected: defaults.into_iter().map(String::from).collect(),
        mode: ProjectionMode::Mixed,
    }
}
pub fn response_schema_for(command: &str) -> Result<Value, CraftError> {
    match command {
        crate::comments::paging::COMMENT_LIST | crate::comments::paging::COMMENT_REPLIES => {
            response_schema::<ListData<crate::comments::CommentSummary>>()
        }
        crate::comments::COMMENT_GET => {
            response_schema::<GetData<crate::comments::CommentSummary>>()
        }
        SPACE_GET => response_schema::<GetData<SpaceSummary>>(),
        SPACE_LIST => response_schema::<ListData<SpaceSummary>>(),
        PAGE_GET => response_schema::<GetData<PageSummary>>(),
        PAGE_LIST | PAGE_SEARCH => response_schema::<ListData<PageSummary>>(),
        _ => Err(CraftError::field("/command", "unknown_confluence_read")),
    }
}
pub(crate) fn route(command: &str, scoped: bool) -> NativeRoute {
    let (id, parts, query, continuation) = match command {
        SPACE_GET => (
            "confluence-space-get",
            vec!["wiki", "api", "v2", "spaces", "{id}"],
            vec!["description-format"],
            vec![],
        ),
        SPACE_LIST => (
            "confluence-space-list",
            vec!["wiki", "api", "v2", "spaces"],
            vec!["keys", "limit", "cursor", "description-format"],
            vec!["cursor"],
        ),
        PAGE_GET => (
            "confluence-page-get",
            vec!["wiki", "api", "v2", "pages", "{id}"],
            vec!["body-format", "status", "get-draft", "include-version"],
            vec![],
        ),
        PAGE_LIST if scoped => (
            "confluence-space-pages",
            vec!["wiki", "api", "v2", "spaces", "{id}", "pages"],
            vec!["body-format", "limit", "cursor", "status", "title"],
            vec!["cursor"],
        ),
        PAGE_LIST => (
            "confluence-page-list",
            vec!["wiki", "api", "v2", "pages"],
            vec!["body-format", "limit", "cursor", "status", "title"],
            vec!["cursor"],
        ),
        _ => (
            "confluence-page-search",
            vec!["wiki", "rest", "api", "search"],
            vec![
                "cql",
                "limit",
                "cursor",
                "start",
                "next",
                "expand",
                "excerpt",
                "includeArchivedSpaces",
                "excludeCurrentSpaces",
            ],
            vec!["cursor", "start", "next"],
        ),
    };
    NativeRoute {
        id: id.into(),
        method: HttpMethod::Get,
        path: parts
            .into_iter()
            .map(|p| {
                if p == "{id}" {
                    RouteSegment::Parameter("id".into())
                } else {
                    RouteSegment::Literal(p.into())
                }
            })
            .collect(),
        query_parameters: query.into_iter().map(String::from).collect(),
        repeated_query_parameters: vec![],
        continuation_parameters: continuation.into_iter().map(String::from).collect(),
        response_max_bytes: 1_048_576,
    }
}
pub(crate) fn validate_target(target: &ResourceRef) -> Result<(), CraftError> {
    target.validate()?;
    let uuid = target.tenant.len() == 36
        && target.tenant.bytes().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        });
    if target.provider != "confluence"
        || target.deployment != "cloud"
        || target.resource_type != "site"
        || target.id != target.tenant
        || !uuid
    {
        return Err(CraftError::field(
            "/target",
            "expected_exact_confluence_cloud_site",
        ));
    }
    Ok(())
}
impl ProviderModule for ConfluenceContext {
    fn manifest(&self) -> ProviderManifest {
        ProviderManifest {
            id: "confluence".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            deployments: vec!["cloud".into()],
            contract_version: SCHEMA_VERSION.into(),
        }
    }
    fn register(&self, registry: &mut Registry) -> Result<(), CraftError> {
        for command in [SPACE_GET, SPACE_LIST, PAGE_GET, PAGE_LIST, PAGE_SEARCH] {
            let input = match command {
                SPACE_GET | PAGE_GET => schema_for::<GetInput>()?,
                SPACE_LIST => schema_for::<SpaceListInput>()?,
                PAGE_LIST => schema_for::<PageListInput>()?,
                _ => schema_for::<SearchInput>()?,
            };
            let example = match command {
                SPACE_GET | PAGE_GET => json!({"id":"100"}),
                SPACE_LIST | PAGE_LIST => json!({"page_size":25}),
                _ => json!({"cql":"type = page","page_size":25}),
            };
            let item = if is_space(command) {
                json!({"id":"100","key":"ENG","name":"Engineering","space_type":"global","status":"current","fields_not_returned":[]})
            } else {
                json!({"id":"100","space_id":"200","title":"Reviewed plan","status":"current","version":1,"body_state":"not_requested","fields_not_returned":[]})
            };
            let target = json!({"provider":"confluence","deployment":"cloud","tenant":"00000000-1111-2222-3333-444444444444","resource_type":"site","id":"00000000-1111-2222-3333-444444444444"});
            let data = if is_get(command) {
                json!({"target":target,"item":item,"observed_at":0,"credential_cache_update":null})
            } else {
                json!({"target":target,"items":[item],"observation":null,"cached":false,"credential_cache_update":null})
            };
            let mut routes = vec![route(command, false)];
            if command == PAGE_LIST {
                routes.push(route(command, true));
            }
            let scopes = if is_space(command) {
                "read:space:confluence"
            } else if command == PAGE_SEARCH {
                "search:confluence"
            } else {
                "read:page:confluence"
            };
            let descriptor=OperationDescriptor {id:command.into(),provider:"confluence".into(),contract_version:SCHEMA_VERSION.into(),api_revision:if command==PAGE_SEARCH{"confluence-cloud-v1-cql"}else{"confluence-cloud-v2"}.into(),description:"Read bounded selected Confluence context without granting publication or review authority.".into(),effect:EffectClass::Read,input_schema:input,response_schema:response_schema_for(command)?,example,response_example:json!(Envelope::success(command,"example".into(),data)),projection:Some(projection(command)),scope_alternatives:vec![vec![scopes.into()]],permissions:vec!["Confluence site/space/content visibility evaluated by the provider".into()],tenant_conditions:vec!["Exact site and verified service actor; live visibility is not absence or mutation authority.".into()],routes,retry_class:RetryClass::SafeRead,reconciliation:ReconciliationClass::NotApplicable};
            match command {
                SPACE_GET | PAGE_GET => {
                    registry.register_typed_handler::<GetInput>(descriptor, Arc::new(Handler))?
                }
                SPACE_LIST => registry
                    .register_typed_handler::<SpaceListInput>(descriptor, Arc::new(Handler))?,
                PAGE_LIST => registry
                    .register_typed_handler::<PageListInput>(descriptor, Arc::new(Handler))?,
                _ => {
                    registry.register_typed_handler::<SearchInput>(descriptor, Arc::new(Handler))?
                }
            }
        }
        crate::comments::register(registry)
    }
}
struct Handler;
impl OperationHandler for Handler {
    fn validate(&self, request: &CanonicalRequest) -> Result<(), CraftError> {
        validate_target(&request.target)?;
        if request.preconditions.is_some()
            || (is_get(&request.command)
                && (request.selection.cursor.is_some()
                    || request.selection.limit
                        != craft_core::contracts::Selection::default().limit))
        {
            return Err(CraftError::field(
                "/selection",
                "unsupported_get_paging_or_read_preconditions",
            ));
        }
        let parse = || CraftError::field("/input", "invalid_confluence_read_input");
        match request.command.as_str() {
            SPACE_GET | PAGE_GET => {
                validate_get(&serde_json::from_value(request.input.clone()).map_err(|_| parse())?)?
            }
            SPACE_LIST => validate_spaces(
                &serde_json::from_value(request.input.clone()).map_err(|_| parse())?,
            )?,
            PAGE_LIST => validate_pages(
                &serde_json::from_value(request.input.clone()).map_err(|_| parse())?,
            )?,
            PAGE_SEARCH => validate_search(
                &serde_json::from_value(request.input.clone()).map_err(|_| parse())?,
            )?,
            _ => return Err(parse()),
        }
        FieldPlan::compile(&request.command, &request.selection.fields)?;
        Ok(())
    }
    fn execute(
        &self,
        _: &PreparedOperation,
        _: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "Confluence reads require the verified bounded host.",
        ))
    }
}
