use std::path::{Path, PathBuf};

use craft_factory_evidence::{
    ci::{LaneApproval, LaneSelector},
    queue_identity::{identifier, repository_key},
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{Result, ci_dispatch::Provider, github, order::WorkOrder, reject, repository, twg};

use super::{Queue, QueueRole, ci_template, load_shared_ci_policy, work_text};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    lane: String,
    observer: String,
    order: PathBuf,
    order_digest: String,
    jira_snapshot: Option<PathBuf>,
    policy: PathBuf,
    policy_digest: String,
    source: Source,
    follow_branch: bool,
    allow_admission: bool,
    daily_limit: u32,
    token_limit: i64,
}

#[derive(Deserialize)]
#[serde(tag = "forge", rename_all = "snake_case", deny_unknown_fields)]
enum Source {
    Github { gh: PathBuf },
    Bitbucket { step_name: String },
}

impl Manifest {
    fn load(path: &Path, digest: &str) -> Result<Self> {
        let manifest: Self = load_shared_ci_policy(path, digest)?;
        if manifest.schema_version != 1
            || !identifier(&manifest.lane)
            || !identifier(&manifest.observer)
            || !(1..=10).contains(&manifest.daily_limit)
            || manifest.token_limit < 1
            || !manifest.order.is_absolute()
            || !manifest.policy.is_absolute()
            || manifest
                .jira_snapshot
                .as_ref()
                .is_some_and(|path| !path.is_absolute())
        {
            return reject("invalid shared CI lane manifest");
        }
        Ok(manifest)
    }

    fn inputs(&self) -> Result<(WorkOrder, Provider, String, LaneSelector)> {
        let order = ci_template(
            &self.order,
            &self.order_digest,
            self.jira_snapshot.as_deref(),
        )?;
        let (provider, repo, selector) = match &self.source {
            Source::Github { gh } => {
                let policy: github::Policy =
                    load_shared_ci_policy(&self.policy, &self.policy_digest)?;
                policy.validate()?;
                if !policy.sensitive_repair_paths().is_empty() {
                    return reject(
                        "sensitive CI repair plan approvals require the local GitHub lane",
                    );
                }
                github::validate_gh(gh)?;
                let repair = policy
                    .ci_repair
                    .as_ref()
                    .ok_or("GitHub lane needs an approved repair job")?;
                let selector = LaneSelector::Github {
                    workflow_id: repair.workflow_id,
                    job_name: repair.job_name.clone(),
                };
                let repo = policy.repository.clone();
                (
                    Provider::Github {
                        policy,
                        gh: gh.clone(),
                        digest: self.policy_digest.clone(),
                    },
                    repo,
                    selector,
                )
            }
            Source::Bitbucket { step_name } => {
                let policy: twg::Policy = load_shared_ci_policy(&self.policy, &self.policy_digest)?;
                twg::validate_policy(&policy, 1)?;
                if !policy.required_steps.contains(step_name) {
                    return reject("the Bitbucket lane step must appear in the pinned policy");
                }
                let repo = policy.repository_name();
                (
                    Provider::Bitbucket(policy),
                    repo,
                    LaneSelector::Bitbucket {
                        step_name: step_name.clone(),
                    },
                )
            }
        };
        if !repository_key(&repo) || !selector.valid() || !work_text(provider.branch(), 200) {
            return reject("invalid shared CI repository, branch, or check identity");
        }
        provider.validate_repository(&order)?;
        Ok((order, provider, repo, selector))
    }
}

pub(super) fn approve(path: &Path, digest: &str, evidence: String) -> Result<Value> {
    let manifest = Manifest::load(path, digest)?;
    let (mut order, provider, repository, selector) = manifest.inputs()?;
    if !work_text(&evidence, 500) {
        return reject("approving a CI lane requires a decision reference");
    }
    order.repository = PathBuf::from(&repository);
    let approval = LaneApproval {
        observer: manifest.observer,
        repository,
        branch: provider.branch().to_owned(),
        selector,
        manifest_digest: digest.to_owned(),
        policy_digest: manifest.policy_digest,
        template: serde_json::to_value(order)?,
        follow_branch: manifest.follow_branch,
        allow_admission: manifest.allow_admission,
        daily_limit: manifest.daily_limit,
        token_limit: manifest.token_limit,
        evidence,
    };
    Ok(json!({"action":"approve_ci_lane", "lane":manifest.lane, "approval":approval}))
}

pub(super) fn poll(
    origin: &str,
    ca: Option<&Path>,
    token_file: Option<&Path>,
    path: &Path,
    digest: &str,
    admit: bool,
) -> Result<Value> {
    let manifest = Manifest::load(path, digest)?;
    let (mut order, provider, _, selector) = manifest.inputs()?;
    let anchor = order.base.clone();
    if manifest.follow_branch {
        let head = provider.branch_tip()?;
        repository::validate_ci_head(&order, &head)?;
        order.base = head;
    }
    let step_name = match &selector {
        LaneSelector::Bitbucket { step_name } => Some(step_name.as_str()),
        LaneSelector::Github { .. } => None,
    };
    let failure = provider.observe(&order, step_name)?;
    let queue = Queue::open_with_token_file(origin, ca, QueueRole::Intake, token_file)?;
    let mut response = queue.request(&json!({
        "action":"observe_ci", "lane":manifest.lane, "manifest_digest":digest, "failure":failure, "admit":admit,
    }))?;
    response["source"] =
        json!({"branch":provider.branch(), "anchor":anchor, "observed_head":order.base});
    Ok(response)
}
