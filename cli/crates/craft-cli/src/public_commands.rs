//! Registered service workflows. Discovery describes implementation, not live credential eligibility.
use craft_core::contracts::CanonicalRequest;
use craft_core::contracts::SCHEMA_VERSION;
use craft_core::error::CraftError;
use craft_core::error::ErrorCode;
use craft_core::execution::{OperationContext, OperationHandler, PreparedOperation};
use craft_core::registry::{ProviderManifest, ProviderModule, Registry};
use serde_json::Value;
use std::sync::Arc;

enum CloudCommands {
    Confluence,
    Jira,
}
impl ProviderModule for CloudCommands {
    fn manifest(&self) -> ProviderManifest {
        ProviderManifest {
            id: match self {
                Self::Confluence => "confluence",
                Self::Jira => "jira",
            }
            .into(),
            version: env!("CARGO_PKG_VERSION").into(),
            deployments: vec!["cloud".into()],
            contract_version: SCHEMA_VERSION.into(),
        }
    }
    fn register(&self, registry: &mut Registry) -> Result<(), CraftError> {
        let mut candidates = Registry::default();
        match self {
            Self::Confluence => {
                crate::confluence_reads::command::register_candidates(&mut candidates)?;
                crate::confluence_pages::command::register_public(&mut candidates)?;
                crate::confluence_attachment_upload::command::register(&mut candidates)?;
                crate::confluence_attachment_download::command::register(&mut candidates)?;
            }
            Self::Jira => {
                crate::issue_mutation::command::register_candidates(&mut candidates)?;
                crate::project_list::command::register_candidate(&mut candidates)?;
                crate::project_context::command::register_candidates(&mut candidates)?;
                crate::field_reads::command::register_candidates(&mut candidates)?;
                crate::issue_context::command::register_candidates(&mut candidates)?;
                crate::permission_reads::command::register_candidate(&mut candidates)?;
                crate::issue_reads::command::register_candidates(&mut candidates)?;
                crate::issue_create::command::register_candidate(&mut candidates)?;
                crate::issue_links::command::register(&mut candidates)?;
                crate::issue_links::reads::command::register(&mut candidates)?;
                crate::issue_comments::command::register(&mut candidates)?;
                crate::attachment_reads::command::register(&mut candidates)?;
                crate::attachment_download::command::register(&mut candidates)?;
                crate::attachment_upload::command::register(&mut candidates)?;
                crate::attachment_delete::command::register(&mut candidates)?;
                crate::comment_writes::command::register(&mut candidates)?;
                crate::issue_links::recovery::register(&mut candidates)?;
            }
        }
        for descriptor in candidates.descriptors().cloned() {
            use crate::confluence_reads::command::{
                ConfluenceGetArgs, ConfluencePageListArgs, ConfluenceSearchArgs,
                ConfluenceSpaceListArgs,
            };
            macro_rules! register {
                ($input:ty) => {
                    registry.register_typed_handler::<$input>(
                        descriptor,
                        Arc::new(VerifiedHostRequired),
                    )?
                };
            }
            match descriptor.id.as_str() {
                "confluence.comment.list" => {
                    register!(crate::confluence_reads::command::ConfluenceCommentListArgs)
                }
                "confluence.comment.replies" => {
                    register!(crate::confluence_reads::command::ConfluenceCommentRepliesArgs)
                }
                "confluence.comment.get" => {
                    register!(crate::confluence_reads::command::ConfluenceCommentGetArgs)
                }
                "confluence.space.get"
                | "confluence.page.get"
                | "confluence.folder.get"
                | "confluence.whiteboard.get" => register!(ConfluenceGetArgs),
                "confluence.folder.create" | "confluence.whiteboard.create" => {
                    register!(crate::confluence_pages::command::ContentCreateArgs)
                }
                "confluence.folder.trash" | "confluence.whiteboard.trash" => {
                    register!(crate::confluence_pages::command::ContentTrashArgs)
                }
                "confluence.space.list" => register!(ConfluenceSpaceListArgs),
                "confluence.page.list" => register!(ConfluencePageListArgs),
                "confluence.page.search" => register!(ConfluenceSearchArgs),
                "confluence.page.create" => {
                    register!(crate::confluence_pages::command::PageCreateArgs)
                }
                "confluence.page.update" => {
                    register!(crate::confluence_pages::command::PageUpdateArgs)
                }
                "confluence.page.trash" => {
                    register!(crate::confluence_pages::command::PageTrashArgs)
                }
                "confluence.comment.create" => {
                    register!(crate::confluence_pages::command::CommentCreateArgs)
                }
                "confluence.comment.update" => {
                    register!(crate::confluence_pages::command::CommentUpdateArgs)
                }
                "confluence.comment.delete" => {
                    register!(crate::confluence_pages::command::CommentDeleteArgs)
                }
                "confluence.content-state.get" | "confluence.content-state.available" => {
                    register!(crate::confluence_reads::command::ConfluenceContentStateArgs)
                }
                "confluence.content-state.set" => {
                    register!(crate::confluence_pages::command::ContentStateSetArgs)
                }
                "confluence.content-state.clear" => {
                    register!(crate::confluence_pages::command::ContentStateClearArgs)
                }
                "confluence.attachment.get" => {
                    register!(crate::confluence_reads::command::ConfluenceAttachmentGetArgs)
                }
                "confluence.attachment.list" => {
                    register!(crate::confluence_reads::command::ConfluenceAttachmentListArgs)
                }
                "confluence.attachment.trash" => {
                    register!(crate::confluence_pages::command::AttachmentTrashArgs)
                }
                "confluence.space.create" => {
                    register!(crate::confluence_pages::command::SpaceCreateArgs)
                }
                "confluence.attachment.upload" | "confluence.attachment.replace" => {
                    register!(crate::confluence_attachment_upload::command::UploadArgs)
                }
                "confluence.attachment.download" => {
                    register!(
                        crate::confluence_attachment_download::command::AttachmentDownloadArgs
                    )
                }
                "jira.comment.get" => register!(crate::issue_comments::command::CommentGetArgs),
                "jira.comment.list" => register!(crate::issue_comments::command::CommentListArgs),
                "jira.attachment.download" => {
                    register!(crate::attachment_download::command::AttachmentDownloadArgs)
                }
                "jira.attachment.upload" => {
                    register!(crate::attachment_upload::command::UploadArgs)
                }
                "jira.attachment.delete" => {
                    register!(crate::attachment_delete::command::AttachmentDeleteArgs)
                }
                "jira.attachment.get" => {
                    register!(crate::attachment_reads::command::AttachmentGetArgs)
                }
                "jira.attachment.list" => {
                    register!(crate::attachment_reads::command::AttachmentListArgs)
                }
                "jira.attachment.settings" => {
                    register!(crate::attachment_reads::command::AttachmentSettingsArgs)
                }
                "jira.comment.create" => {
                    register!(crate::comment_writes::command::CommentCreateArgs)
                }
                "jira.comment.update" => {
                    register!(crate::comment_writes::command::CommentUpdateArgs)
                }
                "jira.comment.delete" => {
                    register!(crate::comment_writes::command::CommentDeleteArgs)
                }
                "jira.issue.get" => register!(crate::issue_reads::command::IssueGetArgs),
                "jira.project.list" => register!(crate::project_list::command::ProjectListArgs),
                "jira.project.get" => register!(crate::project_context::command::ProjectGetArgs),
                "jira.project.statuses" => {
                    register!(crate::project_context::command::ProjectStatusesArgs)
                }
                "jira.field.list" => register!(crate::field_reads::command::FieldListArgs),
                "jira.field.search" => register!(crate::field_reads::command::FieldSearchArgs),
                "jira.metadata.edit" => register!(crate::issue_context::command::EditMetadataArgs),
                "jira.transition.list" => {
                    register!(crate::issue_context::command::TransitionListArgs)
                }
                "jira.permission.check" => {
                    register!(crate::permission_reads::command::PermissionCheckArgs)
                }
                "jira.issue.search" => register!(crate::issue_reads::command::IssueSearchArgs),
                "jira.issue.create" => register!(crate::issue_create::command::IssueCreateArgs),
                "jira.issue.update" => register!(crate::issue_mutation::command::IssueUpdateArgs),
                "jira.issue.assign" => register!(crate::issue_mutation::command::IssueAssignArgs),
                "jira.issue.unassign" => {
                    register!(crate::issue_mutation::command::IssueUnassignArgs)
                }
                "jira.issue.delete" => register!(crate::issue_mutation::command::IssueDeleteArgs),
                "jira.transition.apply" => {
                    register!(crate::issue_mutation::command::TransitionApplyArgs)
                }
                "jira.link.create" => register!(crate::issue_links::command::LinkArgs),
                "jira.link.delete" => register!(crate::issue_links::command::LinkDeleteArgs),
                "jira.link.get" => register!(crate::issue_links::reads::command::LinkGetArgs),
                "jira.link.list" => register!(crate::issue_links::reads::command::LinkListArgs),
                "jira.link.types" => register!(crate::issue_links::reads::command::LinkTypesArgs),
                "jira.link.reconcile" => register!(crate::issue_links::recovery::ReconcileArgs),
                _ => {
                    return Err(CraftError::new(
                        ErrorCode::Internal,
                        "Unknown public cloud adapter.",
                    ));
                }
            }
        }
        Ok(())
    }
}
pub(crate) struct VerifiedHostRequired;
impl OperationHandler for VerifiedHostRequired {
    fn validate(&self, request: &CanonicalRequest) -> Result<(), CraftError> {
        request.target.validate()
    }
    fn execute(
        &self,
        _: &PreparedOperation,
        _: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "Cloud execution requires the verified CLI host, saved authority and journal controls.",
        ))
    }
}
pub(crate) fn register(registry: &mut Registry) -> Result<(), CraftError> {
    registry.register_provider(&CloudCommands::Confluence)?;
    registry.register_provider(&CloudCommands::Jira)?;
    let mut recovery = Registry::default();
    crate::issue_recovery::command::register_candidate(&mut recovery)?;
    registry.register_typed_handler::<crate::issue_recovery::command::OperationReconcileArgs>(
        recovery.descriptor(crate::issue_recovery::COMMAND)?.clone(),
        Arc::new(VerifiedHostRequired),
    )?;
    crate::job_wait::command::register(registry)?;
    crate::scans::command::register(registry)?;
    crate::planning_publication::register(registry)?;
    crate::planning_read::register(registry)
}
