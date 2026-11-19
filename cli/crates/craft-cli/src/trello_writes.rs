//! Trello writes through a provisioned static grant, a separately enabled `trello-write` policy
//! and the local operation journal. One command is one claimed request: it is recorded before it
//! is sent, never repeated automatically, and every outcome keeps its receipt.

use crate::dispatch::CommandResult;
use crate::issue_create::PersistenceState;
use crate::output::OutputPolicy;
use crate::profiles::InvocationOptions;
use clap::Args;
use craft_core::contracts::{
    CanonicalRequest, EffectClass, Envelope, GrantRef, OperationReceipt, RequestState, ResourceRef,
    Selection, validate_identity,
};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{NativeResponse, PreparedOperation, RetryAfter};
use craft_core::registry::{Registry, response_schema, schema_for};
use craft_core::storage::{
    JournalRecord, OperationBinding, OperationJournal, StoreError, SubmissionState,
};
use craft_provider_trello::{self as provider, TrelloContext};
use craft_runtime::auth::ServiceOAuthClient;
use craft_runtime::control::SystemClock;
use craft_runtime::journal::{MutationAuthority, NativeJournal, StepOutcome};
use craft_runtime::profiles::{ProfileStore, resolve_state_root};
use craft_runtime::provider_policy::{ProviderPolicyRecord, TRELLO_WRITE};
use craft_runtime::secrets::{LockedCredentials, NativeCredentials};
use craft_runtime::transport::{PreparedHttpMutation, TlsPolicy};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

/// Receipts carry journal records, so reserve room for them.
pub const MINIMUM_OUTPUT: usize = 65_536;

fn archived_default() -> bool {
    true
}

#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CardCreateArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub list_id: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 16384))]
    pub name: String,
    #[arg(long)]
    #[schemars(length(max = 16384))]
    pub description: Option<String>,
    #[arg(long)]
    #[schemars(length(min = 20, max = 40))]
    pub due: Option<String>,
    #[arg(long, value_delimiter = ',', num_args = 1)]
    #[schemars(length(max = 20))]
    pub label_ids: Option<Vec<String>>,
    #[arg(long, value_delimiter = ',', num_args = 1)]
    #[schemars(length(max = 20))]
    pub member_ids: Option<Vec<String>>,
    #[arg(long)]
    #[schemars(extend("enum" = ["top", "bottom", null]))]
    pub position: Option<String>,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 128))]
    pub operation_id: String,
    #[arg(long)]
    #[serde(default)]
    pub apply: bool,
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CardUpdateArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub id: String,
    #[arg(long)]
    #[schemars(length(min = 1, max = 16384))]
    pub name: Option<String>,
    #[arg(long)]
    #[schemars(length(max = 16384))]
    pub description: Option<String>,
    #[arg(long)]
    #[schemars(length(min = 20, max = 40))]
    pub due: Option<String>,
    #[arg(long)]
    pub due_complete: Option<bool>,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 128))]
    pub operation_id: String,
    #[arg(long)]
    #[serde(default)]
    pub apply: bool,
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CardMoveArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub id: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub list_id: String,
    #[arg(long)]
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub board_id: Option<String>,
    #[arg(long)]
    #[schemars(extend("enum" = ["top", "bottom", null]))]
    pub position: Option<String>,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 128))]
    pub operation_id: String,
    #[arg(long)]
    #[serde(default)]
    pub apply: bool,
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CardArchiveArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub id: String,
    #[arg(long, action = clap::ArgAction::Set, default_value_t = true)]
    #[serde(default = "archived_default")]
    pub archived: bool,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 128))]
    pub operation_id: String,
    #[arg(long)]
    #[serde(default)]
    pub apply: bool,
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentAddArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub card_id: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 16384))]
    pub text: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 128))]
    pub operation_id: String,
    #[arg(long)]
    #[serde(default)]
    pub apply: bool,
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentUpdateArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub card_id: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub comment_id: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 16384))]
    pub text: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 128))]
    pub operation_id: String,
    #[arg(long)]
    #[serde(default)]
    pub apply: bool,
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LabelCreateArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub board_id: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 16384))]
    pub name: String,
    #[arg(long)]
    #[schemars(length(min = 3, max = 6))]
    pub color: Option<String>,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 128))]
    pub operation_id: String,
    #[arg(long)]
    #[serde(default)]
    pub apply: bool,
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CardLabelArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub card_id: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub label_id: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 128))]
    pub operation_id: String,
    #[arg(long)]
    #[serde(default)]
    pub apply: bool,
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CardMemberArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub card_id: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub member_id: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 128))]
    pub operation_id: String,
    #[arg(long)]
    #[serde(default)]
    pub apply: bool,
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChecklistCreateArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub card_id: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 16384))]
    pub name: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 128))]
    pub operation_id: String,
    #[arg(long)]
    #[serde(default)]
    pub apply: bool,
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckItemAddArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub checklist_id: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 16384))]
    pub name: String,
    #[arg(long, action = clap::ArgAction::Set, default_value_t = false)]
    #[serde(default)]
    pub complete: bool,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 128))]
    pub operation_id: String,
    #[arg(long)]
    #[serde(default)]
    pub apply: bool,
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckItemStateArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub card_id: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub item_id: String,
    #[arg(long, action = clap::ArgAction::Set, default_value_t = true)]
    #[serde(default = "archived_default")]
    pub complete: bool,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 128))]
    pub operation_id: String,
    #[arg(long)]
    #[serde(default)]
    pub apply: bool,
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttachmentAddArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 24, max = 24), regex(pattern = "^[0-9a-f]{24}$"))]
    pub card_id: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 9, max = 2048), regex(pattern = r"^https://[^\s]+$"))]
    pub url: String,
    #[arg(long)]
    #[schemars(length(min = 1, max = 256))]
    pub name: Option<String>,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 128))]
    pub operation_id: String,
    #[arg(long)]
    #[serde(default)]
    pub apply: bool,
}

/// Flat view of every Trello write, shared by parsing, JSON input and the host.
#[derive(Clone)]
pub enum TrelloWriteArgs {
    CardCreate(CardCreateArgs),
    CardUpdate(CardUpdateArgs),
    CardMove(CardMoveArgs),
    CardArchive(CardArchiveArgs),
    CommentAdd(CommentAddArgs),
    CommentUpdate(CommentUpdateArgs),
    LabelCreate(LabelCreateArgs),
    LabelAdd(CardLabelArgs),
    LabelRemove(CardLabelArgs),
    MemberAdd(CardMemberArgs),
    MemberRemove(CardMemberArgs),
    ChecklistCreate(ChecklistCreateArgs),
    CheckItemAdd(CheckItemAddArgs),
    CheckItemSetState(CheckItemStateArgs),
    AttachmentAddUrl(AttachmentAddArgs),
}

fn encoded(value: &impl Serialize) -> Result<Value, CraftError> {
    serde_json::to_value(value).map_err(|_| CraftError::field("/input", "invalid_trello_arguments"))
}
fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, CraftError> {
    serde_json::from_value(value)
        .map_err(|_| CraftError::field("/input", "invalid_typed_trello_arguments"))
}

impl TrelloWriteArgs {
    pub fn command(&self) -> &'static str {
        match self {
            Self::CardCreate(_) => "trello.card.create",
            Self::CardUpdate(_) => "trello.card.update",
            Self::CardMove(_) => "trello.card.move",
            Self::CardArchive(_) => "trello.card.archive",
            Self::CommentAdd(_) => "trello.comment.add",
            Self::CommentUpdate(_) => "trello.comment.update",
            Self::LabelCreate(_) => "trello.label.create",
            Self::LabelAdd(_) => "trello.label.add",
            Self::LabelRemove(_) => "trello.label.remove",
            Self::MemberAdd(_) => "trello.member.add",
            Self::MemberRemove(_) => "trello.member.remove",
            Self::ChecklistCreate(_) => "trello.checklist.create",
            Self::CheckItemAdd(_) => "trello.checkitem.add",
            Self::CheckItemSetState(_) => "trello.checkitem.set-state",
            Self::AttachmentAddUrl(_) => "trello.attachment.add-url",
        }
    }

    pub fn arguments(&self) -> Result<Value, CraftError> {
        match self {
            Self::CardCreate(v) => encoded(v),
            Self::CardUpdate(v) => encoded(v),
            Self::CardMove(v) => encoded(v),
            Self::CardArchive(v) => encoded(v),
            Self::CommentAdd(v) => encoded(v),
            Self::CommentUpdate(v) => encoded(v),
            Self::LabelCreate(v) => encoded(v),
            Self::LabelAdd(v) | Self::LabelRemove(v) => encoded(v),
            Self::MemberAdd(v) | Self::MemberRemove(v) => encoded(v),
            Self::ChecklistCreate(v) => encoded(v),
            Self::CheckItemAdd(v) => encoded(v),
            Self::CheckItemSetState(v) => encoded(v),
            Self::AttachmentAddUrl(v) => encoded(v),
        }
    }

    /// Decodes JSON input for the command the parser selected.
    pub fn decode(command: &str, input: Value) -> Result<Self, CraftError> {
        Ok(match command {
            "trello.card.create" => Self::CardCreate(decode(input)?),
            "trello.card.update" => Self::CardUpdate(decode(input)?),
            "trello.card.move" => Self::CardMove(decode(input)?),
            "trello.card.archive" => Self::CardArchive(decode(input)?),
            "trello.comment.add" => Self::CommentAdd(decode(input)?),
            "trello.comment.update" => Self::CommentUpdate(decode(input)?),
            "trello.label.create" => Self::LabelCreate(decode(input)?),
            "trello.label.add" => Self::LabelAdd(decode(input)?),
            "trello.label.remove" => Self::LabelRemove(decode(input)?),
            "trello.member.add" => Self::MemberAdd(decode(input)?),
            "trello.member.remove" => Self::MemberRemove(decode(input)?),
            "trello.checklist.create" => Self::ChecklistCreate(decode(input)?),
            "trello.checkitem.add" => Self::CheckItemAdd(decode(input)?),
            "trello.checkitem.set-state" => Self::CheckItemSetState(decode(input)?),
            "trello.attachment.add-url" => Self::AttachmentAddUrl(decode(input)?),
            _ => return Err(CraftError::field("/command", "unsupported_trello_write")),
        })
    }

    pub fn operation_id(&self) -> &str {
        match self {
            Self::CardCreate(v) => &v.operation_id,
            Self::CardUpdate(v) => &v.operation_id,
            Self::CardMove(v) => &v.operation_id,
            Self::CardArchive(v) => &v.operation_id,
            Self::CommentAdd(v) => &v.operation_id,
            Self::CommentUpdate(v) => &v.operation_id,
            Self::LabelCreate(v) => &v.operation_id,
            Self::LabelAdd(v) | Self::LabelRemove(v) => &v.operation_id,
            Self::MemberAdd(v) | Self::MemberRemove(v) => &v.operation_id,
            Self::ChecklistCreate(v) => &v.operation_id,
            Self::CheckItemAdd(v) => &v.operation_id,
            Self::CheckItemSetState(v) => &v.operation_id,
            Self::AttachmentAddUrl(v) => &v.operation_id,
        }
    }

    pub fn apply(&self) -> bool {
        match self {
            Self::CardCreate(v) => v.apply,
            Self::CardUpdate(v) => v.apply,
            Self::CardMove(v) => v.apply,
            Self::CardArchive(v) => v.apply,
            Self::CommentAdd(v) => v.apply,
            Self::CommentUpdate(v) => v.apply,
            Self::LabelCreate(v) => v.apply,
            Self::LabelAdd(v) | Self::LabelRemove(v) => v.apply,
            Self::MemberAdd(v) | Self::MemberRemove(v) => v.apply,
            Self::ChecklistCreate(v) => v.apply,
            Self::CheckItemAdd(v) => v.apply,
            Self::CheckItemSetState(v) => v.apply,
            Self::AttachmentAddUrl(v) => v.apply,
        }
    }

    /// The provider's input: the native fields without the host's operation controls.
    fn input(&self) -> Result<Value, CraftError> {
        let mut value = self.arguments()?;
        let object = value
            .as_object_mut()
            .ok_or_else(|| CraftError::field("/input", "invalid_trello_arguments"))?;
        object.remove("operation_id");
        object.remove("apply");
        object.retain(|_, field| !field.is_null());
        Ok(value)
    }
}

/// Registers the CLI-level schemas (with operation id and apply) over the provider descriptors.
pub fn register_candidates(registry: &mut Registry) -> Result<(), CraftError> {
    let mut native = Registry::default();
    native.register_provider(&TrelloContext)?;
    for command in provider::write_commands() {
        let mut descriptor = native.descriptor(command)?.clone();
        descriptor.response_schema = response_schema::<TrelloWriteData>()?;
        descriptor.example["operation_id"] = json!("agent-change");
        descriptor.example["apply"] = json!(true);
        descriptor.response_example =
            json!(Envelope::success(command, "example".into(), example_data()));
        macro_rules! typed {
            ($args:ty) => {{
                descriptor.input_schema = schema_for::<$args>()?;
                registry.register_typed::<$args>(descriptor)?
            }};
        }
        match command {
            "trello.card.create" => typed!(CardCreateArgs),
            "trello.card.update" => typed!(CardUpdateArgs),
            "trello.card.move" => typed!(CardMoveArgs),
            "trello.card.archive" => typed!(CardArchiveArgs),
            "trello.comment.add" => typed!(CommentAddArgs),
            "trello.comment.update" => typed!(CommentUpdateArgs),
            "trello.label.create" => typed!(LabelCreateArgs),
            "trello.label.add" | "trello.label.remove" => typed!(CardLabelArgs),
            "trello.member.add" | "trello.member.remove" => typed!(CardMemberArgs),
            "trello.checklist.create" => typed!(ChecklistCreateArgs),
            "trello.checkitem.add" => typed!(CheckItemAddArgs),
            "trello.checkitem.set-state" => typed!(CheckItemStateArgs),
            _ => typed!(AttachmentAddArgs),
        }
    }
    Ok(())
}

/// A complete, schema-valid receipt used as the documented response example.
fn example_data() -> Value {
    let id = "000000000000000000000000";
    json!({
        "receipt": {
            "operation_id": "agent-change",
            "request_state": "confirmed",
            "confirmed_resources": [{
                "provider": "trello", "deployment": "cloud", "tenant": id,
                "resource_type": "card", "id": id
            }]
        },
        "target": {
            "provider": "trello", "deployment": "cloud", "tenant": id,
            "resource_type": "account", "id": id
        },
        "item": {"id": id, "name": "Fix login"},
        "acknowledged": true,
        "desired_fields_verified": true,
        "persistence": "recorded",
        "prior_record": null,
        "latest_record": null,
        "pending_record": null,
        "pending_record_state": "recorded",
        "http_status": 200,
        "provider_request_id": null,
        "retry_after": null,
        "reconciliation_required": false,
        "replay_authorized": false
    })
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TrelloWriteData {
    pub receipt: OperationReceipt,
    pub target: ResourceRef,
    pub item: Option<provider::WriteItem>,
    pub acknowledged: bool,
    pub desired_fields_verified: bool,
    pub persistence: PersistenceState,
    pub prior_record: Option<JournalRecord>,
    pub latest_record: Option<JournalRecord>,
    pub pending_record: Option<JournalRecord>,
    pub pending_record_state: PersistenceState,
    pub http_status: Option<u16>,
    pub provider_request_id: Option<String>,
    pub retry_after: Option<RetryAfter>,
    pub reconciliation_required: bool,
    #[schemars(extend("const" = false))]
    pub replay_authorized: bool,
}

fn denied(message: &str) -> CraftError {
    CraftError::new(ErrorCode::PolicyDenied, message)
}

fn storage(_: StoreError) -> CraftError {
    CraftError::new(
        ErrorCode::Internal,
        "Cannot inspect the exact Trello operation journal.",
    )
}

fn request_state(state: SubmissionState) -> RequestState {
    match state {
        SubmissionState::Prepared => RequestState::NotSent,
        SubmissionState::Confirmed => RequestState::Confirmed,
        SubmissionState::Failed => RequestState::Failed,
        _ => RequestState::SentUnknown,
    }
}

fn empty(binding: &OperationBinding) -> TrelloWriteData {
    TrelloWriteData {
        receipt: OperationReceipt {
            operation_id: binding.operation_id.clone(),
            request_state: RequestState::NotSent,
            confirmed_resources: vec![],
            job: None,
        },
        target: binding.target.clone(),
        item: None,
        acknowledged: false,
        desired_fields_verified: false,
        persistence: PersistenceState::Unrecorded,
        prior_record: None,
        latest_record: None,
        pending_record: None,
        pending_record_state: PersistenceState::Unrecorded,
        http_status: None,
        provider_request_id: None,
        retry_after: None,
        reconciliation_required: false,
        replay_authorized: false,
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

pub struct PreparedTrelloWrite {
    registry: Registry,
    write: PreparedOperation,
    binding: OperationBinding,
    policy: ProviderPolicyRecord,
    step: String,
}

pub struct TrelloWriteEnvironment<'a, 'credentials> {
    pub store: &'a ProfileStore,
    pub client: &'a ServiceOAuthClient,
    pub locked: &'a LockedCredentials<'credentials>,
    pub journal: &'a NativeJournal,
    pub tls: TlsPolicy,
    pub control: InvocationControl,
}

impl PreparedTrelloWrite {
    /// Validates the request, the enabled operation and the exact policy binding. Nothing is
    /// sent, claimed or read from the credential store.
    pub fn prepare(
        request: CanonicalRequest,
        operation_id: &str,
        apply: bool,
        profile: &str,
        store: &ProfileStore,
        control: &InvocationControl,
    ) -> Result<Self, CraftError> {
        if !apply {
            return Err(CraftError::new(
                ErrorCode::ApplyRequired,
                "Trello writes require explicit apply.",
            ));
        }
        if !(MINIMUM_OUTPUT..=16_777_216).contains(&(request.selection.max_bytes as usize)) {
            return Err(CraftError::new(
                ErrorCode::OutputBudgetTooSmall,
                "Reserve --max-bytes 65536 for complete journal and recovery receipts.",
            ));
        }
        let mut registry = Registry::default();
        registry.register_provider(&TrelloContext)?;
        let write = registry.prepare(request)?;
        let command = write.request().command.clone();
        let policy = store
            .provider_policy(profile, TRELLO_WRITE, control)?
            .ok_or_else(|| denied("No trusted Trello write policy is present."))?;
        store.verify_provider_policy(&policy, &command, control)?;
        if policy.target != write.request().target {
            return Err(denied(
                "Trello write target differs from the configured account.",
            ));
        }
        let binding =
            write.mutation_binding(operation_id, &policy.grant, &policy.expected_actor)?;
        let step = provider::step(&command)
            .ok_or_else(|| CraftError::field("/command", "unknown_trello_write"))?;
        Ok(Self {
            registry,
            write,
            binding,
            policy,
            step,
        })
    }

    pub fn grant(&self) -> &GrantRef {
        &self.binding.grant
    }

    pub fn binding(&self) -> &OperationBinding {
        &self.binding
    }

    /// An existing record may only be an untouched prepared step for this exact request.
    fn prior(&self, journal: &NativeJournal, data: &mut TrelloWriteData) -> Result<(), CraftError> {
        let record = journal.get(&self.binding.operation_id).map_err(|error| {
            data.persistence = PersistenceState::Uncertain;
            data.reconciliation_required = true;
            storage(error)
        })?;
        if data.prior_record.is_none() {
            data.prior_record = record.clone();
        } else {
            data.latest_record = record.clone();
        }
        if record.is_some() {
            data.persistence = PersistenceState::Recorded;
        }
        if let Some(record) = record
            && (record.binding != self.binding
                || record
                    .steps
                    .iter()
                    .any(|(name, s)| name != &self.step || s.state != SubmissionState::Prepared))
        {
            return Err(CraftError::new(
                ErrorCode::OperationIdReused,
                "Trello operation changed or already started; retain its journal and do not replay it.",
            ));
        }
        Ok(())
    }

    pub fn execute(
        self,
        environment: TrelloWriteEnvironment<'_, '_>,
        extra: impl Fn() -> Result<(), CraftError>,
    ) -> (TrelloWriteData, Option<CraftError>) {
        let TrelloWriteEnvironment {
            store,
            client,
            locked,
            journal,
            tls,
            control,
        } = environment;
        let journal = journal.for_invocation(&control);
        let mut data = empty(&self.binding);
        let mut claim_attempted = false;
        let command = self.write.request().command.clone();
        let result = (|| {
            let policy_check = || {
                control.check()?;
                extra()?;
                store.verify_provider_policy(&self.policy, &command, &control)?;
                extra()?;
                control.check()
            };
            self.prior(&journal, &mut data)?;
            policy_check()?;
            if locked.grant() != self.binding.grant {
                return Err(denied(
                    "Locked grant differs from the captured Trello write policy.",
                ));
            }
            let call = provider::build(self.write.request())?;
            let client = client.for_invocation(control.clone())?;
            let session = client
                .open_static_session(
                    locked,
                    &self.binding.target.tenant_ref(),
                    &self.binding.actor,
                )
                .map_err(|failure| {
                    let mut error = failure.error;
                    error.details.write_state = RequestState::NotSent;
                    error
                })?;
            policy_check()?;
            self.prior(&journal, &mut data)?;
            let mutation = PreparedHttpMutation::prepare(
                &self.registry,
                &self.write,
                session,
                tls,
                control.clone(),
                &call,
            )?;
            claim_attempted = true;
            let attempt = mutation.send_checked(
                &journal,
                &MutationAuthority {
                    apply: true,
                    operation_id: &self.binding.operation_id,
                    grant: &self.binding.grant,
                    actor: &self.binding.actor,
                },
                &self.step,
                policy_check,
            )?;
            finish(
                &self.write,
                &self.binding,
                &self.step,
                Completion {
                    binding: attempt.binding(),
                    command: attempt.command(),
                    route: attempt.route(),
                    step: attempt.step(),
                    response: attempt.response.as_ref(),
                    pending: attempt.pending_evidence.as_ref(),
                },
                &mut data,
                |outcome| attempt.record_outcome(outcome),
            )
        })();
        let error = result.err().map(|mut error| {
            if data.latest_record.is_none() && claim_attempted {
                data.latest_record = journal.get(&self.binding.operation_id).ok().flatten();
                if data.latest_record.is_some() {
                    data.persistence = PersistenceState::Recorded;
                } else if error.details.write_state == RequestState::SentUnknown {
                    data.persistence = PersistenceState::Uncertain;
                }
            }
            if data.http_status.is_none() {
                if let Some(record) = data.latest_record.as_ref().or(data.prior_record.as_ref())
                    && record.binding == self.binding
                    && record.validate().is_ok()
                    && let Some(saved) = record.steps.get(&self.step)
                {
                    data.receipt.request_state = request_state(saved.state);
                    data.receipt.confirmed_resources = saved.confirmed_resources.clone();
                    data.receipt.job = saved.job.clone();
                }
                if error.details.write_state == RequestState::SentUnknown
                    && data.receipt.request_state == RequestState::NotSent
                {
                    data.receipt.request_state = RequestState::SentUnknown;
                }
            }
            data.reconciliation_required = data.reconciliation_required
                || data.receipt.request_state != RequestState::NotSent
                || data.prior_record.is_some()
                || data.latest_record.is_some();
            error.details.write_state = data.receipt.request_state;
            error.next_action = "Retain all Trello receipts and inspect this operation ID; read the resource before any new attempt. No automatic retry or replay is authorized.".into();
            error
        });
        (data, error)
    }
}

struct Completion<'a> {
    binding: &'a OperationBinding,
    command: &'a str,
    route: &'a str,
    step: &'a str,
    response: Result<&'a NativeResponse, &'a CraftError>,
    pending: Result<&'a JournalRecord, &'a StoreError>,
}

fn merge(
    record: &JournalRecord,
    step: &str,
    state: &mut SubmissionState,
    resources: &mut Vec<ResourceRef>,
) {
    if let Some(saved) = record.steps.get(step) {
        for resource in &saved.confirmed_resources {
            if !resources.contains(resource) {
                resources.push(resource.clone());
            }
        }
        if saved.state == SubmissionState::Confirmed {
            *state = SubmissionState::Confirmed;
        }
        if *state == SubmissionState::Failed && !resources.is_empty() {
            *state = SubmissionState::Unknown;
        }
    }
}

/// Records the interpreted outcome durably and assembles the receipt. A response that cannot be
/// attributed to the claimed step is recorded as unknown, never as a clean failure.
fn finish(
    operation: &PreparedOperation,
    binding: &OperationBinding,
    step: &str,
    view: Completion<'_>,
    data: &mut TrelloWriteData,
    record: impl FnOnce(&StepOutcome) -> Result<JournalRecord, StoreError>,
) -> Result<(), CraftError> {
    let command = &operation.request().command;
    let recognized = view.binding == binding
        && view.command == command
        && view.route == step
        && view.step == step;
    let mut observation = match view.response {
        Ok(response) if recognized => provider::interpret(operation, response)?,
        _ => provider::Observation {
            state: SubmissionState::Unknown,
            resources: vec![],
            item: None,
            acknowledged: false,
            desired_fields_verified: false,
            error: Some(CraftError::new(
                view.response
                    .err()
                    .map_or(ErrorCode::PolicyDenied, |e| e.code),
                "Trello attempt lacks qualified response evidence.",
            )),
        },
    };
    let mut state = observation.state;
    let mut resources = observation.resources.clone();
    let expected = resources.clone();
    data.pending_record = view.pending.ok().cloned();
    data.pending_record_state = match view.pending {
        Ok(_) => PersistenceState::Recorded,
        Err(StoreError::OutcomeUnknown) => PersistenceState::Uncertain,
        Err(_) => PersistenceState::Unrecorded,
    };
    if let Some(pending) = &data.pending_record {
        if pending.binding == *binding && pending.validate().is_ok() {
            merge(pending, step, &mut state, &mut resources);
        } else {
            observation.error = Some(CraftError::new(
                ErrorCode::PartialApply,
                "Initial Trello journal evidence has a different or invalid binding.",
            ));
        }
    }
    match record(&StepOutcome {
        state,
        confirmed_resources: resources.clone(),
        job: None,
    }) {
        Ok(saved) => {
            let valid = saved.binding == *binding && saved.validate().is_ok();
            let consistent = valid
                && saved.steps.get(step).is_some_and(|s| {
                    s.state == state
                        && s.job.is_none()
                        && resources.iter().all(|r| s.confirmed_resources.contains(r))
                });
            if valid {
                merge(&saved, step, &mut state, &mut resources);
            }
            data.persistence = if consistent {
                PersistenceState::Recorded
            } else {
                PersistenceState::Uncertain
            };
            data.latest_record = Some(saved);
            if !consistent {
                observation.error = Some(CraftError::new(
                    ErrorCode::PartialApply,
                    "Trello journal acknowledgment conflicts with observed native effects.",
                ));
            }
        }
        Err(error) => {
            data.persistence = if error == StoreError::OutcomeUnknown {
                PersistenceState::Uncertain
            } else {
                PersistenceState::Unrecorded
            };
            observation.error.get_or_insert_with(|| {
                CraftError::new(
                    ErrorCode::Internal,
                    "Observed Trello effects could not be durably acknowledged locally.",
                )
            });
        }
    }
    if let Ok(response) = view.response {
        data.http_status = Some(response.status);
        data.provider_request_id = response.request_id.clone();
        data.retry_after = response.retry_after;
    }
    if resources.iter().any(|r| !expected.contains(r))
        || data
            .latest_record
            .as_ref()
            .is_some_and(|r| r.steps.len() != 1)
        || data
            .pending_record
            .as_ref()
            .is_some_and(|r| r.steps.len() != 1)
    {
        observation.error = Some(CraftError::new(
            ErrorCode::PartialApply,
            "Additional recorded Trello effects require inspection.",
        ));
    }
    data.receipt.request_state = request_state(state);
    data.receipt.confirmed_resources = resources;
    data.acknowledged = observation.acknowledged;
    data.desired_fields_verified = observation.desired_fields_verified;
    data.item = observation
        .item
        .and_then(|item| serde_json::from_value(item).ok());
    data.reconciliation_required = observation.error.is_some();
    observation.error.map_or(Ok(()), Err)
}

/// Runs one write end to end for the selected profile. Validation precedes any private state.
pub fn execute(
    args: TrelloWriteArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> CommandResult {
    let command = args.command();
    let result = (|| {
        validate_identity("/operation_id", args.operation_id())?;
        validate_identity("/request_id", options.request_id)?;
        let name = options
            .profile
            .ok_or_else(|| CraftError::field("/profile", "explicit_trello_profile_required"))?;
        craft_core::profiles::validate_profile_name(name)?;
        if !fields.is_empty() || options.request_id.len() > 80 {
            return Err(CraftError::field(
                "/output",
                "complete_trello_receipts_required",
            ));
        }
        let root = resolve_state_root(options.state_root, |key| std::env::var_os(key))?;
        let store = ProfileStore::open(&root, control)?;
        let profile = store.get(name, control)?;
        if profile.provider != "trello" {
            return Err(denied("Trello writes require a Trello service profile."));
        }
        let tenant = profile.tenant_ref()?.tenant;
        let prepared = PreparedTrelloWrite::prepare(
            CanonicalRequest {
                command: command.into(),
                target: account(&tenant),
                input: args.input()?,
                selection: Selection {
                    max_bytes: u32::try_from(options.max_bytes)
                        .map_err(|_| CraftError::field("/max_bytes", "invalid_output_budget"))?,
                    ..Selection::default()
                },
                preconditions: None,
            },
            args.operation_id(),
            args.apply(),
            name,
            &store,
            control,
        )?;
        let journal = NativeJournal::open(&store, control).map_err(storage)?;
        let credentials = NativeCredentials::open(&store)?;
        let locked = credentials.lock(prepared.grant(), control)?;
        let client = ServiceOAuthClient::new(
            TlsPolicy::Platform,
            control.clone(),
            Arc::new(SystemClock::default()),
        )?;
        Ok(prepared.execute(
            TrelloWriteEnvironment {
                store: &store,
                client: &client,
                locked: &locked,
                journal: &journal,
                tls: TlsPolicy::Platform,
                control: control.clone(),
            },
            || Ok(()),
        ))
    })();
    let envelope = match result {
        Ok((data, Some(error))) => {
            Envelope::failure_with_data(command, options.request_id.into(), error, json!(data))
        }
        Ok((data, None)) => Envelope::success(command, options.request_id.into(), json!(data)),
        Err(error) => Envelope::failure(command, options.request_id.into(), error),
    };
    CommandResult {
        envelope,
        output_policy: OutputPolicy::PreserveEvidence,
    }
}

const _: fn() = || {
    let _ = EffectClass::Write;
};
