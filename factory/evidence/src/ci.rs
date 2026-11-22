use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
#[serde(tag = "forge", rename_all = "snake_case", deny_unknown_fields)]
pub enum LaneSelector {
    Github { workflow_id: u64, job_name: String },
    Bitbucket { step_name: String },
}

impl LaneSelector {
    pub fn valid(&self) -> bool {
        let name = match self {
            Self::Github {
                workflow_id,
                job_name,
            } if *workflow_id > 0 => job_name,
            Self::Bitbucket { step_name } => step_name,
            _ => return false,
        };
        !name.trim().is_empty() && name.len() <= 200 && !name.chars().any(char::is_control)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LaneApproval {
    pub observer: String,
    pub repository: String,
    pub branch: String,
    pub selector: LaneSelector,
    pub manifest_digest: String,
    pub policy_digest: String,
    pub template: serde_json::Value,
    pub follow_branch: bool,
    pub allow_admission: bool,
    pub daily_limit: u32,
    pub token_limit: i64,
    pub evidence: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FailedJob {
    pub repository: String,
    pub branch: String,
    pub commit: String,
    pub workflow_id: u64,
    pub run_id: u64,
    pub run_attempt: u64,
    pub job_id: u64,
    pub job_name: String,
    pub check_app_id: u64,
    pub completed_at: String,
    pub evidence_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FailedStep {
    pub repository: String,
    pub branch: String,
    pub commit: String,
    pub build: u64,
    pub pipeline_uuid: String,
    pub step_uuid: String,
    pub step_name: String,
    pub completed_on: String,
    pub evidence_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum CiFailure {
    Bitbucket(FailedStep),
    Github(FailedJob),
}

impl CiFailure {
    pub fn poll_digest(&self) -> Result<String, serde_json::Error> {
        use sha2::{Digest, Sha256};
        let identity = match self {
            Self::Bitbucket(step) => serde_json::to_vec(&(
                &step.repository,
                &step.pipeline_uuid,
                &step.step_uuid,
                &step.completed_on,
            ))?,
            Self::Github(job) => serde_json::to_vec(&(
                "github",
                job.repository.to_ascii_lowercase(),
                job.run_id,
                job.run_attempt,
                job.job_id,
            ))?,
        };
        Ok(Sha256::digest(identity)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect())
    }

    pub fn repository(&self) -> &str {
        match self {
            Self::Bitbucket(step) => &step.repository,
            Self::Github(job) => &job.repository,
        }
    }

    pub fn commit(&self) -> &str {
        match self {
            Self::Bitbucket(step) => &step.commit,
            Self::Github(job) => &job.commit,
        }
    }

    pub fn evidence_url(&self) -> &str {
        match self {
            Self::Bitbucket(step) => &step.evidence_url,
            Self::Github(job) => &job.evidence_url,
        }
    }

    pub fn pull_request_prefix(&self) -> String {
        match self {
            Self::Bitbucket(step) => {
                format!("https://bitbucket.org/{}/pull-requests/", step.repository)
            }
            Self::Github(job) => format!("https://github.com/{}/pull/", job.repository),
        }
    }
}
