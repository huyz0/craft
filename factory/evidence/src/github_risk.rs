use serde::{Deserialize, Serialize};

use crate::{
    queue_identity::{identifier, repository_key},
    release_trace::valid_digest,
};

pub const MAX_COMMITS: u32 = 20;
pub const MAX_FILES: usize = 32;
pub const MAX_LINES: u32 = 2000;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeEvidence {
    pub commits: u32,
    pub paths: Vec<String>,
    pub changed_lines: u32,
}

impl ChangeEvidence {
    pub fn valid(&self) -> bool {
        (1..=MAX_COMMITS).contains(&self.commits)
            && (1..=MAX_LINES).contains(&self.changed_lines)
            && valid_paths(&self.paths)
            && self.paths.windows(2).all(|pair| pair[0] < pair[1])
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EnqueuePolicy {
    pub id: String,
    pub repository: String,
    pub base_branch: String,
    pub publisher: String,
    pub shadow_policy_digest: String,
    pub allowed_paths: Vec<String>,
    pub max_files: u32,
    pub max_commits: u32,
    pub max_changed_lines: u32,
    pub daily_limit: u32,
    pub valid_seconds: u32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub close_standalone_work: bool,
    pub evidence: String,
}

impl EnqueuePolicy {
    pub fn valid(&self) -> bool {
        identifier(&self.id)
            && identifier(&self.publisher)
            && repository_key(&self.repository)
            && valid_text(&self.base_branch, 200)
            && valid_text(&self.evidence, 500)
            && valid_digest(&self.shadow_policy_digest)
            && valid_paths(&self.allowed_paths)
            && self
                .allowed_paths
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                == self.allowed_paths.len()
            && (1..=self.allowed_paths.len() as u32).contains(&self.max_files)
            && (1..=MAX_COMMITS).contains(&self.max_commits)
            && (1..=MAX_LINES).contains(&self.max_changed_lines)
            && (1..=10).contains(&self.daily_limit)
            && (1..=2_592_000).contains(&self.valid_seconds)
    }
}

fn valid_paths(paths: &[String]) -> bool {
    (1..=MAX_FILES).contains(&paths.len())
        && paths.iter().all(|path| {
            valid_text(path, 512)
                && !path.contains(['\\', ':', '*', '?', '[', ']'])
                && path.split('/').all(|part| {
                    !matches!(part, "" | "." | "..") && !part.eq_ignore_ascii_case(".git")
                })
                && path.rsplit('/').next().is_some_and(|name| {
                    !name.eq_ignore_ascii_case(".gitattributes")
                        && !name.eq_ignore_ascii_case(".gitmodules")
                })
        })
}

fn valid_text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}
