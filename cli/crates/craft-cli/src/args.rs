use clap::{Args, Parser, Subcommand, ValueEnum};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub fn is_invocation_control(id: &str) -> bool {
    matches!(
        id,
        "input" | "fields" | "max_bytes" | "timeout_ms" | "state_root" | "profile"
    )
}

/// Inspect actual arguments; derived groups can include explicit invocation controls.
pub fn has_action_flags(command: &clap::Command, matches: &clap::ArgMatches) -> bool {
    command.get_arguments().any(|arg| {
        !is_invocation_control(arg.get_id().as_str())
            && matches.value_source(arg.get_id().as_str())
                == Some(clap::parser::ValueSource::CommandLine)
    }) || matches.subcommand().is_some_and(|(name, child)| {
        command
            .find_subcommand(name)
            .is_none_or(|subcommand| has_action_flags(subcommand, child))
    })
}

pub fn uses_state_context(command: &str) -> bool {
    command.starts_with("profile.")
        || command.starts_with("trello.")
        || matches!(
            command,
            "artifact.get"
                | "artifact.export"
                | "review.policy.get"
                | "review.policy.set"
                | "review.prepare"
                | "review.verify"
                | "plan.tickets-preview"
                | "plan.tickets-create"
                | "plan.tickets-resume"
                | "plan.tickets-record-completion"
                | "policy.get"
                | "policy.set"
                | "auth.status"
                | "auth.doctor"
                | "auth.provision"
                | "auth.provision-static"
                | "operation.get"
                | "operation.list"
                | "operation.preview"
                | "operation.reconcile"
                | "operation.wait"
                | "scan.start"
                | "scan.resume"
                | "scan.export"
                | "jira.issue.create"
                | "jira.link.create"
                | "jira.link.delete"
                | "jira.link.get"
                | "jira.comment.get"
                | "jira.comment.list"
                | "jira.attachment.get"
                | "jira.attachment.list"
                | "jira.attachment.settings"
                | "jira.attachment.download"
                | "jira.attachment.upload"
                | "jira.attachment.delete"
                | "jira.comment.create"
                | "jira.comment.update"
                | "jira.comment.delete"
                | "jira.link.list"
                | "jira.link.types"
                | "jira.link.reconcile"
                | "confluence.space.get"
                | "confluence.space.list"
                | "confluence.comment.get"
                | "confluence.comment.list"
                | "confluence.comment.replies"
                | "confluence.page.get"
                | "confluence.page.list"
                | "confluence.page.search"
                | "plan.page-create"
                | "design.page-create"
                | "plan.page-get"
                | "design.page-get"
                | "confluence.page.create"
                | "confluence.page.update"
                | "confluence.page.trash"
                | "confluence.comment.create"
                | "confluence.comment.update"
                | "confluence.comment.delete"
                | "confluence.content-state.get"
                | "confluence.content-state.available"
                | "confluence.content-state.set"
                | "confluence.content-state.clear"
                | "confluence.attachment.get"
                | "confluence.attachment.list"
                | "confluence.attachment.trash"
                | "confluence.space.create"
                | "confluence.attachment.upload"
                | "confluence.attachment.replace"
                | "confluence.attachment.download"
                | "confluence.folder.get"
                | "confluence.folder.create"
                | "confluence.folder.trash"
                | "confluence.whiteboard.get"
                | "confluence.whiteboard.create"
                | "confluence.whiteboard.trash"
                | "confluence.page.width.set"
                | "jira.issue.get"
                | "jira.issue.search"
                | "jira.project.list"
                | "jira.project.get"
                | "jira.project.statuses"
                | "jira.field.list"
                | "jira.field.search"
                | "jira.metadata.edit"
                | "jira.transition.list"
                | "jira.permission.check"
                | "jira.issue.update"
                | "jira.issue.assign"
                | "jira.issue.unassign"
                | "jira.issue.delete"
                | "jira.transition.apply"
        )
}

pub fn accepts_profile_selection(command: &str) -> bool {
    command.starts_with("trello.")
        || matches!(
            command,
            "plan.page-create"
                | "design.page-create"
                | "plan.page-get"
                | "design.page-get"
                | "profile.show"
                | "profile.use"
                | "review.policy.get"
                | "review.policy.set"
                | "review.prepare"
                | "review.verify"
                | "plan.tickets-preview"
                | "plan.tickets-create"
                | "plan.tickets-resume"
                | "plan.tickets-record-completion"
                | "policy.get"
                | "policy.set"
                | "auth.status"
                | "auth.doctor"
                | "jira.issue.create"
                | "jira.link.create"
                | "jira.link.delete"
                | "jira.link.get"
                | "jira.comment.get"
                | "jira.comment.list"
                | "jira.attachment.get"
                | "jira.attachment.list"
                | "jira.attachment.settings"
                | "jira.attachment.download"
                | "jira.attachment.upload"
                | "jira.attachment.delete"
                | "jira.comment.create"
                | "jira.comment.update"
                | "jira.comment.delete"
                | "jira.link.list"
                | "jira.link.types"
                | "jira.link.reconcile"
                | "confluence.space.get"
                | "confluence.space.list"
                | "confluence.comment.get"
                | "confluence.comment.list"
                | "confluence.comment.replies"
                | "confluence.page.get"
                | "confluence.page.list"
                | "confluence.page.search"
                | "confluence.page.create"
                | "confluence.page.update"
                | "confluence.page.trash"
                | "confluence.comment.create"
                | "confluence.comment.update"
                | "confluence.comment.delete"
                | "confluence.content-state.get"
                | "confluence.content-state.available"
                | "confluence.content-state.set"
                | "confluence.content-state.clear"
                | "confluence.attachment.get"
                | "confluence.attachment.list"
                | "confluence.attachment.trash"
                | "confluence.space.create"
                | "confluence.attachment.upload"
                | "confluence.attachment.replace"
                | "confluence.attachment.download"
                | "confluence.folder.get"
                | "confluence.folder.create"
                | "confluence.folder.trash"
                | "confluence.whiteboard.get"
                | "confluence.whiteboard.create"
                | "confluence.whiteboard.trash"
                | "confluence.page.width.set"
                | "jira.issue.get"
                | "jira.issue.search"
                | "jira.project.list"
                | "jira.project.get"
                | "jira.project.statuses"
                | "jira.field.list"
                | "jira.field.search"
                | "jira.metadata.edit"
                | "jira.transition.list"
                | "jira.permission.check"
                | "jira.issue.update"
                | "jira.issue.assign"
                | "jira.issue.unassign"
                | "jira.issue.delete"
                | "jira.transition.apply"
                | "operation.reconcile"
                | "operation.preview"
                | "operation.wait"
                | "scan.start"
                | "scan.resume"
        )
}

#[derive(Parser)]
#[command(name = "craft", version = craft_core::VERSION,
    long_version = concat!("Craft AI Factory CLI ", env!("CARGO_PKG_VERSION")),
    about = "Deterministic cloud tools for the AI software factory",
    disable_help_subcommand = true)]
pub struct Cli {
    #[arg(
        long,
        global = true,
        value_name = "DIRECTORY",
        help = "Trusted private state root for local profiles, diagnostics and operation evidence"
    )]
    pub state_root: Option<std::path::PathBuf>,
    #[arg(
        long,
        global = true,
        help = "Select a trusted existing profile for context-aware commands"
    )]
    pub profile: Option<String>,
    #[arg(long, global = true, default_value_t = 16_384, value_parser = clap::value_parser!(u32).range(512..=16_777_216))]
    pub max_bytes: u32,
    #[arg(long, global = true, value_delimiter = ',', num_args = 1)]
    pub fields: Vec<String>,
    #[arg(
        long,
        global = true,
        value_name = "FILE",
        help = "Read the command argument object from a file, or - for stdin; exclusive with action flags"
    )]
    pub input: Option<String>,
    #[arg(long, global = true, default_value_t = craft_core::control::DEFAULT_TIMEOUT_MS, value_parser = clap::value_parser!(u64).range(1..=craft_core::control::MAX_TIMEOUT_MS))]
    pub timeout_ms: u64,
    #[command(subcommand)]
    pub action: Option<Action>,
}

#[derive(Subcommand)]
pub enum Action {
    Artifact {
        #[command(subcommand)]
        action: crate::local_artifacts::ArtifactAction,
    },
    Review {
        #[command(subcommand)]
        action: ReviewAction,
    },
    Policy {
        #[command(subcommand)]
        action: PolicyAction,
    },
    Confluence {
        #[command(subcommand)]
        action: ConfluenceAction,
    },
    Trello {
        #[command(subcommand)]
        action: crate::trello_reads::TrelloAction,
    },
    Jira {
        #[command(subcommand)]
        action: JiraAction,
    },
    Operation {
        #[command(subcommand)]
        action: OperationAction,
    },
    Scan {
        #[command(subcommand)]
        action: crate::scans::command::ScanAction,
    },
    Plan {
        #[command(subcommand)]
        action: PlanAction,
    },
    Design {
        #[command(subcommand)]
        action: DesignAction,
    },
    Whiteboard {
        #[command(subcommand)]
        action: crate::whiteboard_scene::WhiteboardAction,
    },
    Auth {
        #[command(subcommand)]
        action: AuthAction,
    },
    Profile {
        #[command(subcommand)]
        action: ProfileAction,
    },
    Version,
    Command {
        #[command(subcommand)]
        action: CommandAction,
    },
    Schema(SchemaArgs),
    Input {
        #[command(subcommand)]
        action: InputAction,
    },
    Provider {
        #[command(subcommand)]
        action: ProviderAction,
    },
    Capabilities(CapabilitiesArgs),
    Help(HelpArgs),
}

#[derive(Subcommand)]
pub enum ReviewAction {
    Verify(Box<crate::planning_review::verification::VerifyArgs>),
    Prepare(Box<crate::planning_review::command::PrepareArgs>),
    Policy {
        #[command(subcommand)]
        action: ReviewPolicyAction,
    },
}
#[derive(Subcommand)]
pub enum ReviewPolicyAction {
    Get(Box<crate::review_policy::GetArgs>),
    Set(Box<crate::review_policy::SetArgs>),
}

#[derive(Subcommand)]
pub enum PolicyAction {
    Get(Box<crate::policies::PolicyGetArgs>),
    Set(Box<crate::policies::PolicySetArgs>),
}

#[derive(Subcommand)]
pub enum JiraAction {
    Attachment {
        #[command(subcommand)]
        action: JiraAttachmentAction,
    },
    Comment {
        #[command(subcommand)]
        action: JiraCommentAction,
    },
    Link {
        #[command(subcommand)]
        action: JiraLinkAction,
    },
    Permission {
        #[command(subcommand)]
        action: JiraPermissionAction,
    },
    Metadata {
        #[command(subcommand)]
        action: JiraMetadataAction,
    },
    Transition {
        #[command(subcommand)]
        action: JiraTransitionAction,
    },
    Field {
        #[command(subcommand)]
        action: JiraFieldAction,
    },
    Issue {
        #[command(subcommand)]
        action: JiraIssueAction,
    },
    Project {
        #[command(subcommand)]
        action: JiraProjectAction,
    },
}

#[derive(Subcommand)]
pub enum JiraCommentAction {
    Create(Box<crate::comment_writes::command::CommentCreateArgs>),
    Update(Box<crate::comment_writes::command::CommentUpdateArgs>),
    Delete(Box<crate::comment_writes::command::CommentDeleteArgs>),
    Get(crate::issue_comments::command::CommentGetArgs),
    List(crate::issue_comments::command::CommentListArgs),
}
#[derive(Subcommand)]
pub enum JiraAttachmentAction {
    Upload(Box<crate::attachment_upload::command::UploadArgs>),
    Delete(Box<crate::attachment_delete::command::AttachmentDeleteArgs>),
    Download(crate::attachment_download::command::AttachmentDownloadArgs),
    Get(crate::attachment_reads::command::AttachmentGetArgs),
    List(crate::attachment_reads::command::AttachmentListArgs),
    Settings(crate::attachment_reads::command::AttachmentSettingsArgs),
}

#[derive(Subcommand)]
pub enum JiraLinkAction {
    Create(Box<crate::issue_links::command::LinkArgs>),
    Delete(Box<crate::issue_links::command::LinkDeleteArgs>),
    Get(crate::issue_links::reads::command::LinkGetArgs),
    List(crate::issue_links::reads::command::LinkListArgs),
    Types(crate::issue_links::reads::command::LinkTypesArgs),
    Reconcile(Box<crate::issue_links::recovery::ReconcileArgs>),
}

#[derive(Subcommand)]
pub enum JiraPermissionAction {
    Check(crate::permission_reads::command::PermissionCheckArgs),
}

#[derive(Subcommand)]
pub enum ConfluenceAction {
    Comment {
        #[command(subcommand)]
        action: ConfluenceCommentAction,
    },
    Space {
        #[command(subcommand)]
        action: ConfluenceSpaceAction,
    },
    Page {
        #[command(subcommand)]
        action: ConfluencePageAction,
    },
    #[command(name = "content-state")]
    ContentState {
        #[command(subcommand)]
        action: ConfluenceContentStateAction,
    },
    Attachment {
        #[command(subcommand)]
        action: ConfluenceAttachmentAction,
    },
    Folder {
        #[command(subcommand)]
        action: ConfluenceFolderAction,
    },
    Whiteboard {
        #[command(subcommand)]
        action: ConfluenceWhiteboardAction,
    },
}
#[derive(Subcommand)]
pub enum ConfluenceFolderAction {
    Get(Box<crate::confluence_reads::command::ConfluenceGetArgs>),
    Create(Box<crate::confluence_pages::command::ContentCreateArgs>),
    Trash(Box<crate::confluence_pages::command::ContentTrashArgs>),
}
#[derive(Subcommand)]
pub enum ConfluenceWhiteboardAction {
    Get(Box<crate::confluence_reads::command::ConfluenceGetArgs>),
    Create(Box<crate::confluence_pages::command::ContentCreateArgs>),
    Trash(Box<crate::confluence_pages::command::ContentTrashArgs>),
}
#[derive(Subcommand)]
pub enum ConfluenceCommentAction {
    List(Box<crate::confluence_reads::command::ConfluenceCommentListArgs>),
    Replies(Box<crate::confluence_reads::command::ConfluenceCommentRepliesArgs>),
    Get(Box<crate::confluence_reads::command::ConfluenceCommentGetArgs>),
    Create(Box<crate::confluence_pages::command::CommentCreateArgs>),
    Update(Box<crate::confluence_pages::command::CommentUpdateArgs>),
    Delete(Box<crate::confluence_pages::command::CommentDeleteArgs>),
}

#[derive(Subcommand)]
pub enum ConfluenceSpaceAction {
    Get(Box<crate::confluence_reads::command::ConfluenceGetArgs>),
    List(Box<crate::confluence_reads::command::ConfluenceSpaceListArgs>),
    Create(Box<crate::confluence_pages::command::SpaceCreateArgs>),
}
#[derive(Subcommand)]
pub enum ConfluenceAttachmentAction {
    Get(Box<crate::confluence_reads::command::ConfluenceAttachmentGetArgs>),
    List(Box<crate::confluence_reads::command::ConfluenceAttachmentListArgs>),
    Trash(Box<crate::confluence_pages::command::AttachmentTrashArgs>),
    Upload(Box<crate::confluence_attachment_upload::command::UploadArgs>),
    Replace(Box<crate::confluence_attachment_upload::command::UploadArgs>),
    Download(Box<crate::confluence_attachment_download::command::AttachmentDownloadArgs>),
}
#[derive(Subcommand)]
pub enum ConfluenceContentStateAction {
    Get(Box<crate::confluence_reads::command::ConfluenceContentStateArgs>),
    Available(Box<crate::confluence_reads::command::ConfluenceContentStateArgs>),
    Set(Box<crate::confluence_pages::command::ContentStateSetArgs>),
    Clear(Box<crate::confluence_pages::command::ContentStateClearArgs>),
}
#[derive(Subcommand)]
pub enum ConfluencePageAction {
    Create(Box<crate::confluence_pages::command::PageCreateArgs>),
    Update(Box<crate::confluence_pages::command::PageUpdateArgs>),
    Trash(Box<crate::confluence_pages::command::PageTrashArgs>),
    Get(Box<crate::confluence_reads::command::ConfluenceGetArgs>),
    List(Box<crate::confluence_reads::command::ConfluencePageListArgs>),
    Search(Box<crate::confluence_reads::command::ConfluenceSearchArgs>),
    Width {
        #[command(subcommand)]
        action: ConfluencePageWidthAction,
    },
}
#[derive(Subcommand)]
pub enum ConfluencePageWidthAction {
    Set(Box<crate::confluence_pages::command::PageWidthSetArgs>),
}

#[derive(Subcommand)]
pub enum JiraMetadataAction {
    Edit(crate::issue_context::command::EditMetadataArgs),
}

#[derive(Subcommand)]
pub enum JiraTransitionAction {
    List(crate::issue_context::command::TransitionListArgs),
    Apply(Box<crate::issue_mutation::command::TransitionApplyArgs>),
}

#[derive(Subcommand)]
pub enum JiraFieldAction {
    List(crate::field_reads::command::FieldListArgs),
    Search(crate::field_reads::command::FieldSearchArgs),
}

#[derive(Subcommand)]
pub enum JiraProjectAction {
    List(crate::project_list::command::ProjectListArgs),
    Get(crate::project_context::command::ProjectGetArgs),
    Statuses(crate::project_context::command::ProjectStatusesArgs),
}

#[derive(Subcommand)]
pub enum JiraIssueAction {
    Create(Box<crate::issue_create::command::IssueCreateArgs>),
    Update(Box<crate::issue_mutation::command::IssueUpdateArgs>),
    Assign(Box<crate::issue_mutation::command::IssueAssignArgs>),
    Unassign(Box<crate::issue_mutation::command::IssueUnassignArgs>),
    Delete(Box<crate::issue_mutation::command::IssueDeleteArgs>),
    Get(crate::issue_reads::command::IssueGetArgs),
    Search(crate::issue_reads::command::IssueSearchArgs),
}

#[derive(Subcommand)]
pub enum PlanAction {
    TicketsPreview(Box<crate::planning_review::verification::VerifyArgs>),
    TicketsCreate(Box<crate::planning_review::application::ApplyArgs>),
    TicketsResume(Box<crate::planning_review::application::ApplyArgs>),
    TicketsRecordCompletion(Box<crate::planning_review::application::recovery::RecordArgs>),
    Scaffold(Box<PlanScaffoldArgs>),
    Validate(DocumentFileArgs),
    Render(DocumentRenderArgs),
    PageCreate(crate::planning_publication::DocumentPublicationArgs),
    PageGet(crate::planning_read::DocumentGetArgs),
}

#[derive(Subcommand)]
pub enum DesignAction {
    Scaffold(Box<DesignScaffoldArgs>),
    Validate(DocumentFileArgs),
    Render(DocumentRenderArgs),
    PageCreate(crate::planning_publication::DocumentPublicationArgs),
    PageGet(crate::planning_read::DocumentGetArgs),
}

#[derive(Args, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentTargetArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 36, max = 36))]
    pub confluence_cloud_id: String,
    #[arg(long, default_value = "")]
    #[schemars(regex(pattern = "^[1-9][0-9]{0,31}$"))]
    pub space_id: String,
    #[arg(long)]
    #[schemars(regex(pattern = "^[1-9][0-9]{0,31}$"))]
    pub parent_page_id: Option<String>,
}

impl From<DocumentTargetArgs> for craft_workflows::plans::DocumentTarget {
    fn from(args: DocumentTargetArgs) -> Self {
        Self {
            confluence_cloud_id: args.confluence_cloud_id,
            space_id: args.space_id,
            parent_page_id: args.parent_page_id,
        }
    }
}

#[derive(Args, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanScaffoldArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 64))]
    pub plan_id: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 240))]
    pub title: String,
    #[command(flatten)]
    #[serde(flatten)]
    pub confluence: DocumentTargetArgs,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 36, max = 36))]
    pub jira_cloud_id: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 64))]
    pub project_key: String,
}

#[derive(Args, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DesignScaffoldArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 64))]
    pub design_id: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 240))]
    pub title: String,
    #[arg(long, default_value = "hld")]
    #[serde(default = "default_design_kind")]
    pub kind: craft_workflows::plans::DesignKind,
    #[arg(long)]
    #[serde(default)]
    pub compact: bool,
    #[command(flatten)]
    #[serde(flatten)]
    pub confluence: DocumentTargetArgs,
}

fn default_design_kind() -> craft_workflows::plans::DesignKind {
    craft_workflows::plans::DesignKind::Hld
}

#[derive(ValueEnum, Clone, Copy, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum DocumentSourceFormat {
    #[default]
    Json,
    Storage,
    Adf,
}

#[derive(ValueEnum, Clone, Copy, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum DocumentOutputFormat {
    #[default]
    Storage,
    Adf,
}

#[derive(Args, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentFileArgs {
    #[arg(
        long,
        default_value = "",
        help = "Bounded local document file, or - for stdin"
    )]
    #[schemars(length(min = 1, max = 2048))]
    pub file: String,
    #[arg(long, value_enum, default_value = "json")]
    #[serde(default)]
    pub source_format: DocumentSourceFormat,
}

#[derive(Args, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentRenderArgs {
    #[command(flatten)]
    #[serde(flatten)]
    pub source: DocumentFileArgs,
    #[arg(long, value_enum, default_value = "storage")]
    #[serde(default)]
    pub format: DocumentOutputFormat,
}

#[derive(Subcommand)]
pub enum CommandAction {
    Find(FindArgs),
}
#[derive(Subcommand)]
pub enum InputAction {
    Validate(ValidateArgs),
}
#[derive(Subcommand)]
pub enum ProviderAction {
    List,
}

#[derive(Subcommand)]
pub enum ProfileAction {
    List(ProfileListArgs),
    Show(ProfileNameArgs),
    Create(ProfileCreateArgs),
    Use(ProfileNameArgs),
}

#[derive(Subcommand)]
pub enum AuthAction {
    Status(ProfileNameArgs),
    Doctor(ProfileNameArgs),
    Provision(crate::provision::ProvisionArgs),
    #[command(name = "provision-static")]
    ProvisionStatic(crate::provision_static::ProvisionStaticArgs),
}

#[derive(Subcommand)]
pub enum OperationAction {
    Get(OperationGetArgs),
    List(OperationListArgs),
    Preview(OperationPreviewArgs),
    Reconcile(Box<crate::issue_recovery::command::OperationReconcileArgs>),
    Wait(Box<crate::job_wait::command::OperationWaitArgs>),
}

#[derive(Args, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationGetArgs {
    #[arg(
        long,
        default_value = "",
        help = "Exact operation identity in the selected local installation journal"
    )]
    #[schemars(length(min = 1, max = 512))]
    pub operation_id: String,
}

#[derive(Args, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationPreviewArgs {
    #[arg(
        long,
        default_value = "",
        help = "Mutating command to describe; required in flags or --input"
    )]
    #[schemars(length(min = 1, max = 128))]
    pub command: String,
    #[arg(
        long,
        default_value = "",
        help = "That command's input JSON file, or - for stdin; required in flags or --input"
    )]
    #[schemars(length(min = 1, max = 2048))]
    pub file: String,
    #[arg(
        long,
        help = "Also run the read that observes the existing target, under the selected profile"
    )]
    #[serde(default)]
    pub check_remote: bool,
}

fn default_operation_limit() -> u8 {
    20
}

#[derive(Args, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationListArgs {
    #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u8).range(1..=100))]
    #[serde(default = "default_operation_limit")]
    #[schemars(range(min = 1, max = 100))]
    pub limit: u8,
    #[arg(
        long,
        help = "Continue strictly after this operation identity, as returned in next_after_operation_id"
    )]
    #[serde(default)]
    #[schemars(length(min = 1, max = 512))]
    pub after_operation_id: Option<String>,
    #[arg(
        long,
        help = "Return only records with a started, unknown or unresolved remote job state"
    )]
    #[serde(default)]
    pub unresolved_only: bool,
}

#[derive(Args, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileListArgs {
    #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u16).range(1..=100))]
    #[serde(default = "default_profile_limit")]
    #[schemars(range(min = 1, max = 100))]
    pub limit: u16,
    #[arg(long)]
    #[schemars(regex(pattern = "^cur_[0-9a-f]{32}$"))]
    pub cursor: Option<String>,
}

fn default_profile_limit() -> u16 {
    20
}

#[derive(Args, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileNameArgs {
    #[arg(
        long,
        help = "Profile alias; otherwise resolve --profile, repository hints, then active profile"
    )]
    #[schemars(
        length(min = 1, max = 64),
        regex(pattern = "^[A-Za-z0-9][A-Za-z0-9._-]*$")
    )]
    pub name: Option<String>,
}

#[derive(Args, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileCreateArgs {
    #[arg(long, default_value = "")]
    #[schemars(
        length(min = 1, max = 64),
        regex(pattern = "^[A-Za-z0-9][A-Za-z0-9._-]*$")
    )]
    pub name: String,
    #[arg(
        long,
        default_value = "",
        help = "Configuration template: jira or confluence; does not enable cloud access"
    )]
    #[schemars(regex(pattern = "^(jira|confluence)$"))]
    pub provider: String,
    #[arg(long, default_value = "")]
    #[schemars(regex(pattern = "^cloud$"))]
    pub deployment: String,
    #[arg(
        long,
        default_value = "",
        help = "Intended auth mode: service or delegated"
    )]
    #[schemars(regex(pattern = "^(service|delegated)$"))]
    pub auth_mode: String,
    #[arg(long)]
    #[schemars(length(min = 1, max = 512))]
    pub tenant: Option<String>,
    #[arg(
        long,
        help = "Existing issued local grant reference; omission creates new grant metadata"
    )]
    #[schemars(regex(pattern = "^gr_[0-9a-f]{32}$"))]
    pub grant: Option<String>,
    #[arg(skip)]
    #[serde(default)]
    #[schemars(extend("x-craft-input-channel" = "json_only"))]
    pub defaults: std::collections::BTreeMap<String, craft_core::contracts::ResourceRef>,
}

#[derive(Args, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FindArgs {
    #[arg(long, default_value = "")]
    #[serde(default)]
    #[schemars(length(max = 256))]
    pub query: String,
    #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u16).range(1..=100))]
    #[serde(default = "default_search_limit")]
    #[schemars(range(min = 1, max = 100))]
    pub limit: u16,
}

fn default_search_limit() -> u16 {
    5
}

#[derive(ValueEnum, Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum SchemaView {
    #[default]
    Compact,
    Full,
}

#[derive(ValueEnum, Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExampleKind {
    Minimal,
}

#[derive(Args, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SchemaArgs {
    #[arg(
        long,
        default_value = "",
        help = "Registered command to inspect; required in flags or --input"
    )]
    #[schemars(length(min = 1, max = 128))]
    pub command: String,
    #[arg(long)]
    #[serde(default)]
    pub response: bool,
    #[arg(long, value_enum, default_value = "compact")]
    #[serde(default)]
    pub view: SchemaView,
    #[arg(long)]
    pub path: Option<String>,
    #[arg(long, value_enum)]
    pub example: Option<ExampleKind>,
    #[arg(skip)]
    #[serde(default)]
    #[schemars(length(max = 32))]
    #[schemars(extend("x-craft-input-channel" = "output_control"))]
    pub fields: Vec<String>,
}

#[derive(Args, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ValidateArgs {
    #[arg(
        long,
        default_value = "",
        help = "Registered command whose input is validated; required in flags or --input"
    )]
    #[schemars(length(min = 1, max = 128))]
    pub command: String,
    #[arg(
        long,
        default_value = "",
        help = "Input JSON file, or - for stdin; required in flags or --input"
    )]
    #[schemars(length(min = 1, max = 2048))]
    pub file: String,
}

#[derive(Args, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapabilitiesArgs {
    #[arg(long)]
    pub command: Option<String>,
}

#[derive(Args, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HelpArgs {
    #[arg(long)]
    #[serde(default)]
    pub json: bool,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmptyArgs {}
