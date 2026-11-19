//! Journaled Trello mutations for agents: cards, comments, labels, checklists, link attachments
//! and card members. Nothing here deletes boards, lists, cards or comments, and nothing changes
//! workspace, membership or billing settings.
//!
//! Each command is one native request. The trusted host claims the send in its journal first and
//! never repeats it; this module only builds the request and interprets the answer.

use crate::models::{id_field, trello_id};
use craft_core::contracts::{CanonicalRequest, EffectClass, Envelope, ResourceRef, SCHEMA_VERSION};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{
    HttpMethod, NativeCall, NativeResponse, NativeRoute, OperationContext, OperationHandler,
    PreparedOperation, RouteSegment,
};
use craft_core::registry::{
    OperationDescriptor, ReconciliationClass, Registry, RetryClass, response_schema, schema_for,
};
use craft_core::storage::SubmissionState;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

pub struct WriteEntry {
    pub id: &'static str,
    pub method: HttpMethod,
    /// `{id}` and `{id2}` are the path parameters, in that order.
    pub path: &'static [&'static str],
    /// The kind of resource a confirmed result names.
    pub resource: &'static str,
    pub description: &'static str,
}

pub const WRITES: &[WriteEntry] = &[
    WriteEntry {
        id: "trello.card.create",
        method: HttpMethod::Post,
        path: &["cards"],
        resource: "card",
        description: "Create a card in a list.",
    },
    WriteEntry {
        id: "trello.card.update",
        method: HttpMethod::Put,
        path: &["cards", "{id}"],
        resource: "card",
        description: "Change a card's name, description, due date or completion.",
    },
    WriteEntry {
        id: "trello.card.move",
        method: HttpMethod::Put,
        path: &["cards", "{id}"],
        resource: "card",
        description: "Move a card to another list, optionally on another board.",
    },
    WriteEntry {
        id: "trello.card.archive",
        method: HttpMethod::Put,
        path: &["cards", "{id}"],
        resource: "card",
        description: "Archive or restore a card. Cards are never deleted.",
    },
    WriteEntry {
        id: "trello.comment.add",
        method: HttpMethod::Post,
        path: &["cards", "{id}", "actions", "comments"],
        resource: "comment",
        description: "Add a comment to a card.",
    },
    WriteEntry {
        id: "trello.comment.update",
        method: HttpMethod::Put,
        path: &["cards", "{id}", "actions", "{id2}", "comments"],
        resource: "comment",
        description: "Edit one of the credential's own comments.",
    },
    WriteEntry {
        id: "trello.label.create",
        method: HttpMethod::Post,
        path: &["labels"],
        resource: "label",
        description: "Create a label on a board.",
    },
    WriteEntry {
        id: "trello.label.add",
        method: HttpMethod::Post,
        path: &["cards", "{id}", "idLabels"],
        resource: "card",
        description: "Attach an existing label to a card.",
    },
    WriteEntry {
        id: "trello.label.remove",
        method: HttpMethod::Delete,
        path: &["cards", "{id}", "idLabels", "{id2}"],
        resource: "card",
        description: "Detach a label from a card; the label itself stays.",
    },
    WriteEntry {
        id: "trello.checklist.create",
        method: HttpMethod::Post,
        path: &["checklists"],
        resource: "checklist",
        description: "Create a checklist on a card.",
    },
    WriteEntry {
        id: "trello.checkitem.add",
        method: HttpMethod::Post,
        path: &["checklists", "{id}", "checkItems"],
        resource: "checkitem",
        description: "Add an item to a checklist.",
    },
    WriteEntry {
        id: "trello.checkitem.set-state",
        method: HttpMethod::Put,
        path: &["cards", "{id}", "checkItem", "{id2}"],
        resource: "checkitem",
        description: "Mark a checklist item complete or incomplete.",
    },
    WriteEntry {
        id: "trello.attachment.add-url",
        method: HttpMethod::Post,
        path: &["cards", "{id}", "attachments"],
        resource: "attachment",
        description: "Attach a link to a card.",
    },
    WriteEntry {
        id: "trello.member.add",
        method: HttpMethod::Post,
        path: &["cards", "{id}", "idMembers"],
        resource: "card",
        description: "Assign a board member to a card.",
    },
    WriteEntry {
        id: "trello.member.remove",
        method: HttpMethod::Delete,
        path: &["cards", "{id}", "idMembers", "{id2}"],
        resource: "card",
        description: "Unassign a member from a card.",
    },
];

pub fn write_commands() -> impl Iterator<Item = &'static str> {
    WRITES.iter().map(|entry| entry.id)
}

pub fn write_entry(command: &str) -> Option<&'static WriteEntry> {
    WRITES.iter().find(|entry| entry.id == command)
}

pub fn is_write(command: &str) -> bool {
    write_entry(command).is_some()
}

/// The single step name the journal records for a write: its route id.
pub fn step(command: &str) -> Option<String> {
    write_entry(command).map(|entry| entry.id.replace('.', "-"))
}

pub fn write_route(entry: &WriteEntry) -> NativeRoute {
    NativeRoute {
        id: entry.id.replace('.', "-"),
        method: entry.method,
        path: entry
            .path
            .iter()
            .map(|part| match *part {
                "{id}" => RouteSegment::Parameter("id".into()),
                "{id2}" => RouteSegment::Parameter("id2".into()),
                literal => RouteSegment::Literal(literal.into()),
            })
            .collect(),
        query_parameters: Vec::new(),
        repeated_query_parameters: Vec::new(),
        continuation_parameters: Vec::new(),
        response_max_bytes: 1_048_576,
    }
}

const COLORS: [&str; 10] = [
    "green", "yellow", "orange", "red", "purple", "blue", "sky", "lime", "pink", "black",
];

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Position {
    Top,
    Bottom,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CardCreateInput {
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub list_id: String,
    #[schemars(length(min = 1, max = 16384))]
    pub name: String,
    #[schemars(length(max = 16384))]
    pub description: Option<String>,
    #[schemars(length(min = 20, max = 40))]
    pub due: Option<String>,
    #[schemars(length(max = 20))]
    pub label_ids: Option<Vec<String>>,
    #[schemars(length(max = 20))]
    pub member_ids: Option<Vec<String>>,
    pub position: Option<Position>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CardUpdateInput {
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub id: String,
    #[schemars(length(min = 1, max = 16384))]
    pub name: Option<String>,
    #[schemars(length(max = 16384))]
    pub description: Option<String>,
    #[schemars(length(min = 20, max = 40))]
    pub due: Option<String>,
    pub due_complete: Option<bool>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CardMoveInput {
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub id: String,
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub list_id: String,
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub board_id: Option<String>,
    pub position: Option<Position>,
}

fn archived_default() -> bool {
    true
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CardArchiveInput {
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub id: String,
    #[serde(default = "archived_default")]
    pub archived: bool,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentAddInput {
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub card_id: String,
    #[schemars(length(min = 1, max = 16384))]
    pub text: String,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentUpdateInput {
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub card_id: String,
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub comment_id: String,
    #[schemars(length(min = 1, max = 16384))]
    pub text: String,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LabelCreateInput {
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub board_id: String,
    #[schemars(length(min = 1, max = 16384))]
    pub name: String,
    #[schemars(length(min = 3, max = 6))]
    pub color: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CardLabelInput {
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub card_id: String,
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub label_id: String,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CardMemberInput {
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub card_id: String,
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub member_id: String,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChecklistCreateInput {
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub card_id: String,
    #[schemars(length(min = 1, max = 16384))]
    pub name: String,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckItemAddInput {
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub checklist_id: String,
    #[schemars(length(min = 1, max = 16384))]
    pub name: String,
    #[serde(default)]
    pub complete: bool,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckItemStateInput {
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub card_id: String,
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub item_id: String,
    #[serde(default = "archived_default")]
    pub complete: bool,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttachmentAddInput {
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub card_id: String,
    #[schemars(length(min = 9, max = 2048), regex(pattern = "^https://[^\\s]+$"))]
    pub url: String,
    #[schemars(length(min = 1, max = 256))]
    pub name: Option<String>,
}

fn text(path: &str, value: &str, maximum: usize, minimum: usize) -> Result<(), CraftError> {
    if value.len() < minimum
        || value.len() > maximum
        || (minimum > 0 && value.trim().is_empty())
        || value
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(CraftError::field(path, "expected_bounded_text"));
    }
    Ok(())
}

fn due(value: &str) -> Result<(), CraftError> {
    let bytes = value.as_bytes();
    let shaped = value.len() >= 20
        && value.len() <= 40
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'-' | b':' | b'T' | b'Z' | b'.' | b'+'));
    if shaped {
        Ok(())
    } else {
        Err(CraftError::field("/due", "expected_rfc3339_timestamp"))
    }
}

fn ids(path: &str, values: &Option<Vec<String>>) -> Result<(), CraftError> {
    if let Some(values) = values {
        if values.len() > 20 {
            return Err(CraftError::field(path, "too_many_ids"));
        }
        for (index, value) in values.iter().enumerate() {
            id_field(path, value)?;
            if values[..index].contains(value) {
                return Err(CraftError::field(path, "duplicate_id"));
            }
        }
    }
    Ok(())
}

fn parse<T: DeserializeOwned>(request: &CanonicalRequest) -> Result<T, CraftError> {
    serde_json::from_value(request.input.clone())
        .map_err(|_| CraftError::field("/input", "invalid_trello_write_input"))
}

fn position(value: Option<Position>) -> Option<&'static str> {
    value.map(|value| match value {
        Position::Top => "top",
        Position::Bottom => "bottom",
    })
}

/// Validates the input and builds the single native call. Pure: no state or network.
pub fn build(request: &CanonicalRequest) -> Result<NativeCall, CraftError> {
    let entry = write_entry(&request.command)
        .ok_or_else(|| CraftError::field("/command", "unknown_trello_write"))?;
    let mut path = BTreeMap::new();
    let mut body = Map::new();
    let mut set = |key: &str, value: Value| {
        body.insert(key.into(), value);
    };
    match request.command.as_str() {
        "trello.card.create" => {
            let input: CardCreateInput = parse(request)?;
            id_field("/list_id", &input.list_id)?;
            text("/name", &input.name, 16_384, 1)?;
            ids("/label_ids", &input.label_ids)?;
            ids("/member_ids", &input.member_ids)?;
            set("idList", json!(input.list_id));
            set("name", json!(input.name));
            if let Some(description) = &input.description {
                text("/description", description, 16_384, 0)?;
                set("desc", json!(description));
            }
            if let Some(value) = &input.due {
                due(value)?;
                set("due", json!(value));
            }
            if let Some(labels) = &input.label_ids {
                set("idLabels", json!(labels.join(",")));
            }
            if let Some(members) = &input.member_ids {
                set("idMembers", json!(members.join(",")));
            }
            if let Some(pos) = position(input.position) {
                set("pos", json!(pos));
            }
        }
        "trello.card.update" => {
            let input: CardUpdateInput = parse(request)?;
            id_field("/id", &input.id)?;
            path.insert("id".into(), input.id);
            if let Some(name) = &input.name {
                text("/name", name, 16_384, 1)?;
                set("name", json!(name));
            }
            if let Some(description) = &input.description {
                text("/description", description, 16_384, 0)?;
                set("desc", json!(description));
            }
            if let Some(value) = &input.due {
                due(value)?;
                set("due", json!(value));
            }
            if let Some(done) = input.due_complete {
                set("dueComplete", json!(done));
            }
            if body.is_empty() {
                return Err(CraftError::field("/input", "expected_at_least_one_change"));
            }
        }
        "trello.card.move" => {
            let input: CardMoveInput = parse(request)?;
            id_field("/id", &input.id)?;
            id_field("/list_id", &input.list_id)?;
            path.insert("id".into(), input.id);
            set("idList", json!(input.list_id));
            if let Some(board) = &input.board_id {
                id_field("/board_id", board)?;
                set("idBoard", json!(board));
            }
            if let Some(pos) = position(input.position) {
                set("pos", json!(pos));
            }
        }
        "trello.card.archive" => {
            let input: CardArchiveInput = parse(request)?;
            id_field("/id", &input.id)?;
            path.insert("id".into(), input.id);
            set("closed", json!(input.archived));
        }
        "trello.comment.add" => {
            let input: CommentAddInput = parse(request)?;
            id_field("/card_id", &input.card_id)?;
            text("/text", &input.text, 16_384, 1)?;
            path.insert("id".into(), input.card_id);
            set("text", json!(input.text));
        }
        "trello.comment.update" => {
            let input: CommentUpdateInput = parse(request)?;
            id_field("/card_id", &input.card_id)?;
            id_field("/comment_id", &input.comment_id)?;
            text("/text", &input.text, 16_384, 1)?;
            path.insert("id".into(), input.card_id);
            path.insert("id2".into(), input.comment_id);
            set("text", json!(input.text));
        }
        "trello.label.create" => {
            let input: LabelCreateInput = parse(request)?;
            id_field("/board_id", &input.board_id)?;
            text("/name", &input.name, 16_384, 1)?;
            set("idBoard", json!(input.board_id));
            set("name", json!(input.name));
            if let Some(color) = &input.color {
                if !COLORS.contains(&color.as_str()) {
                    return Err(CraftError::field("/color", "expected_trello_label_color"));
                }
                set("color", json!(color));
            }
        }
        "trello.label.add" | "trello.label.remove" => {
            let input: CardLabelInput = parse(request)?;
            id_field("/card_id", &input.card_id)?;
            id_field("/label_id", &input.label_id)?;
            path.insert("id".into(), input.card_id);
            if request.command == "trello.label.add" {
                set("value", json!(input.label_id));
            } else {
                path.insert("id2".into(), input.label_id);
            }
        }
        "trello.member.add" | "trello.member.remove" => {
            let input: CardMemberInput = parse(request)?;
            id_field("/card_id", &input.card_id)?;
            id_field("/member_id", &input.member_id)?;
            path.insert("id".into(), input.card_id);
            if request.command == "trello.member.add" {
                set("value", json!(input.member_id));
            } else {
                path.insert("id2".into(), input.member_id);
            }
        }
        "trello.checklist.create" => {
            let input: ChecklistCreateInput = parse(request)?;
            id_field("/card_id", &input.card_id)?;
            text("/name", &input.name, 16_384, 1)?;
            set("idCard", json!(input.card_id));
            set("name", json!(input.name));
        }
        "trello.checkitem.add" => {
            let input: CheckItemAddInput = parse(request)?;
            id_field("/checklist_id", &input.checklist_id)?;
            text("/name", &input.name, 16_384, 1)?;
            path.insert("id".into(), input.checklist_id);
            set("name", json!(input.name));
            set("checked", json!(input.complete));
        }
        "trello.checkitem.set-state" => {
            let input: CheckItemStateInput = parse(request)?;
            id_field("/card_id", &input.card_id)?;
            id_field("/item_id", &input.item_id)?;
            path.insert("id".into(), input.card_id);
            path.insert("id2".into(), input.item_id);
            set(
                "state",
                json!(if input.complete {
                    "complete"
                } else {
                    "incomplete"
                }),
            );
        }
        "trello.attachment.add-url" => {
            let input: AttachmentAddInput = parse(request)?;
            id_field("/card_id", &input.card_id)?;
            if !input.url.starts_with("https://")
                || input.url.len() > 2048
                || input
                    .url
                    .chars()
                    .any(|c| c.is_whitespace() || c.is_control())
            {
                return Err(CraftError::field("/url", "expected_https_url"));
            }
            path.insert("id".into(), input.card_id);
            set("url", json!(input.url));
            if let Some(name) = &input.name {
                text("/name", name, 256, 1)?;
                set("name", json!(name));
            }
        }
        _ => return Err(CraftError::field("/command", "unknown_trello_write")),
    }
    Ok(NativeCall {
        route: write_route(entry).id,
        path_parameters: path,
        query: BTreeMap::new(),
        repeated_query: BTreeMap::new(),
        body: if body.is_empty() {
            None
        } else {
            Some(Value::Object(body))
        },
    })
}

/// What a single answer proves about the one request that was sent.
pub struct Observation {
    pub state: SubmissionState,
    pub resources: Vec<ResourceRef>,
    pub item: Option<Value>,
    pub acknowledged: bool,
    pub desired_fields_verified: bool,
    pub error: Option<CraftError>,
}

fn resource(kind: &str, tenant: &str, id: &str) -> ResourceRef {
    ResourceRef {
        provider: "trello".into(),
        deployment: "cloud".into(),
        tenant: tenant.into(),
        resource_type: kind.into(),
        id: id.into(),
    }
}

/// Compares the answer with what was asked, so a 200 that did not apply the change is visible.
fn desired(command: &str, input: &Value, body: &Value) -> bool {
    let same = |key: &str, native: &str| match input.get(key) {
        Some(Value::String(want)) => body.get(native).and_then(Value::as_str) == Some(want),
        Some(Value::Bool(want)) => body.get(native).and_then(Value::as_bool) == Some(*want),
        _ => true,
    };
    match command {
        "trello.card.create" => {
            same("name", "name") && same("list_id", "idList") && same("description", "desc")
        }
        "trello.card.update" => {
            same("name", "name")
                && same("description", "desc")
                && same("due_complete", "dueComplete")
        }
        "trello.card.move" => same("list_id", "idList"),
        "trello.card.archive" => {
            body.get("closed").and_then(Value::as_bool)
                == Some(
                    input
                        .get("archived")
                        .and_then(Value::as_bool)
                        .unwrap_or(true),
                )
        }
        "trello.comment.add" | "trello.comment.update" => {
            body.pointer("/data/text").and_then(Value::as_str)
                == input.get("text").and_then(Value::as_str)
        }
        "trello.label.create" => same("name", "name"),
        "trello.label.add" => body
            .as_array()
            .zip(input.get("label_id").and_then(Value::as_str))
            .is_some_and(|(labels, wanted)| labels.iter().any(|l| l.as_str() == Some(wanted))),
        "trello.member.add" => body
            .as_array()
            .zip(input.get("member_id").and_then(Value::as_str))
            .is_some_and(|(members, wanted)| {
                members
                    .iter()
                    .any(|m| m.get("id").and_then(Value::as_str) == Some(wanted))
            }),
        "trello.label.remove" | "trello.member.remove" => true,
        "trello.checklist.create" | "trello.checkitem.add" => same("name", "name"),
        "trello.checkitem.set-state" => {
            body.get("state").and_then(Value::as_str)
                == Some(
                    if input.get("complete").and_then(Value::as_bool) == Some(true) {
                        "complete"
                    } else {
                        "incomplete"
                    },
                )
        }
        "trello.attachment.add-url" => same("url", "url"),
        _ => false,
    }
}

/// Interprets the answer to the single sent request. A refusal is `Failed`; anything that
/// leaves the effect unknown (server error, malformed success) is `Unknown` and never replayed.
pub fn interpret(
    operation: &PreparedOperation,
    response: &NativeResponse,
) -> Result<Observation, CraftError> {
    let request = operation.request();
    let entry = write_entry(&request.command)
        .ok_or_else(|| CraftError::field("/command", "unknown_trello_write"))?;
    let tenant = &request.target.tenant;
    let refusal = |code: ErrorCode, message: &str| Observation {
        state: SubmissionState::Failed,
        resources: Vec::new(),
        item: None,
        acknowledged: false,
        desired_fields_verified: false,
        error: Some(CraftError::new(code, message)),
    };
    let unknown = |message: &str| Observation {
        state: SubmissionState::Unknown,
        resources: Vec::new(),
        item: None,
        acknowledged: response.status < 300,
        desired_fields_verified: false,
        error: Some(CraftError::new(ErrorCode::WriteOutcomeUnknown, message)),
    };
    Ok(match response.status {
        200 => {
            let body = &response.body;
            // The id Trello names for the touched resource: the new object for creates, the
            // card (or path object) for association changes.
            let id = match entry.resource {
                "card"
                    if matches!(
                        request.command.as_str(),
                        "trello.label.add"
                            | "trello.label.remove"
                            | "trello.member.add"
                            | "trello.member.remove"
                    ) =>
                {
                    request
                        .input
                        .get("card_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                }
                _ => body.get("id").and_then(Value::as_str).map(str::to_owned),
            };
            match id.filter(|id| trello_id(id)) {
                Some(id) => {
                    let verified = desired(&request.command, &request.input, body);
                    let mut observation = Observation {
                        state: SubmissionState::Confirmed,
                        resources: vec![resource(entry.resource, tenant, &id)],
                        item: Some(summary(&request.command, body)),
                        acknowledged: true,
                        desired_fields_verified: verified,
                        error: None,
                    };
                    if !verified {
                        observation.error = Some(CraftError::new(
                            ErrorCode::PartialApply,
                            "Trello acknowledged the request but its answer does not show the requested change; read the resource before any replay.",
                        ));
                    }
                    observation
                }
                None => unknown("Trello answered 200 without the expected resource id."),
            }
        }
        400 | 422 => refusal(
            ErrorCode::InputInvalid,
            "Trello refused the request as invalid.",
        ),
        401 => refusal(ErrorCode::AuthRequired, "Trello rejected the credential."),
        403 => refusal(
            ErrorCode::PermissionDenied,
            "Trello denied permission for this change.",
        ),
        404 => refusal(
            ErrorCode::InputInvalid,
            "Trello resource is absent or not visible; absence was not established.",
        ),
        429 => refusal(
            ErrorCode::RateLimited,
            "Trello rate limited the request; it was not applied.",
        ),
        _ => unknown("Trello's answer does not establish whether the change was applied."),
    })
}

/// Minimal projection of the answer, so receipts stay small and carry no provider extras.
fn summary(command: &str, body: &Value) -> Value {
    let pick = |keys: &[(&str, &str)]| {
        let mut out = Map::new();
        for (native, name) in keys {
            if let Some(value) = body
                .get(*native)
                .filter(|v| !v.is_null() && !v.is_object() && !v.is_array())
            {
                out.insert((*name).into(), value.clone());
            }
        }
        Value::Object(out)
    };
    match command {
        "trello.card.create"
        | "trello.card.update"
        | "trello.card.move"
        | "trello.card.archive" => pick(&[
            ("id", "id"),
            ("name", "name"),
            ("idList", "list_id"),
            ("idBoard", "board_id"),
            ("closed", "closed"),
            ("due", "due"),
            ("dueComplete", "due_complete"),
            ("url", "url"),
        ]),
        "trello.comment.add" | "trello.comment.update" => json!({
            "id": body.get("id"),
            "text": body.pointer("/data/text"),
        }),
        "trello.label.create" => pick(&[
            ("id", "id"),
            ("name", "name"),
            ("color", "color"),
            ("idBoard", "board_id"),
        ]),
        "trello.checklist.create" => pick(&[("id", "id"), ("name", "name"), ("idCard", "card_id")]),
        "trello.checkitem.add" | "trello.checkitem.set-state" => {
            pick(&[("id", "id"), ("name", "name"), ("state", "state")])
        }
        "trello.attachment.add-url" => pick(&[("id", "id"), ("name", "name"), ("url", "url")]),
        _ => Value::Null,
    }
}

pub fn validate(request: &CanonicalRequest) -> Result<(), CraftError> {
    crate::registration::validate_target(&request.target)?;
    if request.preconditions.is_some() {
        return Err(CraftError::field(
            "/preconditions",
            "unsupported_preconditions",
        ));
    }
    build(request).map(|_| ())
}

struct WriteHandler;
impl OperationHandler for WriteHandler {
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
            "Trello writes require the verified journaled host.",
        ))
    }
}

/// The reviewed result a write returns inside its receipt. Fields are only what was asked for
/// and confirmed; provider extras are dropped.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteItem {
    pub id: Option<String>,
    pub name: Option<String>,
    pub text: Option<String>,
    pub url: Option<String>,
    pub list_id: Option<String>,
    pub board_id: Option<String>,
    pub color: Option<String>,
    pub card_id: Option<String>,
    pub state: Option<String>,
    pub closed: Option<bool>,
    pub due: Option<String>,
    pub due_complete: Option<bool>,
}

fn example(command: &str) -> Value {
    let id = "000000000000000000000000";
    match command {
        "trello.card.create" => json!({"list_id":id,"name":"Fix login"}),
        "trello.card.update" => json!({"id":id,"name":"Fix login on Safari"}),
        "trello.card.move" => json!({"id":id,"list_id":id}),
        "trello.card.archive" => json!({"id":id}),
        "trello.comment.add" => json!({"card_id":id,"text":"Fixed in the next release"}),
        "trello.comment.update" => json!({"card_id":id,"comment_id":id,"text":"Edited"}),
        "trello.label.create" => json!({"board_id":id,"name":"bug","color":"red"}),
        "trello.label.add" | "trello.label.remove" => json!({"card_id":id,"label_id":id}),
        "trello.member.add" | "trello.member.remove" => json!({"card_id":id,"member_id":id}),
        "trello.checklist.create" => json!({"card_id":id,"name":"Steps"}),
        "trello.checkitem.add" => json!({"checklist_id":id,"name":"Write test"}),
        "trello.checkitem.set-state" => json!({"card_id":id,"item_id":id,"complete":true}),
        _ => json!({"card_id":id,"url":"https://example.com/spec"}),
    }
}

fn register_write<I: DeserializeOwned + JsonSchema>(
    registry: &mut Registry,
    entry: &WriteEntry,
) -> Result<(), CraftError> {
    let id = "000000000000000000000000";
    let data = json!({"receipt":{"operation_id":"example","request_state":"confirmed","confirmed_resources":[]},"item":null});
    let descriptor = OperationDescriptor {
        id: entry.id.into(),
        provider: "trello".into(),
        contract_version: SCHEMA_VERSION.into(),
        api_revision: "trello-rest-v1".into(),
        description: entry.description.into(),
        effect: EffectClass::Write,
        input_schema: schema_for::<I>()?,
        response_schema: response_schema::<Value>()?,
        example: example(entry.id),
        response_example: json!(Envelope::success(entry.id, "example".into(), data)),
        projection: None,
        scope_alternatives: vec![vec!["read".into(), "write".into()]],
        permissions: vec!["Trello board edit rights evaluated by Trello".into()],
        tenant_conditions: vec![format!(
            "Exact member account {id} bound to the profile; the change is one journaled request and is never replayed automatically."
        )],
        routes: vec![write_route(entry)],
        retry_class: RetryClass::ReconcileBeforeReplay,
        reconciliation: ReconciliationClass::KnownResource,
    };
    registry.register_typed_handler::<I>(descriptor, Arc::new(WriteHandler))
}

pub(crate) fn register(registry: &mut Registry) -> Result<(), CraftError> {
    for entry in WRITES {
        match entry.id {
            "trello.card.create" => register_write::<CardCreateInput>(registry, entry)?,
            "trello.card.update" => register_write::<CardUpdateInput>(registry, entry)?,
            "trello.card.move" => register_write::<CardMoveInput>(registry, entry)?,
            "trello.card.archive" => register_write::<CardArchiveInput>(registry, entry)?,
            "trello.comment.add" => register_write::<CommentAddInput>(registry, entry)?,
            "trello.comment.update" => register_write::<CommentUpdateInput>(registry, entry)?,
            "trello.label.create" => register_write::<LabelCreateInput>(registry, entry)?,
            "trello.label.add" | "trello.label.remove" => {
                register_write::<CardLabelInput>(registry, entry)?
            }
            "trello.member.add" | "trello.member.remove" => {
                register_write::<CardMemberInput>(registry, entry)?
            }
            "trello.checklist.create" => register_write::<ChecklistCreateInput>(registry, entry)?,
            "trello.checkitem.add" => register_write::<CheckItemAddInput>(registry, entry)?,
            "trello.checkitem.set-state" => register_write::<CheckItemStateInput>(registry, entry)?,
            _ => register_write::<AttachmentAddInput>(registry, entry)?,
        }
    }
    Ok(())
}
