use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use clap::Args;
use craft_factory_evidence::{dsse, release_trace::read_evidence, sha256};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    Result,
    dependency_update::{self, LOCKFILE, MANIFEST, MAX_BYTES},
    order::{WorkOrder, valid_oid},
    reject, repository,
};

const RUN_PAYLOAD_TYPE: &str = "application/vnd.craft.dependency-compatibility-run+json";
const MAX_EVIDENCE_BYTES: u64 = 128 * 1024;

#[derive(Args)]
pub struct Options {
    #[command(flatten)]
    pub evidence: Inputs,
    /// Exit with failure after writing the report when compatibility remains open.
    #[arg(long)]
    pub require_complete: bool,
}

#[derive(Args)]
pub struct Inputs {
    #[command(flatten)]
    update: dependency_update::IntakeOptions,
    #[arg(long)]
    policy: PathBuf,
    #[arg(long)]
    baseline_run: PathBuf,
    #[arg(long)]
    candidate_run: PathBuf,
}

#[derive(Args)]
pub struct Pins {
    #[arg(long)]
    policy_digest: String,
    #[arg(long)]
    baseline_run_digest: String,
    #[arg(long)]
    candidate_run_digest: String,
}

#[derive(Args)]
pub struct SealOptions {
    run: PathBuf,
    #[arg(long)]
    policy: PathBuf,
    #[arg(long)]
    policy_digest: String,
    /// Repository identity from the trusted CI job.
    #[arg(long)]
    repository: String,
    /// Full checkout commit from the trusted CI job.
    #[arg(long)]
    commit: String,
    /// Protected Ed25519 PKCS#8 DER key file.
    #[arg(long)]
    signing_key: PathBuf,
    #[arg(long)]
    destination: PathBuf,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    schema_version: u32,
    intake_digest: String,
    candidate_head: String,
    verifier_issuer: String,
    harness_digest: String,
    approval_reference: String,
    /// Base64-encoded raw Ed25519 public key. When set, both CI receipts must be signed.
    run_public_key_base64: Option<String>,
    checks: Vec<Check>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Check {
    id: String,
    expectation: Expectation,
}

#[derive(Clone, Copy, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum Expectation {
    Preserve,
    Regression,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunReceipt {
    schema_version: u32,
    repository: String,
    commit: String,
    issuer: String,
    reference: String,
    harness_digest: String,
    lockfile_digest: String,
    completed: bool,
    cases: Vec<CaseResult>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CaseResult {
    id: String,
    outcome: Outcome,
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum Outcome {
    Passed,
    AssertionFailed,
    Error,
    Skipped,
}

#[derive(Serialize)]
struct CheckResult {
    id: String,
    expectation: Expectation,
    baseline: Outcome,
    candidate: Outcome,
    matched: bool,
}

#[derive(Serialize)]
pub(crate) struct Evidence {
    pub(crate) commit: String,
    reference: String,
    lockfile_digest: String,
    pub(crate) receipt_digest: String,
}

#[derive(Serialize)]
pub struct Report {
    schema_version: u32,
    pub(crate) work_order_id: String,
    pub(crate) repository: String,
    pull_request: String,
    updater_head: String,
    pub(crate) policy_digest: String,
    pub(crate) intake_digest: String,
    verifier_issuer: String,
    harness_digest: String,
    approval_reference: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_signing_key_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scanner_signing_key_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) scanner_fresh_through: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) scanner_refresh_digest: Option<String>,
    pub(crate) baseline: Evidence,
    pub(crate) candidate: Evidence,
    checks: Vec<CheckResult>,
    pub compatibility_verified: bool,
}

fn evidence_file(path: &Path) -> Result<Vec<u8>> {
    read_evidence(path, MAX_EVIDENCE_BYTES).map_err(|error| error.to_string().into())
}

fn label(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 500 && !value.chars().any(char::is_control)
}

fn case_id(value: &str) -> bool {
    (1..=100).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn digest(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hash| hash.len() == 64 && valid_oid(hash))
}

fn pinned_run(
    path: &Path,
    policy: &Policy,
    repo: &str,
    head: &str,
    lockfile_digest: String,
    required: &BTreeSet<&str>,
    expected_digest: Option<&str>,
) -> Result<(Evidence, BTreeMap<String, Outcome>)> {
    let bytes = evidence_file(path)?;
    let receipt_digest = sha256(&bytes);
    if expected_digest.is_some_and(|expected| expected != receipt_digest) {
        return reject("compatibility run does not match the approved receipt digest");
    }
    let receipt: RunReceipt = match policy.signing_key()? {
        Some(key) => {
            let payload =
                dsse::verify(&bytes, RUN_PAYLOAD_TYPE, &key).map_err(|error| error.to_string())?;
            serde_json::from_slice(&payload)?
        }
        None => serde_json::from_slice(&bytes)?,
    };
    if receipt.lockfile_digest != lockfile_digest {
        return reject(
            "compatibility run does not match the approved issuer, harness, commit, or lockfile",
        );
    }
    let outcomes = validate_run(&receipt, policy, repo, head, required)?;
    Ok((
        Evidence {
            commit: receipt.commit,
            reference: receipt.reference,
            lockfile_digest,
            receipt_digest,
        },
        outcomes,
    ))
}

fn validate_run(
    receipt: &RunReceipt,
    policy: &Policy,
    repo: &str,
    head: &str,
    required: &BTreeSet<&str>,
) -> Result<BTreeMap<String, Outcome>> {
    if receipt.schema_version != 1
        || !receipt.completed
        || !label(repo)
        || !valid_oid(head)
        || receipt.repository != repo
        || receipt.commit != head
        || receipt.issuer != policy.verifier_issuer
        || !label(&receipt.reference)
        || receipt.harness_digest != policy.harness_digest
        || !digest(&receipt.lockfile_digest)
    {
        return reject(
            "compatibility run does not match the approved issuer, harness, commit, or lockfile",
        );
    }
    if receipt.cases.len() != required.len() {
        return reject("compatibility run must contain one outcome for each approved check");
    }
    let mut outcomes = BTreeMap::new();
    for case in &receipt.cases {
        if !required.contains(case.id.as_str())
            || outcomes.insert(case.id.clone(), case.outcome).is_some()
        {
            return reject("compatibility run contains a duplicate or unapproved check ID");
        }
    }
    Ok(outcomes)
}

fn committed_digest(order: &WorkOrder, head: &str, path: &str) -> Result<String> {
    Ok(sha256(&repository::committed_blob(
        order, head, path, MAX_BYTES,
    )?))
}

pub fn run(options: Inputs) -> Result<Report> {
    evaluate(options, None, None)
}

pub fn run_pinned(options: Inputs, checkout: &Path, pins: &Pins) -> Result<Report> {
    evaluate(options, Some(checkout), Some(pins))
}

fn load_policy(path: &Path, expected: Option<&str>) -> Result<(Policy, String)> {
    let bytes = evidence_file(path)?;
    let policy_digest = sha256(&bytes);
    if expected.is_some_and(|expected| expected != policy_digest) {
        return reject("compatibility policy does not match the approved digest");
    }
    let policy: Policy = serde_json::from_slice(&bytes)?;
    Ok((policy, policy_digest))
}

impl Policy {
    fn required_checks(&self) -> Result<BTreeSet<&str>> {
        let required: BTreeSet<_> = self.checks.iter().map(|check| check.id.as_str()).collect();
        if self.schema_version != 1
            || !valid_oid(&self.candidate_head)
            || !digest(&self.intake_digest)
            || !digest(&self.harness_digest)
            || !label(&self.verifier_issuer)
            || !label(&self.approval_reference)
            || !(1..=40).contains(&self.checks.len())
            || required.len() != self.checks.len()
            || required.iter().any(|id| !case_id(id))
        {
            return reject("invalid dependency compatibility policy");
        }
        Ok(required)
    }

    fn signing_key(&self) -> Result<Option<[u8; 32]>> {
        self.run_public_key_base64
            .as_deref()
            .map(dsse::public_key)
            .transpose()
            .map_err(|error| error.to_string().into())
    }
}

pub fn seal(options: SealOptions) -> Result<serde_json::Value> {
    let (policy, policy_digest) = load_policy(&options.policy, Some(&options.policy_digest))?;
    let required = policy.required_checks()?;
    let public_key = policy
        .signing_key()?
        .ok_or("the compatibility policy must pin a CI signing key")?;
    let payload = evidence_file(&options.run)?;
    let receipt: RunReceipt = serde_json::from_slice(&payload)?;
    validate_run(
        &receipt,
        &policy,
        &options.repository,
        &options.commit,
        &required,
    )?;
    let private_key = read_evidence(&options.signing_key, 1024)
        .map_err(|_| "could not read the protected signing key file")?;
    let envelope = dsse::seal(
        &payload,
        RUN_PAYLOAD_TYPE,
        &private_key,
        &public_key,
        "CI run",
    )
    .map_err(|error| error.to_string())?;
    if envelope.len() as u64 > MAX_EVIDENCE_BYTES {
        return reject("signed compatibility receipt exceeds the permitted byte limit");
    }
    crate::evidence_capture::write_new(&options.destination, &envelope)?;
    Ok(serde_json::json!({
        "schema_version": 1, "receipt": options.destination, "receipt_digest": sha256(&envelope),
        "policy_digest": policy_digest, "commit": receipt.commit, "repository": receipt.repository,
    }))
}

fn evaluate(options: Inputs, checkout: Option<&Path>, pins: Option<&Pins>) -> Result<Report> {
    let (policy, policy_digest) = load_policy(
        &options.policy,
        pins.map(|pins| pins.policy_digest.as_str()),
    )?;
    let required = policy.required_checks()?;
    let run_signing_key_digest = policy.signing_key()?.map(|key| sha256(&key));
    let intake = dependency_update::run_in_checkout(options.update, checkout)?;
    let order = intake
        .work_order
        .ok_or("dependency intake requires scanner review before compatibility evaluation")?;
    if intake.input_digest != policy.intake_digest {
        return reject("compatibility policy was approved for a different dependency intake");
    }
    let proposal = intake.proposal;
    if policy.candidate_head != proposal.head {
        repository::candidate_history_paths(&order, &proposal.head, &policy.candidate_head)?;
    }
    let baseline_lock = committed_digest(&order, &proposal.base, LOCKFILE)?;
    let candidate_lock = committed_digest(&order, &policy.candidate_head, LOCKFILE)?;
    if policy.candidate_head != proposal.head
        && (candidate_lock != committed_digest(&order, &proposal.head, LOCKFILE)?
            || committed_digest(&order, &policy.candidate_head, MANIFEST)?
                != committed_digest(&order, &proposal.head, MANIFEST)?)
    {
        return reject("compatibility follow-up changed the scanned dependency inputs");
    }

    let (baseline, before) = pinned_run(
        &options.baseline_run,
        &policy,
        &proposal.repository,
        &proposal.base,
        baseline_lock,
        &required,
        pins.map(|pins| pins.baseline_run_digest.as_str()),
    )?;
    let (candidate, after) = pinned_run(
        &options.candidate_run,
        &policy,
        &proposal.repository,
        &policy.candidate_head,
        candidate_lock,
        &required,
        pins.map(|pins| pins.candidate_run_digest.as_str()),
    )?;
    if baseline.reference == candidate.reference {
        return reject("baseline and candidate require separate CI run references");
    }
    let checks: Vec<_> = policy
        .checks
        .iter()
        .map(|check| {
            let baseline = before[&check.id];
            let candidate = after[&check.id];
            let expected_baseline = match check.expectation {
                Expectation::Preserve => Outcome::Passed,
                Expectation::Regression => Outcome::AssertionFailed,
            };
            CheckResult {
                id: check.id.clone(),
                expectation: check.expectation,
                baseline,
                candidate,
                matched: baseline == expected_baseline && candidate == Outcome::Passed,
            }
        })
        .collect();
    let complete = checks.iter().all(|check| check.matched);
    Ok(Report {
        schema_version: 1,
        work_order_id: order.id,
        repository: proposal.repository,
        pull_request: proposal.pull_request,
        updater_head: proposal.head,
        policy_digest,
        intake_digest: intake.input_digest,
        verifier_issuer: policy.verifier_issuer,
        harness_digest: policy.harness_digest,
        approval_reference: policy.approval_reference,
        run_signing_key_digest,
        scanner_signing_key_digest: intake.scanner_signing_key_digest,
        scanner_fresh_through: intake.scanner_fresh_through,
        scanner_refresh_digest: intake.scanner_refresh_digest,
        baseline,
        candidate,
        checks,
        compatibility_verified: complete,
    })
}
