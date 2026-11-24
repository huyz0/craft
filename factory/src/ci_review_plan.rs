use std::path::{Path, PathBuf};

use craft_factory_evidence::sha256;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Result, github, order::WorkOrder, read_json, reject, repository, store::Store};

#[derive(clap::Args)]
pub struct Options {
    order: PathBuf,
    #[arg(long)]
    github_policy: PathBuf,
    /// Link or identifier for the operator's approval of this exact plan.
    #[arg(long)]
    reference: String,
}

#[derive(Deserialize, Serialize)]
pub(crate) struct Approval {
    pub(crate) digest: String,
    pub(crate) reference: String,
}

pub(crate) fn approve(state: &Path, options: Options) -> Result<Value> {
    let order: WorkOrder = read_json(options.order)?;
    order.validate()?;
    if order.ci_failure.is_some() {
        return reject("approve the work order template before admitting the repair");
    }
    repository::validate_base(&order)?;
    let (policy, pin) = github::read_pinned_policy(&options.github_policy, None)?;
    policy.validate()?;
    repository::validate_github_remote(&order, &policy.repository)?;
    let reference = options.reference.trim();
    if reference.is_empty() || reference.len() > 500 || reference.chars().any(char::is_control) {
        return reject("provide a nonempty plan approval reference of at most 500 bytes");
    }
    let digest = sensitive_plan_digest(&order, &policy, &pin)?
        .ok_or("the work order does not touch a configured sensitive repair path")?;
    let reference = Store::open(state)?.approve_ci_repair_plan(&digest, reference)?;
    Ok(
        serde_json::json!({"source": "operator_observed", "approval": Approval { digest, reference }}),
    )
}

fn sensitive_plan_digest(
    order: &WorkOrder,
    policy: &github::Policy,
    pin: &str,
) -> Result<Option<String>> {
    if !overlaps(&order.scope, policy.sensitive_repair_paths()) {
        return Ok(None);
    }
    if order.ci_verification_policy_digest.is_none() {
        return reject("sensitive repair scopes require pinned pre-draft verification");
    }
    // Provider admission adds a failure and may derive an order ID. The approval pins
    // the reviewed work, including its exact base and policy, before these are known.
    let mut plan = order.clone();
    plan.id.clear();
    plan.ci_failure = None;
    plan.ci_policy_digest = None;
    plan.ci_anchor = None;
    Ok(Some(sha256(&serde_json::to_vec(&(
        "craft-ci-repair-plan-v1",
        pin,
        plan,
    ))?)))
}

pub(crate) fn require_approval(
    store: &Store,
    order: &WorkOrder,
    policy: &github::Policy,
    pin: &str,
) -> Result<Option<Approval>> {
    let Some(digest) = sensitive_plan_digest(order, policy, pin)? else {
        return Ok(None);
    };
    let reference = store.ci_repair_plan_approval(&digest)?.ok_or(
        "sensitive scope requires a separate operator plan approval for this base, policy and work order",
    )?;
    Ok(Some(Approval { digest, reference }))
}

fn overlaps(scope: &[String], sensitive: &[String]) -> bool {
    fn covers(prefix: &str, path: &str) -> bool {
        path == prefix
            || path
                .strip_prefix(prefix)
                .is_some_and(|tail| tail.starts_with('/'))
    }
    scope.iter().any(|scope| {
        let scope = scope.to_ascii_lowercase();
        sensitive.iter().any(|path| {
            let path = path.to_ascii_lowercase();
            covers(&scope, &path) || covers(&path, &scope)
        })
    })
}
