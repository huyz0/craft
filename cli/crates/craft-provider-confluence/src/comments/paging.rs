//! Collection pages are observations, not complete review inventories.
use super::*;
use crate::models::ListData;
use craft_core::execution::NativeResponse;
use craft_core::pagination::{NativePage, NativePosition};

pub const COMMENT_LIST: &str = "confluence.comment.list";
pub const COMMENT_REPLIES: &str = "confluence.comment.replies";

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentListInput {
    #[schemars(length(min = 1, max = 19), regex(pattern = "^[1-9][0-9]*$"))]
    pub page_id: String,
    #[serde(default = "crate::models::page_size")]
    #[schemars(range(min = 1, max = 100))]
    pub page_size: u16,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentRepliesInput {
    #[schemars(length(min = 1, max = 19), regex(pattern = "^[1-9][0-9]*$"))]
    pub page_id: String,
    #[schemars(length(min = 1, max = 19), regex(pattern = "^[1-9][0-9]*$"))]
    pub parent_comment_id: String,
    #[serde(default = "crate::models::page_size")]
    #[schemars(range(min = 1, max = 100))]
    pub page_size: u16,
}
fn input(operation: &PreparedOperation) -> Result<(String, Option<String>, u16), CraftError> {
    let request = operation.request();
    let (page, parent, size) = match request.command.as_str() {
        COMMENT_LIST => {
            let value: CommentListInput =
                serde_json::from_value(request.input.clone()).map_err(|_| malformed())?;
            (value.page_id, None, value.page_size)
        }
        COMMENT_REPLIES => {
            let value: CommentRepliesInput =
                serde_json::from_value(request.input.clone()).map_err(|_| malformed())?;
            (
                value.page_id,
                Some(value.parent_comment_id),
                value.page_size,
            )
        }
        _ => return Err(CraftError::field("/command", "expected_comment_collection")),
    };
    if !numeric(&page) || parent.as_ref().is_some_and(|v| !numeric(v)) || !(1..=100).contains(&size)
    {
        return Err(CraftError::field(
            "/input",
            "invalid_comment_collection_selectors",
        ));
    }
    Ok((page, parent, size))
}
pub fn projection() -> ProjectionContract {
    let mut value = super::projection();
    value.item_pointer = "/items".into();
    value.always_returned.push("parent_comment_id".into());
    value.default_selected.push("parent_comment_id".into());
    value
}
fn fields(
    selected_fields: &[String],
) -> Result<(BTreeSet<String>, Option<&'static str>), CraftError> {
    projection().compile(
        &response_schema::<ListData<CommentSummary>>()?,
        selected_fields,
    )?;
    let (mut selected, representation) = selected(selected_fields)?;
    selected.insert("parent_comment_id".into());
    Ok((selected, representation))
}
fn route(replies: bool) -> NativeRoute {
    let parts = if replies {
        vec![
            "wiki",
            "api",
            "v2",
            "footer-comments",
            "{comment-id}",
            "children",
        ]
    } else {
        vec!["wiki", "api", "v2", "pages", "{page-id}", "footer-comments"]
    };
    NativeRoute {
        id: if replies {
            "confluence-comment-replies"
        } else {
            "confluence-comment-list"
        }
        .into(),
        method: HttpMethod::Get,
        path: parts
            .into_iter()
            .map(
                |s| match s.strip_prefix('{').and_then(|v| v.strip_suffix('}')) {
                    Some(p) => RouteSegment::Parameter(p.into()),
                    None => RouteSegment::Literal(s.into()),
                },
            )
            .collect(),
        query_parameters: if replies {
            vec!["body-format", "sort", "cursor", "limit"]
        } else {
            vec!["body-format", "status", "sort", "cursor", "limit"]
        }
        .into_iter()
        .map(String::from)
        .collect(),
        repeated_query_parameters: vec![],
        continuation_parameters: vec!["cursor".into()],
        response_max_bytes: 1_048_576,
    }
}
pub(super) fn register(registry: &mut Registry) -> Result<(), CraftError> {
    let get_route = registry.descriptor(COMMENT_GET)?.routes[0].clone();
    for command in [COMMENT_LIST, COMMENT_REPLIES] {
        let replies = command == COMMENT_REPLIES;
        let mut descriptor = registry.descriptor(COMMENT_GET)?.clone();
        descriptor.id = command.into();
        descriptor.description = if replies {
            "Read direct current footer replies for an authenticated page-bound parent."
        } else {
            "Read a page of current footer roots; replies require separate traversal."
        }
        .into();
        descriptor.example = if replies {
            json!({"page_id":"101","parent_comment_id":"201","page_size":25})
        } else {
            json!({"page_id":"101","page_size":25})
        };
        descriptor.response_schema = response_schema::<ListData<CommentSummary>>()?;
        descriptor.response_example = json!(Envelope::success(
            command,
            "example".into(),
            json!({"target":{"provider":"confluence","deployment":"cloud","tenant":"00000000-1111-2222-3333-444444444444","resource_type":"site","id":"00000000-1111-2222-3333-444444444444"},"items":[],"observation":null,"cached":false,"credential_cache_update":null})
        ));
        descriptor.projection = Some(projection());
        descriptor.routes = vec![route(replies)];
        if replies {
            descriptor.routes.push(get_route.clone());
        }
        if replies {
            descriptor.input_schema = schema_for::<CommentRepliesInput>()?;
            registry.register_typed_handler::<CommentRepliesInput>(
                descriptor,
                Arc::new(CollectionHandler),
            )?;
        } else {
            descriptor.input_schema = schema_for::<CommentListInput>()?;
            registry.register_typed_handler::<CommentListInput>(
                descriptor,
                Arc::new(CollectionHandler),
            )?;
        }
    }
    Ok(())
}
struct CollectionHandler;
impl OperationHandler for CollectionHandler {
    fn validate(&self, request: &CanonicalRequest) -> Result<(), CraftError> {
        validate_target(&request.target)?;
        if request.preconditions.is_some() {
            return Err(CraftError::field(
                "/preconditions",
                "read_has_no_preconditions",
            ));
        }
        let (page, parent, size) = if request.command == COMMENT_LIST {
            let v: CommentListInput = serde_json::from_value(request.input.clone())
                .map_err(|_| CraftError::field("/input", "invalid_comment_list"))?;
            (v.page_id, None, v.page_size)
        } else {
            let v: CommentRepliesInput = serde_json::from_value(request.input.clone())
                .map_err(|_| CraftError::field("/input", "invalid_comment_replies"))?;
            (v.page_id, Some(v.parent_comment_id), v.page_size)
        };
        if !numeric(&page) || parent.is_some_and(|v| !numeric(&v)) || !(1..=100).contains(&size) {
            return Err(CraftError::field(
                "/input",
                "invalid_comment_collection_selectors",
            ));
        }
        fields(&request.selection.fields)?;
        Ok(())
    }
    fn execute(
        &self,
        _: &PreparedOperation,
        _: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "Comment collections require the verified read host.",
        ))
    }
}
fn invoke(
    context: &mut dyn OperationContext,
    call: &NativeCall,
) -> Result<NativeResponse, CraftError> {
    context.checkpoint()?;
    let mut response = context.invoke(call)?;
    if response.status != 200 {
        return Err(CraftError::new(
            match response.status {
                401 => ErrorCode::AuthRequired,
                403 => ErrorCode::PermissionDenied,
                429 => ErrorCode::RateLimited,
                _ => ErrorCode::RemoteUnavailable,
            },
            "Comment collection or parent is unreadable; complete coverage was not established.",
        ));
    }
    response.body = craft_core::input::parse_json(
        &serde_json::to_vec(&response.body).map_err(|_| malformed())?,
        1_048_576,
    )
    .map_err(|_| malformed())?;
    Ok(response)
}
fn parent(context: &mut dyn OperationContext, page: &str, id: &str) -> Result<Value, CraftError> {
    let response = invoke(
        context,
        &NativeCall {
            route: ROUTE.into(),
            path_parameters: BTreeMap::from([("comment-id".into(), id.into())]),
            query: BTreeMap::from([("include-version".into(), "true".into())]),
            repeated_query: BTreeMap::new(),
            body: None,
        },
    )?;
    let (fields, _) = selected(&[])?;
    normalize(
        &response.body,
        &CommentGetInput {
            id: id.into(),
            page_id: page.into(),
        },
        &fields,
        None,
    )
}
pub fn read_page(
    operation: &PreparedOperation,
    context: &mut dyn OperationContext,
    position: Option<&NativePosition>,
) -> Result<NativePage, CraftError> {
    context.verify_operation(operation)?;
    let (page, parent_id, size) = input(operation)?;
    let (selected, representation) = fields(&operation.request().selection.fields)?;
    let proof = parent_id
        .as_ref()
        .map(|id| parent(context, &page, id))
        .transpose()?;
    let mut call = NativeCall {
        route: route(parent_id.is_some()).id,
        path_parameters: BTreeMap::from([(
            if parent_id.is_some() {
                "comment-id"
            } else {
                "page-id"
            }
            .into(),
            parent_id.clone().unwrap_or_else(|| page.clone()),
        )]),
        query: BTreeMap::from([
            ("limit".into(), size.to_string()),
            ("sort".into(), "created-date".into()),
        ]),
        repeated_query: BTreeMap::new(),
        body: None,
    };
    if parent_id.is_none() {
        call.query.insert("status".into(), "current".into());
    }
    if let Some(r) = representation {
        call.query.insert("body-format".into(), r.into());
    }
    if let Some(p) = position {
        validate_position(p)?;
        call.query.extend(p.0.clone());
    }
    let response = invoke(context, &call)?;
    let values = response.body["results"]
        .as_array()
        .filter(|v| v.len() <= size as usize)
        .ok_or_else(malformed)?;
    let next = continuation(context, &call, &response)?;
    let mut ids = BTreeSet::new();
    let mut items = Vec::new();
    for value in values {
        context.checkpoint()?;
        let mut raw = value.clone();
        let id = string(&raw, "id")?.to_owned();
        if !numeric(&id) || !ids.insert(id.clone()) || parent_id.as_ref() == Some(&id) {
            return Err(malformed());
        }
        if let Some(parent) = &parent_id {
            if raw["parentCommentId"] != *parent
                || raw
                    .get("pageId")
                    .is_some_and(|v| !v.is_null() && v != &page)
            {
                return Err(malformed());
            }
            raw["pageId"] = json!(page);
        } else if raw.get("parentCommentId").is_some_and(|v| !v.is_null()) {
            return Err(malformed());
        }
        items.push(normalize(
            &raw,
            &CommentGetInput {
                id,
                page_id: page.clone(),
            },
            &selected,
            representation,
        )?);
    }
    if let Some(id) = &parent_id
        && Some(parent(context, &page, id)?) != proof
    {
        return Err(malformed());
    }
    context.checkpoint()?;
    Ok(NativePage {
        items,
        next,
        context: Some(
            json!({"page_id":page,"parent_comment_id":parent_id,"coverage":if proof.is_some(){"direct_replies"}else{"page_roots"},"parent":proof}),
        ),
    })
}
fn validate_position(position: &NativePosition) -> Result<(), CraftError> {
    position.validate()?;
    if position.0.len() != 1 || position.0.get("cursor").is_none_or(|v| v.is_empty()) {
        return Err(CraftError::field("/cursor", "invalid_comment_position"));
    }
    Ok(())
}
fn normalize_link(
    context: &dyn OperationContext,
    call: &NativeCall,
    link: &str,
) -> Result<NativePosition, CraftError> {
    let relative = if link.starts_with('/') && !link.starts_with("//") {
        let (path, query) = link.split_once('?').ok_or_else(malformed)?;
        let declared = route(call.route == "confluence-comment-replies");
        let parts = declared
            .path
            .iter()
            .map(|p| match p {
                RouteSegment::Literal(v) => Ok(v.clone()),
                RouteSegment::Parameter(k) => {
                    call.path_parameters.get(k).cloned().ok_or_else(malformed)
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        if path != format!("/{}", parts.join("/")) {
            return Err(malformed());
        }
        format!("?{query}")
    } else {
        link.into()
    };
    let normalized = context.normalize_next(call, &relative)?;
    let mut fixed = normalized.query.clone();
    let cursor = fixed.remove("cursor").ok_or_else(malformed)?;
    let mut expected = call.query.clone();
    expected.remove("cursor");
    if normalized.route != call.route
        || normalized.path_parameters != call.path_parameters
        || normalized.repeated_query != call.repeated_query
        || normalized.body.is_some()
        || fixed != expected
        || call.query.get("cursor") == Some(&cursor)
    {
        return Err(CraftError::new(
            ErrorCode::PaginationStalled,
            "Comment continuation changed filters or did not advance.",
        ));
    }
    let position = NativePosition(BTreeMap::from([("cursor".into(), cursor)]));
    validate_position(&position)?;
    Ok(position)
}
fn continuation(
    context: &dyn OperationContext,
    call: &NativeCall,
    response: &NativeResponse,
) -> Result<Option<NativePosition>, CraftError> {
    let header = header_next(response.link_headers.as_deref().ok_or_else(malformed)?)?
        .map(|s| normalize_link(context, call, &s))
        .transpose()?;
    let body = match response.body.get("_links") {
        None => None,
        Some(v) if v.is_object() => v
            .get("next")
            .map(|v| v.as_str().filter(|s| !s.is_empty()).ok_or_else(malformed))
            .transpose()?,
        _ => return Err(malformed()),
    }
    .map(|s| normalize_link(context, call, s))
    .transpose()?;
    if header.is_some() && body.is_some() && header != body {
        return Err(malformed());
    }
    Ok(header.or(body))
}

// Fail closed on unsupported Link syntax rather than silently declaring a terminal page.
fn header_next(headers: &[String]) -> Result<Option<String>, CraftError> {
    if headers.len() > 16 || headers.iter().map(String::len).sum::<usize>() > 8192 {
        return Err(malformed());
    }
    let mut next = None;
    for header in headers {
        let mut rest = header.trim();
        if rest.is_empty() {
            return Err(malformed());
        }
        while !rest.is_empty() {
            rest = rest.strip_prefix('<').ok_or_else(malformed)?;
            let (url, tail) = rest.split_once('>').ok_or_else(malformed)?;
            if url.is_empty()
                || url
                    .chars()
                    .any(|c| c.is_control() || c.is_whitespace() || c == '<')
            {
                return Err(malformed());
            }
            rest = tail.trim_start();
            let mut params = BTreeSet::new();
            let mut relation = None;
            while let Some(tail) = rest.strip_prefix(';') {
                let (key, tail) = tail.trim_start().split_once('=').ok_or_else(malformed)?;
                let key = key.trim().to_ascii_lowercase();
                if key.is_empty()
                    || !key
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
                    || !params.insert(key.clone())
                    || key == "anchor"
                {
                    return Err(malformed());
                }
                rest = tail.trim_start();
                let value;
                if let Some(tail) = rest.strip_prefix('"') {
                    let (v, tail) = tail.split_once('"').ok_or_else(malformed)?;
                    if v.contains('\\') || v.chars().any(char::is_control) {
                        return Err(malformed());
                    }
                    value = v;
                    rest = tail.trim_start();
                } else {
                    let end = rest.find([';', ',']).unwrap_or(rest.len());
                    value = rest[..end].trim();
                    rest = rest[end..].trim_start();
                    if value.is_empty()
                        || !value
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || b"-_.:/".contains(&c))
                    {
                        return Err(malformed());
                    }
                }
                if key == "rel" {
                    relation = Some(value);
                }
            }
            let relation = relation.ok_or_else(malformed)?;
            let relations: Vec<_> = relation.split_ascii_whitespace().collect();
            if relations.is_empty()
                || relations.iter().any(|r| {
                    !r.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
                        || !r
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || b".-".contains(&c))
                })
            {
                return Err(malformed());
            }
            if relations
                .into_iter()
                .any(|r| r.eq_ignore_ascii_case("next"))
            {
                if next.is_some() {
                    return Err(malformed());
                }
                next = Some(url.to_owned());
            }
            if rest.is_empty() {
                break;
            }
            rest = rest.strip_prefix(',').ok_or_else(malformed)?.trim_start();
            if rest.is_empty() {
                return Err(malformed());
            }
        }
    }
    Ok(next)
}
