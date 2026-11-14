use crate::{
    models::*,
    registration::*,
    selection::{FieldPlan, malformed},
};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{
    NativeCall, NativeResponse, OperationContext, PreparedOperation, RouteSegment,
};
use craft_core::pagination::{NativePage, NativePosition};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

fn invoke(context: &mut dyn OperationContext, call: &NativeCall) -> Result<Value, CraftError> {
    context.checkpoint()?;
    let NativeResponse { status, body, .. } = context.invoke(call)?;
    if status != 200 {
        return Err(CraftError::new(
            match status {
                400 | 422 => ErrorCode::InputInvalid,
                401 => ErrorCode::AuthRequired,
                403 => ErrorCode::PermissionDenied,
                429 => ErrorCode::RateLimited,
                _ => ErrorCode::RemoteUnavailable,
            },
            if status == 404 {
                "Confluence resource is absent or not visible; absence was not established."
            } else {
                "Confluence read failed; no automatic retry was performed."
            },
        ));
    }
    let encoded = serde_json::to_vec(&body).map_err(|_| malformed())?;
    if !body.is_object() {
        return Err(malformed());
    }
    craft_core::input::parse_json(&encoded, 1_048_576).map_err(|_| malformed())
}
fn call(
    operation: &PreparedOperation,
    plan: &FieldPlan,
    position: Option<&NativePosition>,
) -> Result<NativeCall, CraftError> {
    let request = operation.request();
    let mut query = BTreeMap::new();
    let mut path_parameters = BTreeMap::new();
    let mut scoped = false;
    match request.command.as_str() {
        SPACE_GET | PAGE_GET => {
            let input: GetInput =
                serde_json::from_value(request.input.clone()).map_err(|_| malformed())?;
            validate_get(&input)?;
            path_parameters.insert("id".into(), input.id);
            if request.command == PAGE_GET {
                query.insert("status".into(), "current".into());
                query.insert("get-draft".into(), "false".into());
                if plan.wants("version") {
                    query.insert("include-version".into(), "true".into());
                }
            }
            if position.is_some() {
                return Err(CraftError::field("/cursor", "get_has_no_continuation"));
            }
        }
        SPACE_LIST => {
            let input: SpaceListInput =
                serde_json::from_value(request.input.clone()).map_err(|_| malformed())?;
            validate_spaces(&input)?;
            query.insert("limit".into(), input.page_size.to_string());
            if let Some(key) = input.key {
                query.insert("keys".into(), key);
            }
        }
        PAGE_LIST => {
            let input: PageListInput =
                serde_json::from_value(request.input.clone()).map_err(|_| malformed())?;
            validate_pages(&input)?;
            query.insert("limit".into(), input.page_size.to_string());
            query.insert("status".into(), input.status.as_str().into());
            if let Some(id) = input.space_id {
                path_parameters.insert("id".into(), id);
                scoped = true;
            }
            if let Some(title) = input.title {
                query.insert("title".into(), title);
            }
        }
        PAGE_SEARCH => {
            let input: SearchInput =
                serde_json::from_value(request.input.clone()).map_err(|_| malformed())?;
            validate_search(&input)?;
            query.insert("limit".into(), input.page_size.to_string());
            query.insert("cql".into(), input.cql);
            query.insert("excerpt".into(), "none".into());
            query.insert("includeArchivedSpaces".into(), "false".into());
            query.insert("excludeCurrentSpaces".into(), "false".into());
            let expand = plan.expansions();
            if !expand.is_empty() {
                query.insert("expand".into(), expand);
            }
        }
        _ => return Err(CraftError::field("/command", "unknown_confluence_read")),
    }
    if let Some(body) = plan.body
        && request.command != PAGE_SEARCH
    {
        query.insert("body-format".into(), body.into());
    }
    if let Some(description) = plan.description {
        query.insert("description-format".into(), description.into());
    }
    if let Some(position) = position {
        validate_position(&request.command, position)?;
        query.extend(position.0.clone());
    }
    Ok(NativeCall {
        route: route(&request.command, scoped).id,
        path_parameters,
        query,
        repeated_query: BTreeMap::new(),
        body: None,
    })
}
fn validate_position(command: &str, position: &NativePosition) -> Result<(), CraftError> {
    position.validate()?;
    let keys: Vec<_> = position.0.keys().map(String::as_str).collect();
    let valid = if command == PAGE_SEARCH {
        match keys.as_slice() {
            ["cursor"] | ["cursor", "next"] => position.0.get("next").is_none_or(|v| v == "true"),
            ["start"] => position.0["start"].parse::<u32>().is_ok_and(|v| {
                v > 0 && v <= i32::MAX as u32 && v.to_string() == position.0["start"]
            }),
            _ => false,
        }
    } else {
        keys == ["cursor"]
    };
    if !valid || position.0.get("cursor").is_some_and(|v| v.is_empty()) {
        return Err(CraftError::field("/cursor", "invalid_confluence_position"));
    }
    Ok(())
}
/// Exact selected resource read. It does not resolve names, follow links or authorize writes.
pub fn read_get(
    operation: &PreparedOperation,
    context: &mut dyn OperationContext,
) -> Result<Value, CraftError> {
    context.verify_operation(operation)?;
    let command = &operation.request().command;
    if command == crate::comments::COMMENT_GET {
        return crate::comments::read_get(operation, context);
    }
    if matches!(
        command.as_str(),
        crate::content_state::CONTENT_STATE_GET | crate::content_state::CONTENT_STATE_AVAILABLE
    ) {
        return crate::content_state::read_get(operation, context);
    }
    if command == crate::attachments::ATTACHMENT_GET {
        return crate::attachments::read_get(operation, context);
    }
    if !is_get(command) {
        return Err(CraftError::field("/command", "expected_confluence_get"));
    }
    let plan = FieldPlan::compile(command, &operation.request().selection.fields)?;
    let call = call(operation, &plan, None)?;
    let raw = invoke(context, &call)?;
    let item = plan.normalize(command, &raw)?;
    if item["id"].as_str() != call.path_parameters.get("id").map(String::as_str)
        || (command == PAGE_GET && item["status"] != "current")
    {
        return Err(malformed());
    }
    Ok(item)
}
fn next_position(
    operation: &PreparedOperation,
    context: &dyn OperationContext,
    call: &NativeCall,
    body: &Value,
) -> Result<Option<NativePosition>, CraftError> {
    let Some(links) = body.get("_links") else {
        return Err(malformed());
    };
    if !links.is_object() {
        return Err(malformed());
    }
    let Some(link) = links.get("next") else {
        return Ok(None);
    };
    let link = link
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or_else(malformed)?;
    let command = &operation.request().command;
    // The product-relative paths are fixed API routes, never a replacement credential origin.
    let relative = if link.starts_with('/') && !link.starts_with("//") {
        let (path, query) = link.split_once('?').ok_or_else(malformed)?;
        let declared = route(command, call.route == "confluence-space-pages");
        let parts = declared
            .path
            .iter()
            .map(|part| match part {
                RouteSegment::Literal(v) => Ok(v.clone()),
                RouteSegment::Parameter(p) => {
                    call.path_parameters.get(p).cloned().ok_or_else(malformed)
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        let expected = format!("/{}", parts.join("/"));
        if path != expected && !(command == PAGE_SEARCH && path == "/rest/api/search") {
            return Err(malformed());
        }
        format!("?{query}")
    } else {
        link.into()
    };
    let mut link_call = call.clone();
    if command == PAGE_SEARCH {
        // A provider clamp constrains its link, while future fetches retain the requested limit.
        let limit = body
            .get("limit")
            .and_then(Value::as_u64)
            .filter(|v| *v > 0)
            .ok_or_else(malformed)?;
        link_call.query.insert("limit".into(), limit.to_string());
    }
    let normalized = context.normalize_next(&link_call, &relative)?;
    if normalized.route != call.route
        || normalized.path_parameters != call.path_parameters
        || normalized.repeated_query != call.repeated_query
        || normalized.body.is_some()
    {
        return Err(malformed());
    }
    let keys = if command == PAGE_SEARCH {
        vec!["cursor", "start", "next"]
    } else {
        vec!["cursor"]
    };
    let mut fixed = normalized.query.clone();
    let mut position = BTreeMap::new();
    for key in &keys {
        if let Some(value) = fixed.remove(*key) {
            position.insert((*key).into(), value);
        }
    }
    let mut expected = link_call.query.clone();
    for key in &keys {
        expected.remove(*key);
    }
    let position = NativePosition(position);
    validate_position(command, &position)?;
    if fixed != expected
        || keys
            .iter()
            .all(|key| normalized.query.get(*key) == call.query.get(*key))
    {
        return Err(CraftError::new(
            ErrorCode::PaginationStalled,
            "Confluence continuation changed filters or did not advance.",
        ));
    }
    if let Some(next) = position.0.get("start") {
        let previous = call
            .query
            .get("start")
            .map(String::as_str)
            .unwrap_or("0")
            .parse::<u32>()
            .map_err(|_| malformed())?;
        let observed = body
            .get("start")
            .and_then(Value::as_u64)
            .ok_or_else(malformed)?;
        if call.query.contains_key("cursor")
            || next.parse::<u32>().map_err(|_| malformed())? <= previous
            || next.parse::<u64>().map_err(|_| malformed())? <= observed
        {
            return Err(malformed());
        }
    }
    if call.query.contains_key("start") && position.0.contains_key("cursor") {
        return Err(malformed());
    }
    Ok(Some(position))
}
/// Fetches one native page. The shared host owns cursor persistence, remainders and access checks.
pub fn read_page(
    operation: &PreparedOperation,
    context: &mut dyn OperationContext,
    position: Option<&NativePosition>,
) -> Result<NativePage, CraftError> {
    context.verify_operation(operation)?;
    let command = &operation.request().command;
    if matches!(
        command.as_str(),
        crate::comments::paging::COMMENT_LIST | crate::comments::paging::COMMENT_REPLIES
    ) {
        return crate::comments::paging::read_page(operation, context, position);
    }
    if command == crate::attachments::ATTACHMENT_LIST {
        return crate::attachments::read_page(operation, context, position);
    }
    if is_get(command) {
        return Err(CraftError::field(
            "/command",
            "expected_confluence_list_or_search",
        ));
    }
    let plan = FieldPlan::compile(command, &operation.request().selection.fields)?;
    let call = call(operation, &plan, position)?;
    let body = invoke(context, &call)?;
    let maximum = call.query["limit"]
        .parse::<usize>()
        .map_err(|_| malformed())?;
    let values = body
        .get("results")
        .and_then(Value::as_array)
        .filter(|v| v.len() <= maximum)
        .ok_or_else(malformed)?;
    if command == PAGE_SEARCH {
        let start = body
            .get("start")
            .and_then(Value::as_u64)
            .filter(|v| *v <= i32::MAX as u64)
            .ok_or_else(malformed)?;
        if !call.query.contains_key("cursor")
            && start
                != call
                    .query
                    .get("start")
                    .map(|v| v.parse::<u64>())
                    .transpose()
                    .map_err(|_| malformed())?
                    .unwrap_or(0)
        {
            return Err(malformed());
        }
        let limit = body
            .get("limit")
            .and_then(Value::as_u64)
            .filter(|v| *v > 0 && *v <= maximum as u64)
            .ok_or_else(malformed)?;
        if values.len() > limit as usize
            || body.get("size").and_then(Value::as_u64) != Some(values.len() as u64)
        {
            return Err(malformed());
        }
    }
    let next = next_position(operation, context, &call, &body)?;
    let mut identities = BTreeSet::new();
    let mut keys = BTreeSet::new();
    let mut items = Vec::new();
    for raw in values {
        context.checkpoint()?;
        let raw = if command == PAGE_SEARCH {
            let content = raw
                .get("content")
                .filter(|v| v.is_object())
                .ok_or_else(malformed)?;
            if content.get("type").and_then(Value::as_str) != Some("page") {
                return Err(CraftError::new(
                    ErrorCode::UnsupportedCapability,
                    "CQL returned a non-page result; restrict the literal query to pages.",
                ));
            }
            content
        } else {
            raw
        };
        let item = plan.normalize(command, raw)?;
        if !identities.insert(item["id"].as_str().ok_or_else(malformed)?.to_owned()) {
            return Err(malformed());
        }
        if command == SPACE_LIST {
            let key = item["key"].as_str().ok_or_else(malformed)?;
            if !keys.insert(key.to_owned())
                || call
                    .query
                    .get("keys")
                    .is_some_and(|expected| expected != key)
            {
                return Err(malformed());
            }
        }
        if command == PAGE_LIST
            && (call
                .path_parameters
                .get("id")
                .is_some_and(|id| item["space_id"].as_str() != Some(id))
                || item["status"].as_str() != call.query.get("status").map(String::as_str))
        {
            return Err(malformed());
        }
        items.push(item);
    }
    Ok(NativePage {
        context: None,
        items,
        next,
    })
}
