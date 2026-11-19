//! Trello reads through a provisioned static grant, a separately enabled read policy and the
//! shared buffered paging engine. Preparation touches no credential or network.

use crate::dispatch::CommandResult;
use crate::output::OutputPolicy;
use crate::profiles::{InvocationOptions, selected_profile_context};
use crate::trello_writes::{
    AttachmentAddArgs, CardArchiveArgs, CardCreateArgs, CardLabelArgs, CardMemberArgs,
    CardMoveArgs, CardUpdateArgs, CheckItemAddArgs, CheckItemStateArgs, ChecklistCreateArgs,
    CommentAddArgs, CommentUpdateArgs, LabelCreateArgs, TrelloWriteArgs,
};
use clap::{Args, Subcommand};
use craft_core::contracts::{
    AuthMode, CanonicalRequest, Envelope, GrantRef, ResourceRef, Selection,
};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::PreparedOperation;
use craft_core::input::canonical_digest;
use craft_core::pagination::{
    ListBinding, ListPage, ListRequest, NativePage, NativePosition, PageSource,
};
use craft_core::projection::CompiledProjection;
use craft_core::registry::{Registry, schema_for};
use craft_provider_trello::{self as provider, TrelloContext, commands, is_get};
use craft_runtime::auth::ServiceOAuthClient;
use craft_runtime::control::SystemClock;
use craft_runtime::pagination::recovery_probe;
use craft_runtime::profiles::{ProfileStore, resolve_state_root};
use craft_runtime::provider_policy::ProviderPolicyRecord;
use craft_runtime::secrets::{LockedCredentials, NativeCredentials};
use craft_runtime::transport::{HttpContext, TlsPolicy};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

pub const READ_CLASS: &str = "trello.read";

fn page_size() -> u16 {
    25
}
fn limit() -> u16 {
    20
}

#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub id: String,
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoardListArgs {
    #[arg(long,default_value_t=25,value_parser=clap::value_parser!(u16).range(1..=100))]
    #[serde(default = "page_size")]
    #[schemars(range(min = 1, max = 100))]
    pub page_size: u16,
    #[arg(long,default_value_t=20,value_parser=clap::value_parser!(u16).range(1..=100))]
    #[serde(default = "limit")]
    #[schemars(range(min = 1, max = 100))]
    pub limit: u16,
    #[arg(long)]
    #[schemars(length(min = 36, max = 36))]
    pub cursor: Option<String>,
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoardArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub board_id: String,
    #[arg(long,default_value_t=25,value_parser=clap::value_parser!(u16).range(1..=100))]
    #[serde(default = "page_size")]
    #[schemars(range(min = 1, max = 100))]
    pub page_size: u16,
    #[arg(long,default_value_t=20,value_parser=clap::value_parser!(u16).range(1..=100))]
    #[serde(default = "limit")]
    #[schemars(range(min = 1, max = 100))]
    pub limit: u16,
    #[arg(long)]
    #[schemars(length(min = 36, max = 36))]
    pub cursor: Option<String>,
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub list_id: String,
    #[arg(long,default_value_t=25,value_parser=clap::value_parser!(u16).range(1..=100))]
    #[serde(default = "page_size")]
    #[schemars(range(min = 1, max = 100))]
    pub page_size: u16,
    #[arg(long,default_value_t=20,value_parser=clap::value_parser!(u16).range(1..=100))]
    #[serde(default = "limit")]
    #[schemars(range(min = 1, max = 100))]
    pub limit: u16,
    #[arg(long)]
    #[schemars(length(min = 36, max = 36))]
    pub cursor: Option<String>,
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CardArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub card_id: String,
    #[arg(long,default_value_t=25,value_parser=clap::value_parser!(u16).range(1..=100))]
    #[serde(default = "page_size")]
    #[schemars(range(min = 1, max = 100))]
    pub page_size: u16,
    #[arg(long,default_value_t=20,value_parser=clap::value_parser!(u16).range(1..=100))]
    #[serde(default = "limit")]
    #[schemars(range(min = 1, max = 100))]
    pub limit: u16,
    #[arg(long)]
    #[schemars(length(min = 36, max = 36))]
    pub cursor: Option<String>,
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 16384))]
    pub query: String,
    #[arg(long)]
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub board_id: Option<String>,
    #[arg(long,default_value_t=25,value_parser=clap::value_parser!(u16).range(1..=100))]
    #[serde(default = "page_size")]
    #[schemars(range(min = 1, max = 100))]
    pub page_size: u16,
    #[arg(long,default_value_t=20,value_parser=clap::value_parser!(u16).range(1..=100))]
    #[serde(default = "limit")]
    #[schemars(range(min = 1, max = 100))]
    pub limit: u16,
    #[arg(long)]
    #[schemars(length(min = 36, max = 36))]
    pub cursor: Option<String>,
}

#[derive(Subcommand)]
pub enum TrelloAction {
    Board {
        #[command(subcommand)]
        action: BoardAction,
    },
    List {
        #[command(subcommand)]
        action: ListAction,
    },
    Card {
        #[command(subcommand)]
        action: CardAction,
    },
    Comment {
        #[command(subcommand)]
        action: CommentAction,
    },
    Label {
        #[command(subcommand)]
        action: LabelAction,
    },
    Checklist {
        #[command(subcommand)]
        action: ChecklistAction,
    },
    Checkitem {
        #[command(subcommand)]
        action: CheckItemAction,
    },
    Attachment {
        #[command(subcommand)]
        action: AttachmentAction,
    },
    Member {
        #[command(subcommand)]
        action: MemberAction,
    },
}
#[derive(Subcommand)]
pub enum BoardAction {
    List(BoardListArgs),
    Get(IdArgs),
}
#[derive(Subcommand)]
pub enum ListAction {
    List(BoardArgs),
}
#[derive(Subcommand)]
pub enum CardAction {
    List(ListArgs),
    BoardList(BoardArgs),
    Get(IdArgs),
    Search(SearchArgs),
    Create(CardCreateArgs),
    Update(CardUpdateArgs),
    Move(CardMoveArgs),
    Archive(CardArchiveArgs),
}
#[derive(Subcommand)]
pub enum CommentAction {
    List(CardArgs),
    Add(CommentAddArgs),
    Update(CommentUpdateArgs),
}
#[derive(Subcommand)]
pub enum LabelAction {
    List(BoardArgs),
    Create(LabelCreateArgs),
    Add(CardLabelArgs),
    Remove(CardLabelArgs),
}
#[derive(Subcommand)]
pub enum ChecklistAction {
    List(CardArgs),
    Create(ChecklistCreateArgs),
}
#[derive(Subcommand)]
pub enum CheckItemAction {
    Add(CheckItemAddArgs),
    SetState(CheckItemStateArgs),
}
#[derive(Subcommand)]
pub enum AttachmentAction {
    List(CardArgs),
    AddUrl(AttachmentAddArgs),
}
#[derive(Subcommand)]
pub enum MemberAction {
    List(BoardArgs),
    Add(CardMemberArgs),
    Remove(CardMemberArgs),
}

/// Flat view of every Trello read, shared by parsing, JSON input and the host.
#[derive(Clone)]
pub enum TrelloReadArgs {
    BoardList(BoardListArgs),
    BoardGet(IdArgs),
    ListList(BoardArgs),
    CardList(ListArgs),
    CardBoardList(BoardArgs),
    CardGet(IdArgs),
    CardSearch(SearchArgs),
    CommentList(CardArgs),
    LabelList(BoardArgs),
    ChecklistList(CardArgs),
    AttachmentList(CardArgs),
    MemberList(BoardArgs),
}

fn encoded(value: &impl Serialize) -> Result<Value, CraftError> {
    serde_json::to_value(value).map_err(|_| CraftError::field("/input", "invalid_trello_arguments"))
}
fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, CraftError> {
    serde_json::from_value(value)
        .map_err(|_| CraftError::field("/input", "invalid_typed_trello_arguments"))
}

/// A parsed Trello command: either a read or a journaled write.
pub enum TrelloOp {
    Read(TrelloReadArgs),
    Write(TrelloWriteArgs),
}

impl TrelloOp {
    pub fn command(&self) -> &'static str {
        match self {
            Self::Read(read) => read.command(),
            Self::Write(write) => write.command(),
        }
    }
    pub fn arguments(&self) -> Result<Value, CraftError> {
        match self {
            Self::Read(read) => read.arguments(),
            Self::Write(write) => write.arguments(),
        }
    }
}

impl TrelloAction {
    pub fn op(&self) -> TrelloOp {
        use TrelloOp::{Read, Write};
        use TrelloReadArgs as R;
        use TrelloWriteArgs as W;
        match self {
            Self::Board {
                action: BoardAction::List(v),
            } => Read(R::BoardList(v.clone())),
            Self::Board {
                action: BoardAction::Get(v),
            } => Read(R::BoardGet(v.clone())),
            Self::List {
                action: ListAction::List(v),
            } => Read(R::ListList(v.clone())),
            Self::Card {
                action: CardAction::List(v),
            } => Read(R::CardList(v.clone())),
            Self::Card {
                action: CardAction::BoardList(v),
            } => Read(R::CardBoardList(v.clone())),
            Self::Card {
                action: CardAction::Get(v),
            } => Read(R::CardGet(v.clone())),
            Self::Card {
                action: CardAction::Search(v),
            } => Read(R::CardSearch(v.clone())),
            Self::Card {
                action: CardAction::Create(v),
            } => Write(W::CardCreate(v.clone())),
            Self::Card {
                action: CardAction::Update(v),
            } => Write(W::CardUpdate(v.clone())),
            Self::Card {
                action: CardAction::Move(v),
            } => Write(W::CardMove(v.clone())),
            Self::Card {
                action: CardAction::Archive(v),
            } => Write(W::CardArchive(v.clone())),
            Self::Comment {
                action: CommentAction::List(v),
            } => Read(R::CommentList(v.clone())),
            Self::Comment {
                action: CommentAction::Add(v),
            } => Write(W::CommentAdd(v.clone())),
            Self::Comment {
                action: CommentAction::Update(v),
            } => Write(W::CommentUpdate(v.clone())),
            Self::Label {
                action: LabelAction::List(v),
            } => Read(R::LabelList(v.clone())),
            Self::Label {
                action: LabelAction::Create(v),
            } => Write(W::LabelCreate(v.clone())),
            Self::Label {
                action: LabelAction::Add(v),
            } => Write(W::LabelAdd(v.clone())),
            Self::Label {
                action: LabelAction::Remove(v),
            } => Write(W::LabelRemove(v.clone())),
            Self::Checklist {
                action: ChecklistAction::List(v),
            } => Read(R::ChecklistList(v.clone())),
            Self::Checklist {
                action: ChecklistAction::Create(v),
            } => Write(W::ChecklistCreate(v.clone())),
            Self::Checkitem {
                action: CheckItemAction::Add(v),
            } => Write(W::CheckItemAdd(v.clone())),
            Self::Checkitem {
                action: CheckItemAction::SetState(v),
            } => Write(W::CheckItemSetState(v.clone())),
            Self::Attachment {
                action: AttachmentAction::List(v),
            } => Read(R::AttachmentList(v.clone())),
            Self::Attachment {
                action: AttachmentAction::AddUrl(v),
            } => Write(W::AttachmentAddUrl(v.clone())),
            Self::Member {
                action: MemberAction::List(v),
            } => Read(R::MemberList(v.clone())),
            Self::Member {
                action: MemberAction::Add(v),
            } => Write(W::MemberAdd(v.clone())),
            Self::Member {
                action: MemberAction::Remove(v),
            } => Write(W::MemberRemove(v.clone())),
        }
    }

    /// Rebuilds the action from a JSON object for the command the parser selected.
    pub fn with_input(&self, input: Value) -> Result<Self, CraftError> {
        let command = self.op().command();
        Ok(match command {
            "trello.board.list" => Self::Board {
                action: BoardAction::List(decode(input)?),
            },
            "trello.board.get" => Self::Board {
                action: BoardAction::Get(decode(input)?),
            },
            "trello.list.list" => Self::List {
                action: ListAction::List(decode(input)?),
            },
            "trello.card.list" => Self::Card {
                action: CardAction::List(decode(input)?),
            },
            "trello.card.board-list" => Self::Card {
                action: CardAction::BoardList(decode(input)?),
            },
            "trello.card.get" => Self::Card {
                action: CardAction::Get(decode(input)?),
            },
            "trello.card.search" => Self::Card {
                action: CardAction::Search(decode(input)?),
            },
            "trello.card.create" => Self::Card {
                action: CardAction::Create(decode(input)?),
            },
            "trello.card.update" => Self::Card {
                action: CardAction::Update(decode(input)?),
            },
            "trello.card.move" => Self::Card {
                action: CardAction::Move(decode(input)?),
            },
            "trello.card.archive" => Self::Card {
                action: CardAction::Archive(decode(input)?),
            },
            "trello.comment.list" => Self::Comment {
                action: CommentAction::List(decode(input)?),
            },
            "trello.comment.add" => Self::Comment {
                action: CommentAction::Add(decode(input)?),
            },
            "trello.comment.update" => Self::Comment {
                action: CommentAction::Update(decode(input)?),
            },
            "trello.label.list" => Self::Label {
                action: LabelAction::List(decode(input)?),
            },
            "trello.label.create" => Self::Label {
                action: LabelAction::Create(decode(input)?),
            },
            "trello.label.add" => Self::Label {
                action: LabelAction::Add(decode(input)?),
            },
            "trello.label.remove" => Self::Label {
                action: LabelAction::Remove(decode(input)?),
            },
            "trello.checklist.list" => Self::Checklist {
                action: ChecklistAction::List(decode(input)?),
            },
            "trello.checklist.create" => Self::Checklist {
                action: ChecklistAction::Create(decode(input)?),
            },
            "trello.checkitem.add" => Self::Checkitem {
                action: CheckItemAction::Add(decode(input)?),
            },
            "trello.checkitem.set-state" => Self::Checkitem {
                action: CheckItemAction::SetState(decode(input)?),
            },
            "trello.attachment.list" => Self::Attachment {
                action: AttachmentAction::List(decode(input)?),
            },
            "trello.attachment.add-url" => Self::Attachment {
                action: AttachmentAction::AddUrl(decode(input)?),
            },
            "trello.member.list" => Self::Member {
                action: MemberAction::List(decode(input)?),
            },
            "trello.member.add" => Self::Member {
                action: MemberAction::Add(decode(input)?),
            },
            "trello.member.remove" => Self::Member {
                action: MemberAction::Remove(decode(input)?),
            },
            _ => return Err(CraftError::field("/command", "unsupported_trello_command")),
        })
    }
}

impl TrelloReadArgs {
    pub fn command(&self) -> &'static str {
        match self {
            Self::BoardList(_) => "trello.board.list",
            Self::BoardGet(_) => "trello.board.get",
            Self::ListList(_) => "trello.list.list",
            Self::CardList(_) => "trello.card.list",
            Self::CardBoardList(_) => "trello.card.board-list",
            Self::CardGet(_) => "trello.card.get",
            Self::CardSearch(_) => "trello.card.search",
            Self::CommentList(_) => "trello.comment.list",
            Self::LabelList(_) => "trello.label.list",
            Self::ChecklistList(_) => "trello.checklist.list",
            Self::AttachmentList(_) => "trello.attachment.list",
            Self::MemberList(_) => "trello.member.list",
        }
    }

    pub fn arguments(&self) -> Result<Value, CraftError> {
        match self {
            Self::BoardList(v) => encoded(v),
            Self::BoardGet(v) | Self::CardGet(v) => encoded(v),
            Self::ListList(v)
            | Self::CardBoardList(v)
            | Self::LabelList(v)
            | Self::MemberList(v) => encoded(v),
            Self::CardList(v) => encoded(v),
            Self::CardSearch(v) => encoded(v),
            Self::CommentList(v) | Self::ChecklistList(v) | Self::AttachmentList(v) => encoded(v),
        }
    }

    /// The provider's input: the native selectors and page size, without output controls.
    fn input(&self) -> Result<Value, CraftError> {
        Ok(match self {
            Self::BoardList(v) => json!({"page_size": v.page_size}),
            Self::BoardGet(v) | Self::CardGet(v) => json!({"id": v.id}),
            Self::ListList(v)
            | Self::CardBoardList(v)
            | Self::LabelList(v)
            | Self::MemberList(v) => json!({"board_id": v.board_id, "page_size": v.page_size}),
            Self::CardList(v) => json!({"list_id": v.list_id, "page_size": v.page_size}),
            Self::CardSearch(v) => {
                json!({"query": v.query, "board_id": v.board_id, "page_size": v.page_size})
            }
            Self::CommentList(v) | Self::ChecklistList(v) | Self::AttachmentList(v) => {
                json!({"card_id": v.card_id, "page_size": v.page_size})
            }
        })
    }

    fn selection(&self, fields: &[String], bytes: usize) -> Result<Selection, CraftError> {
        let (limit, cursor) = match self {
            Self::BoardGet(_) | Self::CardGet(_) => (20, None),
            Self::BoardList(v) => (v.limit, v.cursor.clone()),
            Self::ListList(v)
            | Self::CardBoardList(v)
            | Self::LabelList(v)
            | Self::MemberList(v) => (v.limit, v.cursor.clone()),
            Self::CardList(v) => (v.limit, v.cursor.clone()),
            Self::CardSearch(v) => (v.limit, v.cursor.clone()),
            Self::CommentList(v) | Self::ChecklistList(v) | Self::AttachmentList(v) => {
                (v.limit, v.cursor.clone())
            }
        };
        if cursor.as_ref().is_some_and(|c| {
            !c.strip_prefix("cur_").is_some_and(|suffix| {
                suffix.len() == 32
                    && suffix
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        }) {
            return Err(CraftError::field(
                "/cursor",
                "expected_opaque_cursor_handle",
            ));
        }
        Ok(Selection {
            fields: fields.to_vec(),
            limit,
            cursor,
            max_bytes: bytes
                .try_into()
                .map_err(|_| CraftError::field("/max_bytes", "invalid_output_budget"))?,
        })
    }
}

fn account(tenant: &str) -> ResourceRef {
    ResourceRef {
        provider: "trello".into(),
        deployment: "cloud".into(),
        tenant: tenant.into(),
        resource_type: "account".into(),
        id: tenant.into(),
    }
}

/// Registers the CLI-level schemas (with output controls) over the provider's descriptors.
pub fn register_candidates(registry: &mut Registry) -> Result<(), CraftError> {
    let mut native = Registry::default();
    native.register_provider(&TrelloContext)?;
    for command in commands() {
        let mut descriptor = native.descriptor(command)?.clone();
        macro_rules! typed {
            ($args:ty) => {{
                descriptor.input_schema = schema_for::<$args>()?;
                registry.register_typed::<$args>(descriptor)?
            }};
        }
        match command {
            "trello.board.list" => typed!(BoardListArgs),
            "trello.board.get" | "trello.card.get" => typed!(IdArgs),
            "trello.list.list"
            | "trello.card.board-list"
            | "trello.label.list"
            | "trello.member.list" => typed!(BoardArgs),
            "trello.card.list" => typed!(ListArgs),
            "trello.card.search" => typed!(SearchArgs),
            _ => typed!(CardArgs),
        }
    }
    crate::trello_writes::register_candidates(registry)
}

/// Trusted host authority for one read, independent of any provider response.
pub struct TrelloReadPolicy {
    pub target: ResourceRef,
    pub grant: GrantRef,
    pub expected_actor: String,
    pub enabled: bool,
    pub allowed_commands: Vec<String>,
}

/// A fully prepared read. It performs no state, credential or network access.
pub struct PreparedTrelloRead {
    registry: Registry,
    operation: PreparedOperation,
    projection: CompiledProjection,
    binding: ListBinding,
    request_id: String,
}

fn internal() -> CraftError {
    CraftError::new(ErrorCode::Internal, "Cannot encode Trello read output.")
}

impl PreparedTrelloRead {
    pub fn prepare(
        request: CanonicalRequest,
        request_id: &str,
        policy: TrelloReadPolicy,
    ) -> Result<Self, CraftError> {
        craft_core::contracts::validate_identity("/request_id", request_id)?;
        policy.grant.validate()?;
        if !policy.grant.id.strip_prefix("gr_").is_some_and(|suffix| {
            suffix.len() == 32
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        }) || !policy.enabled
            || !policy
                .allowed_commands
                .iter()
                .any(|command| command == &request.command)
            || policy.target != request.target
            || !commands().any(|command| command == request.command)
        {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Trello read differs from the exact configured account, grant or enabled operations.",
            ));
        }
        let mut registry = Registry::default();
        registry.register_provider(&TrelloContext)?;
        let operation = registry.prepare(request)?;
        let command = operation.request().command.clone();
        let projection = registry
            .projection(&command, &operation.request().selection.fields)?
            .ok_or_else(internal)?;
        let descriptor = registry.prepared_descriptor(&operation)?;
        let binding = ListBinding {
            operation: command.clone(),
            descriptor_digest: canonical_digest(
                &serde_json::to_value(descriptor).map_err(|_| internal())?,
            )?,
            schema_digest: registry.schema_digest(&command, true)?,
            projection_digest: canonical_digest(
                &projection.response_schema(&descriptor.response_schema)?,
            )?,
            api_revision: descriptor.api_revision.clone(),
            target: operation.request().target.clone(),
            grant: policy.grant,
            actor: policy.expected_actor,
            query: BTreeMap::from([("input".into(), operation.request().input.clone())]),
        };
        binding.digest()?;
        let prepared = Self {
            registry,
            operation,
            projection,
            binding,
            request_id: request_id.into(),
        };
        // Reserve room for the largest error envelope before any credential or network use.
        let mut worst = CraftError::new(ErrorCode::Internal, "x".repeat(500));
        worst.next_action = "x".repeat(1000);
        if !is_get(&command)
            && !prepared.fits(&prepared.envelope(Some(&recovery_probe()), None)?)?
            || !prepared.fits(&prepared.envelope(None, Some(worst))?)?
        {
            let mut error = CraftError::new(
                ErrorCode::OutputBudgetTooSmall,
                "Trello reads require room for complete error and paging evidence.",
            );
            error.next_action = "Increase --max-bytes before authentication.".into();
            return Err(error);
        }
        Ok(prepared)
    }

    pub fn grant(&self) -> &GrantRef {
        &self.binding.grant
    }

    fn fits(&self, envelope: &Envelope) -> Result<bool, CraftError> {
        Ok(serde_json::to_vec(envelope)
            .map_err(|_| internal())?
            .len()
            .saturating_add(1)
            <= self.operation.request().selection.max_bytes as usize)
    }

    fn envelope(
        &self,
        page: Option<&ListPage>,
        error: Option<CraftError>,
    ) -> Result<Envelope, CraftError> {
        let command = &self.binding.operation;
        let mut out_page = None;
        let mut error = error;
        let data = if is_get(command) {
            json!({"target":self.binding.target,"item":null,"observed_at":null})
        } else {
            out_page = page.map(|page| page.page.clone());
            error = error.or_else(|| page.and_then(|page| page.error.clone()));
            json!({
                "target":self.binding.target,
                "items":page.map(|page| page.items.as_slice()).unwrap_or(&[]),
                "observation":page.map(|page| &page.observation),
                "cached":page.is_some_and(|page| page.cached),
            })
        };
        self.finish_envelope(data, out_page, error)
    }

    fn finish_envelope(
        &self,
        data: Value,
        page: Option<craft_core::contracts::Page>,
        error: Option<CraftError>,
    ) -> Result<Envelope, CraftError> {
        let data = self.projection.data(&data)?;
        let command = &self.binding.operation;
        let mut envelope = match error {
            Some(error) => {
                Envelope::failure_with_data(command, self.request_id.clone(), error, data)
            }
            None => Envelope::success(command, self.request_id.clone(), data),
        };
        envelope.page = page;
        Ok(envelope)
    }

    fn result(&self, envelope: Result<Envelope, CraftError>) -> CommandResult {
        CommandResult {
            envelope: envelope.unwrap_or_else(|error| {
                Envelope::failure(&self.binding.operation, self.request_id.clone(), error)
            }),
            output_policy: OutputPolicy::Bounded,
        }
    }

    /// Opens the stored static grant, rechecks policy at every native boundary and reads.
    pub fn execute(
        &self,
        client: &ServiceOAuthClient,
        locked: &LockedCredentials<'_>,
        store: &ProfileStore,
        tls: TlsPolicy,
        control: InvocationControl,
        check_policy: &dyn Fn() -> Result<(), CraftError>,
    ) -> CommandResult {
        let before = (|| {
            control.check()?;
            check_policy()?;
            if locked.grant() != self.binding.grant {
                return Err(CraftError::new(
                    ErrorCode::PolicyDenied,
                    "Locked grant differs from the prepared Trello-read binding.",
                ));
            }
            if !is_get(&self.binding.operation) {
                store.check_provider_cursor(&self.request(), &SystemClock::default(), &control)?;
            }
            Ok(())
        })();
        if let Err(error) = before {
            return self.result(self.envelope(None, Some(error)));
        }
        let client = match client.for_invocation(control.clone()) {
            Ok(client) => client,
            Err(error) => return self.result(self.envelope(None, Some(error))),
        };
        let mut source = TrelloSource {
            prepared: self,
            client: &client,
            locked,
            tls: tls.clone(),
            check_policy,
        };
        if is_get(&self.binding.operation) {
            let result = (|| {
                source.check_access(&self.binding, &control)?;
                let mut context = source.context(&control)?;
                let item = provider::read_get(
                    &self.operation,
                    &mut Guarded {
                        native: &mut context,
                        check: check_policy,
                    },
                )?;
                source.check_access(&self.binding, &control)?;
                let envelope = self.finish_envelope(
                    json!({
                        "target":self.binding.target,
                        "item":item,
                        "observed_at":craft_core::storage::Clock::unix_millis(&SystemClock::default()),
                    }),
                    None,
                    None,
                )?;
                if !self.fits(&envelope)? {
                    let mut error = CraftError::new(
                        ErrorCode::ItemTooLarge,
                        "The selected Trello item cannot fit in the complete output envelope.",
                    );
                    error.next_action =
                        "Select fewer fields or increase --max-bytes and perform a new live read."
                            .into();
                    return Err(error);
                }
                check_policy()?;
                control.check()?;
                Ok(envelope)
            })();
            return self.result(result.or_else(|error| self.envelope(None, Some(error))));
        }
        let result = store.provider_page(
            self.request(),
            &SystemClock::default(),
            &control,
            &mut source,
            |page| self.fits(&self.envelope(Some(page), None)?),
        );
        match result {
            Ok(page) => match check_policy().and_then(|()| control.check()) {
                Ok(()) => self.result(self.envelope(Some(&page), None)),
                Err(error) => self.result(self.envelope(None, Some(error))),
            },
            Err(error) => self.result(self.envelope(None, Some(error))),
        }
    }

    fn request(&self) -> ListRequest<'_> {
        ListRequest {
            binding: &self.binding,
            cursor: self.operation.request().selection.cursor.as_deref(),
            limit: self.operation.request().selection.limit,
        }
    }
}

struct TrelloSource<'a> {
    prepared: &'a PreparedTrelloRead,
    client: &'a ServiceOAuthClient,
    locked: &'a LockedCredentials<'a>,
    tls: TlsPolicy,
    check_policy: &'a dyn Fn() -> Result<(), CraftError>,
}

impl TrelloSource<'_> {
    fn context(&self, control: &InvocationControl) -> Result<HttpContext, CraftError> {
        let session = self
            .client
            .open_static_session(
                self.locked,
                &self.prepared.binding.target.tenant_ref(),
                &self.prepared.binding.actor,
            )
            .map_err(|failure| failure.error)?;
        HttpContext::for_operation(
            &self.prepared.registry,
            &self.prepared.operation,
            &self.prepared.binding.grant,
            session,
            self.tls.clone(),
            control.clone(),
        )
    }
}

impl PageSource for TrelloSource<'_> {
    fn check_access(
        &mut self,
        binding: &ListBinding,
        control: &InvocationControl,
    ) -> Result<(), CraftError> {
        control.check()?;
        (self.check_policy)()?;
        if binding != &self.prepared.binding || self.locked.grant() != binding.grant {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Trello traversal binding changed.",
            ));
        }
        Ok(())
    }

    fn fetch(
        &mut self,
        binding: &ListBinding,
        position: Option<&NativePosition>,
        control: &InvocationControl,
    ) -> Result<NativePage, CraftError> {
        self.check_access(binding, control)?;
        let mut context = self.context(control)?;
        provider::read_page(
            &self.prepared.operation,
            &mut Guarded {
                native: &mut context,
                check: self.check_policy,
            },
            position,
        )
    }
}

/// Rechecks trusted policy before every native call the provider makes.
struct Guarded<'a> {
    native: &'a mut dyn craft_core::execution::OperationContext,
    check: &'a dyn Fn() -> Result<(), CraftError>,
}
impl craft_core::execution::OperationContext for Guarded<'_> {
    fn checkpoint(&self) -> Result<(), CraftError> {
        (self.check)()?;
        self.native.checkpoint()
    }
    fn verify_operation(&self, op: &PreparedOperation) -> Result<(), CraftError> {
        self.checkpoint()?;
        self.native.verify_operation(op)
    }
    fn invoke(
        &mut self,
        call: &craft_core::execution::NativeCall,
    ) -> Result<craft_core::execution::NativeResponse, CraftError> {
        self.checkpoint()?;
        self.native.invoke(call)
    }
    fn normalize_next(
        &self,
        call: &craft_core::execution::NativeCall,
        link: &str,
    ) -> Result<craft_core::execution::NativeCall, CraftError> {
        self.checkpoint()?;
        self.native.normalize_next(call, link)
    }
}

/// Everything needed to run one read after resolving profile, policy and request.
pub struct PreparedInvocation {
    read: PreparedTrelloRead,
    policy: ProviderPolicyRecord,
}

impl PreparedInvocation {
    pub fn prepare(
        args: &TrelloReadArgs,
        fields: &[String],
        options: &InvocationOptions<'_>,
        store: &ProfileStore,
        control: &InvocationControl,
    ) -> Result<Self, CraftError> {
        control.check()?;
        let (name, _, _) = selected_profile_context(store, None, options, control)?;
        let profile = store.get(&name, control)?;
        if profile.provider != "trello"
            || profile.deployment != "cloud"
            || profile.auth_mode != AuthMode::Service
        {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Trello reads require a configured Trello Cloud service profile.",
            ));
        }
        let target = account(&profile.tenant_ref()?.tenant);
        let policy = store
            .provider_policy(&name, READ_CLASS, control)?
            .ok_or_else(|| {
                CraftError::new(
                    ErrorCode::PolicyDenied,
                    "No trusted Trello read policy is present.",
                )
            })?;
        store.verify_provider_policy(&policy, args.command(), control)?;
        let read = PreparedTrelloRead::prepare(
            CanonicalRequest {
                command: args.command().into(),
                target: target.clone(),
                input: args.input()?,
                selection: args.selection(fields, options.max_bytes)?,
                preconditions: None,
            },
            options.request_id,
            TrelloReadPolicy {
                target,
                grant: policy.grant.clone(),
                expected_actor: policy.expected_actor.clone(),
                enabled: policy.enabled,
                allowed_commands: policy.operations.clone(),
            },
        )?;
        Ok(Self { read, policy })
    }

    pub fn grant(&self) -> &GrantRef {
        self.read.grant()
    }

    pub fn execute(
        &self,
        store: &ProfileStore,
        client: &ServiceOAuthClient,
        locked: &LockedCredentials<'_>,
        tls: TlsPolicy,
        control: InvocationControl,
    ) -> CommandResult {
        let check = control.clone();
        let command = self.read.binding.operation.clone();
        self.read.execute(client, locked, store, tls, control, &|| {
            store.verify_provider_policy(&self.policy, &command, &check)
        })
    }
}

/// Validates before private state and uses only the selected profile's native credential grant.
pub fn execute(
    args: TrelloReadArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> CommandResult {
    let command = args.command();
    let result = (|| {
        let root = resolve_state_root(options.state_root, |key| std::env::var_os(key))?;
        let store = ProfileStore::open(&root, control)?;
        let prepared = PreparedInvocation::prepare(&args, fields, options, &store, control)?;
        let credentials = NativeCredentials::open(&store)?;
        let locked = credentials.lock(prepared.grant(), control)?;
        let client = ServiceOAuthClient::new(
            TlsPolicy::Platform,
            control.clone(),
            Arc::new(SystemClock::default()),
        )?;
        Ok(prepared.execute(
            &store,
            &client,
            &locked,
            TlsPolicy::Platform,
            control.clone(),
        ))
    })();
    result.unwrap_or_else(|error| CommandResult {
        envelope: Envelope::failure(command, options.request_id.into(), error),
        output_policy: OutputPolicy::Bounded,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEMBER: &str = "5f1d2c3b4a5968778695a4b3";
    const BOARD: &str = "5f1d2c3b4a5968778695a4b4";

    fn policy(command: &str) -> TrelloReadPolicy {
        TrelloReadPolicy {
            target: account(MEMBER),
            grant: GrantRef {
                id: format!("gr_{}", "0".repeat(32)),
            },
            expected_actor: MEMBER.into(),
            enabled: true,
            allowed_commands: vec![command.into()],
        }
    }

    fn request(command: &str, input: Value) -> CanonicalRequest {
        CanonicalRequest {
            command: command.into(),
            target: account(MEMBER),
            input,
            selection: Selection::default(),
            preconditions: None,
        }
    }

    #[test]
    fn preparation_refuses_untrusted_authority_input_fields_and_recovery_budget()
    -> Result<(), Box<dyn std::error::Error>> {
        let input = json!({"id": BOARD});
        for mode in [
            "disabled", "excluded", "grant", "target", "fields", "input", "budget",
        ] {
            let mut policy = policy("trello.board.get");
            let mut request = request("trello.board.get", input.clone());
            match mode {
                "disabled" => policy.enabled = false,
                "excluded" => policy.allowed_commands.clear(),
                "grant" => policy.grant.id = "unissued".into(),
                "target" => request.target.provider = "jira".into(),
                "fields" => request.selection.fields = vec!["secret_token".into()],
                "input" => request.input["board"] = json!("other"),
                _ => request.selection.max_bytes = 512,
            }
            let error = PreparedTrelloRead::prepare(request, "request", policy)
                .err()
                .ok_or("expected refusal")?;
            if mode == "budget" {
                assert_eq!(error.code, ErrorCode::OutputBudgetTooSmall);
            }
        }
        Ok(())
    }

    #[test]
    fn a_policy_for_one_command_does_not_allow_another() {
        let result = PreparedTrelloRead::prepare(
            request("trello.card.get", json!({"id": BOARD})),
            "request",
            policy("trello.board.get"),
        );
        assert!(result.is_err());
    }

    #[test]
    fn cursor_binding_ignores_field_order_and_defaults() -> Result<(), Box<dyn std::error::Error>> {
        let mut first = request(
            "trello.list.list",
            json!({"board_id": BOARD, "page_size": 25}),
        );
        first.selection.fields = vec!["id".into(), "name".into()];
        let mut second = first.clone();
        second.selection.fields.reverse();
        let a = PreparedTrelloRead::prepare(first, "a", policy("trello.list.list"))?;
        let b = PreparedTrelloRead::prepare(second, "b", policy("trello.list.list"))?;
        assert_eq!(a.binding, b.binding);
        Ok(())
    }

    #[test]
    fn flags_and_json_decode_to_the_same_provider_input() -> Result<(), Box<dyn std::error::Error>>
    {
        let args = TrelloReadArgs::CardList(decode(json!({"list_id": BOARD}))?);
        assert_eq!(args.input()?, json!({"list_id": BOARD, "page_size": 25}));
        assert_eq!(args.command(), "trello.card.list");
        assert!(decode::<ListArgs>(json!({"list_id": BOARD, "unknown": 1})).is_err());
        Ok(())
    }
}
