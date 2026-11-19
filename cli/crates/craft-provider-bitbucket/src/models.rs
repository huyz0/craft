use craft_core::contracts::ResourceRef;
use craft_core::error::CraftError;
use craft_core::pagination::PageObservation;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub(crate) fn page_size() -> u16 {
    25
}

pub const MAX_PAGE_SIZE: u16 = 50;

/// A workspace or repository slug: lowercase letters, digits, `-`, `_` or `.`, not starting with
/// punctuation, so it can never become a separator or traversal segment.
pub fn slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_' | b'.')
        })
}

/// Pipeline and step identities are UUIDs. Bitbucket writes them in braces; craft accepts and
/// returns the bare form so no brace reaches a shell or URL by accident.
pub fn bare_uuid(value: &str) -> Option<String> {
    let bare = value.trim_matches(['{', '}']);
    let groups: Vec<&str> = bare.split('-').collect();
    (value.len() <= 38
        && groups.iter().map(|g| g.len()).eq([8, 4, 4, 4, 12])
        && bare.bytes().all(|b| b == b'-' || b.is_ascii_hexdigit()))
    .then(|| bare.to_ascii_lowercase())
}

pub(crate) fn repo_field(value: &str) -> Result<(), CraftError> {
    if slug(value) {
        Ok(())
    } else {
        Err(CraftError::field("/repo", "expected_repository_slug"))
    }
}

pub(crate) fn pull_request_field(value: u64) -> Result<(), CraftError> {
    if (1..=2_147_483_647).contains(&value) {
        Ok(())
    } else {
        Err(CraftError::field(
            "/pull_request_id",
            "expected_positive_pull_request_id",
        ))
    }
}

pub(crate) fn pipeline_field(value: &str) -> Result<(), CraftError> {
    if bare_uuid(value).is_some() {
        Ok(())
    } else {
        Err(CraftError::field(
            "/pipeline_uuid",
            "expected_pipeline_uuid",
        ))
    }
}

pub(crate) fn page_size_field(value: u16) -> Result<(), CraftError> {
    if (1..=MAX_PAGE_SIZE).contains(&value) {
        Ok(())
    } else {
        Err(CraftError::field(
            "/page_size",
            "expected_page_size_1_to_50",
        ))
    }
}

pub(crate) fn branch_field(path: &str, value: &str) -> Result<(), CraftError> {
    if value.trim().is_empty()
        || value.len() > 250
        || value
            .chars()
            .any(|c| c.is_control() || c == '"' || c == '\\')
    {
        return Err(CraftError::field(path, "expected_branch_name"));
    }
    Ok(())
}

/// The branch-name filter becomes part of Bitbucket's query language, so only characters that
/// can appear in a branch name and cannot change the query are allowed.
pub(crate) fn name_filter_field(value: &str) -> Result<(), CraftError> {
    if value.is_empty()
        || value.len() > 100
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'-'))
    {
        return Err(CraftError::field(
            "/name_contains",
            "expected_branch_name_fragment",
        ));
    }
    Ok(())
}

pub const STATES: [&str; 4] = ["OPEN", "MERGED", "DECLINED", "SUPERSEDED"];

pub(crate) fn state_field(value: &str) -> Result<(), CraftError> {
    if STATES.contains(&value) {
        Ok(())
    } else {
        Err(CraftError::field(
            "/state",
            "expected_open_merged_declined_or_superseded",
        ))
    }
}

macro_rules! input {
    ($(#[$meta:meta])* $name:ident { $( $(#[$fmeta:meta])* $field:ident : $ty:ty ),* $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Serialize, Deserialize, JsonSchema)]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            $( $(#[$fmeta])* pub $field: $ty, )*
        }
    };
}

macro_rules! paged_input {
    ($name:ident { $( $(#[$fmeta:meta])* $field:ident : $ty:ty ),* $(,)? }) => {
        input!($name {
            $( $(#[$fmeta])* $field: $ty, )*
            #[serde(default = "page_size")]
            #[schemars(range(min = 1, max = 50))]
            page_size: u16,
        });
    };
}

paged_input!(PageInput {});
input!(RepoInput {
    #[schemars(length(min = 1, max = 100), regex(pattern = "^[a-z0-9][a-z0-9._-]*$"))]
    repo: String,
});
paged_input!(RepoPageInput {
    #[schemars(length(min = 1, max = 100), regex(pattern = "^[a-z0-9][a-z0-9._-]*$"))]
    repo: String,
});
paged_input!(BranchListInput {
    #[schemars(length(min = 1, max = 100), regex(pattern = "^[a-z0-9][a-z0-9._-]*$"))]
    repo: String,
    #[schemars(length(min = 1, max = 100))]
    name_contains: Option<String>,
});
paged_input!(CommitListInput {
    #[schemars(length(min = 1, max = 100), regex(pattern = "^[a-z0-9][a-z0-9._-]*$"))]
    repo: String,
    #[schemars(length(min = 1, max = 250))]
    branch: Option<String>,
});
paged_input!(PullRequestListInput {
    #[schemars(length(min = 1, max = 100), regex(pattern = "^[a-z0-9][a-z0-9._-]*$"))]
    repo: String,
    #[schemars(regex(pattern = "^(OPEN|MERGED|DECLINED|SUPERSEDED)$"))]
    state: Option<String>,
});
input!(PullRequestInput {
    #[schemars(length(min = 1, max = 100), regex(pattern = "^[a-z0-9][a-z0-9._-]*$"))]
    repo: String,
    #[schemars(range(min = 1, max = 2147483647))]
    pull_request_id: u64,
});
paged_input!(PullRequestPageInput {
    #[schemars(length(min = 1, max = 100), regex(pattern = "^[a-z0-9][a-z0-9._-]*$"))]
    repo: String,
    #[schemars(range(min = 1, max = 2147483647))]
    pull_request_id: u64,
});
input!(PipelineInput {
    #[schemars(length(min = 1, max = 100), regex(pattern = "^[a-z0-9][a-z0-9._-]*$"))]
    repo: String,
    #[schemars(length(min = 36, max = 38))]
    pipeline_uuid: String,
});
paged_input!(PipelinePageInput {
    #[schemars(length(min = 1, max = 100), regex(pattern = "^[a-z0-9][a-z0-9._-]*$"))]
    repo: String,
    #[schemars(length(min = 36, max = 38))]
    pipeline_uuid: String,
});

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RepositorySummary {
    /// The repository slug, which is what other commands take as `repo`.
    pub id: String,
    pub uuid: Option<String>,
    pub name: Option<String>,
    pub full_name: Option<String>,
    pub description: Option<String>,
    pub is_private: Option<bool>,
    pub language: Option<String>,
    pub main_branch: Option<String>,
    pub project_key: Option<String>,
    pub created_on: Option<String>,
    pub updated_on: Option<String>,
    pub url: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BranchSummary {
    /// The branch name.
    pub id: String,
    pub commit: Option<String>,
    pub date: Option<String>,
    pub author: Option<String>,
    pub message: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommitSummary {
    /// The full commit hash.
    pub id: String,
    pub message: Option<String>,
    pub date: Option<String>,
    pub author: Option<String>,
    pub parents: Option<Vec<String>>,
    pub url: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ParticipantSummary {
    pub display_name: Option<String>,
    pub role: Option<String>,
    pub approved: Option<bool>,
    pub state: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PullRequestSummary {
    pub id: u64,
    pub title: Option<String>,
    pub description: Option<String>,
    pub state: Option<String>,
    pub author: Option<String>,
    pub source_branch: Option<String>,
    pub source_repository: Option<String>,
    pub destination_branch: Option<String>,
    pub created_on: Option<String>,
    pub updated_on: Option<String>,
    pub comment_count: Option<u32>,
    pub task_count: Option<u32>,
    pub close_source_branch: Option<bool>,
    pub merge_commit: Option<String>,
    pub reviewers: Option<Vec<String>>,
    pub participants: Option<Vec<ParticipantSummary>>,
    pub url: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InlineSummary {
    pub path: Option<String>,
    pub from_line: Option<u32>,
    pub to_line: Option<u32>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentSummary {
    pub id: u64,
    pub text: Option<String>,
    pub author: Option<String>,
    pub created_on: Option<String>,
    pub updated_on: Option<String>,
    pub deleted: Option<bool>,
    pub parent_id: Option<u64>,
    pub inline: Option<InlineSummary>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiffstatSummary {
    /// The changed file's path (the new path, or the old path for a removal).
    pub id: String,
    pub status: Option<String>,
    pub lines_added: Option<u32>,
    pub lines_removed: Option<u32>,
    pub old_path: Option<String>,
    pub new_path: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PipelineSummary {
    /// The pipeline UUID without braces.
    pub id: String,
    pub build_number: Option<u64>,
    pub state: Option<String>,
    pub result: Option<String>,
    pub ref_name: Option<String>,
    pub ref_type: Option<String>,
    pub commit: Option<String>,
    pub creator: Option<String>,
    pub trigger: Option<String>,
    pub created_on: Option<String>,
    pub completed_on: Option<String>,
    pub duration_seconds: Option<u64>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StepSummary {
    /// The step UUID without braces.
    pub id: String,
    pub name: Option<String>,
    pub state: Option<String>,
    pub result: Option<String>,
    pub started_on: Option<String>,
    pub completed_on: Option<String>,
    pub duration_seconds: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetData<T> {
    pub target: ResourceRef,
    pub item: Option<T>,
    pub observed_at: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListData<T> {
    pub target: ResourceRef,
    pub items: Vec<T>,
    pub observation: Option<PageObservation>,
    pub cached: bool,
}
