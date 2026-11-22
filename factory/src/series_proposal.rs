use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fmt::Write as _,
    fs::File,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use craft_factory_evidence::{
    bitbucket_change::valid_slug,
    queue_identity::identifier,
    release_trace::{Target, read_evidence, valid_digest},
    sha256,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    Result, evidence_capture,
    order::{ExecutionProfile, PinnedReference, WorkOrder},
    reject, runner,
};

const PROPOSAL_LIMIT: u64 = 240 * 1024;

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SeriesProposal {
    schema_version: u32,
    id: String,
    owner: String,
    source: PinnedReference,
    compatibility: String,
    rollback: String,
    members: Vec<Member>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Member {
    repository: String,
    observer: String,
    target: Target,
    token_limit: i64,
    work_order: WorkOrder,
    #[serde(default)]
    requires_rollout_proof: bool,
}

#[derive(clap::Args)]
pub struct DraftOptions {
    seed: PathBuf,
    #[arg(long)]
    seed_digest: String,
    #[arg(long)]
    codex: PathBuf,
    #[arg(long)]
    codex_home: PathBuf,
    /// Approved Codex execution profile JSON.
    #[arg(long, requires = "profile_digest")]
    profile: Option<PathBuf>,
    /// SHA-256 of the approved profile file, with the sha256: prefix.
    #[arg(long, requires = "profile")]
    profile_digest: Option<String>,
    /// New directory outside a repository for the draft and its inputs.
    #[arg(long)]
    destination: PathBuf,
    #[arg(long, default_value_t = 180, value_parser = clap::value_parser!(u64).range(1..=900))]
    timeout_seconds: u64,
}

#[derive(clap::Args)]
pub struct ReviewOptions {
    proposal: PathBuf,
    #[command(flatten)]
    seed: SeedGuard,
    /// Print a compact Markdown review against the approved seed.
    #[arg(long, requires = "against_seed")]
    brief: bool,
}

#[derive(clap::Args)]
pub struct SeedGuard {
    /// Approved seed to compare with the proposed work.
    #[arg(long, requires = "seed_digest")]
    against_seed: Option<PathBuf>,
    /// SHA-256 of the approved seed file, with the sha256: prefix.
    #[arg(long, requires = "against_seed")]
    seed_digest: Option<String>,
}

impl SeedGuard {
    fn load(&self) -> Result<Option<(SeriesProposal, String)>> {
        self.against_seed
            .as_deref()
            .zip(self.seed_digest.as_deref())
            .map(|(path, digest)| load_approved_seed(path, digest))
            .transpose()
    }
}

fn load_approved_seed(path: &Path, expected: &str) -> Result<(SeriesProposal, String)> {
    let (seed, digest) = SeriesProposal::load(path)?;
    if digest != expected {
        return reject("series seed does not match the approved digest");
    }
    Ok((seed, digest))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Suggestion {
    outcome: Outcome,
    summary: String,
    compatibility: String,
    rollback: String,
    members: Vec<WorkSuggestion>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum Outcome {
    Proposed,
    Blocked,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WorkSuggestion {
    work_id: String,
    intent: String,
    acceptance: Vec<String>,
}

fn present(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

impl SeriesProposal {
    pub(crate) fn load(path: &Path) -> Result<(Self, String)> {
        let bytes = read_evidence(path, PROPOSAL_LIMIT).map_err(|error| error.to_string())?;
        let proposal: Self = serde_json::from_slice(&bytes)?;
        proposal.validate()?;
        Ok((proposal, sha256(&bytes)))
    }

    fn validate(&self) -> Result<()> {
        if self.schema_version != 1
            || !identifier(&self.id)
            || !present(&self.owner, 200)
            || !present(&self.source.location, 500)
            || !present(&self.source.revision, 200)
            || !present(&self.compatibility, 2000)
            || !present(&self.rollback, 2000)
            || !(2..=12).contains(&self.members.len())
        {
            return reject(
                "a series proposal needs a pinned source, owner, compatibility and rollback plans, and 2 to 12 members",
            );
        }
        let mut repositories = HashSet::new();
        let mut work_ids = HashSet::new();
        for (index, member) in self.members.iter().enumerate() {
            let order = &member.work_order;
            order.validate_shared().map_err(|error| {
                format!(
                    "series member {} has an invalid work order: {error}",
                    index + 1
                )
            })?;
            if !member
                .repository
                .split_once('/')
                .is_some_and(|(owner, repo)| valid_slug(owner) && valid_slug(repo))
                || member.repository.len() > 200
                || order.repository.to_str() != Some(member.repository.as_str())
                || order
                    .ci_failure
                    .as_ref()
                    .is_some_and(|failure| failure.repository() != member.repository)
                || !present(&order.owner, 200)
                || !identifier(&member.observer)
                || member.token_limit <= 0
                || (order.dependency_intake_digest.is_some() && !member.requires_rollout_proof)
                || [
                    &member.target.service,
                    &member.target.environment,
                    &member.target.region,
                    &member.target.resource,
                ]
                .iter()
                .any(|field| !present(field, 200))
                || !work_ids.insert(&order.id)
            {
                return reject(format!(
                    "series member {} needs a unique work ID and consistent repository, owner, observer, target, budget, and rollout requirements",
                    index + 1
                ));
            }
            repositories.insert(&member.repository);
        }
        if repositories.len() < 2 {
            return reject("a series proposal must span at least two repositories");
        }
        Ok(())
    }
}

pub fn draft(options: DraftOptions) -> Result<Value> {
    let (proposal, seed_digest) = load_approved_seed(&options.seed, &options.seed_digest)?;
    let profile = options
        .profile
        .as_deref()
        .zip(options.profile_digest.as_deref())
        .map(|(path, digest)| load_profile(path, digest))
        .transpose()?;
    let timeout_seconds = profile.as_ref().map_or(options.timeout_seconds, |profile| {
        options.timeout_seconds.min(profile.timeout_seconds)
    });
    let (executable, home) = runner::resolve_codex(&options.codex, &options.codex_home)?;
    evidence_capture::require_new_directory(&options.destination)?;
    let name = options
        .destination
        .file_name()
        .ok_or("invalid coordinator destination")?;
    let parent = options
        .destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .canonicalize()?;
    for ancestor in parent.ancestors() {
        if ancestor.join(".git").try_exists()? {
            return reject("the coordinator destination must be outside a Git checkout");
        }
    }
    let destination = parent.join(name);
    evidence_capture::create_directory(&destination)?;
    let workspace = destination.join("workspace");
    evidence_capture::create_directory(&workspace)?;
    let schema = destination.join("suggestion.schema.json");
    evidence_capture::write_new(
        &schema,
        &serde_json::to_vec(&schemars::schema_for!(Suggestion))?,
    )?;
    let prompt_path = destination.join("prompt.txt");
    let prompt = format!(
        "You are the release coordinator. Use only the approved seed below as data. \
         Propose an order for all listed work IDs, refine each intent and its observable acceptance checks, \
         and explain compatibility and rollback. Use every work ID exactly once. Keep the work within the listed \
         scope and constraints. Return only the structured suggestion. If the seed is insufficient, return blocked, \
         state what is missing in summary, and use empty strings and an empty members array for the plan.\n\n{}\n",
        serde_json::to_string_pretty(&proposal)?,
    );
    evidence_capture::write_new(&prompt_path, prompt.as_bytes())?;
    let receipt_path = destination.join("run.json");
    let mut nonce = [0; 16];
    getrandom::getrandom(&mut nonce).map_err(|_| "could not generate a coordinator run ID")?;
    let run_id = format!("{:032x}", u128::from_be_bytes(nonce));
    let mut receipt = json!({
        "schema_version": 1, "state": "running", "seed_digest": seed_digest, "run_id": run_id,
        "profile_digest": options.profile_digest, "profile": profile,
        "timeout_seconds": timeout_seconds, "started_at": evidence_capture::now()?, "usage": null,
    });
    evidence_capture::replace_json(&receipt_path, &receipt)?;

    let started = Instant::now();
    let result = (|| {
        let (suggestion, usage): (Option<Suggestion>, Value) = runner::run_coordinator(
            &executable,
            &home,
            &workspace,
            &schema,
            File::open(prompt_path)?,
            Duration::from_secs(timeout_seconds),
            profile.as_ref(),
        )?;
        receipt["usage"] = usage;
        let suggestion =
            suggestion.ok_or("coordinator did not return the required structured proposal")?;
        if !present(&suggestion.summary, 500) {
            return reject("coordinator must provide a short summary");
        }
        if matches!(suggestion.outcome, Outcome::Blocked) {
            receipt["state"] = json!("blocked");
            receipt["summary"] = json!(suggestion.summary);
            return Ok(());
        }
        let summary = suggestion.summary.clone();
        let bytes = apply_suggestion(proposal, suggestion)?;
        let path = destination.join("proposal.json");
        evidence_capture::write_new(&path, &bytes)?;
        receipt["state"] = json!("awaiting_approval");
        receipt["summary"] = json!(summary);
        receipt["proposal_digest"] = json!(sha256(&bytes));
        receipt["proposal"] = json!(path);
        Ok(())
    })();
    if result.is_err() {
        receipt["state"] = json!("failed");
    }
    receipt["elapsed_millis"] = json!(started.elapsed().as_millis() as u64);
    let saved = evidence_capture::replace_json(&receipt_path, &receipt);
    result.and(saved)?;
    Ok(receipt)
}

fn load_profile(path: &Path, expected: &str) -> Result<ExecutionProfile> {
    let bytes = read_evidence(path, 4096).map_err(|error| error.to_string())?;
    if sha256(&bytes) != expected {
        return reject("coordinator profile does not match the approved digest");
    }
    let profile: ExecutionProfile = serde_json::from_slice(&bytes)?;
    if !profile.valid() {
        return reject("invalid approved coordinator execution profile");
    }
    Ok(profile)
}

fn apply_suggestion(mut proposal: SeriesProposal, suggestion: Suggestion) -> Result<Vec<u8>> {
    if suggestion.members.len() != proposal.members.len() {
        return reject("coordinator must include each approved work ID exactly once");
    }
    let mut approved: HashMap<_, _> = proposal
        .members
        .into_iter()
        .map(|member| (member.work_order.id.clone(), member))
        .collect();
    let mut members = Vec::with_capacity(suggestion.members.len());
    for work in suggestion.members {
        let mut member = approved
            .remove(&work.work_id)
            .ok_or("coordinator returned an unknown or repeated work ID")?;
        member.work_order.intent = work.intent;
        member.work_order.acceptance = work.acceptance;
        members.push(member);
    }
    proposal.members = members;
    proposal.compatibility = suggestion.compatibility;
    proposal.rollback = suggestion.rollback;
    proposal.validate()?;
    let mut bytes = serde_json::to_vec_pretty(&proposal)?;
    bytes.push(b'\n');
    if bytes.len() as u64 > PROPOSAL_LIMIT {
        return reject("coordinator proposal exceeds its byte limit");
    }
    Ok(bytes)
}

pub fn review(options: ReviewOptions) -> Result<()> {
    let (proposal, digest) = SeriesProposal::load(&options.proposal)?;
    let seed = options.seed.load()?;
    let changes = seed
        .as_ref()
        .map(|(seed, digest)| compare_seed(seed, &proposal, digest))
        .transpose()?;
    if options.brief {
        let changes = changes
            .as_ref()
            .ok_or("a review brief requires an approved seed")?;
        let brief = render_brief(&proposal, &digest, changes)?;
        std::io::Write::write_all(&mut std::io::stdout().lock(), brief.as_bytes())?;
    } else {
        let mut result = review_report(&proposal, &digest);
        if let Some(changes) = changes.as_ref() {
            result["seed_review"] = serde_json::to_value(changes)?;
        }
        crate::print_json(&result)?;
    }
    if changes.is_some_and(|changes| !changes.seed_compatible) {
        return reject(
            "proposal changes approved constraints; review a revised seed before registration",
        );
    }
    Ok(())
}

fn review_report(proposal: &SeriesProposal, digest: &str) -> Value {
    let members: Vec<_> = proposal
        .members
        .iter()
        .enumerate()
        .map(|(index, member)| {
            let waits_for = index
                .checked_sub(1)
                .map(|previous| &proposal.members[previous].work_order.id);
            json!({"position": index + 1, "waits_for": waits_for, "member": member})
        })
        .collect();
    json!({
        "schema_version": 1, "state": "awaiting_approval", "proposal_digest": digest,
        "id": proposal.id, "owner": proposal.owner, "source": proposal.source,
        "compatibility": proposal.compatibility, "rollback": proposal.rollback, "members": members,
    })
}

pub fn registration(
    path: &Path,
    expected: &str,
    reference: &str,
    guard: &SeedGuard,
) -> Result<Value> {
    if !valid_digest(expected) || !present(reference, 400) {
        return reject(
            "registration requires the approved proposal SHA-256 and approval reference",
        );
    }
    let (proposal, digest) = SeriesProposal::load(path)?;
    if digest != expected {
        return reject("series proposal changed after approval");
    }
    let mut approval = format!("{reference} [proposal {digest}]");
    if let Some((seed, seed_digest)) = guard.load()? {
        if !compare_seed(&seed, &proposal, &seed_digest)?.seed_compatible {
            return reject(
                "proposal changes approved constraints; review a revised seed before registration",
            );
        }
        write!(approval, " [seed {seed_digest}]")?;
        if approval.len() > 500 {
            return reject("the approval reference is too long to record with both digests");
        }
    }
    Ok(json!({
        "action": "register", "id": proposal.id, "owner": proposal.owner,
        "approval": approval, "members": proposal.members,
    }))
}

#[derive(Serialize)]
struct SeedReview {
    seed_digest: String,
    seed_compatible: bool,
    locked_fields: Vec<&'static str>,
    compatibility_changed: bool,
    rollback_changed: bool,
    members: Vec<MemberChange>,
}

#[derive(Serialize)]
struct MemberChange {
    work_id: String,
    previous_position: Option<usize>,
    position: Option<usize>,
    intent_changed: bool,
    acceptance_changed: bool,
    locked_fields: Vec<String>,
}

fn compare_seed(
    seed: &SeriesProposal,
    proposal: &SeriesProposal,
    digest: &str,
) -> Result<SeedReview> {
    let mut locked_fields = Vec::new();
    for (changed, name) in [
        (seed.id != proposal.id, "id"),
        (seed.owner != proposal.owner, "owner"),
        (seed.source != proposal.source, "source"),
    ] {
        if changed {
            locked_fields.push(name);
        }
    }
    let mut remaining: HashMap<_, _> = seed
        .members
        .iter()
        .enumerate()
        .map(|(index, member)| (member.work_order.id.as_str(), (index + 1, member)))
        .collect();
    let mut members = Vec::new();
    for (index, current) in proposal.members.iter().enumerate() {
        let mut change = MemberChange {
            work_id: current.work_order.id.clone(),
            previous_position: None,
            position: Some(index + 1),
            intent_changed: true,
            acceptance_changed: true,
            locked_fields: Vec::new(),
        };
        if let Some((position, previous)) = remaining.remove(current.work_order.id.as_str()) {
            change.previous_position = Some(position);
            change.intent_changed = previous.work_order.intent != current.work_order.intent;
            change.acceptance_changed =
                previous.work_order.acceptance != current.work_order.acceptance;
            change.locked_fields = changed_fields(
                &serde_json::to_value(previous)?,
                &serde_json::to_value(current)?,
            );
        } else {
            change.locked_fields.push("member_added".into());
        }
        members.push(change);
    }
    for (index, member) in seed.members.iter().enumerate() {
        if remaining.contains_key(member.work_order.id.as_str()) {
            members.push(MemberChange {
                work_id: member.work_order.id.clone(),
                previous_position: Some(index + 1),
                position: None,
                intent_changed: false,
                acceptance_changed: false,
                locked_fields: vec!["member_removed".into()],
            });
        }
    }
    let seed_compatible =
        locked_fields.is_empty() && members.iter().all(|member| member.locked_fields.is_empty());
    Ok(SeedReview {
        seed_digest: digest.into(),
        seed_compatible,
        locked_fields,
        compatibility_changed: seed.compatibility != proposal.compatibility,
        rollback_changed: seed.rollback != proposal.rollback,
        members,
    })
}

fn changed_fields(previous: &Value, current: &Value) -> Vec<String> {
    fn keys<'a>(left: &'a Value, right: &'a Value) -> BTreeSet<&'a str> {
        [left, right]
            .into_iter()
            .flat_map(Value::as_object)
            .flat_map(|object| object.keys().map(String::as_str))
            .collect()
    }
    let mut changed = Vec::new();
    for key in keys(previous, current) {
        if key == "work_order" {
            let left = &previous[key];
            let right = &current[key];
            for field in keys(left, right) {
                if !matches!(field, "intent" | "acceptance") && left.get(field) != right.get(field)
                {
                    changed.push(format!("work_order.{field}"));
                }
            }
        } else if previous.get(key) != current.get(key) {
            changed.push(key.into());
        }
    }
    changed
}

fn snippet(text: &str) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut characters = normalized.chars().filter(|ch| {
        !ch.is_control() && !matches!(ch, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
    });
    let mut result = String::new();
    for ch in characters.by_ref().take(180) {
        if matches!(
            ch,
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '#' | '!' | '|' | '~'
        ) {
            result.push('\\');
        }
        result.push(ch);
    }
    if characters.next().is_some() {
        result.push('…');
    }
    result
}

fn render_brief(proposal: &SeriesProposal, digest: &str, changes: &SeedReview) -> Result<String> {
    let mut text = format!(
        "# Release proposal `{}`\n\nProposal `{digest}`\n\nApproved seed `{}`\n\n",
        proposal.id, changes.seed_digest
    );
    if changes.seed_compatible {
        text.push_str("Approved constraints match. Review the proposed sequence and changes before signing off.\n\n");
    } else {
        text.push_str(
            "**Needs updated seed approval.** The proposal changes approved constraints.\n\n",
        );
        if !changes.locked_fields.is_empty() {
            writeln!(
                text,
                "Changed series fields — `{}`.\n",
                changes.locked_fields.join("`, `")
            )?;
        }
    }
    if changes.compatibility_changed {
        writeln!(
            text,
            "**Compatibility** — {}\n",
            snippet(&proposal.compatibility)
        )?;
    }
    if changes.rollback_changed {
        writeln!(text, "**Rollback** — {}\n", snippet(&proposal.rollback))?;
    }
    text.push_str("## Proposed sequence\n\n");
    for (member, change) in proposal.members.iter().zip(&changes.members) {
        let position = change.position.ok_or("missing proposal position")?;
        write!(
            text,
            "{position}. `{}` in `{}`",
            change.work_id, member.repository
        )?;
        if let Some(previous) = change
            .previous_position
            .filter(|previous| *previous != position)
        {
            write!(text, " (was {previous})")?;
        }
        text.push('\n');
        if !change.locked_fields.is_empty() {
            writeln!(
                text,
                "    - Changed approved fields — `{}`.",
                change.locked_fields.join("`, `")
            )?;
        }
        if change.intent_changed {
            writeln!(
                text,
                "    - Intent — {}",
                snippet(&member.work_order.intent)
            )?;
        }
        if change.acceptance_changed {
            for criterion in member.work_order.acceptance.iter().take(3) {
                writeln!(text, "    - Acceptance — {}", snippet(criterion))?;
            }
            if member.work_order.acceptance.len() > 3 {
                writeln!(
                    text,
                    "    - {} more acceptance criteria in the full proposal.",
                    member.work_order.acceptance.len() - 3
                )?;
            }
        }
    }
    for change in changes
        .members
        .iter()
        .filter(|change| change.position.is_none())
    {
        writeln!(
            text,
            "\nRemoved from the approved sequence — `{}`.",
            change.work_id
        )?;
    }
    Ok(text)
}
