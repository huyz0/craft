use std::collections::BTreeSet;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use clap::Args;
use craft_factory_evidence::sha256;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::order::{WorkOrder, valid_oid, valid_scope};
use crate::{Result, bitbucket_shadow, reject, repository};

#[derive(Args)]
pub struct Options {
    work_order: PathBuf,
    /// Full commit ID from the candidate handoff.
    #[arg(long)]
    head: String,
    #[arg(long)]
    policy: PathBuf,
    /// SHA-256 digest issued with the approved policy.
    #[arg(long)]
    approved_policy_digest: String,
    #[arg(long)]
    observation: PathBuf,
    /// Compare the exported evidence with current Bitbucket data.
    #[arg(long, conflicts_with = "as_of")]
    native_preflight: bool,
    /// Replay the evaluation at this Unix timestamp.
    #[arg(long)]
    as_of: Option<u64>,
    #[arg(long)]
    pub require_eligible: bool,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    schema_version: u32,
    repository: String,
    base_branch: String,
    protection_revision: String,
    observation_issuer: String,
    approval_reference: String,
    allowed_paths: Vec<String>,
    protected_paths: Vec<String>,
    max_changed_paths: usize,
    max_snapshot_age_seconds: u64,
    max_review_age_seconds: u64,
    minimum_approvals: usize,
    eligible_reviewers: Vec<String>,
    required_checks: Vec<RequiredCheck>,
    native_standard_checks_digest: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RequiredCheck {
    name: String,
    issuer: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    schema_version: u32,
    issuer: String,
    evidence: String,
    policy_digest: String,
    repository: String,
    draft_url: String,
    base_branch: String,
    base: String,
    head: String,
    author: String,
    open: bool,
    draft: bool,
    mergeable: bool,
    protected: bool,
    protection_revision: String,
    observed_at: u64,
    checks: Vec<ObservedCheck>,
    reviews: Vec<ObservedReview>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ObservedCheck {
    name: String,
    issuer: String,
    head: String,
    passed: bool,
    evidence: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ObservedReview {
    actor: String,
    head: String,
    decision: ReviewDecision,
    decided_at: u64,
    evidence: String,
}

#[derive(Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ReviewDecision {
    Approved,
    ChangesRequested,
    Dismissed,
}

fn supplied(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 500 && !value.chars().any(char::is_control)
}

impl Policy {
    fn valid(&self) -> bool {
        let reviewers: BTreeSet<_> = self.eligible_reviewers.iter().collect();
        let checks: BTreeSet<_> = self
            .required_checks
            .iter()
            .map(|c| (&c.name, &c.issuer))
            .collect();
        self.schema_version == 1
            && self.repository.split('/').count() == 2
            && self.repository.split('/').all(|part| {
                !part.is_empty()
                    && part.len() <= 100
                    && part
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            })
            && [
                &self.base_branch,
                &self.protection_revision,
                &self.observation_issuer,
                &self.approval_reference,
            ]
            .into_iter()
            .all(|value| supplied(value))
            && (1..=32).contains(&self.allowed_paths.len())
            && self.protected_paths.len() <= 32
            && self
                .allowed_paths
                .iter()
                .chain(&self.protected_paths)
                .all(|path| path.len() <= 240 && valid_scope(path))
            && (1..=100).contains(&self.max_changed_paths)
            && (1..=900).contains(&self.max_snapshot_age_seconds)
            && (1..=86400).contains(&self.max_review_age_seconds)
            && (1..=32).contains(&reviewers.len())
            && reviewers.len() == self.eligible_reviewers.len()
            && (1..=reviewers.len()).contains(&self.minimum_approvals)
            && reviewers.iter().all(|reviewer| supplied(reviewer))
            && (1..=16).contains(&checks.len())
            && checks.len() == self.required_checks.len()
            && checks
                .iter()
                .all(|(name, issuer)| supplied(name) && supplied(issuer))
            && self
                .native_standard_checks_digest
                .as_deref()
                .is_none_or(|digest| {
                    digest.strip_prefix("sha256:").is_some_and(|value| {
                        value.len() == 64
                            && value
                                .bytes()
                                .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
                    })
                })
    }
}

pub(crate) fn load<T: serde::de::DeserializeOwned>(path: &Path) -> Result<(T, String)> {
    const LIMIT: u64 = 256 * 1024;
    let mut bytes = Vec::new();
    File::open(path)?.take(LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > LIMIT {
        return reject("shadow evaluation input exceeds 256 KiB");
    }
    Ok((serde_json::from_slice(&bytes)?, sha256(&bytes)))
}

fn within(path: &str, prefix: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

pub fn run(options: Options) -> Result<Value> {
    let (order, work_digest): (WorkOrder, _) = load(&options.work_order)?;
    order.validate()?;
    let (policy, policy_digest): (Policy, _) = load(&options.policy)?;
    let (observed, observation_digest): (Observation, _) = load(&options.observation)?;
    let use_native = options.native_preflight || policy.native_standard_checks_digest.is_some();
    if use_native && options.as_of.is_some() {
        return reject("native preflight requires the current clock");
    }
    let as_of = match options.as_of {
        Some(as_of) => as_of,
        None => SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
    };
    if !policy.valid()
        || observed.schema_version != 1
        || !valid_oid(&observed.head)
        || !valid_oid(&options.head)
        || !supplied(&observed.evidence)
        || !supplied(&observed.author)
        || observed.checks.len() > 128
        || observed.reviews.len() > 128
    {
        return reject("invalid shadow merge policy or observation");
    }
    if policy_digest != options.approved_policy_digest {
        return reject("policy content differs from the approved digest");
    }
    repository::validate_base(&order)?;
    repository::validate_bitbucket_remote(&order, &policy.repository)?;

    let mut blockers = Vec::new();
    let prefix = format!("https://bitbucket.org/{}/pull-requests/", policy.repository);
    let same_revision = observed.head == options.head && observed.base == order.base;
    let pull_request = observed
        .draft_url
        .strip_prefix(&prefix)
        .filter(|number| number.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|number| number.parse::<u64>().ok())
        .filter(|id| *id > 0);
    let same_target = same_revision
        && observed.repository == policy.repository
        && observed.base_branch == policy.base_branch;
    if !same_target || pull_request.is_none() {
        blockers.push("wrong_repository_or_revision");
    }
    let same_authority =
        observed.issuer == policy.observation_issuer && observed.policy_digest == policy_digest;
    if !same_authority {
        blockers.push("wrong_observation_authority");
    }
    if !observed.protected || observed.protection_revision != policy.protection_revision {
        blockers.push("branch_protection_mismatch");
    }
    if !observed.open || observed.draft || !observed.mergeable {
        blockers.push("pull_request_not_mergeable");
    }
    if as_of
        .checked_sub(observed.observed_at)
        .is_none_or(|age| age > policy.max_snapshot_age_seconds)
    {
        blockers.push("stale_observation");
    }

    let paths = repository::candidate_paths(&order, &options.head)
        .and_then(|_| repository::candidate_history_paths(&order, &order.base, &options.head));
    let mut changed_path_count = None;
    let mut restricted_paths = Vec::new();
    if let Ok(paths) = paths {
        changed_path_count = Some(paths.len());
        if paths.len() > policy.max_changed_paths {
            blockers.push("change_too_large");
        }
        for path in paths {
            let lower = path.to_ascii_lowercase();
            let protected = [
                ".github",
                ".craft",
                ".agents",
                "bitbucket-pipelines.yml",
                ".gitmodules",
            ]
            .iter()
            .any(|prefix| within(&lower, prefix))
                || lower
                    .split('/')
                    .next_back()
                    .is_some_and(|name| matches!(name, "codeowners" | "agents.md"))
                || policy
                    .protected_paths
                    .iter()
                    .any(|prefix| within(&lower, &prefix.to_ascii_lowercase()));
            if protected
                || !order.allows(&path)
                || !policy
                    .allowed_paths
                    .iter()
                    .any(|prefix| within(&path, prefix))
            {
                restricted_paths.push(path);
            }
        }
        if !restricted_paths.is_empty() {
            blockers.push("restricted_paths");
        }
    } else {
        blockers.push("unverifiable_candidate_history");
    }
    restricted_paths.truncate(20);

    let mut missing_checks = Vec::new();
    let mut passing_checks = Vec::new();
    for required in &policy.required_checks {
        let mut checks = observed
            .checks
            .iter()
            .filter(|check| check.name == required.name && check.issuer == required.issuer);
        if let (Some(check), None) = (checks.next(), checks.next())
            && same_revision
            && check.head == options.head
            && check.passed
            && supplied(&check.evidence)
        {
            passing_checks.push(json!({"name": required.name, "issuer": required.issuer, "evidence": check.evidence}));
        } else {
            missing_checks.push(json!({"name": required.name, "issuer": required.issuer}));
        }
    }
    if !missing_checks.is_empty() {
        blockers.push("required_checks_missing_or_failed");
    }

    let mut review_actors = BTreeSet::new();
    let mut counted_reviewers = BTreeSet::new();
    let mut approval_evidence = Vec::new();
    for review in &observed.reviews {
        if !supplied(&review.actor)
            || !supplied(&review.evidence)
            || !review_actors.insert(&review.actor)
            || review.decided_at > observed.observed_at
        {
            blockers.push("invalid_review_history");
            break;
        }
        if review.actor != observed.author && review.decision == ReviewDecision::ChangesRequested {
            blockers.push("changes_requested");
        }
        if same_revision
            && review.actor != observed.author
            && review.head == options.head
            && review.decision == ReviewDecision::Approved
            && policy.eligible_reviewers.contains(&review.actor)
            && as_of
                .checked_sub(review.decided_at)
                .is_some_and(|age| age <= policy.max_review_age_seconds)
        {
            counted_reviewers.insert(&review.actor);
            approval_evidence.push(json!({"actor": review.actor, "evidence": review.evidence}));
        }
    }
    if counted_reviewers.len() < policy.minimum_approvals {
        blockers.push("insufficient_current_approvals");
    }
    blockers.dedup();
    let mut native_preflight = None;
    let mut native_preflight_completed_at = None;
    if use_native {
        if let Some(pull_request) = pull_request.filter(|_| same_target && same_authority) {
            let target = bitbucket_shadow::Target {
                repository: &policy.repository,
                pull_request,
                head: &options.head,
                base: &order.base,
                base_branch: &policy.base_branch,
                author: &observed.author,
                counted_reviewers: &counted_reviewers,
                minimum_approvals: policy.minimum_approvals,
                standard_checks_digest: policy.native_standard_checks_digest.as_deref(),
            };
            match bitbucket_shadow::observe(&target) {
                Ok(report) => {
                    let completed_at = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
                    native_preflight_completed_at = Some(completed_at);
                    if completed_at
                        .checked_sub(observed.observed_at)
                        .is_none_or(|age| age > policy.max_snapshot_age_seconds)
                    {
                        blockers.push("observation_expired_during_native_preflight");
                    }
                    if observed.reviews.iter().any(|review| {
                        counted_reviewers.contains(&review.actor)
                            && completed_at
                                .checked_sub(review.decided_at)
                                .is_none_or(|age| age > policy.max_review_age_seconds)
                    }) {
                        blockers.push("review_expired_during_native_preflight");
                    }
                    blockers.extend(report.blockers.iter().copied());
                    native_preflight = Some(report);
                }
                Err(_) => blockers.push("native_preflight_unavailable"),
            }
        } else {
            blockers.push("native_preflight_input_mismatch");
        }
    }
    Ok(json!({
        "schema_version": 1, "shadow_eligible": blockers.is_empty(),
        "repository": policy.repository, "work_id": order.id, "head": options.head, "base": order.base,
        "observed_head": observed.head, "observed_base": observed.base,
        "pull_request": observed.draft_url, "policy_approval": policy.approval_reference,
        "as_of": as_of, "observed_at": observed.observed_at, "work_order_digest": work_digest,
        "policy_digest": policy_digest, "observation_digest": observation_digest,
        "evidence": observed.evidence, "blocking_reasons": blockers, "changed_path_count": changed_path_count,
        "restricted_paths": restricted_paths, "missing_checks": missing_checks, "counted_reviewers": counted_reviewers,
        "passing_checks": passing_checks, "approval_evidence": approval_evidence,
        "native_preflight_requested": use_native, "native_preflight": native_preflight,
        "native_preflight_completed_at": native_preflight_completed_at,
    }))
}
