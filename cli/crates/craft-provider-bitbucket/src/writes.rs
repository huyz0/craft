//! Journaled Bitbucket mutations for agents: branches, pull requests, comments, approvals and
//! the merge or decline of a pull request. Nothing here deletes branches, repositories or
//! comments, and nothing changes workspace, permission, webhook or deployment settings.
//!
//! Each command is one native request. The trusted host claims the send in its journal first and
//! never repeats it; this module only builds the request and interprets the answer.

use crate::models::*;
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
    /// `{workspace}` comes from the target; `{repo}` and `{pull_request_id}` from the input.
    pub path: &'static [&'static str],
    /// The kind of resource a confirmed result names.
    pub resource: &'static str,
    pub description: &'static str,
}

const PR: &[&str] = &[
    "repositories",
    "{workspace}",
    "{repo}",
    "pullrequests",
    "{pull_request_id}",
];

pub const WRITES: &[WriteEntry] = &[
    WriteEntry {
        id: "bitbucket.branch.create",
        method: HttpMethod::Post,
        path: &["repositories", "{workspace}", "{repo}", "refs", "branches"],
        resource: "branch",
        description: "Create a branch at an exact commit.",
    },
    WriteEntry {
        id: "bitbucket.pullrequest.create",
        method: HttpMethod::Post,
        path: &["repositories", "{workspace}", "{repo}", "pullrequests"],
        resource: "pullrequest",
        description: "Open a pull request from an existing branch.",
    },
    WriteEntry {
        id: "bitbucket.pullrequest.update",
        method: HttpMethod::Put,
        path: PR,
        resource: "pullrequest",
        description: "Change a pull request's title, description, destination or close-source setting.",
    },
    WriteEntry {
        id: "bitbucket.comment.add",
        method: HttpMethod::Post,
        path: &[
            "repositories",
            "{workspace}",
            "{repo}",
            "pullrequests",
            "{pull_request_id}",
            "comments",
        ],
        resource: "comment",
        description: "Comment on a pull request: general, inline on a file line, or a reply.",
    },
    WriteEntry {
        id: "bitbucket.pullrequest.approve",
        method: HttpMethod::Post,
        path: &[
            "repositories",
            "{workspace}",
            "{repo}",
            "pullrequests",
            "{pull_request_id}",
            "approve",
        ],
        resource: "pullrequest",
        description: "Approve a pull request as the credential's account.",
    },
    WriteEntry {
        id: "bitbucket.pullrequest.unapprove",
        method: HttpMethod::Delete,
        path: &[
            "repositories",
            "{workspace}",
            "{repo}",
            "pullrequests",
            "{pull_request_id}",
            "approve",
        ],
        resource: "pullrequest",
        description: "Withdraw the credential account's approval.",
    },
    WriteEntry {
        id: "bitbucket.pullrequest.request-changes",
        method: HttpMethod::Post,
        path: &[
            "repositories",
            "{workspace}",
            "{repo}",
            "pullrequests",
            "{pull_request_id}",
            "request-changes",
        ],
        resource: "pullrequest",
        description: "Request changes on a pull request as the credential's account.",
    },
    WriteEntry {
        id: "bitbucket.pullrequest.unrequest-changes",
        method: HttpMethod::Delete,
        path: &[
            "repositories",
            "{workspace}",
            "{repo}",
            "pullrequests",
            "{pull_request_id}",
            "request-changes",
        ],
        resource: "pullrequest",
        description: "Withdraw the credential account's change request.",
    },
    WriteEntry {
        id: "bitbucket.pullrequest.merge",
        method: HttpMethod::Post,
        path: &[
            "repositories",
            "{workspace}",
            "{repo}",
            "pullrequests",
            "{pull_request_id}",
            "merge",
        ],
        resource: "pullrequest",
        description: "Merge a pull request with an explicit strategy. Enable it separately.",
    },
    WriteEntry {
        id: "bitbucket.pullrequest.decline",
        method: HttpMethod::Post,
        path: &[
            "repositories",
            "{workspace}",
            "{repo}",
            "pullrequests",
            "{pull_request_id}",
            "decline",
        ],
        resource: "pullrequest",
        description: "Decline a pull request. Enable it separately.",
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
            .map(
                |part| match part.strip_prefix('{').and_then(|p| p.strip_suffix('}')) {
                    Some(name) => RouteSegment::Parameter(name.into()),
                    None => RouteSegment::Literal((*part).into()),
                },
            )
            .collect(),
        query_parameters: Vec::new(),
        repeated_query_parameters: Vec::new(),
        continuation_parameters: Vec::new(),
        response_max_bytes: 1_048_576,
    }
}

pub const STRATEGIES: [&str; 3] = ["merge_commit", "squash", "fast_forward"];

macro_rules! write_input {
    ($name:ident { $( $(#[$fmeta:meta])* $field:ident : $ty:ty ),* $(,)? }) => {
        #[derive(Clone, Serialize, Deserialize, JsonSchema)]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            $( $(#[$fmeta])* pub $field: $ty, )*
        }
    };
}

write_input!(BranchCreateInput {
    #[schemars(length(min = 1, max = 100), regex(pattern = "^[a-z0-9][a-z0-9._-]*$"))]
    repo: String,
    #[schemars(length(min = 1, max = 250))]
    name: String,
    #[schemars(length(min = 40, max = 64), regex(pattern = "^[0-9a-fA-F]{40,64}$"))]
    source_commit: String,
});
write_input!(PullRequestCreateInput {
    #[schemars(length(min = 1, max = 100), regex(pattern = "^[a-z0-9][a-z0-9._-]*$"))]
    repo: String,
    #[schemars(length(min = 1, max = 255))]
    title: String,
    #[schemars(length(min = 1, max = 250))]
    source_branch: String,
    #[schemars(length(min = 1, max = 250))]
    destination_branch: Option<String>,
    #[schemars(length(max = 32768))]
    description: Option<String>,
    close_source_branch: Option<bool>,
    #[schemars(length(max = 10))]
    reviewer_uuids: Option<Vec<String>>,
});
write_input!(PullRequestUpdateInput {
    #[schemars(length(min = 1, max = 100), regex(pattern = "^[a-z0-9][a-z0-9._-]*$"))]
    repo: String,
    #[schemars(range(min = 1, max = 2147483647))]
    pull_request_id: u64,
    #[schemars(length(min = 1, max = 255))]
    title: Option<String>,
    #[schemars(length(max = 32768))]
    description: Option<String>,
    #[schemars(length(min = 1, max = 250))]
    destination_branch: Option<String>,
    close_source_branch: Option<bool>,
});
write_input!(CommentAddInput {
    #[schemars(length(min = 1, max = 100), regex(pattern = "^[a-z0-9][a-z0-9._-]*$"))]
    repo: String,
    #[schemars(range(min = 1, max = 2147483647))]
    pull_request_id: u64,
    #[schemars(length(min = 1, max = 32768))]
    text: String,
    /// Reply to this comment. Cannot be combined with an inline position.
    #[schemars(range(min = 1))]
    parent_id: Option<u64>,
    /// File path for an inline comment; needs `line`.
    #[schemars(length(min = 1, max = 1024))]
    path: Option<String>,
    /// Line in the new version of `path`.
    #[schemars(range(min = 1, max = 10000000))]
    line: Option<u32>,
});
write_input!(PullRequestDeclineInput {
    #[schemars(length(min = 1, max = 100), regex(pattern = "^[a-z0-9][a-z0-9._-]*$"))]
    repo: String,
    #[schemars(range(min = 1, max = 2147483647))]
    pull_request_id: u64,
    #[schemars(length(max = 1000))]
    reason: Option<String>,
});
write_input!(PullRequestMergeInput {
    #[schemars(length(min = 1, max = 100), regex(pattern = "^[a-z0-9][a-z0-9._-]*$"))]
    repo: String,
    #[schemars(range(min = 1, max = 2147483647))]
    pull_request_id: u64,
    #[schemars(regex(pattern = "^(merge_commit|squash|fast_forward)$"))]
    merge_strategy: String,
    #[schemars(length(max = 4096))]
    message: Option<String>,
    close_source_branch: Option<bool>,
});

fn text(path: &str, value: &str, max: usize, min: usize) -> Result<(), CraftError> {
    if value.len() < min
        || value.len() > max
        || value.contains('\0')
        || (min > 0 && value.trim().is_empty())
    {
        return Err(CraftError::field(path, "expected_bounded_text"));
    }
    Ok(())
}

/// A new branch name must be one Bitbucket's git accepts, so a refusal is not a surprise later.
fn new_branch(path: &str, value: &str) -> Result<(), CraftError> {
    branch_field(path, value)?;
    if value.len() > 250
        || value
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '~' | '^' | ':' | '?' | '*' | '[' | '\\'))
        || value.contains("..")
        || value.contains("//")
        || value.contains("@{")
        || value.starts_with(['/', '.', '-'])
        || value.ends_with(['/', '.'])
        || value.ends_with(".lock")
    {
        return Err(CraftError::field(path, "expected_valid_branch_name"));
    }
    Ok(())
}

fn commit(path: &str, value: &str) -> Result<(), CraftError> {
    if (40..=64).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(CraftError::field(path, "expected_full_commit_hash"))
    }
}

fn parse<T: DeserializeOwned>(request: &CanonicalRequest) -> Result<T, CraftError> {
    serde_json::from_value(request.input.clone())
        .map_err(|_| CraftError::field("/input", "invalid_bitbucket_write_input"))
}

/// Validates the input and builds the single native call. Pure: no state or network.
pub fn build(request: &CanonicalRequest) -> Result<NativeCall, CraftError> {
    validate_target_only(request)?;
    let entry = write_entry(&request.command)
        .ok_or_else(|| CraftError::field("/command", "unknown_bitbucket_write"))?;
    let mut path = BTreeMap::from([("workspace".to_owned(), request.target.tenant.clone())]);
    let mut body = Map::new();
    let mut set = |key: &str, value: Value| {
        body.insert(key.into(), value);
    };
    let pull_request = |repo: &str, id: u64, path: &mut BTreeMap<String, String>| {
        repo_field(repo)?;
        pull_request_field(id)?;
        path.insert("repo".into(), repo.to_owned());
        path.insert("pull_request_id".into(), id.to_string());
        Ok::<(), CraftError>(())
    };
    match request.command.as_str() {
        "bitbucket.branch.create" => {
            let input: BranchCreateInput = parse(request)?;
            repo_field(&input.repo)?;
            new_branch("/name", &input.name)?;
            commit("/source_commit", &input.source_commit)?;
            path.insert("repo".into(), input.repo);
            set("name", json!(input.name));
            set(
                "target",
                json!({"hash": input.source_commit.to_ascii_lowercase()}),
            );
        }
        "bitbucket.pullrequest.create" => {
            let input: PullRequestCreateInput = parse(request)?;
            repo_field(&input.repo)?;
            text("/title", &input.title, 255, 1)?;
            branch_field("/source_branch", &input.source_branch)?;
            path.insert("repo".into(), input.repo);
            set("title", json!(input.title));
            set("source", json!({"branch": {"name": input.source_branch}}));
            if let Some(destination) = &input.destination_branch {
                branch_field("/destination_branch", destination)?;
                set("destination", json!({"branch": {"name": destination}}));
            }
            if let Some(description) = &input.description {
                text("/description", description, 32_768, 0)?;
                set("description", json!(description));
            }
            if let Some(close) = input.close_source_branch {
                set("close_source_branch", json!(close));
            }
            if let Some(reviewers) = &input.reviewer_uuids {
                let mut unique = std::collections::BTreeSet::new();
                let mut mapped = Vec::new();
                for reviewer in reviewers {
                    let uuid = bare_uuid(reviewer).ok_or_else(|| {
                        CraftError::field("/reviewer_uuids", "expected_account_uuid")
                    })?;
                    if !unique.insert(uuid.clone()) {
                        return Err(CraftError::field(
                            "/reviewer_uuids",
                            "expected_unique_uuids",
                        ));
                    }
                    mapped.push(json!({"uuid": format!("{{{uuid}}}")}));
                }
                if mapped.len() > 10 {
                    return Err(CraftError::field("/reviewer_uuids", "expected_at_most_10"));
                }
                set("reviewers", json!(mapped));
            }
        }
        "bitbucket.pullrequest.update" => {
            let input: PullRequestUpdateInput = parse(request)?;
            pull_request(&input.repo, input.pull_request_id, &mut path)?;
            if let Some(title) = &input.title {
                text("/title", title, 255, 1)?;
                set("title", json!(title));
            }
            if let Some(description) = &input.description {
                text("/description", description, 32_768, 0)?;
                set("description", json!(description));
            }
            if let Some(destination) = &input.destination_branch {
                branch_field("/destination_branch", destination)?;
                set("destination", json!({"branch": {"name": destination}}));
            }
            if let Some(close) = input.close_source_branch {
                set("close_source_branch", json!(close));
            }
            if body.is_empty() {
                return Err(CraftError::field("/input", "expected_at_least_one_change"));
            }
        }
        "bitbucket.comment.add" => {
            let input: CommentAddInput = parse(request)?;
            pull_request(&input.repo, input.pull_request_id, &mut path)?;
            text("/text", &input.text, 32_768, 1)?;
            set("content", json!({"raw": input.text}));
            match (&input.path, input.line, input.parent_id) {
                (None, None, None) => {}
                (None, None, Some(parent)) => set("parent", json!({"id": parent})),
                (Some(file), Some(line), None) => {
                    text("/path", file, 1024, 1)?;
                    set("inline", json!({"path": file, "to": line}));
                }
                _ => {
                    return Err(CraftError::field(
                        "/input",
                        "expected_general_inline_with_path_and_line_or_reply",
                    ));
                }
            }
        }
        "bitbucket.pullrequest.approve"
        | "bitbucket.pullrequest.unapprove"
        | "bitbucket.pullrequest.request-changes"
        | "bitbucket.pullrequest.unrequest-changes" => {
            let input: PullRequestInput = parse(request)?;
            pull_request(&input.repo, input.pull_request_id, &mut path)?;
        }
        "bitbucket.pullrequest.merge" => {
            let input: PullRequestMergeInput = parse(request)?;
            pull_request(&input.repo, input.pull_request_id, &mut path)?;
            if !STRATEGIES.contains(&input.merge_strategy.as_str()) {
                return Err(CraftError::field(
                    "/merge_strategy",
                    "expected_merge_commit_squash_or_fast_forward",
                ));
            }
            set("type", json!("pullrequest_merge_commit"));
            set("merge_strategy", json!(input.merge_strategy));
            if let Some(message) = &input.message {
                text("/message", message, 4096, 0)?;
                set("message", json!(message));
            }
            if let Some(close) = input.close_source_branch {
                set("close_source_branch", json!(close));
            }
        }
        "bitbucket.pullrequest.decline" => {
            let input: PullRequestDeclineInput = parse(request)?;
            pull_request(&input.repo, input.pull_request_id, &mut path)?;
            if let Some(reason) = &input.reason {
                text("/reason", reason, 1000, 0)?;
                set("reason", json!(reason));
            }
        }
        _ => return Err(CraftError::field("/command", "unknown_bitbucket_write")),
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

fn validate_target_only(request: &CanonicalRequest) -> Result<(), CraftError> {
    crate::registration::validate_target(&request.target)
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
        provider: "bitbucket".into(),
        deployment: "cloud".into(),
        tenant: tenant.into(),
        resource_type: kind.into(),
        id: id.into(),
    }
}

fn str_at<'a>(body: &'a Value, pointer: &str) -> Option<&'a str> {
    body.pointer(pointer).and_then(Value::as_str)
}

/// Compares the answer with what was asked, so a success that did not apply the change is
/// visible. Commands whose success status alone is the proof return true on that status.
fn desired(command: &str, input: &Value, body: &Value) -> bool {
    let want = |key: &str| input.get(key).and_then(Value::as_str);
    match command {
        "bitbucket.branch.create" => {
            str_at(body, "/name") == want("name")
                && str_at(body, "/target/hash")
                    .zip(want("source_commit"))
                    .is_some_and(|(native, asked)| {
                        let (native, asked) =
                            (native.to_ascii_lowercase(), asked.to_ascii_lowercase());
                        native.starts_with(&asked) || asked.starts_with(&native)
                    })
        }
        "bitbucket.pullrequest.create" => {
            str_at(body, "/title") == want("title")
                && str_at(body, "/source/branch/name") == want("source_branch")
                && want("destination_branch")
                    .is_none_or(|asked| str_at(body, "/destination/branch/name") == Some(asked))
                && str_at(body, "/state") == Some("OPEN")
        }
        "bitbucket.pullrequest.update" => {
            want("title").is_none_or(|asked| str_at(body, "/title") == Some(asked))
                && want("description")
                    .is_none_or(|asked| str_at(body, "/description") == Some(asked))
                && want("destination_branch")
                    .is_none_or(|asked| str_at(body, "/destination/branch/name") == Some(asked))
                && input
                    .get("close_source_branch")
                    .and_then(Value::as_bool)
                    .is_none_or(|asked| {
                        body.get("close_source_branch").and_then(Value::as_bool) == Some(asked)
                    })
        }
        "bitbucket.comment.add" => str_at(body, "/content/raw") == want("text"),
        "bitbucket.pullrequest.approve" => {
            body.get("approved").and_then(Value::as_bool) == Some(true)
        }
        "bitbucket.pullrequest.request-changes" => {
            str_at(body, "/state") == Some("changes_requested")
        }
        "bitbucket.pullrequest.merge" => str_at(body, "/state") == Some("MERGED"),
        "bitbucket.pullrequest.decline" => str_at(body, "/state") == Some("DECLINED"),
        // A DELETE that answered 2xx removed the approval or change request.
        "bitbucket.pullrequest.unapprove" | "bitbucket.pullrequest.unrequest-changes" => true,
        _ => false,
    }
}

fn success_statuses(method: HttpMethod, command: &str) -> &'static [u16] {
    match (method, command) {
        (HttpMethod::Delete, _) => &[204, 200],
        (
            HttpMethod::Post,
            "bitbucket.branch.create" | "bitbucket.pullrequest.create" | "bitbucket.comment.add",
        ) => &[201, 200],
        _ => &[200],
    }
}

/// Interprets the answer to the single sent request. A refusal is `Failed`; anything that
/// leaves the effect unknown (server error, asynchronous acceptance, malformed success) is
/// `Unknown` and never replayed.
pub fn interpret(
    operation: &PreparedOperation,
    response: &NativeResponse,
) -> Result<Observation, CraftError> {
    let request = operation.request();
    let entry = write_entry(&request.command)
        .ok_or_else(|| CraftError::field("/command", "unknown_bitbucket_write"))?;
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
    let status = response.status;
    if success_statuses(entry.method, &request.command).contains(&status) {
        let body = &response.body;
        let requested_pr = request
            .input
            .get("pull_request_id")
            .and_then(Value::as_u64)
            .map(|id| id.to_string());
        // The identity Bitbucket names for the touched resource. Creates name the new object;
        // pull request changes must name the requested pull request when they return one.
        let id = match entry.resource {
            "branch" => str_at(body, "/name").map(str::to_owned),
            "comment" => body
                .get("id")
                .and_then(Value::as_u64)
                .map(|id| id.to_string()),
            _ if request.command == "bitbucket.pullrequest.create" => body
                .get("id")
                .and_then(Value::as_u64)
                .map(|id| id.to_string()),
            _ => match body.get("id").and_then(Value::as_u64) {
                Some(named) if requested_pr.as_deref() != Some(&named.to_string()) => None,
                _ => requested_pr,
            },
        };
        let Some(id) = id else {
            return Ok(unknown(
                "Bitbucket answered success without the expected resource identity.",
            ));
        };
        let verified = desired(&request.command, &request.input, body);
        let mut observation = Observation {
            state: SubmissionState::Confirmed,
            resources: vec![resource(entry.resource, tenant, &id)],
            item: Some(summary(&request.command, &id, body)),
            acknowledged: true,
            desired_fields_verified: verified,
            error: None,
        };
        if !verified {
            observation.error = Some(CraftError::new(
                ErrorCode::PartialApply,
                "Bitbucket acknowledged the request but its answer does not show the requested change; read the resource before any replay.",
            ));
        }
        return Ok(observation);
    }
    Ok(match status {
        400 | 422 => refusal(
            ErrorCode::InputInvalid,
            "Bitbucket refused the request as invalid.",
        ),
        401 => refusal(
            ErrorCode::AuthRequired,
            "Bitbucket rejected the credential.",
        ),
        403 => refusal(
            ErrorCode::PermissionDenied,
            "Bitbucket denied permission for this change.",
        ),
        404 => refusal(
            ErrorCode::InputInvalid,
            "Bitbucket resource is absent or not visible; absence was not established.",
        ),
        409 => refusal(
            ErrorCode::VersionConflict,
            "Bitbucket reports a conflict (for example merge conflicts or a changed pull request); nothing was applied.",
        ),
        429 => refusal(
            ErrorCode::RateLimited,
            "Bitbucket rate limited the request; it was not applied.",
        ),
        202 => unknown(
            "Bitbucket accepted the request asynchronously; read the resource to learn the outcome.",
        ),
        _ => unknown("Bitbucket's answer does not establish whether the change was applied."),
    })
}

/// Minimal projection of the answer, so receipts stay small and carry no provider extras.
fn summary(command: &str, id: &str, body: &Value) -> Value {
    let pick = |pairs: &[(&str, &str)]| {
        let mut out = Map::new();
        out.insert("id".into(), json!(id));
        for (pointer, name) in pairs {
            if let Some(value) = body
                .pointer(pointer)
                .filter(|v| v.is_string() || v.is_boolean())
            {
                out.insert((*name).into(), value.clone());
            }
        }
        Value::Object(out)
    };
    match command {
        "bitbucket.branch.create" => pick(&[("/name", "name"), ("/target/hash", "commit")]),
        "bitbucket.pullrequest.create"
        | "bitbucket.pullrequest.update"
        | "bitbucket.pullrequest.merge"
        | "bitbucket.pullrequest.decline" => pick(&[
            ("/title", "title"),
            ("/state", "state"),
            ("/source/branch/name", "source_branch"),
            ("/destination/branch/name", "destination_branch"),
            ("/links/html/href", "url"),
        ]),
        "bitbucket.comment.add" => pick(&[("/content/raw", "text"), ("/links/html/href", "url")]),
        "bitbucket.pullrequest.approve" => pick(&[("/approved", "approved"), ("/state", "state")]),
        "bitbucket.pullrequest.request-changes" => pick(&[("/state", "state")]),
        _ => pick(&[]),
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
            "Bitbucket writes require the verified journaled host.",
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
    pub commit: Option<String>,
    pub title: Option<String>,
    pub text: Option<String>,
    pub state: Option<String>,
    pub approved: Option<bool>,
    pub source_branch: Option<String>,
    pub destination_branch: Option<String>,
    pub url: Option<String>,
}

fn example(command: &str) -> Value {
    let commit = "0123456789abcdef0123456789abcdef01234567";
    match command {
        "bitbucket.branch.create" => {
            json!({"repo":"service","name":"feature/login","source_commit":commit})
        }
        "bitbucket.pullrequest.create" => json!({
            "repo":"service","title":"Fix login","source_branch":"feature/login",
            "destination_branch":"main"
        }),
        "bitbucket.pullrequest.update" => {
            json!({"repo":"service","pull_request_id":1,"title":"Fix login on Safari"})
        }
        "bitbucket.comment.add" => {
            json!({"repo":"service","pull_request_id":1,"text":"Looks good"})
        }
        "bitbucket.pullrequest.merge" => {
            json!({"repo":"service","pull_request_id":1,"merge_strategy":"squash"})
        }
        _ => json!({"repo":"service","pull_request_id":1}),
    }
}

fn register_write<I: DeserializeOwned + JsonSchema>(
    registry: &mut Registry,
    entry: &WriteEntry,
) -> Result<(), CraftError> {
    let data = json!({"receipt":{"operation_id":"example","request_state":"confirmed","confirmed_resources":[]},"item":null});
    let descriptor = OperationDescriptor {
        id: entry.id.into(),
        provider: "bitbucket".into(),
        contract_version: SCHEMA_VERSION.into(),
        api_revision: "bitbucket-rest-2.0".into(),
        description: entry.description.into(),
        effect: EffectClass::Write,
        input_schema: schema_for::<I>()?,
        response_schema: response_schema::<Value>()?,
        example: example(entry.id),
        response_example: json!(Envelope::success(entry.id, "example".into(), data)),
        projection: None,
        scope_alternatives: vec![vec!["write".into()]],
        permissions: vec!["Bitbucket repository write or pull request rights evaluated by Bitbucket".into()],
        tenant_conditions: vec![
            "Exact workspace bound to the profile; the change is one journaled request and is never replayed automatically."
                .into(),
        ],
        routes: vec![write_route(entry)],
        retry_class: RetryClass::ReconcileBeforeReplay,
        reconciliation: ReconciliationClass::KnownResource,
    };
    registry.register_typed_handler::<I>(descriptor, Arc::new(WriteHandler))
}

pub(crate) fn register(registry: &mut Registry) -> Result<(), CraftError> {
    for entry in WRITES {
        match entry.id {
            "bitbucket.branch.create" => register_write::<BranchCreateInput>(registry, entry)?,
            "bitbucket.pullrequest.create" => {
                register_write::<PullRequestCreateInput>(registry, entry)?
            }
            "bitbucket.pullrequest.update" => {
                register_write::<PullRequestUpdateInput>(registry, entry)?
            }
            "bitbucket.comment.add" => register_write::<CommentAddInput>(registry, entry)?,
            "bitbucket.pullrequest.merge" => {
                register_write::<PullRequestMergeInput>(registry, entry)?
            }
            "bitbucket.pullrequest.decline" => {
                register_write::<PullRequestDeclineInput>(registry, entry)?
            }
            _ => register_write::<PullRequestInput>(registry, entry)?,
        }
    }
    Ok(())
}
