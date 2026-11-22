use std::{
    collections::{BTreeMap, HashSet},
    path::{Component, Path, PathBuf},
};

use craft_factory_evidence::{
    queue_identity::identifier,
    release_trace::{read_evidence, valid_digest},
    sha256,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::{Result, order::ExecutionProfile, reject, series_proposal::SeriesProposal};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Comparison {
    schema_version: u32,
    baseline_profile: Pin,
    challenger_profile: Pin,
    cases: Vec<Case>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Pin {
    path: PathBuf,
    digest: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    task_class: String,
    seed_digest: String,
    baseline: Arm,
    challenger: Arm,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Arm {
    attempts: Vec<Pin>,
    review: Option<Review>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Review {
    proposal_digest: String,
    reviewer: String,
    evidence: String,
    decision: Decision,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum Decision {
    Accepted,
    ChangesRequested,
}

#[derive(Deserialize)]
struct Receipt {
    schema_version: u32,
    run_id: Option<String>,
    state: State,
    seed_digest: String,
    profile_digest: String,
    profile: ExecutionProfile,
    timeout_seconds: u64,
    elapsed_millis: Option<u64>,
    usage: Option<Usage>,
    proposal_digest: Option<String>,
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum State {
    Running,
    Failed,
    Blocked,
    AwaitingApproval,
}

#[derive(Deserialize, Serialize)]
struct Usage {
    input_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}

impl Usage {
    fn total(&self) -> Result<Option<u64>> {
        if matches!((self.input_tokens, self.cached_input_tokens), (Some(input), Some(cached)) if cached > input)
        {
            return reject("coordinator cached usage exceeds input usage");
        }
        self.input_tokens
            .zip(self.output_tokens)
            .map(|(input, output)| {
                input
                    .checked_add(output)
                    .ok_or_else(|| "coordinator usage total exceeds its range".into())
            })
            .transpose()
    }
}

#[derive(Default, Serialize)]
struct Totals {
    cases: usize,
    attempts: usize,
    drafts: usize,
    blocked: usize,
    failed: usize,
    unfinished: usize,
    accepted: usize,
    changes_requested: usize,
    awaiting_review: usize,
    attempts_with_usage: usize,
    attempts_without_usage: usize,
    reported_tokens: u64,
    cases_over_attempt_limit: usize,
}

impl Totals {
    fn add(
        &mut self,
        attempts: &[(Receipt, Option<u64>)],
        review: Option<&Review>,
        limit: u64,
    ) -> Result<()> {
        self.cases += 1;
        self.attempts += attempts.len();
        self.cases_over_attempt_limit += usize::from(attempts.len() as u64 > limit);
        let mut has_draft = false;
        for (receipt, tokens) in attempts {
            match receipt.state {
                State::Running => self.unfinished += 1,
                State::Failed => self.failed += 1,
                State::Blocked => self.blocked += 1,
                State::AwaitingApproval => {
                    self.drafts += 1;
                    has_draft = true;
                }
            }
            if let Some(tokens) = tokens {
                self.attempts_with_usage += 1;
                self.reported_tokens = self
                    .reported_tokens
                    .checked_add(*tokens)
                    .ok_or("combined coordinator usage exceeds its range")?;
            } else {
                self.attempts_without_usage += 1;
            }
        }
        match review.map(|review| &review.decision) {
            Some(Decision::Accepted) => self.accepted += 1,
            Some(Decision::ChangesRequested) => self.changes_requested += 1,
            None if has_draft => self.awaiting_review += 1,
            None => (),
        }
        Ok(())
    }

    fn report(&self) -> Value {
        let complete =
            self.attempts_without_usage == 0 && self.unfinished == 0 && self.awaiting_review == 0;
        let cost = (complete && self.accepted > 0)
            .then(|| self.reported_tokens as f64 / self.accepted as f64);
        json!({"totals": self, "tokens_per_accepted_case": cost})
    }
}

fn read_json<T: DeserializeOwned>(path: &Path, maximum: u64) -> Result<T> {
    let bytes = read_evidence(path, maximum).map_err(|error| error.to_string())?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn confined(root: &Path, path: &Path) -> Result<PathBuf> {
    let path = path.canonicalize()?;
    if !path.starts_with(root) {
        return reject("comparison evidence must resolve inside the manifest directory");
    }
    Ok(path)
}

impl Pin {
    fn read<T: DeserializeOwned>(&self, root: &Path) -> Result<(PathBuf, T)> {
        if self.path.as_os_str().is_empty()
            || self
                .path
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
            || !valid_digest(&self.digest)
        {
            return reject("comparison evidence requires a relative path and SHA-256 digest");
        }
        let path = confined(root, &root.join(&self.path))?;
        let bytes = read_evidence(&path, 16 * 1024).map_err(|error| error.to_string())?;
        if sha256(&bytes) != self.digest {
            return reject("comparison evidence does not match its pinned digest");
        }
        Ok((path, serde_json::from_slice(&bytes)?))
    }
}

#[derive(Default)]
struct Evidence {
    paths: HashSet<PathBuf>,
    run_ids: HashSet<String>,
}

impl Evidence {
    fn arm(
        &mut self,
        root: &Path,
        arm: &Arm,
        seed: &str,
        profile_pin: &Pin,
        profile: &ExecutionProfile,
    ) -> Result<Vec<(Receipt, Option<u64>)>> {
        if !(1..=32).contains(&arm.attempts.len()) {
            return reject("a comparison arm must list 1 to 32 attempts");
        }
        let mut attempts = Vec::with_capacity(arm.attempts.len());
        for pin in &arm.attempts {
            let (path, receipt): (_, Receipt) = pin.read(root)?;
            if receipt.run_id.as_deref().is_some_and(|id| {
                id.len() != 32
                    || !id
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            }) {
                return reject("invalid coordinator run ID");
            }
            let identity = receipt.run_id.as_ref().unwrap_or(&pin.digest);
            if !self.paths.insert(path.clone()) || !self.run_ids.insert(identity.clone()) {
                return reject("a coordinator attempt may only be counted once");
            }
            if receipt.schema_version != 1
                || receipt.seed_digest != seed
                || receipt.profile_digest != profile_pin.digest
                || receipt.profile != *profile
                || !(1..=profile.timeout_seconds).contains(&receipt.timeout_seconds)
                || (receipt.state == State::Running) != receipt.elapsed_millis.is_none()
                || (receipt.state == State::Running && receipt.usage.is_some())
            {
                return reject(
                    "coordinator receipt does not match the case seed, profile, or run state",
                );
            }
            match (receipt.state, &receipt.proposal_digest) {
                (State::AwaitingApproval, Some(digest)) => {
                    let candidate = confined(root, &path.with_file_name("proposal.json"))?;
                    let (_, actual) = SeriesProposal::load(&candidate)?;
                    if actual != *digest {
                        return reject("the saved proposal does not match its coordinator receipt");
                    }
                }
                (State::AwaitingApproval, None) | (_, Some(_)) => {
                    return reject("invalid proposal reference in coordinator receipt");
                }
                _ => (),
            }
            let tokens = receipt
                .usage
                .as_ref()
                .map(Usage::total)
                .transpose()?
                .flatten();
            attempts.push((receipt, tokens));
        }
        if let Some(review) = arm.review.as_ref()
            && (!valid_digest(&review.proposal_digest)
                || !review_text(&review.evidence, 500)
                || !review_text(&review.reviewer, 200)
                || !attempts.iter().any(|(receipt, _)| {
                    receipt.proposal_digest.as_ref() == Some(&review.proposal_digest)
                }))
        {
            return reject(
                "review must identify an exact proposal from the arm and include reviewer evidence",
            );
        }
        Ok(attempts)
    }
}

fn review_text(text: &str, maximum: usize) -> bool {
    !text.trim().is_empty() && text.len() <= maximum && !text.chars().any(char::is_control)
}

fn attempt_rows(attempts: &[(Receipt, Option<u64>)], pins: &[Pin]) -> Vec<Value> {
    attempts.iter().zip(pins).map(|((receipt, tokens), pin)| json!({
        "receipt_digest": pin.digest,
        "run_id": receipt.run_id, "state": receipt.state, "usage": receipt.usage, "reported_tokens": tokens,
        "proposal_digest": receipt.proposal_digest, "timeout_seconds": receipt.timeout_seconds,
        "elapsed_millis": receipt.elapsed_millis,
    })).collect()
}

pub fn compare(path: &Path) -> Result<Value> {
    let path = path.canonicalize()?;
    let root = path
        .parent()
        .ok_or("comparison manifest has no directory")?;
    let input: Comparison = read_json(&path, 256 * 1024)?;
    if input.schema_version != 1 || !(1..=100).contains(&input.cases.len()) {
        return reject("a version 1 comparison requires 1 to 100 paired cases");
    }
    let (_, baseline): (_, ExecutionProfile) = input.baseline_profile.read(root)?;
    let (_, challenger): (_, ExecutionProfile) = input.challenger_profile.read(root)?;
    if !baseline.valid()
        || !challenger.valid()
        || (baseline.model == challenger.model
            && baseline.reasoning_effort == challenger.reasoning_effort
            && baseline.timeout_seconds == challenger.timeout_seconds
            && baseline.max_attempts == challenger.max_attempts)
    {
        return reject(
            "choose valid profiles with different models, effort, timeouts, or attempt limits",
        );
    }
    let mut seen = HashSet::new();
    let mut seeds = HashSet::new();
    let mut evidence = Evidence::default();
    let mut overall = (Totals::default(), Totals::default());
    let mut groups = BTreeMap::<&str, (Totals, Totals)>::new();
    let mut rows = Vec::with_capacity(input.cases.len());
    for case in &input.cases {
        if !identifier(&case.id)
            || !identifier(&case.task_class)
            || !valid_digest(&case.seed_digest)
            || !seen.insert(&case.id)
            || !seeds.insert(&case.seed_digest)
        {
            return reject(
                "comparison cases require unique IDs and seed digests and a valid task class",
            );
        }
        let base = evidence.arm(
            root,
            &case.baseline,
            &case.seed_digest,
            &input.baseline_profile,
            &baseline,
        )?;
        let challenge = evidence.arm(
            root,
            &case.challenger,
            &case.seed_digest,
            &input.challenger_profile,
            &challenger,
        )?;
        let group = groups.entry(&case.task_class).or_default();
        for (totals, group, attempts, arm, profile) in [
            (
                &mut overall.0,
                &mut group.0,
                &base,
                &case.baseline,
                &baseline,
            ),
            (
                &mut overall.1,
                &mut group.1,
                &challenge,
                &case.challenger,
                &challenger,
            ),
        ] {
            totals.add(attempts, arm.review.as_ref(), profile.max_attempts)?;
            group.add(attempts, arm.review.as_ref(), profile.max_attempts)?;
        }
        rows.push(json!({"id": case.id, "task_class": case.task_class, "seed_digest": case.seed_digest,
            "baseline": {"attempts": attempt_rows(&base, &case.baseline.attempts), "review": case.baseline.review},
            "challenger": {"attempts": attempt_rows(&challenge, &case.challenger.attempts), "review": case.challenger.review},
        }));
    }
    let classes: BTreeMap<_, _> = groups
        .iter()
        .map(|(class, (base, challenger))| {
            (
                *class,
                json!({"baseline": base.report(), "challenger": challenger.report()}),
            )
        })
        .collect();
    Ok(
        json!({"schema_version": 1, "baseline_profile": baseline, "challenger_profile": challenger,
            "baseline_profile_digest": input.baseline_profile.digest, "challenger_profile_digest": input.challenger_profile.digest,
            "overall": {"baseline": overall.0.report(), "challenger": overall.1.report()},
            "by_task_class": classes, "cases": rows,
        }),
    )
}
