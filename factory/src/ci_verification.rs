use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use clap::Args;
use craft_factory_evidence::{dsse, release_trace::read_evidence, sha256};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    Result,
    dependency_compatibility::{Expectation, Outcome},
    evidence_capture, github,
    order::{CiFailure, valid_oid, valid_scope},
    reject, repository,
    review::Draft,
    store::{Receipt, State, Store},
};

const PAYLOAD_TYPE: &str = "application/vnd.craft.ci-repair-run+json";
const LIMIT: u64 = 128 * 1024;

#[derive(Args)]
pub struct PublishOptions {
    id: String,
    #[arg(long)]
    policy: PathBuf,
    #[arg(long)]
    github_policy: PathBuf,
    #[arg(long)]
    baseline_run: PathBuf,
    #[arg(long)]
    candidate_run: PathBuf,
    #[arg(long)]
    gh: PathBuf,
}

#[derive(Args)]
pub struct SealOptions {
    receipt: PathBuf,
    #[arg(long)]
    policy: PathBuf,
    #[arg(long)]
    policy_digest: String,
    /// Protected Ed25519 PKCS#8 DER key held only by the verifier.
    #[arg(long)]
    signing_key: PathBuf,
    #[arg(long)]
    destination: PathBuf,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    schema_version: u32,
    verifier_issuer: String,
    verifier_public_key_base64: String,
    harness_digest: String,
    protected_paths: Vec<String>,
    checks: BTreeMap<String, Expectation>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Run {
    schema_version: u32,
    work_order_id: String,
    repository: String,
    commit: String,
    issuer: String,
    reference: String,
    policy_digest: String,
    harness_digest: String,
    completed: bool,
    checks: Vec<Check>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Check {
    id: String,
    outcome: Outcome,
}

#[derive(Serialize)]
struct Proof {
    policy_digest: String,
    harness_digest: String,
    baseline: String,
    head: String,
    baseline_receipt: String,
    candidate_receipt: String,
    baseline_reference: String,
    candidate_reference: String,
}

fn label(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 500 && !value.chars().any(char::is_control)
}

fn load_policy(path: &Path, pin: &str) -> Result<(Policy, [u8; 32])> {
    let bytes = read_evidence(path, LIMIT).map_err(|e| e.to_string())?;
    if sha256(&bytes) != pin {
        return reject("pre-draft verification policy differs from the approved pin");
    }
    let policy: Policy = serde_json::from_slice(&bytes)?;
    if policy.schema_version != 1
        || !label(&policy.verifier_issuer)
        || !policy
            .harness_digest
            .strip_prefix("sha256:")
            .is_some_and(|hex| hex.len() == 64 && valid_oid(hex))
        || policy.protected_paths.is_empty()
        || policy.protected_paths.len() > 40
        || policy.protected_paths.iter().any(|path| !valid_scope(path))
        || policy.checks.len() > 40
        || policy.checks.keys().any(|id| !label(id))
        || !policy
            .checks
            .values()
            .any(|value| matches!(value, Expectation::Preserve))
        || !policy
            .checks
            .values()
            .any(|value| matches!(value, Expectation::Regression))
    {
        return reject("invalid pre-draft verification policy");
    }
    let key = dsse::public_key(&policy.verifier_public_key_base64).map_err(|e| e.to_string())?;
    Ok((policy, key))
}

fn outcomes<'a>(run: &'a Run, policy: &Policy, pin: &str) -> Result<BTreeMap<&'a str, Outcome>> {
    if run.schema_version != 1
        || !run.completed
        || !valid_oid(&run.commit)
        || !label(&run.work_order_id)
        || !label(&run.repository)
        || !label(&run.reference)
        || run.issuer != policy.verifier_issuer
        || run.policy_digest != pin
        || run.harness_digest != policy.harness_digest
        || run.checks.len() != policy.checks.len()
    {
        return reject(
            "verification run does not match the pinned harness, issuer or approved checks",
        );
    }
    let mut checks = BTreeMap::new();
    for check in &run.checks {
        if !policy.checks.contains_key(&check.id)
            || checks.insert(check.id.as_str(), check.outcome).is_some()
        {
            return reject("duplicate or unexpected verification check");
        }
    }
    Ok(checks)
}

pub fn seal(options: SealOptions) -> Result<Value> {
    let (policy, key) = load_policy(&options.policy, &options.policy_digest)?;
    let bytes = read_evidence(&options.receipt, LIMIT).map_err(|e| e.to_string())?;
    let run: Run = serde_json::from_slice(&bytes)?;
    outcomes(&run, &policy, &options.policy_digest)?;
    let secret = read_evidence(&options.signing_key, 1024)
        .map_err(|_| "could not read the protected verifier signing key")?;
    let envelope = dsse::seal(&bytes, PAYLOAD_TYPE, &secret, &key, "CI repair verifier")
        .map_err(|e| e.to_string())?;
    if envelope.len() as u64 > LIMIT {
        return reject("the signed verification receipt is oversized");
    }
    evidence_capture::write_new(&options.destination, &envelope)?;
    Ok(json!({"receipt": options.destination, "receipt_digest": sha256(&envelope)}))
}

fn verify_pair(
    options: &PublishOptions,
    receipt: &Receipt,
    forge: &github::Policy,
    head: &str,
) -> Result<Proof> {
    let order = &receipt.work_order;
    let pin = order
        .ci_verification_policy_digest
        .as_deref()
        .ok_or("missing pre-admission verification policy pin")?;
    let (policy, key) = load_policy(&options.policy, pin)?;
    for path in repository::candidate_history_paths(order, &order.base, head)? {
        let folded = path.to_ascii_lowercase();
        if forge.protects(&path)
            || policy.protected_paths.iter().any(|prefix| {
                let prefix = prefix.to_ascii_lowercase();
                folded == prefix
                    || folded
                        .strip_prefix(&prefix)
                        .is_some_and(|suffix| suffix.starts_with('/'))
            })
        {
            return reject(format!(
                "candidate history changes a protected check definition: {path}"
            ));
        }
    }
    let read_run = |path: &Path, commit: &str| -> Result<(Run, String)> {
        let envelope = read_evidence(path, LIMIT).map_err(|e| e.to_string())?;
        let payload = dsse::verify(&envelope, PAYLOAD_TYPE, &key).map_err(|e| e.to_string())?;
        let run: Run = serde_json::from_slice(&payload)?;
        if run.work_order_id != order.id
            || run.repository != forge.repository
            || run.commit != commit
        {
            return reject(
                "verification run belongs to a different work order, repository or commit",
            );
        }
        Ok((run, sha256(&envelope)))
    };
    let (before, baseline_receipt) = read_run(&options.baseline_run, &order.base)?;
    let (after, candidate_receipt) = read_run(&options.candidate_run, head)?;
    if before.reference == after.reference {
        return reject("baseline and candidate require distinct verification runs");
    }
    let baseline_checks = outcomes(&before, &policy, pin)?;
    let candidate_checks = outcomes(&after, &policy, pin)?;
    for (id, expectation) in &policy.checks {
        if !expectation.satisfied(baseline_checks[id.as_str()], candidate_checks[id.as_str()]) {
            return reject(format!("paired verification failed for {id}"));
        }
    }
    Ok(Proof {
        policy_digest: pin.to_owned(),
        harness_digest: policy.harness_digest,
        baseline: order.base.clone(),
        head: head.to_owned(),
        baseline_receipt,
        candidate_receipt,
        baseline_reference: before.reference,
        candidate_reference: after.reference,
    })
}

pub fn publish(state: &Path, options: PublishOptions) -> Result<Value> {
    let mut store = Store::open(state)?;
    store.verify_snapshot(&options.id)?;
    let receipt = store.status(&options.id)?;
    if receipt.state != State::Candidate
        || receipt.draft.is_some()
        || receipt.publication_hold.is_some()
    {
        return reject("an unpublished current candidate with no publication hold is required");
    }
    let order = &receipt.work_order;
    let head = receipt.candidate.as_deref().ok_or("missing candidate")?;
    let Some(CiFailure::Github(failure)) = &order.ci_failure else {
        return reject("pre-draft publication requires an admitted GitHub CI repair");
    };
    let forge_bytes = read_evidence(&options.github_policy, LIMIT).map_err(|e| e.to_string())?;
    if order.ci_policy_digest.as_deref() != Some(sha256(&forge_bytes).as_str()) {
        return reject("GitHub policy differs from the admitted policy pin");
    }
    let forge: github::Policy = serde_json::from_slice(&forge_bytes)?;
    forge.validate()?;
    repository::validate_github_remote(order, &forge.repository)?;
    let proof = verify_pair(&options, &receipt, &forge, head)?;
    github::validate_gh(&options.gh)?;
    if github::poll_failed_job(&forge, &options.gh, &order.base)?.as_ref() != Some(failure) {
        return reject("the admitted CI failure is no longer current");
    }
    let branch = format!("craft-factory/verified/{head}");
    github::check_ci_draft_refs(&options.gh, &forge, &order.base, &branch, head)?;
    store.reserve_verified_ci_publication(
        &options.id,
        head,
        &proof.policy_digest,
        &serde_json::to_string(&proof)?,
    )?;
    let body = format!(
        "Paired verification passed for candidate `{head}` against base `{}`. Review the exact diff before approval.",
        order.base
    );
    let draft =
        github::create_verified_ci_draft(&options.gh, &forge, &branch, &order.base, head, &body)?;
    let stored = store.bind_draft(
        &options.id,
        Draft {
            head: head.to_owned(),
            reference: draft,
        },
    )?;
    Ok(json!({"verification": proof, "receipt": stored}))
}
