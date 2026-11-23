pub mod bitbucket_change;
pub mod bitbucket_review;
pub mod build_provenance;
pub mod ci;
pub mod diagnosis;
pub mod dsse;
pub mod github_change;
pub mod github_risk;
pub mod kubernetes_inventory;
pub mod queue_identity;
pub mod release_sources;
pub mod release_trace;
pub mod remediation;

pub fn valid_github_node_id(value: &str) -> bool {
    (1..=256).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_graphic())
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct GithubMergeQueueReadiness {
    pub pull_request_node_id: String,
    pub merge_queue_ruleset_id: u64,
    pub active_ruleset_ids: Vec<u64>,
}

impl GithubMergeQueueReadiness {
    pub fn valid(&self) -> bool {
        valid_github_node_id(&self.pull_request_node_id)
            && self.merge_queue_ruleset_id > 0
            && (1..=8).contains(&self.active_ruleset_ids.len())
            && self.active_ruleset_ids[0] > 0
            && self
                .active_ruleset_ids
                .windows(2)
                .all(|pair| pair[0] < pair[1])
            && self
                .active_ruleset_ids
                .contains(&self.merge_queue_ruleset_id)
    }
}

use sha2::{Digest, Sha256};

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeProposal {
    kind: String,
    location: String,
    body: String,
    evidence: String,
}

impl KnowledgeProposal {
    pub fn valid(&self) -> bool {
        let (prefix, max_bytes) = match self.kind.as_str() {
            "skill" => ("team:", 32768),
            "decision" | "runbook" => ("project:", 4096),
            _ => return false,
        };
        let Some(slug) = self.location.strip_prefix(prefix) else {
            return false;
        };
        (1..=80).contains(&slug.len())
            && slug.as_bytes()[0].is_ascii_alphanumeric()
            && slug.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
            })
            && !self.body.trim().is_empty()
            && self.body.len() <= max_bytes
            && !self
                .body
                .chars()
                .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
            && !self.evidence.trim().is_empty()
            && self.evidence.chars().count() <= 500
            && !self.evidence.chars().any(char::is_control)
    }
}

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn reject<T>(message: impl Into<String>) -> Result<T> {
    Err(message.into().into())
}

pub fn valid_oid(oid: &str) -> bool {
    matches!(oid.len(), 40 | 64)
        && oid
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

pub fn sha256(bytes: &[u8]) -> String {
    let digest: String = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("sha256:{digest}")
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct GithubShadowPolicy {
    pub policy_file_digest: String,
    pub protection_digest: String,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct GithubMergeOutcome {
    pub pull_request: String,
    pub head: String,
    pub pinned_base: String,
    pub destination_branch: String,
    pub result_commit: String,
    pub destination_tip: String,
    pub merged_at: i64,
    pub actor: String,
    pub reference: String,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct BitbucketMergeOutcome {
    pub pull_request: String,
    pub head: String,
    pub pinned_base: String,
    pub destination_branch: String,
    pub repository_uuid: String,
    pub result_commit: String,
    pub merged_at: i64,
    pub actor: String,
    pub reference: String,
}

impl GithubShadowPolicy {
    pub fn digest(&self) -> Option<String> {
        let valid = |digest: &str| {
            digest
                .strip_prefix("sha256:")
                .is_some_and(|hex| hex.len() == 64 && valid_oid(hex))
        };
        (valid(&self.policy_file_digest) && valid(&self.protection_digest)).then(|| {
            sha256(
                format!(
                    "github-shadow-v1\n{}\n{}\n",
                    self.policy_file_digest, self.protection_digest
                )
                .as_bytes(),
            )
        })
    }
}
