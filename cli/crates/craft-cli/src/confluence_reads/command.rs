//! Typed resource selectors resolve one profile; private configuration supplies read authority.
use super::{ConfluenceReadPolicy, PreparedConfluenceRead};
use crate::dispatch::CommandResult;
use crate::output::OutputPolicy;
use crate::profiles::{InvocationOptions, selected_profile_context};
use clap::Args;
use craft_core::contracts::{
    AuthMode, CanonicalRequest, Envelope, GrantRef, ResourceRef, Selection,
};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::profiles::validate_profile_name;
use craft_core::registry::{Registry, schema_for};
use craft_provider_confluence::{
    ConfluenceContext, GetInput, ListStatus, PAGE_GET, PAGE_LIST, PAGE_SEARCH, PageListInput,
    SPACE_GET, SPACE_LIST, SearchInput, SpaceListInput,
};
use craft_runtime::auth::ServiceOAuthClient;
use craft_runtime::confluence_read_policy::{
    ConfluenceReadOperation as Operation, ConfluenceReadPolicyRecord,
};
use craft_runtime::control::SystemClock;
use craft_runtime::profiles::{ProfileStore, resolve_state_root};
use craft_runtime::secrets::{LockedCredentials, NativeCredentials};
use craft_runtime::transport::TlsPolicy;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
#[cfg(test)]
mod tests;
fn page_size() -> u16 {
    25
}
fn limit() -> u16 {
    20
}
fn status(value: &str) -> Result<ListStatus, String> {
    serde_json::from_value(json!(value))
        .map_err(|_| "expected current, archived, deleted or trashed".into())
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfluenceGetArgs {
    #[arg(long)]
    #[schemars(length(min = 36, max = 36))]
    pub cloud_id: Option<String>,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 19), regex(pattern = "^[1-9][0-9]*$"))]
    pub id: String,
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfluenceSpaceListArgs {
    #[arg(long)]
    #[schemars(length(min = 36, max = 36))]
    pub cloud_id: Option<String>,
    #[arg(long)]
    #[schemars(length(min = 1, max = 256))]
    pub key: Option<String>,
    #[arg(long,default_value_t=25,value_parser=clap::value_parser!(u16).range(1..=250))]
    #[serde(default = "page_size")]
    #[schemars(range(min = 1, max = 250))]
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
pub struct ConfluencePageListArgs {
    #[arg(long)]
    #[schemars(length(min = 36, max = 36))]
    pub cloud_id: Option<String>,
    #[arg(long)]
    #[schemars(length(min = 1, max = 19), regex(pattern = "^[1-9][0-9]*$"))]
    pub space_id: Option<String>,
    #[arg(long)]
    #[schemars(length(min = 1, max = 4096))]
    pub title: Option<String>,
    #[arg(long,value_parser=status,default_value="current")]
    #[serde(default)]
    pub status: ListStatus,
    #[arg(long,default_value_t=25,value_parser=clap::value_parser!(u16).range(1..=250))]
    #[serde(default = "page_size")]
    #[schemars(range(min = 1, max = 250))]
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
pub struct ConfluenceSearchArgs {
    #[arg(long)]
    #[schemars(length(min = 36, max = 36))]
    pub cloud_id: Option<String>,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 16384))]
    pub cql: String,
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
pub struct ConfluenceCommentGetArgs {
    #[arg(long)]
    #[schemars(length(min = 36, max = 36))]
    pub cloud_id: Option<String>,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 19), regex(pattern = "^[1-9][0-9]*$"))]
    pub id: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 19), regex(pattern = "^[1-9][0-9]*$"))]
    pub page_id: String,
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfluenceCommentListArgs {
    #[arg(long)]
    #[schemars(length(min = 36, max = 36))]
    pub cloud_id: Option<String>,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 19), regex(pattern = "^[1-9][0-9]*$"))]
    pub page_id: String,
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
pub struct ConfluenceCommentRepliesArgs {
    #[arg(long)]
    #[schemars(length(min = 36, max = 36))]
    pub cloud_id: Option<String>,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 19), regex(pattern = "^[1-9][0-9]*$"))]
    pub page_id: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 19), regex(pattern = "^[1-9][0-9]*$"))]
    pub parent_comment_id: String,
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
pub struct ConfluenceContentStateArgs {
    #[arg(long)]
    #[schemars(length(min = 36, max = 36))]
    pub cloud_id: Option<String>,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 19), regex(pattern = "^[1-9][0-9]*$"))]
    pub page_id: String,
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfluenceAttachmentGetArgs {
    #[arg(long)]
    #[schemars(length(min = 36, max = 36))]
    pub cloud_id: Option<String>,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 4, max = 22), regex(pattern = "^att[1-9][0-9]*$"))]
    pub id: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 19), regex(pattern = "^[1-9][0-9]*$"))]
    pub page_id: String,
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfluenceAttachmentListArgs {
    #[arg(long)]
    #[schemars(length(min = 36, max = 36))]
    pub cloud_id: Option<String>,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 19), regex(pattern = "^[1-9][0-9]*$"))]
    pub page_id: String,
    #[arg(long,default_value_t=25,value_parser=clap::value_parser!(u16).range(1..=250))]
    #[serde(default = "page_size")]
    #[schemars(range(min = 1, max = 250))]
    pub page_size: u16,
    #[arg(long,default_value_t=20,value_parser=clap::value_parser!(u16).range(1..=100))]
    #[serde(default = "limit")]
    #[schemars(range(min = 1, max = 100))]
    pub limit: u16,
    #[arg(long)]
    #[schemars(length(min = 36, max = 36))]
    pub cursor: Option<String>,
}
pub enum ConfluenceReadArgs {
    AttachmentGet(ConfluenceAttachmentGetArgs),
    AttachmentList(ConfluenceAttachmentListArgs),
    ContentStateGet(ConfluenceContentStateArgs),
    ContentStateAvailable(ConfluenceContentStateArgs),
    CommentList(ConfluenceCommentListArgs),
    CommentReplies(ConfluenceCommentRepliesArgs),
    CommentGet(ConfluenceCommentGetArgs),
    SpaceGet(ConfluenceGetArgs),
    SpaceList(ConfluenceSpaceListArgs),
    PageGet(ConfluenceGetArgs),
    PageList(ConfluencePageListArgs),
    PageSearch(ConfluenceSearchArgs),
    FolderGet(ConfluenceGetArgs),
    WhiteboardGet(ConfluenceGetArgs),
}
pub fn action_command(action: &crate::args::ConfluenceAction) -> &'static str {
    use crate::args::{
        ConfluenceAction as A, ConfluenceFolderAction as F, ConfluencePageAction as P,
        ConfluenceSpaceAction as S, ConfluenceWhiteboardAction as W,
    };
    match action {
        A::Comment {
            action: crate::args::ConfluenceCommentAction::List(_),
        } => craft_provider_confluence::comments::paging::COMMENT_LIST,
        A::Comment {
            action: crate::args::ConfluenceCommentAction::Replies(_),
        } => craft_provider_confluence::comments::paging::COMMENT_REPLIES,
        A::Comment {
            action: crate::args::ConfluenceCommentAction::Get(_),
        } => craft_provider_confluence::comments::COMMENT_GET,
        A::ContentState {
            action: crate::args::ConfluenceContentStateAction::Get(_),
        } => craft_provider_confluence::content_state::CONTENT_STATE_GET,
        A::Attachment {
            action: crate::args::ConfluenceAttachmentAction::Get(_),
        } => craft_provider_confluence::attachments::ATTACHMENT_GET,
        A::Attachment {
            action: crate::args::ConfluenceAttachmentAction::List(_),
        } => craft_provider_confluence::attachments::ATTACHMENT_LIST,
        A::Attachment {
            action: crate::args::ConfluenceAttachmentAction::Trash(_),
        } => craft_provider_confluence::pages::ATTACHMENT_TRASH,
        A::Attachment {
            action: crate::args::ConfluenceAttachmentAction::Upload(_),
        } => craft_provider_confluence::attachment_upload::UPLOAD,
        A::Attachment {
            action: crate::args::ConfluenceAttachmentAction::Replace(_),
        } => craft_provider_confluence::attachment_upload::REPLACE,
        A::Attachment {
            action: crate::args::ConfluenceAttachmentAction::Download(_),
        } => craft_provider_confluence::attachment_download::DOWNLOAD,
        A::ContentState {
            action: crate::args::ConfluenceContentStateAction::Available(_),
        } => craft_provider_confluence::content_state::CONTENT_STATE_AVAILABLE,
        A::ContentState {
            action: crate::args::ConfluenceContentStateAction::Set(_),
        } => craft_provider_confluence::pages::CONTENT_STATE_SET,
        A::ContentState {
            action: crate::args::ConfluenceContentStateAction::Clear(_),
        } => craft_provider_confluence::pages::CONTENT_STATE_CLEAR,
        A::Page {
            action: P::Create(_),
        } => craft_provider_confluence::pages::PAGE_CREATE,
        A::Page {
            action: P::Update(_),
        } => craft_provider_confluence::pages::PAGE_UPDATE,
        A::Page {
            action: P::Trash(_),
        } => craft_provider_confluence::pages::PAGE_TRASH,
        A::Comment {
            action: crate::args::ConfluenceCommentAction::Create(_),
        } => craft_provider_confluence::pages::COMMENT_CREATE,
        A::Comment {
            action: crate::args::ConfluenceCommentAction::Update(_),
        } => craft_provider_confluence::pages::COMMENT_UPDATE,
        A::Comment {
            action: crate::args::ConfluenceCommentAction::Delete(_),
        } => craft_provider_confluence::pages::COMMENT_DELETE,
        A::Space { action: S::Get(_) } => SPACE_GET,
        A::Space {
            action: S::Create(_),
        } => craft_provider_confluence::pages::SPACE_CREATE,
        A::Space { action: S::List(_) } => SPACE_LIST,
        A::Page { action: P::Get(_) } => PAGE_GET,
        A::Page { action: P::List(_) } => PAGE_LIST,
        A::Page {
            action: P::Search(_),
        } => PAGE_SEARCH,
        A::Page {
            action: P::Width { .. },
        } => craft_provider_confluence::pages::PAGE_WIDTH_SET,
        A::Folder { action: F::Get(_) } => craft_provider_confluence::content::FOLDER_GET,
        A::Folder {
            action: F::Create(_),
        } => craft_provider_confluence::pages::FOLDER_CREATE,
        A::Folder {
            action: F::Trash(_),
        } => craft_provider_confluence::pages::FOLDER_TRASH,
        A::Whiteboard { action: W::Get(_) } => craft_provider_confluence::content::WHITEBOARD_GET,
        A::Whiteboard {
            action: W::Create(_),
        } => craft_provider_confluence::pages::WHITEBOARD_CREATE,
        A::Whiteboard {
            action: W::Trash(_),
        } => craft_provider_confluence::pages::WHITEBOARD_TRASH,
    }
}
pub fn action_arguments(action: &crate::args::ConfluenceAction) -> Result<Value, CraftError> {
    use crate::args::{
        ConfluenceAction as A, ConfluenceFolderAction as F, ConfluencePageAction as P,
        ConfluenceSpaceAction as S, ConfluenceWhiteboardAction as W,
    };
    match action {
        A::Comment {
            action: crate::args::ConfluenceCommentAction::List(v),
        } => encoded(v),
        A::Comment {
            action: crate::args::ConfluenceCommentAction::Replies(v),
        } => encoded(v),
        A::Comment {
            action: crate::args::ConfluenceCommentAction::Get(v),
        } => encoded(v),
        A::ContentState {
            action:
                crate::args::ConfluenceContentStateAction::Get(v)
                | crate::args::ConfluenceContentStateAction::Available(v),
        } => encoded(v),
        A::ContentState {
            action: crate::args::ConfluenceContentStateAction::Set(v),
        } => encoded(v),
        A::Attachment {
            action: crate::args::ConfluenceAttachmentAction::Get(v),
        } => encoded(v),
        A::Attachment {
            action: crate::args::ConfluenceAttachmentAction::List(v),
        } => encoded(v),
        A::Attachment {
            action: crate::args::ConfluenceAttachmentAction::Trash(v),
        } => encoded(v),
        A::Attachment {
            action:
                crate::args::ConfluenceAttachmentAction::Upload(v)
                | crate::args::ConfluenceAttachmentAction::Replace(v),
        } => encoded(v),
        A::Attachment {
            action: crate::args::ConfluenceAttachmentAction::Download(v),
        } => encoded(v),
        A::ContentState {
            action: crate::args::ConfluenceContentStateAction::Clear(v),
        } => encoded(v),
        A::Page {
            action: P::Create(v),
        } => encoded(v),
        A::Page {
            action: P::Update(v),
        } => encoded(v),
        A::Page {
            action: P::Trash(v),
        } => encoded(v),
        A::Comment {
            action: crate::args::ConfluenceCommentAction::Create(v),
        } => encoded(v),
        A::Comment {
            action: crate::args::ConfluenceCommentAction::Update(v),
        } => encoded(v),
        A::Comment {
            action: crate::args::ConfluenceCommentAction::Delete(v),
        } => encoded(v),
        A::Space { action: S::Get(v) } | A::Page { action: P::Get(v) } => encoded(v),
        A::Space { action: S::List(v) } => encoded(v),
        A::Space {
            action: S::Create(v),
        } => encoded(v),
        A::Page { action: P::List(v) } => encoded(v),
        A::Page {
            action: P::Search(v),
        } => encoded(v),
        A::Page {
            action:
                P::Width {
                    action: crate::args::ConfluencePageWidthAction::Set(v),
                },
        } => encoded(v),
        A::Folder { action: F::Get(v) } | A::Whiteboard { action: W::Get(v) } => encoded(v),
        A::Folder {
            action: F::Create(v),
        }
        | A::Whiteboard {
            action: W::Create(v),
        } => encoded(v),
        A::Folder {
            action: F::Trash(v),
        }
        | A::Whiteboard {
            action: W::Trash(v),
        } => encoded(v),
    }
}
impl From<ConfluenceReadArgs> for crate::args::ConfluenceAction {
    fn from(value: ConfluenceReadArgs) -> Self {
        use crate::args::{ConfluencePageAction as P, ConfluenceSpaceAction as S};
        match value {
            ConfluenceReadArgs::CommentList(v) => Self::Comment {
                action: crate::args::ConfluenceCommentAction::List(Box::new(v)),
            },
            ConfluenceReadArgs::CommentReplies(v) => Self::Comment {
                action: crate::args::ConfluenceCommentAction::Replies(Box::new(v)),
            },
            ConfluenceReadArgs::CommentGet(v) => Self::Comment {
                action: crate::args::ConfluenceCommentAction::Get(Box::new(v)),
            },
            ConfluenceReadArgs::AttachmentGet(v) => Self::Attachment {
                action: crate::args::ConfluenceAttachmentAction::Get(Box::new(v)),
            },
            ConfluenceReadArgs::AttachmentList(v) => Self::Attachment {
                action: crate::args::ConfluenceAttachmentAction::List(Box::new(v)),
            },
            ConfluenceReadArgs::ContentStateGet(v) => Self::ContentState {
                action: crate::args::ConfluenceContentStateAction::Get(Box::new(v)),
            },
            ConfluenceReadArgs::ContentStateAvailable(v) => Self::ContentState {
                action: crate::args::ConfluenceContentStateAction::Available(Box::new(v)),
            },
            ConfluenceReadArgs::SpaceGet(v) => Self::Space {
                action: S::Get(Box::new(v)),
            },
            ConfluenceReadArgs::SpaceList(v) => Self::Space {
                action: S::List(Box::new(v)),
            },
            ConfluenceReadArgs::PageGet(v) => Self::Page {
                action: P::Get(Box::new(v)),
            },
            ConfluenceReadArgs::PageList(v) => Self::Page {
                action: P::List(Box::new(v)),
            },
            ConfluenceReadArgs::PageSearch(v) => Self::Page {
                action: P::Search(Box::new(v)),
            },
            ConfluenceReadArgs::FolderGet(v) => Self::Folder {
                action: crate::args::ConfluenceFolderAction::Get(Box::new(v)),
            },
            ConfluenceReadArgs::WhiteboardGet(v) => Self::Whiteboard {
                action: crate::args::ConfluenceWhiteboardAction::Get(Box::new(v)),
            },
        }
    }
}
impl TryFrom<crate::args::ConfluenceAction> for ConfluenceReadArgs {
    type Error = CraftError;
    fn try_from(value: crate::args::ConfluenceAction) -> Result<Self, Self::Error> {
        use crate::args::{
            ConfluenceAction as A, ConfluencePageAction as P, ConfluenceSpaceAction as S,
        };
        Ok(match value {
            A::Comment {
                action: crate::args::ConfluenceCommentAction::List(v),
            } => Self::CommentList(*v),
            A::Comment {
                action: crate::args::ConfluenceCommentAction::Replies(v),
            } => Self::CommentReplies(*v),
            A::Comment {
                action: crate::args::ConfluenceCommentAction::Get(v),
            } => Self::CommentGet(*v),
            A::ContentState {
                action: crate::args::ConfluenceContentStateAction::Get(v),
            } => Self::ContentStateGet(*v),
            A::Attachment {
                action: crate::args::ConfluenceAttachmentAction::Get(v),
            } => Self::AttachmentGet(*v),
            A::Attachment {
                action: crate::args::ConfluenceAttachmentAction::List(v),
            } => Self::AttachmentList(*v),
            A::ContentState {
                action: crate::args::ConfluenceContentStateAction::Available(v),
            } => Self::ContentStateAvailable(*v),
            A::Space { action: S::Get(v) } => Self::SpaceGet(*v),
            A::Space { action: S::List(v) } => Self::SpaceList(*v),
            A::Page { action: P::Get(v) } => Self::PageGet(*v),
            A::Page { action: P::List(v) } => Self::PageList(*v),
            A::Page {
                action: P::Search(v),
            } => Self::PageSearch(*v),
            A::Folder {
                action: crate::args::ConfluenceFolderAction::Get(v),
            } => Self::FolderGet(*v),
            A::Whiteboard {
                action: crate::args::ConfluenceWhiteboardAction::Get(v),
            } => Self::WhiteboardGet(*v),
            _ => return Err(CraftError::field("/command", "expected_confluence_read")),
        })
    }
}
fn site(cloud: &str) -> ResourceRef {
    ResourceRef {
        provider: "confluence".into(),
        deployment: "cloud".into(),
        tenant: cloud.into(),
        resource_type: "site".into(),
        id: cloud.into(),
    }
}
fn encoded(value: &impl Serialize) -> Result<Value, CraftError> {
    serde_json::to_value(value)
        .map_err(|_| CraftError::field("/input", "invalid_confluence_arguments"))
}
fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, CraftError> {
    serde_json::from_value(value)
        .map_err(|_| CraftError::field("/input", "invalid_typed_confluence_arguments"))
}
impl ConfluenceReadArgs {
    pub fn operation(&self) -> Operation {
        match self {
            Self::AttachmentGet(_) => Operation::AttachmentGet,
            Self::AttachmentList(_) => Operation::AttachmentList,
            Self::ContentStateGet(_) => Operation::ContentStateGet,
            Self::ContentStateAvailable(_) => Operation::ContentStateAvailable,
            Self::CommentList(_) => Operation::CommentList,
            Self::CommentReplies(_) => Operation::CommentReplies,
            Self::CommentGet(_) => Operation::CommentGet,
            Self::SpaceGet(_) => Operation::SpaceGet,
            Self::SpaceList(_) => Operation::SpaceList,
            Self::PageGet(_) => Operation::PageGet,
            Self::PageList(_) => Operation::PageList,
            Self::PageSearch(_) => Operation::PageSearch,
            Self::FolderGet(_) => Operation::FolderGet,
            Self::WhiteboardGet(_) => Operation::WhiteboardGet,
        }
    }
    pub fn decode(command: &str, value: Value) -> Result<Self, CraftError> {
        Ok(match command {
            craft_provider_confluence::comments::paging::COMMENT_LIST => {
                Self::CommentList(decode(value)?)
            }
            craft_provider_confluence::comments::paging::COMMENT_REPLIES => {
                Self::CommentReplies(decode(value)?)
            }
            craft_provider_confluence::comments::COMMENT_GET => Self::CommentGet(decode(value)?),
            craft_provider_confluence::attachments::ATTACHMENT_GET => {
                Self::AttachmentGet(decode(value)?)
            }
            craft_provider_confluence::attachments::ATTACHMENT_LIST => {
                Self::AttachmentList(decode(value)?)
            }
            craft_provider_confluence::content_state::CONTENT_STATE_GET => {
                Self::ContentStateGet(decode(value)?)
            }
            craft_provider_confluence::content_state::CONTENT_STATE_AVAILABLE => {
                Self::ContentStateAvailable(decode(value)?)
            }
            SPACE_GET => Self::SpaceGet(decode(value)?),
            SPACE_LIST => Self::SpaceList(decode(value)?),
            PAGE_GET => Self::PageGet(decode(value)?),
            PAGE_LIST => Self::PageList(decode(value)?),
            PAGE_SEARCH => Self::PageSearch(decode(value)?),
            craft_provider_confluence::content::FOLDER_GET => Self::FolderGet(decode(value)?),
            craft_provider_confluence::content::WHITEBOARD_GET => {
                Self::WhiteboardGet(decode(value)?)
            }
            _ => return Err(CraftError::field("/command", "unsupported_confluence_read")),
        })
    }
    pub fn arguments(&self) -> Result<Value, CraftError> {
        match self {
            Self::CommentList(v) => encoded(v),
            Self::CommentReplies(v) => encoded(v),
            Self::CommentGet(v) => encoded(v),
            Self::ContentStateGet(v) | Self::ContentStateAvailable(v) => encoded(v),
            Self::AttachmentGet(v) => encoded(v),
            Self::AttachmentList(v) => encoded(v),
            Self::SpaceGet(v) | Self::PageGet(v) | Self::FolderGet(v) | Self::WhiteboardGet(v) => {
                encoded(v)
            }
            Self::SpaceList(v) => encoded(v),
            Self::PageList(v) => encoded(v),
            Self::PageSearch(v) => encoded(v),
        }
    }
    fn cloud(&self) -> Option<&str> {
        match self {
            Self::CommentList(v) => v.cloud_id.as_deref(),
            Self::CommentReplies(v) => v.cloud_id.as_deref(),
            Self::CommentGet(v) => v.cloud_id.as_deref(),
            Self::ContentStateGet(v) | Self::ContentStateAvailable(v) => v.cloud_id.as_deref(),
            Self::AttachmentGet(v) => v.cloud_id.as_deref(),
            Self::AttachmentList(v) => v.cloud_id.as_deref(),
            Self::SpaceGet(v) | Self::PageGet(v) | Self::FolderGet(v) | Self::WhiteboardGet(v) => {
                v.cloud_id.as_deref()
            }
            Self::SpaceList(v) => v.cloud_id.as_deref(),
            Self::PageList(v) => v.cloud_id.as_deref(),
            Self::PageSearch(v) => v.cloud_id.as_deref(),
        }
    }
    fn input(&self) -> Result<Value, CraftError> {
        match self {
            Self::CommentList(v) => encoded(
                &craft_provider_confluence::comments::paging::CommentListInput {
                    page_id: v.page_id.clone(),
                    page_size: v.page_size,
                },
            ),
            Self::CommentReplies(v) => encoded(
                &craft_provider_confluence::comments::paging::CommentRepliesInput {
                    page_id: v.page_id.clone(),
                    parent_comment_id: v.parent_comment_id.clone(),
                    page_size: v.page_size,
                },
            ),
            Self::CommentGet(v) => encoded(&craft_provider_confluence::comments::CommentGetInput {
                id: v.id.clone(),
                page_id: v.page_id.clone(),
            }),
            Self::ContentStateGet(v) | Self::ContentStateAvailable(v) => encoded(
                &craft_provider_confluence::content_state::ContentStateInput {
                    page_id: v.page_id.clone(),
                },
            ),
            Self::AttachmentGet(v) => encoded(
                &craft_provider_confluence::attachments::AttachmentGetInput {
                    id: v.id.clone(),
                    page_id: v.page_id.clone(),
                },
            ),
            Self::AttachmentList(v) => encoded(
                &craft_provider_confluence::attachments::AttachmentListInput {
                    page_id: v.page_id.clone(),
                    page_size: v.page_size,
                },
            ),
            Self::SpaceGet(v) | Self::PageGet(v) => encoded(&GetInput { id: v.id.clone() }),
            Self::FolderGet(v) | Self::WhiteboardGet(v) => {
                encoded(&craft_provider_confluence::content::ContentGetInput { id: v.id.clone() })
            }
            Self::SpaceList(v) => encoded(&SpaceListInput {
                key: v.key.clone(),
                page_size: v.page_size,
            }),
            Self::PageList(v) => encoded(&PageListInput {
                space_id: v.space_id.clone(),
                title: v.title.clone(),
                status: v.status,
                page_size: v.page_size,
            }),
            Self::PageSearch(v) => encoded(&SearchInput {
                cql: v.cql.clone(),
                page_size: v.page_size,
            }),
        }
    }
    fn selection(&self, fields: &[String], bytes: usize) -> Result<Selection, CraftError> {
        let (limit, cursor) = match self {
            Self::SpaceGet(_)
            | Self::PageGet(_)
            | Self::CommentGet(_)
            | Self::ContentStateGet(_)
            | Self::ContentStateAvailable(_)
            | Self::AttachmentGet(_)
            | Self::FolderGet(_)
            | Self::WhiteboardGet(_) => (20, None),
            Self::AttachmentList(v) => (v.limit, v.cursor.clone()),
            Self::CommentList(v) => (v.limit, v.cursor.clone()),
            Self::CommentReplies(v) => (v.limit, v.cursor.clone()),
            Self::SpaceList(v) => (v.limit, v.cursor.clone()),
            Self::PageList(v) => (v.limit, v.cursor.clone()),
            Self::PageSearch(v) => (v.limit, v.cursor.clone()),
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
    pub fn validate(
        &self,
        fields: &[String],
        options: &InvocationOptions<'_>,
    ) -> Result<(), CraftError> {
        if let Some(profile) = options.profile {
            validate_profile_name(profile)?;
        }
        craft_core::input::parse_object(
            &serde_json::to_vec(&self.arguments()?)
                .map_err(|_| CraftError::field("/input", "invalid_confluence_arguments"))?,
        )?;
        let target = site(
            self.cloud()
                .unwrap_or("00000000-1111-2222-3333-444444444444"),
        );
        PreparedConfluenceRead::prepare(
            CanonicalRequest {
                command: self.operation().command().into(),
                target: target.clone(),
                input: self.input()?,
                selection: self.selection(fields, options.max_bytes)?,
                preconditions: None,
            },
            options.request_id,
            ConfluenceReadPolicy {
                target,
                grant: GrantRef {
                    id: "gr_00000000000000000000000000000000".into(),
                },
                expected_actor: "actor-one".into(),
                enabled: true,
                allowed_commands: vec![self.operation().command().into()],
            },
        )?;
        Ok(())
    }
}
/// Registers candidate schemas only in a caller-supplied internal registry.
pub fn register_candidates(registry: &mut Registry) -> Result<(), CraftError> {
    let mut native = Registry::default();
    native.register_provider(&ConfluenceContext)?;
    for command in [
        SPACE_GET,
        SPACE_LIST,
        PAGE_GET,
        PAGE_LIST,
        PAGE_SEARCH,
        craft_provider_confluence::comments::COMMENT_GET,
        craft_provider_confluence::comments::paging::COMMENT_LIST,
        craft_provider_confluence::comments::paging::COMMENT_REPLIES,
        craft_provider_confluence::content_state::CONTENT_STATE_GET,
        craft_provider_confluence::content_state::CONTENT_STATE_AVAILABLE,
        craft_provider_confluence::attachments::ATTACHMENT_GET,
        craft_provider_confluence::attachments::ATTACHMENT_LIST,
        craft_provider_confluence::content::FOLDER_GET,
        craft_provider_confluence::content::WHITEBOARD_GET,
    ] {
        let mut descriptor = native.descriptor(command)?.clone();
        match command {
            craft_provider_confluence::attachments::ATTACHMENT_GET => {
                descriptor.input_schema = schema_for::<ConfluenceAttachmentGetArgs>()?;
                registry.register_typed::<ConfluenceAttachmentGetArgs>(descriptor)?;
            }
            craft_provider_confluence::attachments::ATTACHMENT_LIST => {
                descriptor.input_schema = schema_for::<ConfluenceAttachmentListArgs>()?;
                registry.register_typed::<ConfluenceAttachmentListArgs>(descriptor)?;
            }
            craft_provider_confluence::content_state::CONTENT_STATE_GET
            | craft_provider_confluence::content_state::CONTENT_STATE_AVAILABLE => {
                descriptor.input_schema = schema_for::<ConfluenceContentStateArgs>()?;
                registry.register_typed::<ConfluenceContentStateArgs>(descriptor)?;
            }
            craft_provider_confluence::comments::paging::COMMENT_LIST => {
                descriptor.input_schema = schema_for::<ConfluenceCommentListArgs>()?;
                registry.register_typed::<ConfluenceCommentListArgs>(descriptor)?;
            }
            craft_provider_confluence::comments::paging::COMMENT_REPLIES => {
                descriptor.input_schema = schema_for::<ConfluenceCommentRepliesArgs>()?;
                registry.register_typed::<ConfluenceCommentRepliesArgs>(descriptor)?;
            }
            craft_provider_confluence::comments::COMMENT_GET => {
                descriptor.input_schema = schema_for::<ConfluenceCommentGetArgs>()?;
                registry.register_typed::<ConfluenceCommentGetArgs>(descriptor)?;
            }
            SPACE_GET
            | PAGE_GET
            | craft_provider_confluence::content::FOLDER_GET
            | craft_provider_confluence::content::WHITEBOARD_GET => {
                descriptor.input_schema = schema_for::<ConfluenceGetArgs>()?;
                registry.register_typed::<ConfluenceGetArgs>(descriptor)?;
            }
            SPACE_LIST => {
                descriptor.input_schema = schema_for::<ConfluenceSpaceListArgs>()?;
                registry.register_typed::<ConfluenceSpaceListArgs>(descriptor)?;
            }
            PAGE_LIST => {
                descriptor.input_schema = schema_for::<ConfluencePageListArgs>()?;
                registry.register_typed::<ConfluencePageListArgs>(descriptor)?;
            }
            _ => {
                descriptor.input_schema = schema_for::<ConfluenceSearchArgs>()?;
                registry.register_typed::<ConfluenceSearchArgs>(descriptor)?;
            }
        }
    }
    Ok(())
}
pub struct PreparedInvocation {
    host: PreparedConfluenceRead,
    policy: ConfluenceReadPolicyRecord,
    operation: Operation,
}
impl PreparedInvocation {
    pub fn prepare(
        args: ConfluenceReadArgs,
        fields: &[String],
        options: &InvocationOptions<'_>,
        store: &ProfileStore,
        control: &InvocationControl,
    ) -> Result<Self, CraftError> {
        args.validate(fields, options)?;
        control.check()?;
        let (name, _, hints) = selected_profile_context(store, None, options, control)?;
        let profile = store.get(&name, control)?;
        if profile.provider != "confluence"
            || profile.deployment != "cloud"
            || profile.auth_mode != AuthMode::Service
        {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Confluence reads require a configured Confluence Cloud service profile.",
            ));
        }
        let explicit = args.cloud().map(site);
        let target = if explicit.is_some()
            || hints.targets.contains_key("site")
            || profile.defaults.contains_key("site")
        {
            profile.resolve_target("site", explicit.as_ref(), &hints)?
        } else {
            site(&profile.tenant_ref()?.tenant)
        };
        profile.validate_target("site", &target)?;
        let policy = store
            .confluence_read_policy(&name, control)?
            .ok_or_else(|| {
                CraftError::new(
                    ErrorCode::PolicyDenied,
                    "No trusted Confluence read configuration is present.",
                )
            })?;
        if policy.target != target {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Selected site differs from trusted Confluence read configuration.",
            ));
        }
        let operation = args.operation();
        store.verify_confluence_read_policy(&policy, operation, control)?;
        let host = PreparedConfluenceRead::prepare(
            CanonicalRequest {
                command: operation.command().into(),
                target: target.clone(),
                input: args.input()?,
                selection: args.selection(fields, options.max_bytes)?,
                preconditions: None,
            },
            options.request_id,
            ConfluenceReadPolicy {
                target,
                grant: policy.grant.clone(),
                expected_actor: policy.expected_actor.clone(),
                enabled: policy.enabled,
                allowed_commands: policy
                    .operations
                    .iter()
                    .map(|op| op.command().into())
                    .collect(),
            },
        )?;
        Ok(Self {
            host,
            policy,
            operation,
        })
    }
    pub fn grant(&self) -> &GrantRef {
        &self.policy.grant
    }
    pub fn execute_cached(
        &self,
        store: &ProfileStore,
        client: &ServiceOAuthClient,
        locked: &LockedCredentials<'_>,
        tls: TlsPolicy,
        control: InvocationControl,
    ) -> CommandResult {
        self.execute_cached_with_check(store, client, locked, tls, control, || Ok(()))
    }
    /// Adds a workflow's captured authority checks at the existing native read boundaries.
    pub fn execute_cached_with_check(
        &self,
        store: &ProfileStore,
        client: &ServiceOAuthClient,
        locked: &LockedCredentials<'_>,
        tls: TlsPolicy,
        control: InvocationControl,
        check_workflow: impl Fn() -> Result<(), CraftError>,
    ) -> CommandResult {
        let check = control.clone();
        self.host
            .execute_cached_with_policy_check(client, locked, store, tls, control, || {
                check_workflow()?;
                store.verify_confluence_read_policy(&self.policy, self.operation, &check)
            })
    }
}
impl PreparedInvocation {
    pub fn binding(&self) -> &craft_core::pagination::ListBinding {
        self.host.binding()
    }
    pub fn scan_cached(
        &self,
        store: &ProfileStore,
        client: &ServiceOAuthClient,
        locked: &LockedCredentials<'_>,
        tls: TlsPolicy,
        control: InvocationControl,
        run: crate::scans::ScanRunner<'_>,
    ) -> (
        Result<craft_runtime::scan::Advanced, CraftError>,
        Option<craft_core::contracts::CredentialCacheUpdate>,
    ) {
        let check = control.clone();
        self.host.scan_cached(
            client,
            locked,
            tls,
            control,
            || store.verify_confluence_read_policy(&self.policy, self.operation, &check),
            run,
        )
    }
}
/// Validates before private state and uses only the selected profile's native credential grant.
pub fn execute(
    args: ConfluenceReadArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> CommandResult {
    let command = args.operation().command();
    let result = (|| {
        args.validate(fields, options)?;
        let root = resolve_state_root(options.state_root, |key| std::env::var_os(key))?;
        let store = ProfileStore::open(&root, control)?;
        let prepared = PreparedInvocation::prepare(args, fields, options, &store, control)?;
        let credentials = NativeCredentials::open(&store)?;
        let locked = credentials.lock(prepared.grant(), control)?;
        let client = ServiceOAuthClient::new(
            TlsPolicy::Platform,
            control.clone(),
            Arc::new(SystemClock::default()),
        )?;
        Ok(prepared.execute_cached(
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
