use crate::args::{Action, CommandAction, InputAction, ProviderAction, SchemaArgs, SchemaView};
use crate::catalog::{
    self, CapabilitiesData, CapabilitySummary, HelpData, ProvidersData, SearchData, ValidationData,
    VersionData,
};
use craft_core::contracts::{Availability, Capability, EffectClass, Envelope, SCHEMA_VERSION};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::input::{MAX_INPUT_BYTES, canonical_digest, parse_object};
use craft_core::registry::Registry;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::io;
use std::path::Path;

pub struct DispatchOutput {
    pub data: Value,
    pub page: Option<craft_core::contracts::Page>,
}

pub struct CommandResult {
    pub envelope: Envelope,
    pub output_policy: crate::output::OutputPolicy,
}

#[cfg(test)]
mod tests;

pub fn command_id(action: &Action) -> &'static str {
    match action {
        Action::Artifact { action } => action.command(),
        Action::Review {
            action: crate::args::ReviewAction::Verify(_),
        } => crate::planning_review::verification::VERIFY,
        Action::Review {
            action: crate::args::ReviewAction::Prepare(_),
        } => crate::planning_review::command::PREPARE,
        Action::Review {
            action: crate::args::ReviewAction::Policy { action },
        } => match action {
            crate::args::ReviewPolicyAction::Get(_) => crate::review_policy::GET,
            crate::args::ReviewPolicyAction::Set(_) => crate::review_policy::SET,
        },
        Action::Policy { action } => match action {
            crate::args::PolicyAction::Get(_) => crate::policies::GET,
            crate::args::PolicyAction::Set(_) => crate::policies::SET,
        },
        Action::Confluence { action } => crate::confluence_reads::command::action_command(action),
        Action::Jira {
            action: crate::args::JiraAction::Attachment { action },
        } => match action {
            crate::args::JiraAttachmentAction::Download(_) => {
                craft_provider_jira::attachment_download::DOWNLOAD
            }
            crate::args::JiraAttachmentAction::Upload(_) => {
                craft_provider_jira::attachment_upload::UPLOAD
            }
            crate::args::JiraAttachmentAction::Delete(_) => {
                craft_provider_jira::attachment_delete::DELETE
            }
            crate::args::JiraAttachmentAction::Get(_) => craft_provider_jira::attachments::GET,
            crate::args::JiraAttachmentAction::List(_) => craft_provider_jira::attachments::LIST,
            crate::args::JiraAttachmentAction::Settings(_) => {
                craft_provider_jira::attachments::SETTINGS
            }
        },
        Action::Jira {
            action: crate::args::JiraAction::Comment { action },
        } => match action {
            crate::args::JiraCommentAction::Get(_) => craft_provider_jira::comments::GET,
            crate::args::JiraCommentAction::List(_) => craft_provider_jira::comments::LIST,
            crate::args::JiraCommentAction::Create(_) => {
                craft_provider_jira::comments::mutation::CREATE
            }
            crate::args::JiraCommentAction::Update(_) => {
                craft_provider_jira::comments::mutation::UPDATE
            }
            crate::args::JiraCommentAction::Delete(_) => {
                craft_provider_jira::comments::mutation::DELETE
            }
        },
        Action::Jira {
            action: crate::args::JiraAction::Link { action },
        } => match action {
            crate::args::JiraLinkAction::Create(_) => "jira.link.create",
            crate::args::JiraLinkAction::Delete(_) => "jira.link.delete",
            crate::args::JiraLinkAction::Get(_) => "jira.link.get",
            crate::args::JiraLinkAction::List(_) => "jira.link.list",
            crate::args::JiraLinkAction::Types(_) => "jira.link.types",
            crate::args::JiraLinkAction::Reconcile(_) => "jira.link.reconcile",
        },
        Action::Jira {
            action: crate::args::JiraAction::Permission { .. },
        } => "jira.permission.check",
        Action::Jira {
            action: crate::args::JiraAction::Metadata { .. },
        } => "jira.metadata.edit",
        Action::Jira {
            action: crate::args::JiraAction::Transition { action },
        } => match action {
            crate::args::JiraTransitionAction::List(_) => "jira.transition.list",
            crate::args::JiraTransitionAction::Apply(_) => "jira.transition.apply",
        },
        Action::Jira {
            action: crate::args::JiraAction::Field { action },
        } => match action {
            crate::args::JiraFieldAction::List(_) => "jira.field.list",
            crate::args::JiraFieldAction::Search(_) => "jira.field.search",
        },
        Action::Jira {
            action: crate::args::JiraAction::Issue { action },
        } => match action {
            crate::args::JiraIssueAction::Create(_) => "jira.issue.create",
            crate::args::JiraIssueAction::Update(_) => "jira.issue.update",
            crate::args::JiraIssueAction::Assign(_) => "jira.issue.assign",
            crate::args::JiraIssueAction::Unassign(_) => "jira.issue.unassign",
            crate::args::JiraIssueAction::Delete(_) => "jira.issue.delete",
            crate::args::JiraIssueAction::Get(_) => "jira.issue.get",
            crate::args::JiraIssueAction::Search(_) => "jira.issue.search",
        },
        Action::Jira {
            action: crate::args::JiraAction::Project { action },
        } => match action {
            crate::args::JiraProjectAction::List(_) => "jira.project.list",
            crate::args::JiraProjectAction::Get(_) => "jira.project.get",
            crate::args::JiraProjectAction::Statuses(_) => "jira.project.statuses",
        },
        Action::Operation {
            action: crate::args::OperationAction::Get(_),
        } => "operation.get",
        Action::Operation {
            action: crate::args::OperationAction::List(_),
        } => "operation.list",
        Action::Operation {
            action: crate::args::OperationAction::Preview(_),
        } => "operation.preview",
        Action::Operation {
            action: crate::args::OperationAction::Reconcile(_),
        } => "operation.reconcile",
        Action::Operation {
            action: crate::args::OperationAction::Wait(_),
        } => crate::job_wait::COMMAND,
        Action::Scan { action } => action.command(),
        Action::Plan { action } => match action {
            crate::args::PlanAction::Scaffold(_) => "plan.scaffold",
            crate::args::PlanAction::Validate(_) => "plan.validate",
            crate::args::PlanAction::Render(_) => "plan.render",
            crate::args::PlanAction::PageCreate(_) => crate::planning_publication::PLAN_CREATE,
            crate::args::PlanAction::PageGet(_) => crate::planning_read::PLAN_GET,
            crate::args::PlanAction::TicketsPreview(_) => crate::planning_review::tickets::PREVIEW,
            crate::args::PlanAction::TicketsCreate(_) => {
                crate::planning_review::application::CREATE
            }
            crate::args::PlanAction::TicketsResume(_) => {
                crate::planning_review::application::RESUME
            }
            crate::args::PlanAction::TicketsRecordCompletion(_) => {
                crate::planning_review::application::recovery::COMMAND
            }
        },
        Action::Design { action } => match action {
            crate::args::DesignAction::Scaffold(_) => "design.scaffold",
            crate::args::DesignAction::Validate(_) => "design.validate",
            crate::args::DesignAction::Render(_) => "design.render",
            crate::args::DesignAction::PageCreate(_) => crate::planning_publication::DESIGN_CREATE,
            crate::args::DesignAction::PageGet(_) => crate::planning_read::DESIGN_GET,
        },
        Action::Auth { action } => match action {
            crate::args::AuthAction::Status(_) => "auth.status",
            crate::args::AuthAction::Doctor(_) => "auth.doctor",
            crate::args::AuthAction::Provision(_) => "auth.provision",
        },
        Action::Profile { action } => match action {
            crate::args::ProfileAction::List(_) => "profile.list",
            crate::args::ProfileAction::Show(_) => "profile.show",
            crate::args::ProfileAction::Create(_) => "profile.create",
            crate::args::ProfileAction::Use(_) => "profile.use",
        },
        Action::Version => "version",
        Action::Command { .. } => "command.find",
        Action::Schema(_) => "schema",
        Action::Input { .. } => "input.validate",
        Action::Provider { .. } => "provider.list",
        Action::Capabilities(_) => "capabilities",
        Action::Help(_) => "help",
    }
}

fn value<T: Serialize>(input: T) -> Result<Value, CraftError> {
    serde_json::to_value(input)
        .map_err(|_| CraftError::new(ErrorCode::Internal, "Cannot encode operation data."))
}
fn argument_value_for_read(
    input: &crate::issue_reads::command::IssueReadArgs,
) -> Result<Value, CraftError> {
    match input {
        crate::issue_reads::command::IssueReadArgs::Get(args) => value(args),
        crate::issue_reads::command::IssueReadArgs::Search(args) => value(args),
    }
}
fn project_arguments(action: &crate::args::JiraProjectAction) -> Result<Value, CraftError> {
    match action {
        crate::args::JiraProjectAction::List(args) => value(args),
        crate::args::JiraProjectAction::Get(args) => value(args),
        crate::args::JiraProjectAction::Statuses(args) => value(args),
    }
}

fn field_arguments(action: &crate::args::JiraFieldAction) -> Result<Value, CraftError> {
    match action {
        crate::args::JiraFieldAction::List(args) => value(args),
        crate::args::JiraFieldAction::Search(args) => value(args),
    }
}

pub(crate) fn read_source(file: &str, control: &InvocationControl) -> Result<Vec<u8>, CraftError> {
    let bytes = if file == "-" {
        craft_runtime::control::read_bounded(io::stdin(), MAX_INPUT_BYTES, control)?
    } else {
        let root = std::env::current_dir().map_err(|_| {
            CraftError::new(ErrorCode::InputInvalid, "Cannot resolve the input root.")
        })?;
        craft_runtime::inputs::read_project_file(&root, Path::new(file), MAX_INPUT_BYTES, control)?
    };
    Ok(bytes)
}

pub(crate) fn read_input(file: &str, control: &InvocationControl) -> Result<Value, CraftError> {
    parse_object(&read_source(file, control)?)
}

fn reads_stdin(action: &Action) -> bool {
    match action {
        Action::Input {
            action: InputAction::Validate(args),
        } => args.file == "-",
        Action::Plan {
            action: crate::args::PlanAction::Validate(args),
        }
        | Action::Design {
            action: crate::args::DesignAction::Validate(args),
        } => args.file == "-",
        Action::Plan {
            action: crate::args::PlanAction::Render(args),
        }
        | Action::Design {
            action: crate::args::DesignAction::Render(args),
        } => args.source.file == "-",
        Action::Plan {
            action: crate::args::PlanAction::PageCreate(args),
        }
        | Action::Design {
            action: crate::args::DesignAction::PageCreate(args),
        } => args.source.file == "-",
        _ => false,
    }
}

fn decode<T: DeserializeOwned>(input: Value) -> Result<T, CraftError> {
    serde_json::from_value(input).map_err(|_| {
        CraftError::new(
            ErrorCode::InputInvalid,
            "Input does not match the typed command contract.",
        )
    })
}

pub fn load_input(
    registry: &Registry,
    action: Action,
    file: &str,
    control: &InvocationControl,
) -> Result<Action, CraftError> {
    let command = command_id(&action);
    let input = read_input(file, control)?;
    registry.validate(command, &input)?;
    let loaded = decode_action(action, input)?;
    if file == "-" && reads_stdin(&loaded) {
        return Err(CraftError::field(
            "/file",
            "stdin_can_only_be_consumed_once",
        ));
    }
    Ok(loaded)
}

/// Decodes only arguments; resource execution and input I/O remain separate.
pub(crate) fn decode_action(action: Action, input: Value) -> Result<Action, CraftError> {
    Ok(match action {
        Action::Review {
            action: crate::args::ReviewAction::Verify(_),
        } => Action::Review {
            action: crate::args::ReviewAction::Verify(decode(input)?),
        },
        Action::Review {
            action: crate::args::ReviewAction::Prepare(_),
        } => Action::Review {
            action: crate::args::ReviewAction::Prepare(decode(input)?),
        },
        Action::Review {
            action: crate::args::ReviewAction::Policy { action },
        } => Action::Review {
            action: crate::args::ReviewAction::Policy {
                action: match action {
                    crate::args::ReviewPolicyAction::Get(_) => {
                        crate::args::ReviewPolicyAction::Get(decode(input)?)
                    }
                    crate::args::ReviewPolicyAction::Set(_) => {
                        crate::args::ReviewPolicyAction::Set(decode(input)?)
                    }
                },
            },
        },
        Action::Policy { action } => Action::Policy {
            action: match action {
                crate::args::PolicyAction::Get(_) => crate::args::PolicyAction::Get(decode(input)?),
                crate::args::PolicyAction::Set(_) => crate::args::PolicyAction::Set(decode(input)?),
            },
        },
        Action::Confluence {
            action:
                crate::args::ConfluenceAction::Attachment {
                    action: crate::args::ConfluenceAttachmentAction::Download(_),
                },
        } => Action::Confluence {
            action: crate::args::ConfluenceAction::Attachment {
                action: crate::args::ConfluenceAttachmentAction::Download(decode(input)?),
            },
        },
        Action::Confluence {
            action:
                crate::args::ConfluenceAction::Attachment {
                    action: crate::args::ConfluenceAttachmentAction::Upload(_),
                },
        } => Action::Confluence {
            action: crate::args::ConfluenceAction::Attachment {
                action: crate::args::ConfluenceAttachmentAction::Upload(decode(input)?),
            },
        },
        Action::Confluence {
            action:
                crate::args::ConfluenceAction::Attachment {
                    action: crate::args::ConfluenceAttachmentAction::Replace(_),
                },
        } => Action::Confluence {
            action: crate::args::ConfluenceAction::Attachment {
                action: crate::args::ConfluenceAttachmentAction::Replace(decode(input)?),
            },
        },
        Action::Confluence { action }
            if craft_provider_confluence::pages::PageWriteKind::from_command(
                crate::confluence_reads::command::action_command(&action),
            )
            .is_some() =>
        {
            Action::Confluence {
                action: crate::confluence_pages::command::PageWriteArgs::decode(
                    crate::confluence_reads::command::action_command(&action),
                    input,
                )?
                .into(),
            }
        }
        Action::Confluence { action } => Action::Confluence {
            action: crate::confluence_reads::command::ConfluenceReadArgs::decode(
                crate::confluence_reads::command::action_command(&action),
                input,
            )?
            .into(),
        },
        Action::Jira {
            action: crate::args::JiraAction::Attachment { action },
        } => Action::Jira {
            action: crate::args::JiraAction::Attachment {
                action: match action {
                    crate::args::JiraAttachmentAction::Download(_) => {
                        crate::args::JiraAttachmentAction::Download(decode(input)?)
                    }
                    crate::args::JiraAttachmentAction::Upload(_) => {
                        crate::args::JiraAttachmentAction::Upload(decode(input)?)
                    }
                    crate::args::JiraAttachmentAction::Delete(_) => {
                        crate::args::JiraAttachmentAction::Delete(decode(input)?)
                    }
                    crate::args::JiraAttachmentAction::Get(_) => {
                        crate::args::JiraAttachmentAction::Get(decode(input)?)
                    }
                    crate::args::JiraAttachmentAction::List(_) => {
                        crate::args::JiraAttachmentAction::List(decode(input)?)
                    }
                    crate::args::JiraAttachmentAction::Settings(_) => {
                        crate::args::JiraAttachmentAction::Settings(decode(input)?)
                    }
                },
            },
        },
        Action::Jira {
            action: crate::args::JiraAction::Comment { action },
        } => Action::Jira {
            action: crate::args::JiraAction::Comment {
                action: match action {
                    crate::args::JiraCommentAction::Create(_) => {
                        crate::args::JiraCommentAction::Create(decode(input)?)
                    }
                    crate::args::JiraCommentAction::Update(_) => {
                        crate::args::JiraCommentAction::Update(decode(input)?)
                    }
                    crate::args::JiraCommentAction::Delete(_) => {
                        crate::args::JiraCommentAction::Delete(decode(input)?)
                    }
                    crate::args::JiraCommentAction::Get(_) => {
                        crate::args::JiraCommentAction::Get(decode(input)?)
                    }
                    crate::args::JiraCommentAction::List(_) => {
                        crate::args::JiraCommentAction::List(decode(input)?)
                    }
                },
            },
        },
        Action::Jira {
            action: crate::args::JiraAction::Link { action },
        } => Action::Jira {
            action: crate::args::JiraAction::Link {
                action: match action {
                    crate::args::JiraLinkAction::Create(_) => {
                        crate::args::JiraLinkAction::Create(decode(input)?)
                    }
                    crate::args::JiraLinkAction::Delete(_) => {
                        crate::args::JiraLinkAction::Delete(decode(input)?)
                    }
                    crate::args::JiraLinkAction::Get(_) => {
                        crate::args::JiraLinkAction::Get(decode(input)?)
                    }
                    crate::args::JiraLinkAction::List(_) => {
                        crate::args::JiraLinkAction::List(decode(input)?)
                    }
                    crate::args::JiraLinkAction::Types(_) => {
                        crate::args::JiraLinkAction::Types(decode(input)?)
                    }
                    crate::args::JiraLinkAction::Reconcile(_) => {
                        crate::args::JiraLinkAction::Reconcile(decode(input)?)
                    }
                },
            },
        },
        Action::Jira {
            action: crate::args::JiraAction::Permission { .. },
        } => Action::Jira {
            action: crate::args::JiraAction::Permission {
                action: crate::args::JiraPermissionAction::Check(decode(input)?),
            },
        },
        Action::Jira {
            action: crate::args::JiraAction::Metadata { .. },
        } => Action::Jira {
            action: crate::args::JiraAction::Metadata {
                action: crate::args::JiraMetadataAction::Edit(decode(input)?),
            },
        },
        Action::Jira {
            action: crate::args::JiraAction::Transition { action },
        } => Action::Jira {
            action: crate::args::JiraAction::Transition {
                action: match action {
                    crate::args::JiraTransitionAction::List(_) => {
                        crate::args::JiraTransitionAction::List(decode(input)?)
                    }
                    crate::args::JiraTransitionAction::Apply(_) => {
                        crate::args::JiraTransitionAction::Apply(decode(input)?)
                    }
                },
            },
        },
        Action::Jira {
            action: crate::args::JiraAction::Field { action },
        } => Action::Jira {
            action: crate::args::JiraAction::Field {
                action: match action {
                    crate::args::JiraFieldAction::List(_) => {
                        crate::args::JiraFieldAction::List(decode(input)?)
                    }
                    crate::args::JiraFieldAction::Search(_) => {
                        crate::args::JiraFieldAction::Search(decode(input)?)
                    }
                },
            },
        },
        Action::Jira {
            action: crate::args::JiraAction::Issue { action },
        } => Action::Jira {
            action: crate::args::JiraAction::Issue {
                action: match action {
                    crate::args::JiraIssueAction::Create(_) => {
                        crate::args::JiraIssueAction::Create(decode(input)?)
                    }
                    crate::args::JiraIssueAction::Update(_) => {
                        crate::args::JiraIssueAction::Update(decode(input)?)
                    }
                    crate::args::JiraIssueAction::Assign(_) => {
                        crate::args::JiraIssueAction::Assign(decode(input)?)
                    }
                    crate::args::JiraIssueAction::Unassign(_) => {
                        crate::args::JiraIssueAction::Unassign(decode(input)?)
                    }
                    crate::args::JiraIssueAction::Delete(_) => {
                        crate::args::JiraIssueAction::Delete(decode(input)?)
                    }
                    crate::args::JiraIssueAction::Get(_) => {
                        crate::args::JiraIssueAction::Get(decode(input)?)
                    }
                    crate::args::JiraIssueAction::Search(_) => {
                        crate::args::JiraIssueAction::Search(decode(input)?)
                    }
                },
            },
        },
        Action::Jira {
            action: crate::args::JiraAction::Project { action },
        } => Action::Jira {
            action: crate::args::JiraAction::Project {
                action: match action {
                    crate::args::JiraProjectAction::List(_) => {
                        crate::args::JiraProjectAction::List(decode(input)?)
                    }
                    crate::args::JiraProjectAction::Get(_) => {
                        crate::args::JiraProjectAction::Get(decode(input)?)
                    }
                    crate::args::JiraProjectAction::Statuses(_) => {
                        crate::args::JiraProjectAction::Statuses(decode(input)?)
                    }
                },
            },
        },
        Action::Artifact { action } => Action::Artifact {
            action: match action {
                crate::local_artifacts::ArtifactAction::Get(_) => {
                    crate::local_artifacts::ArtifactAction::Get(decode(input)?)
                }
                crate::local_artifacts::ArtifactAction::Export(_) => {
                    crate::local_artifacts::ArtifactAction::Export(decode(input)?)
                }
            },
        },
        Action::Scan { action } => Action::Scan {
            action: match action {
                crate::scans::command::ScanAction::Start(_) => {
                    crate::scans::command::ScanAction::Start(Box::new(decode(input)?))
                }
                crate::scans::command::ScanAction::Resume(_) => {
                    crate::scans::command::ScanAction::Resume(Box::new(decode(input)?))
                }
                crate::scans::command::ScanAction::Export(_) => {
                    crate::scans::command::ScanAction::Export(Box::new(decode(input)?))
                }
            },
        },
        Action::Operation { action } => Action::Operation {
            action: match action {
                crate::args::OperationAction::Get(_) => {
                    crate::args::OperationAction::Get(decode(input)?)
                }
                crate::args::OperationAction::List(_) => {
                    crate::args::OperationAction::List(decode(input)?)
                }
                crate::args::OperationAction::Preview(_) => {
                    crate::args::OperationAction::Preview(decode(input)?)
                }
                crate::args::OperationAction::Reconcile(_) => {
                    crate::args::OperationAction::Reconcile(Box::new(decode(input)?))
                }
                crate::args::OperationAction::Wait(_) => {
                    crate::args::OperationAction::Wait(Box::new(decode(input)?))
                }
            },
        },
        Action::Plan { action } => Action::Plan {
            action: match action {
                crate::args::PlanAction::Scaffold(_) => {
                    crate::args::PlanAction::Scaffold(decode(input)?)
                }
                crate::args::PlanAction::Validate(_) => {
                    crate::args::PlanAction::Validate(decode(input)?)
                }
                crate::args::PlanAction::Render(_) => {
                    crate::args::PlanAction::Render(decode(input)?)
                }
                crate::args::PlanAction::PageCreate(_) => {
                    crate::args::PlanAction::PageCreate(decode(input)?)
                }
                crate::args::PlanAction::PageGet(_) => {
                    crate::args::PlanAction::PageGet(decode(input)?)
                }
                crate::args::PlanAction::TicketsPreview(_) => {
                    crate::args::PlanAction::TicketsPreview(decode(input)?)
                }
                crate::args::PlanAction::TicketsCreate(_) => {
                    crate::args::PlanAction::TicketsCreate(decode(input)?)
                }
                crate::args::PlanAction::TicketsResume(_) => {
                    crate::args::PlanAction::TicketsResume(decode(input)?)
                }
                crate::args::PlanAction::TicketsRecordCompletion(_) => {
                    crate::args::PlanAction::TicketsRecordCompletion(decode(input)?)
                }
            },
        },
        Action::Design { action } => Action::Design {
            action: match action {
                crate::args::DesignAction::Scaffold(_) => {
                    crate::args::DesignAction::Scaffold(decode(input)?)
                }
                crate::args::DesignAction::Validate(_) => {
                    crate::args::DesignAction::Validate(decode(input)?)
                }
                crate::args::DesignAction::Render(_) => {
                    crate::args::DesignAction::Render(decode(input)?)
                }
                crate::args::DesignAction::PageCreate(_) => {
                    crate::args::DesignAction::PageCreate(decode(input)?)
                }
                crate::args::DesignAction::PageGet(_) => {
                    crate::args::DesignAction::PageGet(decode(input)?)
                }
            },
        },
        Action::Auth { action } => Action::Auth {
            action: match action {
                crate::args::AuthAction::Status(_) => {
                    crate::args::AuthAction::Status(decode(input)?)
                }
                crate::args::AuthAction::Doctor(_) => {
                    crate::args::AuthAction::Doctor(decode(input)?)
                }
                crate::args::AuthAction::Provision(_) => {
                    crate::args::AuthAction::Provision(decode(input)?)
                }
            },
        },
        Action::Profile { action } => Action::Profile {
            action: match action {
                crate::args::ProfileAction::List(_) => {
                    crate::args::ProfileAction::List(decode(input)?)
                }
                crate::args::ProfileAction::Show(_) => {
                    crate::args::ProfileAction::Show(decode(input)?)
                }
                crate::args::ProfileAction::Create(_) => {
                    crate::args::ProfileAction::Create(decode(input)?)
                }
                crate::args::ProfileAction::Use(_) => {
                    crate::args::ProfileAction::Use(decode(input)?)
                }
            },
        },
        Action::Version | Action::Provider { .. } => {
            let _: crate::args::EmptyArgs = decode(input)?;
            action
        }
        Action::Command { .. } => Action::Command {
            action: CommandAction::Find(decode(input)?),
        },
        Action::Schema(_) => Action::Schema(decode(input)?),
        Action::Input { .. } => Action::Input {
            action: InputAction::Validate(decode(input)?),
        },
        Action::Capabilities(_) => Action::Capabilities(decode(input)?),
        Action::Help(_) => Action::Help(decode(input)?),
    })
}

pub(crate) fn argument_value(action: &Action) -> Result<Value, CraftError> {
    match action {
        Action::Review {
            action: crate::args::ReviewAction::Verify(args),
        } => value(args),
        Action::Review {
            action: crate::args::ReviewAction::Prepare(args),
        } => value(args),
        Action::Review {
            action: crate::args::ReviewAction::Policy { action },
        } => match action {
            crate::args::ReviewPolicyAction::Get(v) => value(v),
            crate::args::ReviewPolicyAction::Set(v) => value(v),
        },
        Action::Policy { action } => match action {
            crate::args::PolicyAction::Get(v) => value(v),
            crate::args::PolicyAction::Set(v) => value(v),
        },
        Action::Confluence { action } => crate::confluence_reads::command::action_arguments(action),
        Action::Jira {
            action: crate::args::JiraAction::Attachment { action },
        } => match action {
            crate::args::JiraAttachmentAction::Download(v) => value(v),
            crate::args::JiraAttachmentAction::Upload(v) => value(v),
            crate::args::JiraAttachmentAction::Delete(v) => value(v),
            crate::args::JiraAttachmentAction::Get(v) => value(v),
            crate::args::JiraAttachmentAction::List(v) => value(v),
            crate::args::JiraAttachmentAction::Settings(v) => value(v),
        },
        Action::Jira {
            action: crate::args::JiraAction::Comment { action },
        } => match action {
            crate::args::JiraCommentAction::Create(args) => value(args),
            crate::args::JiraCommentAction::Update(args) => value(args),
            crate::args::JiraCommentAction::Delete(args) => value(args),
            crate::args::JiraCommentAction::Get(args) => value(args),
            crate::args::JiraCommentAction::List(args) => value(args),
        },
        Action::Jira {
            action: crate::args::JiraAction::Link { action },
        } => match action {
            crate::args::JiraLinkAction::Create(v) => value(v),
            crate::args::JiraLinkAction::Delete(v) => value(v),
            crate::args::JiraLinkAction::Get(v) => value(v),
            crate::args::JiraLinkAction::List(v) => value(v),
            crate::args::JiraLinkAction::Types(v) => value(v),
            crate::args::JiraLinkAction::Reconcile(v) => value(v),
        },
        Action::Jira {
            action:
                crate::args::JiraAction::Permission {
                    action: crate::args::JiraPermissionAction::Check(args),
                },
        } => value(args),
        Action::Jira {
            action:
                crate::args::JiraAction::Metadata {
                    action: crate::args::JiraMetadataAction::Edit(args),
                },
        } => value(args),
        Action::Jira {
            action:
                crate::args::JiraAction::Transition {
                    action: crate::args::JiraTransitionAction::List(args),
                },
        } => value(args),
        Action::Jira {
            action:
                crate::args::JiraAction::Transition {
                    action: crate::args::JiraTransitionAction::Apply(args),
                },
        } => value(args),
        Action::Jira {
            action: crate::args::JiraAction::Field { action },
        } => field_arguments(action),
        Action::Jira {
            action: crate::args::JiraAction::Project { action },
        } => project_arguments(action),
        Action::Jira {
            action: crate::args::JiraAction::Issue { action },
        } => match action {
            crate::args::JiraIssueAction::Create(arguments) => value(arguments),
            crate::args::JiraIssueAction::Update(arguments) => value(arguments),
            crate::args::JiraIssueAction::Assign(arguments) => value(arguments),
            crate::args::JiraIssueAction::Unassign(arguments) => value(arguments),
            crate::args::JiraIssueAction::Delete(arguments) => value(arguments),
            crate::args::JiraIssueAction::Get(arguments) => value(arguments),
            crate::args::JiraIssueAction::Search(arguments) => value(arguments),
        },
        Action::Artifact { action } => action.arguments(),
        Action::Scan { action } => action.arguments(),
        Action::Operation {
            action: crate::args::OperationAction::Get(arguments),
        } => value(arguments),
        Action::Operation {
            action: crate::args::OperationAction::List(arguments),
        } => value(arguments),
        Action::Operation {
            action: crate::args::OperationAction::Preview(arguments),
        } => value(arguments),
        Action::Operation {
            action: crate::args::OperationAction::Reconcile(arguments),
        } => value(arguments),
        Action::Operation {
            action: crate::args::OperationAction::Wait(arguments),
        } => value(arguments),
        Action::Plan { action } => match action {
            crate::args::PlanAction::Scaffold(args) => value(args),
            crate::args::PlanAction::Validate(args) => value(args),
            crate::args::PlanAction::Render(args) => value(args),
            crate::args::PlanAction::PageCreate(args) => value(args),
            crate::args::PlanAction::PageGet(args) => value(args),
            crate::args::PlanAction::TicketsPreview(args) => value(args),
            crate::args::PlanAction::TicketsCreate(args)
            | crate::args::PlanAction::TicketsResume(args) => value(args),
            crate::args::PlanAction::TicketsRecordCompletion(args) => value(args),
        },
        Action::Design { action } => match action {
            crate::args::DesignAction::Scaffold(args) => value(args),
            crate::args::DesignAction::Validate(args) => value(args),
            crate::args::DesignAction::Render(args) => value(args),
            crate::args::DesignAction::PageCreate(args) => value(args),
            crate::args::DesignAction::PageGet(args) => value(args),
        },
        Action::Auth { action } => match action {
            crate::args::AuthAction::Status(arguments)
            | crate::args::AuthAction::Doctor(arguments) => value(arguments),
            crate::args::AuthAction::Provision(arguments) => value(arguments),
        },
        Action::Profile { action } => match action {
            crate::args::ProfileAction::List(arguments) => value(arguments),
            crate::args::ProfileAction::Show(arguments)
            | crate::args::ProfileAction::Use(arguments) => value(arguments),
            crate::args::ProfileAction::Create(arguments) => value(arguments),
        },
        Action::Version | Action::Provider { .. } => Ok(json!({})),
        Action::Command {
            action: CommandAction::Find(arguments),
        } => value(arguments),
        Action::Schema(arguments) => value(arguments),
        Action::Input {
            action: InputAction::Validate(arguments),
        } => value(arguments),
        Action::Capabilities(arguments) => value(arguments),
        Action::Help(arguments) => value(arguments),
    }
}

fn schema_data(registry: &Registry, arguments: SchemaArgs) -> Result<Value, CraftError> {
    if arguments.example.is_some() && arguments.path.is_some() {
        return Err(CraftError::field(
            "/path",
            "example_and_property_path_are_exclusive",
        ));
    }
    let descriptor = registry.descriptor(&arguments.command)?;
    let provider_version = if descriptor.provider == "craft" {
        Some(craft_core::VERSION.to_owned())
    } else {
        registry
            .providers()
            .find(|provider| provider.id == descriptor.provider)
            .map(|provider| provider.version.clone())
    };
    if !arguments.response && !arguments.fields.is_empty() {
        return Err(CraftError::field(
            "/fields",
            "response_fields_require_response_schema",
        ));
    }
    let projection = if arguments.response {
        registry.projection(&arguments.command, &arguments.fields)?
    } else {
        None
    };
    let source = if arguments.response {
        match &projection {
            Some(projection) => projection.response_schema(&descriptor.response_schema)?,
            None => descriptor.response_schema.clone(),
        }
    } else {
        descriptor.input_schema.clone()
    };
    let digest = canonical_digest(&source)?;
    let selectable_fields = if arguments.response {
        descriptor
            .projection
            .as_ref()
            .map(|contract| contract.selectable_fields(&descriptor.response_schema))
            .transpose()?
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    if arguments.example.is_some() {
        let example = if arguments.response {
            let mut example = descriptor.response_example.clone();
            if let Some(projection) = projection {
                example["data"] = projection.data(&example["data"])?;
            }
            example
        } else {
            descriptor.example.clone()
        };
        return Ok(
            json!({"command":arguments.command,"contract_version":descriptor.contract_version,"provider":descriptor.provider,"provider_version":provider_version,"api_revision":descriptor.api_revision,"direction":if arguments.response {"response"} else {"input"}, "view":"example","complete":true,
            "digest":digest, "schema":example, "projection":if arguments.response {descriptor.projection.as_ref()} else {None}, "selectable_fields":selectable_fields}),
        );
    }
    let (schema, complete, view) = if let Some(path) = arguments.path {
        if path.len() > 2048 || (!path.is_empty() && !path.starts_with('/')) {
            return Err(CraftError::field("/path", "expected_bounded_json_pointer"));
        }
        (
            source
                .pointer(&path)
                .cloned()
                .ok_or_else(|| CraftError::field("/path", "schema_property_not_found"))?,
            false,
            "fragment",
        )
    } else if matches!(arguments.view, SchemaView::Full) {
        (source.clone(), true, "full")
    } else {
        let properties = source.get("properties").and_then(Value::as_object).map(|properties| {
            properties.iter().map(|(name, schema)| (name.clone(), json!({"type":schema.get("type"),"reference":schema.get("$ref"),"default":schema.get("default")}))).collect::<serde_json::Map<_, _>>()
        }).unwrap_or_default();
        (
            json!({"type":source.get("type"),"required":source.get("required"),"properties":properties,"decoder":source.get("x-craft-input-decoder"),
            "detail":"Use --view full or --path for definitions, nested constraints and variants."}),
            false,
            "compact",
        )
    };
    Ok(
        json!({"command":arguments.command,"contract_version":descriptor.contract_version,"provider":descriptor.provider,"provider_version":provider_version,"api_revision":descriptor.api_revision,"direction":if arguments.response {"response"} else {"input"}, "view":view,"complete":complete,
        "digest":digest,"schema":schema,"projection":if arguments.response {descriptor.projection.as_ref()} else {None},"selectable_fields":selectable_fields}),
    )
}

pub fn execute(
    registry: &Registry,
    action: Action,
    fields: &[String],
    control: &InvocationControl,
    options: &crate::profiles::InvocationOptions<'_>,
    stdin_consumed: bool,
) -> Result<CommandResult, CraftError> {
    if let Action::Artifact { action } = action {
        registry.validate(action.command(), &action.arguments()?)?;
        return Ok(crate::local_artifacts::execute(
            action, fields, options, control,
        ));
    }
    if matches!(
        &action,
        Action::Jira {
            action: crate::args::JiraAction::Attachment { .. }
        }
    ) {
        registry.validate(command_id(&action), &argument_value(&action)?)?;
        let Action::Jira {
            action: crate::args::JiraAction::Attachment { action },
        } = action
        else {
            return Err(CraftError::field("/command", "expected_attachment_read"));
        };
        use crate::attachment_reads::command::AttachmentReadArgs as A;
        let args = match action {
            crate::args::JiraAttachmentAction::Download(v) => {
                return Ok(crate::attachment_download::command::execute(
                    v, fields, options, control,
                ));
            }
            crate::args::JiraAttachmentAction::Upload(v) => {
                return Ok(crate::attachment_upload::command::execute(
                    *v, fields, options, control,
                ));
            }
            crate::args::JiraAttachmentAction::Delete(v) => {
                return Ok(crate::attachment_delete::command::execute(
                    *v, fields, options, control,
                ));
            }
            crate::args::JiraAttachmentAction::Get(v) => A::Get(v),
            crate::args::JiraAttachmentAction::List(v) => A::List(v),
            crate::args::JiraAttachmentAction::Settings(v) => A::Settings(v),
        };
        return Ok(crate::attachment_reads::command::execute(
            args, fields, options, control,
        ));
    }
    if matches!(
        &action,
        Action::Jira {
            action: crate::args::JiraAction::Comment { .. }
        }
    ) {
        registry.validate(command_id(&action), &argument_value(&action)?)?;
        let Action::Jira {
            action: crate::args::JiraAction::Comment { action },
        } = action
        else {
            return Err(CraftError::field("/command", "expected_comment_read"));
        };
        use crate::comment_writes::command::{WriteArgs, execute as write};
        use crate::issue_comments::command::CommentReadArgs;
        return Ok(match action {
            crate::args::JiraCommentAction::Get(args) => crate::issue_comments::command::execute(
                CommentReadArgs::Get(args),
                fields,
                options,
                control,
            ),
            crate::args::JiraCommentAction::List(args) => crate::issue_comments::command::execute(
                CommentReadArgs::List(args),
                fields,
                options,
                control,
            ),
            crate::args::JiraCommentAction::Create(args) => {
                write(WriteArgs::Create(*args), fields, options, control)
            }
            crate::args::JiraCommentAction::Update(args) => {
                write(WriteArgs::Update(*args), fields, options, control)
            }
            crate::args::JiraCommentAction::Delete(args) => {
                write(WriteArgs::Delete(*args), fields, options, control)
            }
        });
    }
    if matches!(
        &action,
        Action::Plan {
            action: crate::args::PlanAction::TicketsRecordCompletion(_)
        }
    ) {
        registry.validate(
            crate::planning_review::application::recovery::COMMAND,
            &argument_value(&action)?,
        )?;
        let Action::Plan {
            action: crate::args::PlanAction::TicketsRecordCompletion(args),
        } = action
        else {
            return Err(CraftError::field("/command", "expected_completion_import"));
        };
        return Ok(crate::planning_review::application::recovery::execute(
            *args, fields, options, control,
        ));
    }
    if matches!(
        &action,
        Action::Plan {
            action: crate::args::PlanAction::TicketsCreate(_)
                | crate::args::PlanAction::TicketsResume(_)
        }
    ) {
        let command = command_id(&action);
        registry.validate(command, &argument_value(&action)?)?;
        let args = match action {
            Action::Plan {
                action:
                    crate::args::PlanAction::TicketsCreate(args)
                    | crate::args::PlanAction::TicketsResume(args),
            } => args,
            _ => return Err(CraftError::field("/command", "expected_ticket_application")),
        };
        return Ok(crate::planning_review::application::command::execute(
            command, *args, fields, options, control,
        ));
    }
    if matches!(
        &action,
        Action::Jira {
            action: crate::args::JiraAction::Link { .. }
        }
    ) {
        registry.validate(command_id(&action), &argument_value(&action)?)?;
        let Action::Jira {
            action: crate::args::JiraAction::Link { action },
        } = action
        else {
            return Err(CraftError::field("/command", "expected_link_operation"));
        };
        use crate::args::JiraLinkAction;
        use crate::issue_links::reads::command::LinkReadArgs;
        return Ok(match action {
            JiraLinkAction::Create(v) => {
                crate::issue_links::command::execute(*v, fields, options, control)
            }
            JiraLinkAction::Delete(v) => {
                crate::issue_links::command::execute_delete(*v, fields, options, control)
            }
            JiraLinkAction::Get(v) => crate::issue_links::reads::command::execute(
                LinkReadArgs::Get(v),
                fields,
                options,
                control,
            ),
            JiraLinkAction::List(v) => crate::issue_links::reads::command::execute(
                LinkReadArgs::List(v),
                fields,
                options,
                control,
            ),
            JiraLinkAction::Types(v) => crate::issue_links::reads::command::execute(
                LinkReadArgs::Types(v),
                fields,
                options,
                control,
            ),
            JiraLinkAction::Reconcile(v) => {
                crate::issue_links::recovery::execute(*v, fields, options, control)
            }
        });
    }
    if matches!(
        &action,
        Action::Plan {
            action: crate::args::PlanAction::TicketsPreview(_)
        }
    ) {
        registry.validate(command_id(&action), &argument_value(&action)?)?;
        let Action::Plan {
            action: crate::args::PlanAction::TicketsPreview(args),
        } = action
        else {
            return Err(CraftError::field("/command", "expected_ticket_preview"));
        };
        return Ok(crate::planning_review::tickets::execute(
            *args, fields, options, control,
        ));
    }
    if matches!(
        &action,
        Action::Review {
            action: crate::args::ReviewAction::Verify(_)
        }
    ) {
        registry.validate(command_id(&action), &argument_value(&action)?)?;
        let Action::Review {
            action: crate::args::ReviewAction::Verify(args),
        } = action
        else {
            return Err(CraftError::field(
                "/command",
                "expected_native_review_verification",
            ));
        };
        return Ok(crate::planning_review::verification::execute(
            *args, fields, options, control,
        ));
    }
    if matches!(
        &action,
        Action::Review {
            action: crate::args::ReviewAction::Prepare(_)
        }
    ) {
        registry.validate(command_id(&action), &argument_value(&action)?)?;
        let Action::Review {
            action: crate::args::ReviewAction::Prepare(args),
        } = action
        else {
            return Err(CraftError::field("/command", "expected_review_preparation"));
        };
        return Ok(crate::planning_review::command::execute(
            *args, fields, options, control,
        ));
    }
    if let Action::Review {
        action: crate::args::ReviewAction::Policy { action: review },
    } = &action
    {
        registry.validate(command_id(&action), &argument_value(&action)?)?;
        let args = match review {
            crate::args::ReviewPolicyAction::Get(v) => {
                crate::review_policy::PolicyArgs::Get((**v).clone())
            }
            crate::args::ReviewPolicyAction::Set(v) => {
                crate::review_policy::PolicyArgs::Set((**v).clone())
            }
        };
        return Ok(crate::review_policy::execute(
            args, fields, options, control,
        ));
    }
    if matches!(
        command_id(&action),
        crate::planning_read::PLAN_GET | crate::planning_read::DESIGN_GET
    ) {
        let command = command_id(&action);
        let (Action::Plan {
            action: crate::args::PlanAction::PageGet(args),
        }
        | Action::Design {
            action: crate::args::DesignAction::PageGet(args),
        }) = action
        else {
            return Err(CraftError::field(
                "/command",
                "expected_remote_document_read",
            ));
        };
        return crate::planning_read::execute(registry, command, args, fields, options, control);
    }
    if matches!(
        command_id(&action),
        crate::planning_publication::PLAN_CREATE | crate::planning_publication::DESIGN_CREATE
    ) {
        let command = command_id(&action);
        registry.validate(command, &argument_value(&action)?)?;
        let (Action::Plan {
            action: crate::args::PlanAction::PageCreate(args),
        }
        | Action::Design {
            action: crate::args::DesignAction::PageCreate(args),
        }) = action
        else {
            return Err(CraftError::field(
                "/command",
                "expected_document_publication",
            ));
        };
        return crate::planning_publication::execute(command, args, fields, options, control);
    }
    if matches!(&action, Action::Policy { .. }) {
        return execute_policy_candidate(
            registry,
            action,
            fields,
            control,
            options,
            |args, fields, control, options| {
                crate::policies::execute(args, fields, options, control)
            },
        );
    }
    if let Action::Confluence {
        action:
            crate::args::ConfluenceAction::Attachment {
                action: crate::args::ConfluenceAttachmentAction::Download(v),
            },
    } = action
    {
        registry.descriptor(craft_provider_confluence::attachment_download::DOWNLOAD)?;
        return Ok(crate::confluence_attachment_download::command::execute(
            *v, fields, options, control,
        ));
    }
    if let Action::Confluence {
        action:
            crate::args::ConfluenceAction::Attachment {
                action:
                    crate::args::ConfluenceAttachmentAction::Upload(v)
                    | crate::args::ConfluenceAttachmentAction::Replace(v),
            },
    } = action
    {
        let command = v.command();
        registry.descriptor(command)?;
        return Ok(crate::confluence_attachment_upload::command::execute(
            *v, fields, options, control,
        ));
    }
    if craft_provider_confluence::pages::PageWriteKind::from_command(command_id(&action)).is_some()
    {
        return execute_page_write_candidate(
            registry,
            action,
            fields,
            control,
            options,
            |args, fields, control, options| {
                crate::confluence_pages::command::execute(args, fields, options, control)
            },
        );
    }
    if matches!(&action, Action::Confluence { .. }) {
        return execute_confluence_candidate(
            registry,
            action,
            fields,
            control,
            options,
            |args, fields, control, options| {
                crate::confluence_reads::command::execute(args, fields, options, control)
            },
        );
    }
    if let Action::Scan { action } = action {
        registry.validate(action.command(), &action.arguments()?)?;
        return Ok(crate::scans::command::execute(
            action, fields, options, control,
        ));
    }
    if let Action::Operation {
        action: crate::args::OperationAction::Wait(arguments),
    } = &action
    {
        registry.validate(crate::job_wait::COMMAND, &value(arguments)?)?;
        let Action::Operation {
            action: crate::args::OperationAction::Wait(arguments),
        } = action
        else {
            return Err(CraftError::field("/command", "expected_operation_wait"));
        };
        return Ok(crate::job_wait::command::execute(
            *arguments, fields, options, control,
        ));
    }
    if matches!(
        &action,
        Action::Operation {
            action: crate::args::OperationAction::Reconcile(_)
        }
    ) {
        return execute_recovery_candidate(
            registry,
            action,
            fields,
            control,
            options,
            |args, fields, control, options| {
                crate::issue_recovery::command::execute(args, fields, options, control)
            },
        );
    }
    if matches!(
        &action,
        Action::Jira {
            action: crate::args::JiraAction::Issue {
                action: crate::args::JiraIssueAction::Update(_)
                    | crate::args::JiraIssueAction::Assign(_)
                    | crate::args::JiraIssueAction::Unassign(_)
                    | crate::args::JiraIssueAction::Delete(_)
            }
        } | Action::Jira {
            action: crate::args::JiraAction::Transition {
                action: crate::args::JiraTransitionAction::Apply(_)
            }
        }
    ) {
        return execute_mutation_candidate(
            registry,
            action,
            fields,
            control,
            options,
            |args, fields, control, options| {
                crate::issue_mutation::command::execute(args, fields, options, control)
            },
        );
    }
    execute_with_candidates(
        registry,
        action,
        fields,
        control,
        options,
        stdin_consumed,
        (
            |args, fields, control, options| {
                crate::issue_create::command::execute(args, fields, options, control)
            },
            |action, fields, control, options| match action {
                crate::args::JiraProjectAction::List(args) => {
                    crate::project_list::command::execute(args, fields, options, control)
                }
                crate::args::JiraProjectAction::Get(args) => {
                    crate::project_context::command::execute(
                        crate::project_context::command::ProjectContextArgs::Get(args),
                        fields,
                        options,
                        control,
                    )
                }
                crate::args::JiraProjectAction::Statuses(args) => {
                    crate::project_context::command::execute(
                        crate::project_context::command::ProjectContextArgs::Statuses(args),
                        fields,
                        options,
                        control,
                    )
                }
            },
            |args, fields, control, options| {
                crate::issue_reads::command::execute(args, fields, options, control)
            },
            |args, fields, control, options| {
                crate::field_reads::command::execute(args, fields, options, control)
            },
            |args, fields, control, options| {
                crate::issue_context::command::execute(args, fields, options, control)
            },
            |args, fields, control, options| {
                crate::permission_reads::command::execute(args, fields, options, control)
            },
        ),
    )
}

fn execute_confluence_candidate(
    registry: &Registry,
    action: Action,
    fields: &[String],
    control: &InvocationControl,
    options: &crate::profiles::InvocationOptions<'_>,
    handler: impl FnOnce(
        crate::confluence_reads::command::ConfluenceReadArgs,
        &[String],
        &InvocationControl,
        &crate::profiles::InvocationOptions<'_>,
    ) -> CommandResult,
) -> Result<CommandResult, CraftError> {
    let command = command_id(&action);
    registry.descriptor(command)?;
    registry.validate(command, &argument_value(&action)?)?;
    let Action::Confluence { action } = action else {
        return Err(CraftError::new(
            ErrorCode::Internal,
            "Non-Confluence arguments reached Confluence dispatch.",
        ));
    };
    Ok(handler(action.try_into()?, fields, control, options))
}

fn execute_policy_candidate(
    registry: &Registry,
    action: Action,
    fields: &[String],
    control: &InvocationControl,
    options: &crate::profiles::InvocationOptions<'_>,
    handler: impl FnOnce(
        crate::policies::PolicyArgs,
        &[String],
        &InvocationControl,
        &crate::profiles::InvocationOptions<'_>,
    ) -> CommandResult,
) -> Result<CommandResult, CraftError> {
    let command = command_id(&action);
    registry.validate(command, &argument_value(&action)?)?;
    let args = match action {
        Action::Policy {
            action: crate::args::PolicyAction::Get(v),
        } => crate::policies::PolicyArgs::Get(*v),
        Action::Policy {
            action: crate::args::PolicyAction::Set(v),
        } => crate::policies::PolicyArgs::Set(*v),
        _ => {
            return Err(CraftError::new(
                ErrorCode::Internal,
                "Non-policy arguments reached policy dispatch.",
            ));
        }
    };
    Ok(handler(args, fields, control, options))
}

fn execute_page_write_candidate(
    registry: &Registry,
    action: Action,
    fields: &[String],
    control: &InvocationControl,
    options: &crate::profiles::InvocationOptions<'_>,
    handler: impl FnOnce(
        crate::confluence_pages::command::PageWriteArgs,
        &[String],
        &InvocationControl,
        &crate::profiles::InvocationOptions<'_>,
    ) -> CommandResult,
) -> Result<CommandResult, CraftError> {
    let command = command_id(&action);
    registry.descriptor(command)?;
    registry.validate(command, &argument_value(&action)?)?;
    let Action::Confluence { action } = action else {
        return Err(CraftError::new(
            ErrorCode::Internal,
            "Non-Confluence arguments reached page-write dispatch.",
        ));
    };
    Ok(handler(action.try_into()?, fields, control, options))
}

fn execute_recovery_candidate(
    registry: &Registry,
    action: Action,
    fields: &[String],
    control: &InvocationControl,
    options: &crate::profiles::InvocationOptions<'_>,
    handler: impl FnOnce(
        crate::issue_recovery::command::OperationReconcileArgs,
        &[String],
        &InvocationControl,
        &crate::profiles::InvocationOptions<'_>,
    ) -> CommandResult,
) -> Result<CommandResult, CraftError> {
    let command = command_id(&action);
    registry.descriptor(command)?;
    registry.validate(command, &argument_value(&action)?)?;
    let Action::Operation {
        action: crate::args::OperationAction::Reconcile(args),
    } = action
    else {
        return Err(CraftError::new(
            ErrorCode::Internal,
            "Non-recovery arguments reached recovery dispatch.",
        ));
    };
    Ok(handler(*args, fields, control, options))
}

fn execute_mutation_candidate(
    registry: &Registry,
    action: Action,
    fields: &[String],
    control: &InvocationControl,
    options: &crate::profiles::InvocationOptions<'_>,
    handler: impl FnOnce(
        crate::issue_mutation::command::IssueMutationArgs,
        &[String],
        &InvocationControl,
        &crate::profiles::InvocationOptions<'_>,
    ) -> CommandResult,
) -> Result<CommandResult, CraftError> {
    let command = command_id(&action);
    registry.descriptor(command)?;
    registry.validate(command, &argument_value(&action)?)?;
    let args = match action {
        Action::Jira {
            action:
                crate::args::JiraAction::Issue {
                    action: crate::args::JiraIssueAction::Update(args),
                },
        } => crate::issue_mutation::command::IssueMutationArgs::Edit(*args),
        Action::Jira {
            action:
                crate::args::JiraAction::Issue {
                    action: crate::args::JiraIssueAction::Assign(args),
                },
        } => crate::issue_mutation::command::IssueMutationArgs::Assign(*args),
        Action::Jira {
            action:
                crate::args::JiraAction::Issue {
                    action: crate::args::JiraIssueAction::Unassign(args),
                },
        } => crate::issue_mutation::command::IssueMutationArgs::Unassign(*args),
        Action::Jira {
            action:
                crate::args::JiraAction::Issue {
                    action: crate::args::JiraIssueAction::Delete(args),
                },
        } => crate::issue_mutation::command::IssueMutationArgs::Delete(*args),
        Action::Jira {
            action:
                crate::args::JiraAction::Transition {
                    action: crate::args::JiraTransitionAction::Apply(args),
                },
        } => crate::issue_mutation::command::IssueMutationArgs::Transition(*args),
        _ => {
            return Err(CraftError::new(
                ErrorCode::Internal,
                "Non-mutation arguments reached mutation dispatch.",
            ));
        }
    };
    Ok(handler(args, fields, control, options))
}

fn execute_with_candidates(
    registry: &Registry,
    action: Action,
    fields: &[String],
    control: &InvocationControl,
    options: &crate::profiles::InvocationOptions<'_>,
    stdin_consumed: bool,
    handlers: (
        impl FnOnce(
            crate::issue_create::command::IssueCreateArgs,
            &[String],
            &InvocationControl,
            &crate::profiles::InvocationOptions<'_>,
        ) -> CommandResult,
        impl FnOnce(
            crate::args::JiraProjectAction,
            &[String],
            &InvocationControl,
            &crate::profiles::InvocationOptions<'_>,
        ) -> CommandResult,
        impl FnOnce(
            crate::issue_reads::command::IssueReadArgs,
            &[String],
            &InvocationControl,
            &crate::profiles::InvocationOptions<'_>,
        ) -> CommandResult,
        impl FnOnce(
            crate::field_reads::command::FieldReadArgs,
            &[String],
            &InvocationControl,
            &crate::profiles::InvocationOptions<'_>,
        ) -> CommandResult,
        impl FnOnce(
            crate::issue_context::command::IssueContextArgs,
            &[String],
            &InvocationControl,
            &crate::profiles::InvocationOptions<'_>,
        ) -> CommandResult,
        impl FnOnce(
            crate::permission_reads::command::PermissionCheckArgs,
            &[String],
            &InvocationControl,
            &crate::profiles::InvocationOptions<'_>,
        ) -> CommandResult,
    ),
) -> Result<CommandResult, CraftError> {
    let (create, project_read, issue_read, field_read, issue_context, permissions) = handlers;
    let command = command_id(&action);
    let descriptor = registry.descriptor(command)?;
    let output_policy = if descriptor.effect == EffectClass::Read {
        crate::output::OutputPolicy::Bounded
    } else {
        crate::output::OutputPolicy::PreserveEvidence
    };
    let envelope = match action {
        Action::Jira {
            action:
                crate::args::JiraAction::Transition {
                    action: crate::args::JiraTransitionAction::Apply(_),
                },
        } => {
            return Err(CraftError::new(
                ErrorCode::Internal,
                "Mutation arguments require their own dispatch.",
            ));
        }
        Action::Jira {
            action:
                crate::args::JiraAction::Permission {
                    action: crate::args::JiraPermissionAction::Check(args),
                },
        } => {
            registry.validate(command, &value(&args)?)?;
            return Ok(permissions(args, fields, control, options));
        }
        Action::Jira {
            action:
                crate::args::JiraAction::Metadata {
                    action: crate::args::JiraMetadataAction::Edit(args),
                },
        } => {
            registry.validate(command, &value(&args)?)?;
            return Ok(issue_context(
                crate::issue_context::command::IssueContextArgs::Edit(args),
                fields,
                control,
                options,
            ));
        }
        Action::Jira {
            action:
                crate::args::JiraAction::Transition {
                    action: crate::args::JiraTransitionAction::List(args),
                },
        } => {
            registry.validate(command, &value(&args)?)?;
            return Ok(issue_context(
                crate::issue_context::command::IssueContextArgs::Transitions(args),
                fields,
                control,
                options,
            ));
        }
        Action::Jira {
            action: crate::args::JiraAction::Field { action },
        } => {
            registry.validate(command, &field_arguments(&action)?)?;
            let args = match action {
                crate::args::JiraFieldAction::List(args) => {
                    crate::field_reads::command::FieldReadArgs::List(args)
                }
                crate::args::JiraFieldAction::Search(args) => {
                    crate::field_reads::command::FieldReadArgs::Search(args)
                }
            };
            return Ok(field_read(args, fields, control, options));
        }
        Action::Jira {
            action: crate::args::JiraAction::Project { action },
        } => {
            registry.validate(command, &project_arguments(&action)?)?;
            return Ok(project_read(action, fields, control, options));
        }
        Action::Jira {
            action:
                crate::args::JiraAction::Issue {
                    action: crate::args::JiraIssueAction::Create(arguments),
                },
        } => {
            registry.validate(command, &value(&arguments)?)?;
            return Ok(create(*arguments, fields, control, options));
        }
        Action::Jira {
            action: crate::args::JiraAction::Issue { action },
        } => {
            let arguments = match action {
                crate::args::JiraIssueAction::Get(arguments) => {
                    crate::issue_reads::command::IssueReadArgs::Get(arguments)
                }
                crate::args::JiraIssueAction::Search(arguments) => {
                    crate::issue_reads::command::IssueReadArgs::Search(arguments)
                }
                crate::args::JiraIssueAction::Create(_) => {
                    return Err(CraftError::new(
                        ErrorCode::Internal,
                        "Create arguments reached read dispatch.",
                    ));
                }
                crate::args::JiraIssueAction::Update(_)
                | crate::args::JiraIssueAction::Assign(_)
                | crate::args::JiraIssueAction::Unassign(_)
                | crate::args::JiraIssueAction::Delete(_) => {
                    return Err(CraftError::new(
                        ErrorCode::Internal,
                        "Mutation arguments reached read dispatch.",
                    ));
                }
            };
            registry.validate(command, &argument_value_for_read(&arguments)?)?;
            return Ok(issue_read(arguments, fields, control, options));
        }
        Action::Auth {
            action: crate::args::AuthAction::Provision(arguments),
        } => {
            registry.validate(command, &value(&arguments)?)?;
            crate::provision::execute(arguments, fields, stdin_consumed, options, control)
        }
        Action::Operation {
            action: crate::args::OperationAction::Preview(arguments),
        } => {
            registry.validate(command, &value(&arguments)?)?;
            let result = if fields.is_empty() {
                crate::operation_preview::execute(registry, arguments, control, options)
            } else {
                Err(CraftError::field(
                    "/fields",
                    "preview_returns_complete_data",
                ))
            };
            match result {
                Ok(data) => Envelope::success(command, options.request_id.into(), data),
                Err(error) => Envelope::failure(command, options.request_id.into(), error),
            }
        }
        action => match execute_data(registry, action, fields, control, options) {
            Ok(output) => {
                let mut envelope =
                    Envelope::success(command, options.request_id.into(), output.data);
                envelope.page = output.page;
                envelope
            }
            Err(error) => Envelope::failure(command, options.request_id.into(), error),
        },
    };
    Ok(CommandResult {
        envelope,
        output_policy,
    })
}

fn execute_data(
    registry: &Registry,
    mut action: Action,
    fields: &[String],
    control: &InvocationControl,
    options: &crate::profiles::InvocationOptions<'_>,
) -> Result<DispatchOutput, CraftError> {
    if !crate::args::uses_state_context(command_id(&action))
        && (options.state_root.is_some() || options.profile.is_some())
    {
        return Err(CraftError::field(
            "/context",
            "state_and_profile_options_not_applicable",
        ));
    }
    if let Action::Schema(arguments) = &mut action
        && !fields.is_empty()
    {
        if !arguments.fields.is_empty() {
            return Err(CraftError::field(
                "/fields",
                "fields_in_flags_and_input_are_exclusive",
            ));
        }
        arguments.fields = fields.to_vec();
    }
    let input = argument_value(&action)?;
    registry.validate(command_id(&action), &input)?;
    crate::profiles::validate_input(command_id(&action), &input)?;
    crate::planning::validate_input(command_id(&action), &input)?;
    crate::operations::validate_input(command_id(&action), &input)?;
    let projection = if matches!(&action, Action::Schema(_)) {
        None
    } else {
        registry.projection(command_id(&action), fields)?
    };
    let data = match action {
        Action::Artifact { .. }
        | Action::Scan { .. }
        | Action::Jira { .. }
        | Action::Confluence { .. }
        | Action::Policy { .. }
        | Action::Review { .. } => {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Jira candidates require complete command-host dispatch.",
            ));
        }
        Action::Operation { action } => {
            return crate::operations::execute(action, projection, options, control);
        }
        action @ (Action::Plan { .. } | Action::Design { .. }) => {
            return crate::planning::execute(action, projection, control);
        }
        Action::Auth { action } => {
            return crate::auth::execute(action, projection, options, control);
        }
        Action::Profile { action } => {
            return crate::profiles::execute(registry, action, projection, options, control);
        }
        Action::Version => value(VersionData {
            product: craft_core::PRODUCT.into(),
            version: craft_core::VERSION.into(),
            contract: SCHEMA_VERSION.into(),
        }),
        Action::Command {
            action: CommandAction::Find(arguments),
        } => {
            if arguments.query.len() > 256 || arguments.query.chars().any(char::is_control) {
                return Err(CraftError::field("/query", "expected_bounded_search_text"));
            }
            let words: Vec<_> = arguments
                .query
                .to_lowercase()
                .split_whitespace()
                .map(str::to_owned)
                .collect();
            let found: Vec<_> = registry
                .descriptors()
                .filter(|operation| {
                    let searchable =
                        format!("{} {}", operation.id, operation.description).to_lowercase();
                    words.iter().all(|word| searchable.contains(word))
                })
                .collect();
            value(SearchData {
                more: found.len() > usize::from(arguments.limit),
                matches: found
                    .into_iter()
                    .take(usize::from(arguments.limit))
                    .map(catalog::summary)
                    .collect(),
            })
        }
        Action::Schema(arguments) => schema_data(registry, arguments),
        Action::Input {
            action: InputAction::Validate(arguments),
        } => {
            registry.descriptor(&arguments.command)?;
            let input = read_input(&arguments.file, control)?;
            registry.validate(&arguments.command, &input)?;
            crate::profiles::validate_input(&arguments.command, &input)?;
            crate::planning::validate_input(&arguments.command, &input)?;
            crate::operations::validate_input(&arguments.command, &input)?;
            value(ValidationData {
                command: arguments.command,
                valid: true,
                digest: canonical_digest(&input)?,
                live_conditions_checked: false,
            })
        }
        Action::Provider {
            action: ProviderAction::List,
        } => value(ProvidersData {
            items: registry.providers().cloned().collect(),
        }),
        Action::Capabilities(arguments) => {
            if let Some(command) = &arguments.command {
                registry.descriptor(command)?;
            }
            value(CapabilitiesData {
                items: registry
                    .descriptors()
                    .filter(|operation| {
                        arguments
                            .command
                            .as_ref()
                            .is_none_or(|command| &operation.id == command)
                    })
                    .map(|operation| CapabilitySummary {
                        command: operation.id.clone(),
                        authentication: if operation.provider != "craft" {
                            "service_oauth"
                        } else if operation.id == "auth.provision" {
                            "service_credentials"
                        } else {
                            "none"
                        }
                        .into(),
                        effect: operation.effect,
                        scope_alternatives: operation.scope_alternatives.clone(),
                        permissions: operation.permissions.clone(),
                        tenant_conditions: operation.tenant_conditions.clone(),
                        retry_class: operation.retry_class,
                        reconciliation: operation.reconciliation,
                        availability: Capability {
                            implemented: true,
                            policy_enabled: if operation.provider == "craft" {
                                Availability::NotApplicable
                            } else {
                                Availability::Unknown
                            },
                            credential_scoped: if operation.provider == "craft" {
                                Availability::NotApplicable
                            } else {
                                Availability::Unknown
                            },
                            tenant_supported: if operation.provider == "craft" {
                                Availability::NotApplicable
                            } else {
                                Availability::Unknown
                            },
                            verified: Availability::Unknown,
                        },
                    })
                    .collect(),
            })
        }
        Action::Help(_) => value(HelpData {
            commands: registry.descriptors().map(catalog::summary).collect(),
        }),
    }?;
    let data = match projection {
        Some(projection) => projection.data(&data),
        None => Ok(data),
    }?;
    Ok(DispatchOutput { data, page: None })
}
