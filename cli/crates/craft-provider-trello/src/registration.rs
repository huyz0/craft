use crate::models::*;
use crate::table::{ENTRIES, Entry, is_get, route};
use craft_core::contracts::{CanonicalRequest, EffectClass, Envelope, ResourceRef, SCHEMA_VERSION};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{OperationContext, OperationHandler, PreparedOperation};
use craft_core::projection::{ProjectionContract, ProjectionMode};
use craft_core::registry::{
    OperationDescriptor, ProviderManifest, ProviderModule, ReconciliationClass, Registry,
    RetryClass, response_schema, schema_for,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::sync::Arc;

pub struct TrelloContext;

const ZERO_ID: &str = "000000000000000000000000";

/// The exact Trello account a profile is bound to. Board access is whatever the credential can
/// see; a policy narrows operations, not boards.
pub fn validate_target(target: &ResourceRef) -> Result<(), CraftError> {
    target.validate()?;
    if target.provider != "trello"
        || target.deployment != "cloud"
        || target.resource_type != "account"
        || target.id != target.tenant
        || !trello_id(&target.tenant)
    {
        return Err(CraftError::field(
            "/target",
            "expected_exact_trello_cloud_account",
        ));
    }
    Ok(())
}

fn defaults(command: &str) -> &'static [&'static str] {
    match command {
        "trello.board.list" | "trello.board.get" => &["id", "name", "closed", "url"],
        "trello.list.list" => &["id", "name", "position", "closed"],
        "trello.card.list"
        | "trello.card.board-list"
        | "trello.card.get"
        | "trello.card.search" => &[
            "id",
            "name",
            "list_id",
            "due",
            "label_ids",
            "member_ids",
            "closed",
        ],
        "trello.comment.list" => &["id", "text", "date", "author_username"],
        "trello.label.list" => &["id", "name", "color"],
        "trello.checklist.list" => &["id", "name", "items"],
        "trello.attachment.list" => &["id", "name", "url"],
        _ => &["id", "username", "full_name"],
    }
}

pub fn projection(command: &str) -> ProjectionContract {
    ProjectionContract {
        item_pointer: if is_get(command) { "/item" } else { "/items" }.into(),
        always_returned: vec!["id".into()],
        default_selected: defaults(command).iter().map(|f| (*f).into()).collect(),
        mode: ProjectionMode::Mixed,
    }
}

pub fn response_schema_for(command: &str) -> Result<Value, CraftError> {
    macro_rules! shape {
        ($item:ty) => {
            if is_get(command) {
                response_schema::<GetData<$item>>()
            } else {
                response_schema::<ListData<$item>>()
            }
        };
    }
    match command {
        "trello.board.list" | "trello.board.get" => shape!(BoardSummary),
        "trello.list.list" => shape!(ListSummary),
        "trello.card.list"
        | "trello.card.board-list"
        | "trello.card.get"
        | "trello.card.search" => shape!(CardSummary),
        "trello.comment.list" => shape!(CommentSummary),
        "trello.label.list" => shape!(LabelSummary),
        "trello.checklist.list" => shape!(ChecklistSummary),
        "trello.attachment.list" => shape!(AttachmentSummary),
        "trello.member.list" => shape!(MemberSummary),
        _ => Err(CraftError::field("/command", "unknown_trello_read")),
    }
}

fn item_example(command: &str) -> Value {
    match command {
        "trello.board.list" | "trello.board.get" => {
            json!({"id":ZERO_ID,"name":"Sprint board","description":null,"closed":false,"url":"https://trello.com/b/example","last_activity":null})
        }
        "trello.list.list" => {
            json!({"id":ZERO_ID,"name":"Doing","closed":false,"position":16384.0,"board_id":ZERO_ID})
        }
        "trello.card.list"
        | "trello.card.board-list"
        | "trello.card.get"
        | "trello.card.search" => {
            json!({"id":ZERO_ID,"name":"Fix login","description":null,"closed":false,"due":null,"due_complete":null,"list_id":ZERO_ID,"board_id":ZERO_ID,"label_ids":[],"member_ids":[],"url":null,"short_link":null,"last_activity":null,"position":null,"badges":null})
        }
        "trello.comment.list" => {
            json!({"id":ZERO_ID,"text":"Looks good","date":null,"author_id":ZERO_ID,"author_username":"agent","card_id":ZERO_ID})
        }
        "trello.label.list" => json!({"id":ZERO_ID,"name":"bug","color":"red","board_id":ZERO_ID}),
        "trello.checklist.list" => {
            json!({"id":ZERO_ID,"name":"Steps","card_id":ZERO_ID,"items":[{"id":ZERO_ID,"name":"Write test","complete":false,"position":1.0}]})
        }
        "trello.attachment.list" => {
            json!({"id":ZERO_ID,"name":"spec.pdf","url":"https://example.invalid/spec.pdf","bytes":10,"date":null,"mime_type":null,"is_upload":true})
        }
        _ => json!({"id":ZERO_ID,"username":"agent","full_name":"Agent"}),
    }
}

fn input_example(command: &str) -> Value {
    match command {
        "trello.board.list" => json!({"page_size":25}),
        "trello.board.get" | "trello.card.get" => json!({"id":ZERO_ID}),
        "trello.list.list"
        | "trello.label.list"
        | "trello.member.list"
        | "trello.card.board-list" => json!({"board_id":ZERO_ID}),
        "trello.card.list" => json!({"list_id":ZERO_ID}),
        "trello.card.search" => json!({"query":"login","page_size":25}),
        _ => json!({"card_id":ZERO_ID}),
    }
}

fn register_one<I: DeserializeOwned + schemars::JsonSchema>(
    registry: &mut Registry,
    entry: &Entry,
) -> Result<(), CraftError> {
    let command = entry.id;
    let target = json!({"provider":"trello","deployment":"cloud","tenant":ZERO_ID,"resource_type":"account","id":ZERO_ID});
    let data = if is_get(command) {
        json!({"target":target,"item":item_example(command),"observed_at":0})
    } else {
        json!({"target":target,"items":[item_example(command)],"observation":null,"cached":false})
    };
    let descriptor = OperationDescriptor {
        id: command.into(),
        provider: "trello".into(),
        contract_version: SCHEMA_VERSION.into(),
        api_revision: "trello-rest-v1".into(),
        description: entry.description.into(),
        effect: EffectClass::Read,
        input_schema: schema_for::<I>()?,
        response_schema: response_schema_for(command)?,
        example: input_example(command),
        response_example: json!(Envelope::success(command, "example".into(), data)),
        projection: Some(projection(command)),
        scope_alternatives: vec![vec!["read".into()]],
        permissions: vec!["Trello board and card visibility evaluated by Trello".into()],
        tenant_conditions: vec![
            "Exact member account bound to the profile; visibility is not absence or write authority.".into(),
        ],
        routes: vec![route(entry)],
        retry_class: RetryClass::SafeRead,
        reconciliation: ReconciliationClass::NotApplicable,
    };
    registry.register_typed_handler::<I>(descriptor, Arc::new(Handler))
}

impl ProviderModule for TrelloContext {
    fn manifest(&self) -> ProviderManifest {
        ProviderManifest {
            id: "trello".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            deployments: vec!["cloud".into()],
            contract_version: SCHEMA_VERSION.into(),
        }
    }

    fn register(&self, registry: &mut Registry) -> Result<(), CraftError> {
        for entry in ENTRIES {
            match entry.id {
                "trello.board.list" => register_one::<BoardListInput>(registry, entry)?,
                "trello.board.get" | "trello.card.get" => register_one::<IdInput>(registry, entry)?,
                "trello.list.list"
                | "trello.label.list"
                | "trello.member.list"
                | "trello.card.board-list" => register_one::<BoardScopedInput>(registry, entry)?,
                "trello.card.list" => register_one::<ListScopedInput>(registry, entry)?,
                "trello.card.search" => register_one::<SearchInput>(registry, entry)?,
                _ => register_one::<CardScopedInput>(registry, entry)?,
            }
        }
        crate::writes::register(registry)
    }
}

struct Handler;
impl OperationHandler for Handler {
    fn validate(&self, request: &CanonicalRequest) -> Result<(), CraftError> {
        validate_target(&request.target)?;
        let get = is_get(&request.command);
        if request.preconditions.is_some()
            || (get
                && (request.selection.cursor.is_some()
                    || request.selection.limit
                        != craft_core::contracts::Selection::default().limit))
        {
            return Err(CraftError::field(
                "/selection",
                "unsupported_get_paging_or_read_preconditions",
            ));
        }
        let parse = || CraftError::field("/input", "invalid_trello_read_input");
        match request.command.as_str() {
            "trello.board.list" => {
                let input: BoardListInput =
                    serde_json::from_value(request.input.clone()).map_err(|_| parse())?;
                page_size_field(input.page_size)?;
            }
            "trello.board.get" | "trello.card.get" => {
                let input: IdInput =
                    serde_json::from_value(request.input.clone()).map_err(|_| parse())?;
                id_field("/id", &input.id)?;
            }
            "trello.list.list"
            | "trello.label.list"
            | "trello.member.list"
            | "trello.card.board-list" => {
                let input: BoardScopedInput =
                    serde_json::from_value(request.input.clone()).map_err(|_| parse())?;
                id_field("/board_id", &input.board_id)?;
                page_size_field(input.page_size)?;
            }
            "trello.card.list" => {
                let input: ListScopedInput =
                    serde_json::from_value(request.input.clone()).map_err(|_| parse())?;
                id_field("/list_id", &input.list_id)?;
                page_size_field(input.page_size)?;
            }
            "trello.card.search" => {
                let input: SearchInput =
                    serde_json::from_value(request.input.clone()).map_err(|_| parse())?;
                validate_search(&input)?;
            }
            "trello.comment.list" | "trello.checklist.list" | "trello.attachment.list" => {
                let input: CardScopedInput =
                    serde_json::from_value(request.input.clone()).map_err(|_| parse())?;
                id_field("/card_id", &input.card_id)?;
                page_size_field(input.page_size)?;
            }
            _ => return Err(parse()),
        }
        Ok(())
    }

    fn execute(
        &self,
        _: &PreparedOperation,
        _: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "Trello reads require the verified bounded host.",
        ))
    }
}
