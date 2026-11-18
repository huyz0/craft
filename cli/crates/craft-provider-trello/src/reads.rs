use crate::models::*;
use crate::table::{Kind, entry, route};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{NativeCall, NativeResponse, OperationContext, PreparedOperation};
use craft_core::pagination::{NativePage, NativePosition};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

fn malformed() -> CraftError {
    CraftError::new(
        ErrorCode::RemoteUnavailable,
        "Trello returned a response that does not match its documented shape.",
    )
}

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
                "Trello resource is absent or not visible; absence was not established."
            } else {
                "Trello read failed; no further automatic retry is made."
            },
        ));
    }
    Ok(body)
}

fn text(raw: &Value, key: &str) -> Value {
    raw.get(key)
        .and_then(Value::as_str)
        .map_or(Value::Null, |value| json!(value))
}
fn flag(raw: &Value, key: &str) -> Value {
    raw.get(key)
        .and_then(Value::as_bool)
        .map_or(Value::Null, |value| json!(value))
}
fn number(raw: &Value, key: &str) -> Value {
    raw.get(key)
        .and_then(Value::as_f64)
        .map_or(Value::Null, |value| json!(value))
}
fn count(raw: &Value, key: &str) -> Value {
    raw.get(key)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .map_or(Value::Null, |value| json!(value))
}
fn strings(raw: &Value, key: &str) -> Value {
    raw.get(key)
        .and_then(Value::as_array)
        .map_or(Value::Null, |values| {
            json!(values.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        })
}

fn identity(raw: &Value) -> Result<String, CraftError> {
    raw.get("id")
        .and_then(Value::as_str)
        .filter(|id| trello_id(id))
        .map(str::to_owned)
        .ok_or_else(malformed)
}

/// Maps one native Trello object to craft's stable, bounded summary shape.
fn normalize(command: &str, raw: &Value) -> Result<Value, CraftError> {
    if !raw.is_object() {
        return Err(malformed());
    }
    let mut item = Map::new();
    item.insert("id".into(), json!(identity(raw)?));
    let mut put = |key: &str, value: Value| {
        item.insert(key.into(), value);
    };
    match command {
        "trello.board.list" | "trello.board.get" => {
            put("name", text(raw, "name"));
            put("description", text(raw, "desc"));
            put("closed", flag(raw, "closed"));
            put("url", text(raw, "url"));
            put("last_activity", text(raw, "dateLastActivity"));
        }
        "trello.list.list" => {
            put("name", text(raw, "name"));
            put("closed", flag(raw, "closed"));
            put("position", number(raw, "pos"));
            put("board_id", text(raw, "idBoard"));
        }
        "trello.card.list" | "trello.card.board-list" | "trello.card.get" | "trello.card.search" => {
            put("name", text(raw, "name"));
            put("description", text(raw, "desc"));
            put("closed", flag(raw, "closed"));
            put("due", text(raw, "due"));
            put("due_complete", flag(raw, "dueComplete"));
            put("list_id", text(raw, "idList"));
            put("board_id", text(raw, "idBoard"));
            put("label_ids", strings(raw, "idLabels"));
            put("member_ids", strings(raw, "idMembers"));
            put("url", text(raw, "url"));
            put("short_link", text(raw, "shortLink"));
            put("last_activity", text(raw, "dateLastActivity"));
            put("position", number(raw, "pos"));
            let badges = raw.get("badges").filter(|badges| badges.is_object());
            put(
                "badges",
                badges.map_or(Value::Null, |badges| {
                    json!({
                        "comments": count(badges, "comments"),
                        "attachments": count(badges, "attachments"),
                        "checklist_items": count(badges, "checkItems"),
                        "checklist_items_checked": count(badges, "checkItemsChecked"),
                    })
                }),
            );
        }
        "trello.comment.list" => {
            let data = raw.get("data").filter(|data| data.is_object());
            let creator = raw.get("memberCreator").filter(|member| member.is_object());
            put("text", data.map_or(Value::Null, |data| text(data, "text")));
            put("date", text(raw, "date"));
            put("author_id", text(raw, "idMemberCreator"));
            put(
                "author_username",
                creator.map_or(Value::Null, |member| text(member, "username")),
            );
            put(
                "card_id",
                data.and_then(|data| data.get("card"))
                    .map_or(Value::Null, |card| text(card, "id")),
            );
        }
        "trello.label.list" => {
            put("name", text(raw, "name"));
            put("color", text(raw, "color"));
            put("board_id", text(raw, "idBoard"));
        }
        "trello.checklist.list" => {
            put("name", text(raw, "name"));
            put("card_id", text(raw, "idCard"));
            let checks = raw
                .get("checkItems")
                .and_then(Value::as_array)
                .ok_or_else(malformed)?;
            let mut mapped = Vec::new();
            for check in checks {
                mapped.push(json!({
                    "id": identity(check)?,
                    "name": text(check, "name"),
                    "complete": check.get("state").and_then(Value::as_str) == Some("complete"),
                    "position": number(check, "pos"),
                }));
            }
            put("items", json!(mapped));
        }
        "trello.attachment.list" => {
            put("name", text(raw, "name"));
            put("url", text(raw, "url"));
            put(
                "bytes",
                raw.get("bytes")
                    .and_then(Value::as_u64)
                    .map_or(Value::Null, |value| json!(value)),
            );
            put("date", text(raw, "date"));
            put("mime_type", text(raw, "mimeType"));
            put("is_upload", flag(raw, "isUpload"));
        }
        "trello.member.list" => {
            put("username", text(raw, "username"));
            put("full_name", text(raw, "fullName"));
        }
        _ => return Err(CraftError::field("/command", "unknown_trello_read")),
    }
    Ok(Value::Object(item))
}

/// The single path id, whichever scoped input field carried it.
fn scoped_id(command: &str, input: &Value) -> Result<(String, u16), CraftError> {
    let invalid = || CraftError::field("/input", "invalid_trello_read_input");
    let take = |key: &str| {
        input
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(invalid)
    };
    let page = input
        .get("page_size")
        .and_then(Value::as_u64)
        .map_or(Ok(25), |value| u16::try_from(value).map_err(|_| invalid()))?;
    page_size_field(page)?;
    let id = match command {
        "trello.board.get" | "trello.card.get" => take("id")?,
        "trello.list.list"
        | "trello.label.list"
        | "trello.member.list"
        | "trello.card.board-list" => take("board_id")?,
        "trello.card.list" => take("list_id")?,
        "trello.comment.list" | "trello.checklist.list" | "trello.attachment.list" => {
            take("card_id")?
        }
        "trello.board.list" | "trello.card.search" => String::new(),
        _ => return Err(invalid()),
    };
    if !id.is_empty() {
        id_field("/id", &id)?;
    }
    Ok((id, page))
}

fn call(
    operation: &PreparedOperation,
    position: Option<&NativePosition>,
) -> Result<(NativeCall, u16), CraftError> {
    let request = operation.request();
    let entry = entry(&request.command)
        .ok_or_else(|| CraftError::field("/command", "unknown_trello_read"))?;
    let (id, page) = scoped_id(&request.command, &request.input)?;
    let mut query: BTreeMap<String, String> = entry
        .query
        .iter()
        .map(|(name, value)| ((*name).into(), (*value).into()))
        .collect();
    let mut path_parameters = BTreeMap::new();
    if !id.is_empty() {
        path_parameters.insert("id".into(), id);
    }
    match entry.kind {
        Kind::Get | Kind::Sliced => {}
        Kind::Before => {
            query.insert("limit".into(), page.to_string());
        }
        Kind::SearchPage => {
            let input: SearchInput =
                serde_json::from_value(request.input.clone()).map_err(|_| malformed())?;
            validate_search(&input)?;
            query.insert("query".into(), input.query);
            if let Some(board) = input.board_id {
                query.insert("idBoards".into(), board);
            }
            query.insert("cards_limit".into(), page.to_string());
            query.insert("cards_page".into(), "0".into());
        }
    }
    if let Some(position) = position {
        validate_position(entry.kind, position)?;
        match entry.kind {
            Kind::Before => {
                query.insert("before".into(), position.0["before"].clone());
            }
            Kind::SearchPage => {
                query.insert("cards_page".into(), position.0["page"].clone());
            }
            _ => {}
        }
    }
    Ok((
        NativeCall {
            route: route(entry).id,
            path_parameters,
            query,
            repeated_query: BTreeMap::new(),
            body: None,
        },
        page,
    ))
}

fn validate_position(kind: Kind, position: &NativePosition) -> Result<(), CraftError> {
    position.validate()?;
    let keys: Vec<_> = position.0.keys().map(String::as_str).collect();
    let valid = match kind {
        Kind::Sliced => keys == ["offset"] && offset(position).is_some(),
        Kind::Before => keys == ["before"] && trello_id(&position.0["before"]),
        Kind::SearchPage => {
            keys == ["page"]
                && position.0["page"]
                    .parse::<u32>()
                    .is_ok_and(|page| (1..=99).contains(&page))
        }
        Kind::Get => false,
    };
    if valid {
        Ok(())
    } else {
        Err(CraftError::field("/cursor", "invalid_trello_position"))
    }
}

fn offset(position: &NativePosition) -> Option<usize> {
    position
        .0
        .get("offset")
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0 && *value <= 100_000)
}

/// Exact selected resource read. It never resolves names or follows links.
pub fn read_get(
    operation: &PreparedOperation,
    context: &mut dyn OperationContext,
) -> Result<Value, CraftError> {
    context.verify_operation(operation)?;
    let command = &operation.request().command;
    if !crate::table::is_get(command) {
        return Err(CraftError::field("/command", "expected_trello_get"));
    }
    let (call, _) = call(operation, None)?;
    let raw = invoke(context, &call)?;
    let item = normalize(command, &raw)?;
    if item["id"].as_str() != call.path_parameters.get("id").map(String::as_str) {
        return Err(malformed());
    }
    Ok(item)
}

/// Fetches one native page. The shared host owns cursor persistence and access checks.
pub fn read_page(
    operation: &PreparedOperation,
    context: &mut dyn OperationContext,
    position: Option<&NativePosition>,
) -> Result<NativePage, CraftError> {
    context.verify_operation(operation)?;
    let command = &operation.request().command;
    let kind = crate::table::kind_of(command)
        .filter(|kind| *kind != Kind::Get)
        .ok_or_else(|| CraftError::field("/command", "expected_trello_list_or_search"))?;
    let (call, page) = call(operation, position)?;
    let body = invoke(context, &call)?;
    let page = usize::from(page);
    let values: &[Value] = match kind {
        Kind::SearchPage => body
            .get("cards")
            .and_then(Value::as_array)
            .ok_or_else(malformed)?,
        _ => body.as_array().ok_or_else(malformed)?,
    };
    let (window, next) = match kind {
        Kind::Sliced => {
            let start = position.and_then(offset).unwrap_or(0);
            let window = values.get(start..).unwrap_or(&[]);
            let take = window.len().min(page);
            let next = (window.len() > take).then(|| {
                NativePosition(BTreeMap::from([(
                    "offset".into(),
                    (start + take).to_string(),
                )]))
            });
            (&window[..take], next)
        }
        Kind::Before => {
            if values.len() > page {
                return Err(malformed());
            }
            let next = if values.len() == page {
                values
                    .last()
                    .map(identity)
                    .transpose()?
                    .map(|id| NativePosition(BTreeMap::from([("before".into(), id)])))
            } else {
                None
            };
            (values, next)
        }
        Kind::SearchPage => {
            if values.len() > page {
                return Err(malformed());
            }
            let current = position
                .and_then(|p| p.0.get("page"))
                .and_then(|p| p.parse::<u32>().ok())
                .unwrap_or(0);
            let next = (values.len() == page && current < 99).then(|| {
                NativePosition(BTreeMap::from([("page".into(), (current + 1).to_string())]))
            });
            (values, next)
        }
        Kind::Get => return Err(malformed()),
    };
    let mut seen = BTreeSet::new();
    let mut items = Vec::new();
    for raw in window {
        context.checkpoint()?;
        let item = normalize(command, raw)?;
        if !seen.insert(item["id"].as_str().ok_or_else(malformed)?.to_owned()) {
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
