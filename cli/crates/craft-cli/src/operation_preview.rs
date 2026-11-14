//! Describes a mutation before apply. A preview never claims, journals or sends a write, is
//! not a lock or an approval, and grants nothing the later apply would not recheck itself.

use craft_core::contracts::{EffectClass, Envelope};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::input::canonical_digest;
use craft_core::registry::{ReconciliationClass, Registry, RetryClass};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::args::{
    Action, JiraAction, JiraAttachmentAction, JiraCommentAction, JiraIssueAction, JiraLinkAction,
};
use crate::profiles::InvocationOptions;

/// Input fields that state what the caller expects to find before a write.
const PRECONDITION_FIELDS: &[&str] = &[
    "expected",
    "expected_revision",
    "expected_assignee",
    "expected_status_id",
    "expected_digest",
];

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoteObservation {
    /// The read command run instead of the write; never a mutating command.
    pub command: String,
    pub ok: bool,
    /// The read's own data or error, unchanged. Comparing it with the preconditions is the
    /// caller's decision; apply compares them again against a fresh read.
    pub result: Value,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewData {
    pub command: String,
    pub valid: bool,
    /// Canonical digest of the complete input, including `apply`, so a changed payload is
    /// visibly a different request.
    pub input_digest: String,
    pub descriptor_digest: String,
    pub effect: EffectClass,
    /// True for trash and permanent deletion.
    pub destructive: bool,
    pub retry_class: RetryClass,
    pub reconciliation: ReconciliationClass,
    pub operation_id: Option<String>,
    /// The supplied precondition fields, by name; empty when the command takes none.
    pub preconditions: Map<String, Value>,
    /// Apply needs `apply: true` (or `--apply`) on the original command.
    pub apply_required: bool,
    pub remote: Option<RemoteObservation>,
    #[schemars(extend("const" = false))]
    pub write_sent: bool,
    #[schemars(extend("const" = false))]
    pub lock_held: bool,
    #[schemars(extend("const" = false))]
    pub approval_granted: bool,
}

fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, CraftError> {
    serde_json::from_value(value)
        .map_err(|_| CraftError::field("/input", "remote_read_input_invalid"))
}

fn field(input: &Value, name: &str) -> Result<Value, CraftError> {
    input
        .get(name)
        .cloned()
        .ok_or_else(|| CraftError::field(format!("/{name}"), "required_for_remote_check"))
}

/// The one read that observes the write's existing target, or None when the write creates
/// something new or acts only on local state.
fn remote_read(command: &str, input: &Value) -> Result<Option<(String, Action)>, CraftError> {
    let mut read = Map::new();
    if let Some(cloud) = input.get("cloud_id") {
        read.insert("cloud_id".into(), cloud.clone());
    }
    let (id, action) = match command {
        "jira.issue.update"
        | "jira.issue.assign"
        | "jira.issue.unassign"
        | "jira.issue.delete"
        | "jira.transition.apply" => {
            read.insert("issue".into(), field(input, "issue")?);
            (
                "jira.issue.get",
                JiraAction::Issue {
                    action: JiraIssueAction::Get(decode(Value::Object(read))?),
                },
            )
        }
        "jira.comment.update" | "jira.comment.delete" => {
            read.insert("issue_id".into(), field(input, "issue_id")?);
            read.insert("comment_id".into(), field(input, "comment_id")?);
            (
                "jira.comment.get",
                JiraAction::Comment {
                    action: JiraCommentAction::Get(decode(Value::Object(read))?),
                },
            )
        }
        "jira.attachment.delete" => {
            read.insert("id".into(), field(input, "attachment_id")?);
            (
                "jira.attachment.get",
                JiraAction::Attachment {
                    action: JiraAttachmentAction::Get(decode(Value::Object(read))?),
                },
            )
        }
        "jira.link.delete" => {
            read.insert("link_id".into(), field(input, "link_id")?);
            (
                "jira.link.get",
                JiraAction::Link {
                    action: JiraLinkAction::Get(decode(Value::Object(read))?),
                },
            )
        }
        _ => return Ok(None),
    };
    Ok(Some((id.into(), Action::Jira { action })))
}

pub fn execute(
    registry: &Registry,
    arguments: crate::args::OperationPreviewArgs,
    control: &InvocationControl,
    options: &InvocationOptions<'_>,
) -> Result<Value, CraftError> {
    let descriptor = registry.descriptor(&arguments.command)?.clone();
    if matches!(descriptor.effect, EffectClass::Read) {
        return Err(CraftError::new(
            ErrorCode::UnsupportedCapability,
            "Preview describes mutating commands; run the read directly.",
        ));
    }
    let input = crate::dispatch::read_input(&arguments.file, control)?;
    registry.validate(&arguments.command, &input)?;
    crate::profiles::validate_input(&arguments.command, &input)?;
    crate::planning::validate_input(&arguments.command, &input)?;
    crate::operations::validate_input(&arguments.command, &input)?;
    // Everything above is local. Remote observation is opt-in and limited to a declared read.
    let remote = if arguments.check_remote {
        let (command, action) = remote_read(&arguments.command, &input)?.ok_or_else(|| {
            CraftError::new(
                ErrorCode::UnsupportedCapability,
                "This command has no existing remote target to observe; apply re-reads its context.",
            )
        })?;
        if registry.descriptor(&command)?.effect != EffectClass::Read {
            return Err(CraftError::new(
                ErrorCode::Internal,
                "Remote preview mapping must select a read.",
            ));
        }
        let observed = crate::dispatch::execute(registry, action, &[], control, options, false)?;
        Some(observation(command, observed.envelope))
    } else {
        if options.profile.is_some() {
            return Err(CraftError::field(
                "/profile",
                "profile_requires_check_remote",
            ));
        }
        None
    };
    let preconditions = PRECONDITION_FIELDS
        .iter()
        .filter_map(|name| {
            input
                .get(*name)
                .map(|value| ((*name).into(), value.clone()))
        })
        .collect();
    serde_json::to_value(PreviewData {
        command: arguments.command,
        valid: true,
        input_digest: canonical_digest(&input)?,
        descriptor_digest: canonical_digest(
            &serde_json::to_value(&descriptor).map_err(|_| {
                CraftError::new(ErrorCode::Internal, "Cannot encode the descriptor.")
            })?,
        )?,
        effect: descriptor.effect,
        destructive: matches!(
            descriptor.effect,
            EffectClass::Trash | EffectClass::Permanent
        ),
        retry_class: descriptor.retry_class,
        reconciliation: descriptor.reconciliation,
        operation_id: input
            .get("operation_id")
            .and_then(Value::as_str)
            .map(str::to_owned),
        preconditions,
        apply_required: true,
        remote,
        write_sent: false,
        lock_held: false,
        approval_granted: false,
    })
    .map_err(|_| CraftError::new(ErrorCode::Internal, "Cannot encode the preview."))
}

fn observation(command: String, envelope: Envelope) -> RemoteObservation {
    let ok = envelope.ok;
    let result = if ok {
        envelope.data.unwrap_or(Value::Null)
    } else {
        envelope
            .error
            .and_then(|error| serde_json::to_value(error).ok())
            .unwrap_or_else(|| json!(null))
    };
    RemoteObservation {
        command,
        ok,
        result,
    }
}
