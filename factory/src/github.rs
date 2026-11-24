use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::order::{CiFailure, WorkOrder, valid_oid};
use crate::review::{Draft, EvidenceSource, MergeApproval, MergeObservation, ReviewRecord};
use crate::store::Receipt;
use crate::{Result, reject, repository};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub schema_version: u32,
    pub repository: String,
    pub base_branch: String,
    pub check_definitions: Vec<String>,
    pub required_checks: Vec<RequiredCheck>,
    pub ci_repair: Option<RepairPolicy>,
    #[serde(default)]
    pub reviewer_ids: Vec<u64>,
    #[serde(default)]
    pub minimum_merge_approvals: usize,
    #[serde(default)]
    pub require_merge_queue: bool,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RepairPolicy {
    pub(crate) workflow_id: u64,
    pub(crate) job_name: String,
}

use craft_factory_evidence::ci::FailedJob;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RequiredCheck {
    pub name: String,
    pub app_id: u64,
}

#[derive(Debug, Serialize)]
pub struct ObservedCheck {
    id: u64,
    name: String,
    app_id: u64,
}

impl Policy {
    pub(crate) fn validate(&self) -> Result<()> {
        let valid_name = |name: &str| {
            !matches!(name, "" | "." | "..")
                && name.len() <= 100
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
        };
        let mut checks = BTreeSet::new();
        let mut reviewers = BTreeSet::new();
        if self.schema_version != 1
            || self.repository.split('/').count() != 2
            || !self.repository.split('/').all(valid_name)
            || self.base_branch.trim().is_empty()
            || self.required_checks.is_empty()
            || self.required_checks.len() > 16
            || self.reviewer_ids.len() > 32
            || self.minimum_merge_approvals > self.reviewer_ids.len()
            || self
                .reviewer_ids
                .iter()
                .any(|id| *id == 0 || !reviewers.insert(id))
            || self.required_checks.iter().any(|check| {
                check.name.trim().is_empty()
                    || check.name.len() > 200
                    || check.app_id == 0
                    || !checks.insert((&check.name, check.app_id))
            })
            || self.ci_repair.as_ref().is_some_and(|repair| {
                repair.workflow_id == 0
                    || self
                        .required_checks
                        .iter()
                        .filter(|check| check.name == repair.job_name)
                        .count()
                        != 1
            })
            || self.check_definitions.iter().any(|path| {
                path.contains(['\\', ':'])
                    || path.split('/').any(|part| matches!(part, "" | "." | ".."))
            })
        {
            return reject("invalid GitHub pilot policy");
        }
        Ok(())
    }

    pub fn merge_approval_policy(&self) -> Result<Option<String>> {
        if self.minimum_merge_approvals == 0 {
            return Ok(None);
        }
        let reviewers: BTreeSet<_> = self.reviewer_ids.iter().collect();
        Ok(Some(craft_factory_evidence::sha256(&serde_json::to_vec(
            &(self.minimum_merge_approvals, reviewers),
        )?)))
    }

    pub(crate) fn protects(&self, path: &str) -> bool {
        let path = path.to_ascii_lowercase();
        [
            ".github/workflows",
            ".github/actions",
            ".github/codeowners",
            "docs/codeowners",
            "codeowners",
        ]
        .into_iter()
        .chain(self.check_definitions.iter().map(String::as_str))
        .any(|protected| {
            let protected = protected.to_ascii_lowercase();
            path == protected
                || path
                    .strip_prefix(&protected)
                    .is_some_and(|suffix| suffix.starts_with('/'))
        })
    }
}

pub fn observe(
    receipt: &Receipt,
    policy: &Policy,
    gh: PathBuf,
) -> Result<(String, Vec<ObservedCheck>)> {
    validate_gh(&gh)?;
    observe_with(receipt, policy, |endpoint, fields| {
        read(&gh, endpoint, fields)
    })
}

pub(crate) fn validate_gh(gh: &Path) -> Result<()> {
    if !gh.is_absolute() || !gh.is_file() {
        return reject("an absolute path to the trusted GitHub CLI executable is required");
    }
    #[cfg(windows)]
    if gh
        .extension()
        .and_then(|ext| ext.to_str())
        .is_none_or(|ext| !ext.eq_ignore_ascii_case("exe"))
    {
        return reject("the Windows GitHub CLI must be an executable file");
    }
    Ok(())
}

pub(crate) fn read_pinned_policy(path: &Path) -> Result<(Policy, String)> {
    let bytes = craft_factory_evidence::release_trace::read_evidence(path, 128 * 1024)
        .map_err(|e| e.to_string())?;
    Ok((
        serde_json::from_slice(&bytes)?,
        craft_factory_evidence::sha256(&bytes),
    ))
}

fn read(gh: &Path, endpoint: &str, fields: &[String]) -> Result<Value> {
    Ok(serde_json::from_slice(&read_bytes(gh, endpoint, fields)?)?)
}

pub(crate) fn read_bytes(gh: &Path, endpoint: &str, fields: &[String]) -> Result<Vec<u8>> {
    read_bytes_for(gh, endpoint, fields, Duration::from_secs(12))
}

fn read_bytes_for(
    gh: &Path,
    endpoint: &str,
    fields: &[String],
    timeout: Duration,
) -> Result<Vec<u8>> {
    let mut command = evidence_command(gh);
    command.args(["--method", "GET", endpoint]);
    for field in fields {
        command.arg("-f").arg(field);
    }
    read_evidence_output(&mut command, timeout)
}

fn evidence_command(gh: &Path) -> Command {
    let mut command = Command::new(gh);
    command
        .args(["api", "--hostname", "github.com"])
        .env("GH_PROMPT_DISABLED", "1")
        .env_remove("CREDENTIALS_DIRECTORY")
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    for (name, _) in std::env::vars_os() {
        if name
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("CRAFT_FACTORY_")
        {
            command.env_remove(name);
        }
    }
    command
}

fn read_evidence_output(command: &mut Command, timeout: Duration) -> Result<Vec<u8>> {
    let (status, output) =
        crate::process_tree::read_bounded(command, "GitHub CLI", timeout, 4 * 1024 * 1024)?;
    if !status.success() {
        return reject("GitHub CLI could not read the configured repository evidence");
    }
    Ok(output)
}

pub(crate) fn check_ci_draft_refs(
    gh: &Path,
    policy: &Policy,
    base: &str,
    branch: &str,
    head: &str,
) -> Result<()> {
    let source = read(
        gh,
        &format!("repos/{}/git/ref/heads/{branch}", policy.repository),
        &[],
    )?;
    if source["ref"] != format!("refs/heads/{branch}")
        || source["object"]["type"] != "commit"
        || source["object"]["sha"] != head
        || ci_branch_tip(policy, gh)? != base
    {
        return reject("GitHub base or staged candidate branch has moved");
    }
    Ok(())
}

pub(crate) fn create_verified_ci_draft(
    gh: &Path,
    policy: &Policy,
    branch: &str,
    base: &str,
    head: &str,
    body: &str,
) -> Result<String> {
    let mut command = evidence_command(gh);
    command
        .args([
            "--method",
            "POST",
            &format!("repos/{}/pulls", policy.repository),
        ])
        .args(["-f", "title=Repair the approved CI failure"])
        .arg("-f")
        .arg(format!("head={branch}"))
        .arg("-f")
        .arg(format!("base={}", policy.base_branch))
        .arg("-f")
        .arg(format!("body={body}"))
        .args(["-F", "draft=true"]);
    let response = (|| {
        let (status, bytes) = crate::process_tree::read_bounded(
            &mut command,
            "GitHub draft publication",
            Duration::from_secs(12),
            1024 * 1024,
        )?;
        if !status.success() {
            return reject("GitHub did not confirm the write");
        }
        let pr: Value = serde_json::from_slice(&bytes)?;
        let number = pr["number"]
            .as_u64()
            .filter(|number| *number > 0)
            .ok_or("missing PR number")?;
        let url = format!("https://github.com/{}/pull/{number}", policy.repository);
        if pr["html_url"] != url
            || pr["draft"] != true
            || pr["state"] != "open"
            || pr["head"]["ref"] != branch
            || pr["head"]["sha"] != head
            || pr["head"]["repo"]["full_name"] != policy.repository
            || pr["base"]["ref"] != policy.base_branch
            || pr["base"]["sha"] != base
            || pr["base"]["repo"]["full_name"] != policy.repository
        {
            return reject("GitHub returned a draft for a different target");
        }
        check_ci_draft_refs(gh, policy, base, branch, head)?;
        Ok(url)
    })();
    response.map_err(|_: Box<dyn std::error::Error>| {
        "draft publication could not be confirmed; inspect the publication hold and reconcile the native PR before retrying".into()
    })
}

fn read_queue_pr(gh: &Path, repository: &str, number: u64) -> Result<Value> {
    let (owner, name) = repository
        .split_once('/')
        .ok_or("invalid GitHub repository")?;
    let mut command = evidence_command(gh);
    command.args(["--method", "POST", "graphql", "-f", concat!(
        "query=query CraftMergeQueueReadiness($owner:String!,$name:String!,$number:Int!) {",
        "repository(owner:$owner,name:$name) { nameWithOwner pullRequest(number:$number) {",
        "id number url state isDraft isCrossRepository baseRefName baseRefOid headRefOid ",
        "isMergeQueueEnabled isInMergeQueue mergeQueueEntry { id } autoMergeRequest { enabledAt } stack { id }",
        "} } }"
    )])
    .arg("-f").arg(format!("owner={owner}"))
    .arg("-f").arg(format!("name={name}"))
    .arg("-F").arg(format!("number={number}"));
    let data: Value = serde_json::from_slice(&read_evidence_output(
        &mut command,
        Duration::from_secs(12),
    )?)?;
    if data.get("errors").is_some() || data["data"]["repository"]["nameWithOwner"] != repository {
        return reject("GitHub GraphQL did not return the expected repository");
    }
    Ok(data["data"]["repository"]["pullRequest"].clone())
}

pub(crate) fn enqueue_reviewed_head(
    gh: &Path,
    writer_token: &str,
    intent: &str,
    pull_request_node: &str,
    head: &str,
) -> Result<String> {
    let mut command = evidence_command(gh);
    command
        .env("GH_TOKEN", writer_token)
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_ENTERPRISE_TOKEN")
        .env_remove("GITHUB_ENTERPRISE_TOKEN")
        .args(["--method", "POST", "graphql", "-f", concat!(
            "query=mutation CraftEnqueue($pull:ID!,$head:GitObjectID!,$intent:String!) {",
            "enqueuePullRequest(input:{pullRequestId:$pull,expectedHeadOid:$head,clientMutationId:$intent,jump:false}) {",
            "clientMutationId mergeQueueEntry { id pullRequest { id headRefOid } }",
            "} }"
        )])
        .arg("-f").arg(format!("pull={pull_request_node}"))
        .arg("-f").arg(format!("head={head}"))
        .arg("-f").arg(format!("intent={intent}"));
    let response: Value = serde_json::from_slice(&read_evidence_output(
        &mut command,
        Duration::from_secs(12),
    )?)?;
    let result = &response["data"]["enqueuePullRequest"];
    let entry = &result["mergeQueueEntry"];
    if response.get("errors").is_some()
        || result["clientMutationId"] != intent
        || entry["pullRequest"]["id"] != pull_request_node
        || entry["pullRequest"]["headRefOid"] != head
    {
        return reject("GitHub did not confirm a queue entry for the exact reviewed PR head");
    }
    entry["id"]
        .as_str()
        .filter(|id| craft_factory_evidence::valid_github_node_id(id))
        .map(str::to_owned)
        .ok_or_else(|| "GitHub did not return a valid merge queue entry ID".into())
}

pub(crate) fn observe_enqueued_head(
    gh: &Path,
    repository: &str,
    draft: &Draft,
    base_branch: &str,
    node: &str,
) -> Result<String> {
    validate_gh(gh)?;
    let number = queue_pr_number(repository, draft)?;
    let response = read_queue_pr(gh, repository, number)?;
    if !matches_queue_pr(&response, draft, base_branch, node, number)
        || response["state"] != "OPEN"
        || response["isDraft"] != false
        || response["isMergeQueueEnabled"] != true
        || response["isInMergeQueue"] != true
        || response["headRefOid"] != draft.head
    {
        return reject("GitHub did not show the exact reviewed PR head in the merge queue");
    }
    response["mergeQueueEntry"]["id"]
        .as_str()
        .filter(|id| craft_factory_evidence::valid_github_node_id(id))
        .map(str::to_owned)
        .ok_or_else(|| "GitHub did not return a valid merge queue entry ID".into())
}

pub(crate) fn observe_queue_clearance(
    gh: &Path,
    repository: &str,
    draft: &Draft,
    base_branch: &str,
    node: &str,
) -> Result<String> {
    validate_gh(gh)?;
    let number = queue_pr_number(repository, draft)?;
    let response = read_queue_pr(gh, repository, number)?;
    if !matches_queue_pr(&response, draft, base_branch, node, number)
        || !matches!(response["state"].as_str(), Some("OPEN" | "CLOSED"))
        || response["isMergeQueueEnabled"].as_bool().is_none()
        || response["isInMergeQueue"] != false
        || response.get("mergeQueueEntry") != Some(&Value::Null)
        || response.get("autoMergeRequest") != Some(&Value::Null)
        || response.get("stack") != Some(&Value::Null)
    {
        return reject(
            "GitHub did not confirm an unmerged PR with no active queue entry or auto-merge",
        );
    }
    let head = response["headRefOid"]
        .as_str()
        .filter(|head| valid_oid(head))
        .ok_or("GitHub did not identify the current PR head")?;
    if read_queue_pr(gh, repository, number)? != response {
        return reject("GitHub queue evidence changed during clearance");
    }
    Ok(head.to_owned())
}

fn queue_pr_number(repository: &str, draft: &Draft) -> Result<u64> {
    let prefix = format!("https://github.com/{repository}/pull/");
    Ok(draft
        .reference
        .strip_prefix(&prefix)
        .ok_or("wrong GitHub PR repository")?
        .parse()?)
}

fn matches_queue_pr(
    response: &Value,
    draft: &Draft,
    base_branch: &str,
    node: &str,
    number: u64,
) -> bool {
    response["id"] == node
        && response["number"].as_u64() == Some(number)
        && response["url"] == draft.reference
        && response["isCrossRepository"] == false
        && response["baseRefName"] == base_branch
}

fn verify_unqueued_pr(
    snapshot: &Value,
    rest: &Value,
    draft: &Draft,
    branch: &str,
    base: &str,
    number: u64,
) -> Result<String> {
    if snapshot["number"].as_u64() != Some(number)
        || snapshot["id"] != rest["node_id"]
        || snapshot["url"] != draft.reference
        || snapshot["state"] != "OPEN"
        || snapshot["isDraft"] != false
        || snapshot["isCrossRepository"] != false
        || snapshot["baseRefName"] != branch
        || snapshot["baseRefOid"] != base
        || snapshot["headRefOid"] != draft.head
        || snapshot["isMergeQueueEnabled"] != true
        || snapshot["isInMergeQueue"] != false
        || snapshot.get("mergeQueueEntry") != Some(&Value::Null)
        || snapshot.get("autoMergeRequest") != Some(&Value::Null)
        || snapshot.get("stack") != Some(&Value::Null)
    {
        return reject("GitHub did not confirm an unchanged, unqueued PR with merge queue enabled");
    }
    let node = snapshot["id"]
        .as_str()
        .filter(|node| craft_factory_evidence::valid_github_node_id(node))
        .ok_or("GitHub did not identify the pull request node")?;
    Ok(node.to_owned())
}

fn read_active_rulesets(gh: &Path, repository: &str, active: &[Value]) -> Result<Vec<Value>> {
    let mut sources = BTreeMap::new();
    for rule in active {
        let id = rule["ruleset_id"]
            .as_u64()
            .filter(|id| *id > 0)
            .ok_or("GitHub active rule has no ruleset identity")?;
        let source = (&rule["ruleset_source_type"], &rule["ruleset_source"]);
        if sources
            .insert(id, source)
            .is_some_and(|prior| prior != source)
        {
            return reject("GitHub active rules disagree about a ruleset source");
        }
    }
    if sources.len() > 8 {
        return reject("GitHub active rulesets exceed the inspection limit");
    }
    sources.into_iter().map(|(id, (source_type, source))| {
        let ruleset = read(gh, &format!("repos/{repository}/rulesets/{id}"), &["includes_parents=true".into()])?;
        if ruleset["id"].as_u64() != Some(id) || ruleset["target"] != "branch"
            || ruleset["source_type"] != *source_type || ruleset["source"] != *source
            || ruleset["enforcement"] != "active"
            || !ruleset["bypass_actors"].as_array().is_some_and(Vec::is_empty)
        {
            return reject("GitHub ruleset identity, enforcement, or empty bypass list could not be verified");
        }
        Ok(json!({"id":id, "target":ruleset["target"], "source_type":source_type, "source":source,
            "enforcement":ruleset["enforcement"], "bypass_actors":ruleset["bypass_actors"],
            "conditions":ruleset["conditions"]}))
    }).collect()
}

pub fn shadow_merge(
    receipt: &Receipt,
    policy: &Policy,
    gh: &Path,
    approved_protection: &str,
    as_of: i64,
) -> Result<Value> {
    let draft = receipt
        .draft
        .as_ref()
        .ok_or("bind the draft before checking GitHub merge readiness")?;
    repository::validate_github_remote(&receipt.work_order, &policy.repository)?;
    shadow_merge_candidate(
        &receipt.work_order,
        draft,
        receipt.review_ready && receipt.candidate.as_deref() == Some(draft.head.as_str()),
        policy,
        gh,
        approved_protection,
        as_of,
    )
}

pub fn shadow_merge_candidate(
    order: &WorkOrder,
    draft: &Draft,
    independently_verified: bool,
    policy: &Policy,
    gh: &Path,
    approved_protection: &str,
    as_of: i64,
) -> Result<Value> {
    validate_gh(gh)?;
    policy.validate()?;
    let id = pr_number(draft, policy)?;
    let head = &draft.head;
    let base = &order.base;
    let endpoint = format!("repos/{}/pulls/{id}", policy.repository);
    let branch_name = encode_branch(&policy.base_branch);
    let branch = format!("repos/{}/branches/{branch_name}", policy.repository);
    let protection = format!("{branch}/protection");
    let rules_endpoint = format!("repos/{}/rules/branches/{branch_name}", policy.repository);
    let read_rules = || -> Result<Vec<Value>> {
        let value = read(gh, &rules_endpoint, &["per_page=100".into()])?;
        let Value::Array(mut rules) = value else {
            return reject("GitHub did not return an active branch rule list");
        };
        if rules.len() >= 100 {
            return reject("GitHub active branch rule list exceeds the inspection limit");
        }
        rules.sort_by_cached_key(Value::to_string);
        Ok(rules)
    };
    let mut observed_protection = None;
    let mut active_rule_count = None;
    let mut evidence = None;
    let mut stage = "github_candidate_verification";
    let validation = (|| -> Result<()> {
        if !independently_verified {
            return reject(
                "the current candidate requires independent verification before shadow merge",
            );
        }
        stage = "github_protection_unavailable";
        let classic = read(gh, &protection, &[])?;
        let active = read_rules()?;
        active_rule_count = Some(active.len());
        let rulesets = if policy.require_merge_queue {
            stage = "github_queue_rulesets_unavailable";
            read_active_rulesets(gh, &policy.repository, &active)?
        } else {
            Vec::new()
        };
        let bytes = if policy.require_merge_queue {
            serde_json::to_vec(
                &json!({"classic":classic, "active_branch_rules":active, "rulesets":rulesets}),
            )?
        } else if active.is_empty() {
            serde_json::to_vec(&classic)?
        } else {
            serde_json::to_vec(&json!({"classic": classic, "active_branch_rules": active}))?
        };
        let observed = craft_factory_evidence::sha256(&bytes);
        observed_protection = Some(observed.clone());
        stage = "github_protection_drift";
        if observed != approved_protection {
            return reject("GitHub branch protection differs from the approved fingerprint");
        }
        stage = "github_protection_policy";
        validate_shadow_rules(policy, &classic)?;
        let queue_ruleset = validate_active_branch_rules(policy, &active)?;
        stage = "github_base_changed";
        let tip = read(gh, &branch, &[])?;
        if tip["name"] != policy.base_branch
            || tip["protected"] != true
            || tip["commit"]["sha"] != *base
        {
            return reject("GitHub protected branch no longer points to the approved base");
        }
        stage = "github_pr_not_ready";
        let observed_pr = read(gh, &endpoint, &[])?;
        validate_pr(&observed_pr, policy, id, head, base)?;
        if observed_pr["draft"] != false
            || observed_pr["mergeable"] != true
            || observed_pr["mergeable_state"] != "clean"
        {
            return reject("GitHub did not report a ready and mergeable pull request");
        }
        let queue_pr = if let Some(ruleset_id) = queue_ruleset {
            stage = "github_queue_pr_unavailable";
            let snapshot = read_queue_pr(gh, &policy.repository, id)?;
            let node = verify_unqueued_pr(
                &snapshot,
                &observed_pr,
                draft,
                &policy.base_branch,
                base,
                id,
            )?;
            Some((
                snapshot,
                craft_factory_evidence::GithubMergeQueueReadiness {
                    pull_request_node_id: node,
                    merge_queue_ruleset_id: ruleset_id,
                    active_ruleset_ids: rulesets
                        .iter()
                        .filter_map(|row| row["id"].as_u64())
                        .collect(),
                },
            ))
        } else {
            None
        };
        stage = "github_native_checks";
        let (_, checks) = observe_candidate_checks(order, draft, policy, |endpoint, fields| {
            read(gh, endpoint, fields)
        })?;
        stage = "github_approvals";
        let author = observed_pr["user"]["id"]
            .as_u64()
            .filter(|id| *id > 0)
            .ok_or("GitHub pull request author is unavailable")?;
        let reviews = read_review_history(gh, &format!("{endpoint}/reviews"))?;
        let approvals = merge_approvals(
            &reviews,
            policy,
            draft,
            as_of,
            author,
            &format!("https://api.github.com/{endpoint}"),
        )?;
        stage = "github_evidence_changed";
        if read_review_history(gh, &format!("{endpoint}/reviews"))? != reviews
            || read(gh, &endpoint, &[])? != observed_pr
            || read(gh, &branch, &[])? != tip
            || read(gh, &protection, &[])? != classic
            || read_rules()? != active
        {
            return reject(
                "GitHub pull request or branch policy evidence changed during inspection",
            );
        }
        let mut observed = json!({"checks": checks, "approvals": approvals});
        if let Some((snapshot, queue)) = queue_pr {
            if read_active_rulesets(gh, &policy.repository, &active)? != rulesets
                || read_queue_pr(gh, &policy.repository, id)? != snapshot
            {
                return reject("GitHub merge queue evidence changed during inspection");
            }
            observed["merge_queue"] = serde_json::to_value(queue)?;
        }
        evidence = Some(observed);
        Ok(())
    })();
    let blockers: Vec<_> = validation
        .err()
        .map(|error| error.to_string())
        .into_iter()
        .collect();
    let blocker_codes: Vec<_> = (!blockers.is_empty())
        .then_some(stage)
        .into_iter()
        .collect();
    Ok(
        json!({"schema_version": 1, "provider": "github", "work_id": order.id,
        "repository": policy.repository, "pull_request": draft.reference, "base": base, "head": head,
        "as_of": as_of, "shadow_eligible": blockers.is_empty(), "blocking_reasons": blockers,
        "blocking_codes": blocker_codes,
        "observed_protection_digest": observed_protection, "active_branch_rule_count": active_rule_count,
        "merge_queue_required": policy.require_merge_queue,
        "evidence": evidence}),
    )
}

fn encode_branch(branch: &str) -> String {
    branch
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MergeQueueRule {
    grouping_strategy: String,
    merge_method: String,
    check_response_timeout_minutes: u64,
    max_entries_to_build: u64,
    min_entries_to_merge: u64,
    max_entries_to_merge: u64,
    min_entries_to_merge_wait_minutes: u64,
}

impl MergeQueueRule {
    fn valid(&self) -> bool {
        self.grouping_strategy == "ALLGREEN"
            && matches!(self.merge_method.as_str(), "MERGE" | "SQUASH" | "REBASE")
            && self.check_response_timeout_minutes > 0
            && (1..=100).contains(&self.max_entries_to_build)
            && (1..=self.max_entries_to_merge).contains(&self.min_entries_to_merge)
            && self.max_entries_to_merge <= 100
            && (self.min_entries_to_merge == 1 || self.min_entries_to_merge_wait_minutes > 0)
    }
}

fn validate_active_branch_rules(policy: &Policy, rules: &[Value]) -> Result<Option<u64>> {
    let mut merge_queue = None;
    for rule in rules {
        let parameters = &rule["parameters"];
        let ruleset = rule["ruleset_id"]
            .as_u64()
            .filter(|id| *id > 0)
            .ok_or("GitHub has an active branch rule without a ruleset identity")?;
        let supported = match rule["type"].as_str() {
            Some("deletion" | "non_fast_forward") => {
                parameters.is_null() || parameters.as_object().is_some_and(|map| map.is_empty())
            }
            Some("pull_request") => {
                parameters["dismiss_stale_reviews_on_push"] == true
                    && parameters["required_approving_review_count"].as_u64()
                        == Some(policy.minimum_merge_approvals as u64)
                    && [
                        "require_code_owner_review",
                        "require_last_push_approval",
                        "required_review_thread_resolution",
                    ]
                    .iter()
                    .all(|key| parameters[key] == false)
                    && parameters.as_object().is_some_and(|map| {
                        map.keys().all(|key| {
                            matches!(
                                key.as_str(),
                                "dismiss_stale_reviews_on_push"
                                    | "required_approving_review_count"
                                    | "require_code_owner_review"
                                    | "require_last_push_approval"
                                    | "required_review_thread_resolution"
                                    | "allowed_merge_methods"
                            )
                        })
                    })
                    && (parameters["allowed_merge_methods"].is_null()
                        || parameters["allowed_merge_methods"]
                            .as_array()
                            .is_some_and(|methods| {
                                !methods.is_empty()
                                    && methods.iter().all(|method| {
                                        matches!(
                                            method.as_str(),
                                            Some("merge" | "squash" | "rebase")
                                        )
                                    })
                            }))
            }
            Some("required_status_checks") => {
                current_base_setting(&parameters["strict_required_status_checks_policy"], policy)
                    && parameters.as_object().is_some_and(|map| {
                        map.keys().all(|key| {
                            matches!(
                                key.as_str(),
                                "strict_required_status_checks_policy"
                                    | "required_status_checks"
                                    | "do_not_enforce_on_create"
                            )
                        })
                    })
                    && parameters["required_status_checks"]
                        .as_array()
                        .is_some_and(|checks| {
                            let expected: BTreeSet<_> = policy
                                .required_checks
                                .iter()
                                .map(|check| (check.name.as_str(), check.app_id))
                                .collect();
                            let actual: BTreeSet<_> = checks
                                .iter()
                                .filter_map(|check| {
                                    Some((
                                        check["context"].as_str()?,
                                        check["integration_id"].as_u64()?,
                                    ))
                                })
                                .collect();
                            actual.len() == checks.len() && actual == expected
                        })
            }
            Some("merge_queue") if policy.require_merge_queue && merge_queue.is_none() => {
                merge_queue = Some(ruleset);
                MergeQueueRule::deserialize(parameters).is_ok_and(|rule| rule.valid())
            }
            _ => false,
        };
        if !supported
            || rule["ruleset_source_type"]
                .as_str()
                .is_none_or(str::is_empty)
            || rule["ruleset_source"].as_str().is_none_or(str::is_empty)
        {
            return reject("GitHub has an active branch rule outside the supported shadow checks");
        }
    }
    if policy.require_merge_queue && merge_queue.is_none() {
        return reject("GitHub did not report a required merge queue ruleset");
    }
    Ok(merge_queue)
}

fn current_base_setting(strict: &Value, policy: &Policy) -> bool {
    strict
        .as_bool()
        .is_some_and(|enabled| enabled || policy.require_merge_queue)
}

fn validate_shadow_rules(policy: &Policy, rules: &Value) -> Result<()> {
    let reviews = &rules["required_pull_request_reviews"];
    let bypass = &reviews["bypass_pull_request_allowances"];
    let approved = policy
        .required_checks
        .iter()
        .map(|check| (check.name.as_str(), check.app_id))
        .collect::<BTreeSet<_>>();
    let required = &rules["required_status_checks"];
    let checks = required["checks"]
        .as_array()
        .ok_or("GitHub did not show required check app identities")?;
    let observed = checks
        .iter()
        .filter_map(|check| Some((check["context"].as_str()?, check["app_id"].as_u64()?)))
        .collect::<BTreeSet<_>>();
    let contexts = required["contexts"]
        .as_array()
        .ok_or("GitHub did not show required check contexts")?;
    let approved_names = approved
        .iter()
        .map(|(name, _)| *name)
        .collect::<BTreeSet<_>>();
    if policy.minimum_merge_approvals == 0
        || policy.minimum_merge_approvals
            != reviews["required_approving_review_count"]
                .as_u64()
                .unwrap_or(0) as usize
        || reviews["dismiss_stale_reviews"] != true
        || rules["enforce_admins"]["enabled"] != true
        || rules["allow_force_pushes"]["enabled"] != false
        || rules["allow_deletions"]["enabled"] != false
        || !current_base_setting(&required["strict"], policy)
        || checks.len() != observed.len()
        || approved != observed
        || contexts.len() != approved_names.len()
        || contexts
            .iter()
            .filter_map(Value::as_str)
            .collect::<BTreeSet<_>>()
            != approved_names
        || (!bypass.is_null()
            && bypass.as_object().is_none_or(|groups| {
                !groups
                    .values()
                    .all(|group| group.as_array().is_some_and(Vec::is_empty))
            }))
    {
        return reject(
            "GitHub branch protection does not enforce the approved checks and review count",
        );
    }
    Ok(())
}

pub fn failed_job(
    policy: &Policy,
    gh: &Path,
    run: u64,
    attempt: u64,
    base: &str,
) -> Result<FailedJob> {
    validate_gh(gh)?;
    failed_job_with(policy, run, attempt, base, |endpoint, fields| {
        read(gh, endpoint, fields)
    })?
    .ok_or_else(|| "the approved GitHub repair job did not fail".into())
}

#[derive(Debug)]
pub(crate) enum ObsoleteFailure {
    BranchMoved,
    AttemptSuperseded,
    WorkflowSuperseded,
}

impl ObsoleteFailure {
    pub(crate) fn reason(&self) -> &'static str {
        match self {
            Self::BranchMoved => "branch_moved",
            Self::AttemptSuperseded | Self::WorkflowSuperseded => "workflow_superseded",
        }
    }
}

impl std::fmt::Display for ObsoleteFailure {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output.write_str(match self {
            Self::BranchMoved => "GitHub branch has moved away from the approved failure commit",
            Self::AttemptSuperseded => "GitHub run is not the current failed push attempt for the approved workflow and commit",
            Self::WorkflowSuperseded => {
                "a newer GitHub workflow run or attempt superseded the selected failure"
            }
        })
    }
}

impl std::error::Error for ObsoleteFailure {}

pub fn poll_failed_job(policy: &Policy, gh: &Path, base: &str) -> Result<Option<FailedJob>> {
    validate_gh(gh)?;
    policy.validate()?;
    let repair = policy
        .ci_repair
        .as_ref()
        .ok_or("GitHub policy must approve a CI repair job")?;
    let mut get = |endpoint: &str, fields: &[String]| read(gh, endpoint, fields);
    let Some(run) = latest_push_run(policy, repair.workflow_id, base, &mut get)? else {
        return Ok(None);
    };
    if !run.failed {
        return Ok(None);
    }
    failed_job_with(policy, run.id, run.attempt, base, get)
}

pub fn ci_branch_tip(policy: &Policy, gh: &Path) -> Result<String> {
    validate_gh(gh)?;
    policy.validate()?;
    let endpoint = format!(
        "repos/{}/branches/{}",
        policy.repository,
        encode_branch(&policy.base_branch)
    );
    let branch = read(gh, &endpoint, &[])?;
    branch["commit"]["sha"]
        .as_str()
        .filter(|sha| branch["name"] == policy.base_branch && valid_oid(sha))
        .map(str::to_owned)
        .ok_or_else(|| "GitHub did not identify the configured CI branch tip".into())
}

struct PushRun {
    id: u64,
    attempt: u64,
    number: u64,
    failed: bool,
}

fn latest_push_run(
    policy: &Policy,
    workflow: u64,
    base: &str,
    get: &mut impl FnMut(&str, &[String]) -> Result<Value>,
) -> Result<Option<PushRun>> {
    let endpoint = format!(
        "repos/{}/actions/workflows/{workflow}/runs",
        policy.repository
    );
    let response = get(
        &endpoint,
        &[
            format!("branch={}", policy.base_branch),
            format!("head_sha={base}"),
            "event=push".into(),
            "per_page=30".into(),
        ],
    )?;
    let rows = response["workflow_runs"]
        .as_array()
        .ok_or("missing GitHub workflow runs")?;
    if response["total_count"].as_u64() != Some(rows.len() as u64) || rows.len() > 30 {
        return reject(
            "GitHub returned incomplete workflow history for the approved branch commit",
        );
    }
    let mut ids = BTreeSet::new();
    let mut numbers = BTreeSet::new();
    let mut latest: Option<PushRun> = None;
    for row in rows {
        let positive = |key: &str| row[key].as_u64().filter(|value| *value > 0);
        let (Some(id), Some(attempt), Some(number)) = (
            positive("id"),
            positive("run_attempt"),
            positive("run_number"),
        ) else {
            return reject("GitHub workflow history is missing run identity");
        };
        if row["workflow_id"].as_u64() != Some(workflow)
            || row["event"] != "push"
            || row["head_branch"] != policy.base_branch
            || row["head_sha"] != base
            || !same_repository(row, &policy.repository)
            || !ids.insert(id)
            || !numbers.insert(number)
        {
            return reject("GitHub workflow history contains unexpected or duplicate run evidence");
        }
        if latest.as_ref().is_none_or(|prior| number > prior.number) {
            latest = Some(PushRun {
                id,
                attempt,
                number,
                failed: row["status"] == "completed" && row["conclusion"] == "failure",
            });
        }
    }
    Ok(latest)
}

fn same_repository(run: &Value, repository: &str) -> bool {
    ["/repository/full_name", "/head_repository/full_name"]
        .iter()
        .all(|path| {
            run.pointer(path)
                .and_then(Value::as_str)
                .is_some_and(|name| name.eq_ignore_ascii_case(repository))
        })
}

fn failed_job_with(
    policy: &Policy,
    run: u64,
    attempt: u64,
    base: &str,
    mut get: impl FnMut(&str, &[String]) -> Result<Value>,
) -> Result<Option<FailedJob>> {
    policy.validate()?;
    let repair = policy
        .ci_repair
        .as_ref()
        .ok_or("GitHub policy must approve a CI repair job")?;
    let check = policy
        .required_checks
        .iter()
        .find(|check| check.name == repair.job_name)
        .ok_or("GitHub repair job must be required on the candidate")?;
    if run == 0 || attempt == 0 || !valid_oid(base) {
        return reject("a workflow run, its attempt, and its full commit are required");
    }
    let repo = &policy.repository;
    let branch = format!(
        "repos/{repo}/branches/{}",
        encode_branch(&policy.base_branch)
    );
    let assert_branch = |value: Value| -> Result<()> {
        if value["name"] != policy.base_branch || value["commit"]["sha"] != base {
            if value["name"] == policy.base_branch
                && value["commit"]["sha"].as_str().is_some_and(valid_oid)
            {
                return Err(ObsoleteFailure::BranchMoved.into());
            }
            return reject("GitHub branch has moved away from the approved failure commit");
        }
        Ok(())
    };
    let endpoint = format!("repos/{repo}/actions/runs/{run}");
    let assert_run = |value: &Value| -> Result<()> {
        let same_run = value["id"].as_u64() == Some(run)
            && value["workflow_id"].as_u64() == Some(repair.workflow_id)
            && value["event"] == "push"
            && value["head_sha"] == base
            && value["head_branch"] == policy.base_branch
            && same_repository(value, repo);
        if same_run
            && value["run_attempt"]
                .as_u64()
                .is_some_and(|newer| newer > attempt)
        {
            return Err(ObsoleteFailure::AttemptSuperseded.into());
        }
        if !same_run
            || value["run_attempt"].as_u64() != Some(attempt)
            || value["status"] != "completed"
            || value["conclusion"] != "failure"
        {
            return reject(
                "GitHub run is not the current failed push attempt for the approved workflow and commit",
            );
        }
        Ok(())
    };
    assert_branch(get(&branch, &[])?)?;
    let initial_run = get(&endpoint, &[])?;
    assert_run(&initial_run)?;
    let jobs = get(
        &format!("{endpoint}/attempts/{attempt}/jobs"),
        &["per_page=100".into()],
    )?;
    let listed = jobs["jobs"]
        .as_array()
        .ok_or("missing GitHub jobs for the workflow attempt")?;
    if jobs["total_count"].as_u64() != Some(listed.len() as u64) {
        return reject("GitHub returned an incomplete job list for the workflow attempt");
    }
    let mut matching = listed.iter().filter(|job| job["name"] == repair.job_name);
    let Some(job) = matching.next() else {
        return Ok(None);
    };
    if matching.next().is_some() || job["run_id"].as_u64() != Some(run) || job["head_sha"] != base {
        return reject(
            "the approved GitHub job is ambiguous or refers to a different run or commit",
        );
    }
    if job["status"] != "completed" || job["conclusion"] != "failure" {
        return Ok(None);
    }
    let job_id = job["id"]
        .as_u64()
        .filter(|id| *id > 0)
        .ok_or("missing GitHub job identity")?;
    let completed_at = job["completed_at"]
        .as_str()
        .filter(|stamp| {
            time::OffsetDateTime::parse(stamp, &time::format_description::well_known::Rfc3339)
                .is_ok()
        })
        .ok_or("missing GitHub job completion time")?;
    assert_run(&get(&endpoint, &[])?)?;
    let latest = latest_push_run(policy, repair.workflow_id, base, &mut get)?;
    if latest.as_ref().is_some_and(|latest| {
        (latest.id == run && latest.attempt > attempt)
            || (latest.id != run
                && initial_run["run_number"]
                    .as_u64()
                    .is_some_and(|number| number > 0 && latest.number > number))
    }) {
        return Err(ObsoleteFailure::WorkflowSuperseded.into());
    }
    if latest.is_none_or(|latest| latest.id != run || latest.attempt != attempt || !latest.failed) {
        return reject("a newer GitHub workflow run or attempt superseded the selected failure");
    }
    assert_branch(get(&branch, &[])?)?;
    Ok(Some(FailedJob {
        repository: repo.clone(),
        branch: policy.base_branch.clone(),
        commit: base.to_owned(),
        workflow_id: repair.workflow_id,
        run_id: run,
        run_attempt: attempt,
        job_id,
        job_name: repair.job_name.clone(),
        check_app_id: check.app_id,
        completed_at: completed_at.to_owned(),
        evidence_url: format!("https://github.com/{repo}/actions/runs/{run}/job/{job_id}"),
    }))
}

fn observe_with(
    receipt: &Receipt,
    policy: &Policy,
    get: impl FnMut(&str, &[String]) -> Result<Value>,
) -> Result<(String, Vec<ObservedCheck>)> {
    if receipt.publication_hold.is_some() {
        return reject("reconcile publication before verifying GitHub checks");
    }
    let draft = receipt
        .draft
        .as_ref()
        .ok_or("bind the human-published pull request first")?;
    let head = receipt.candidate.as_deref().ok_or("no current candidate")?;
    if draft.head != head {
        return reject("bind the draft to the current candidate before verifying checks");
    }
    observe_candidate_checks(&receipt.work_order, draft, policy, get)
}

fn observe_candidate_checks(
    order: &WorkOrder,
    draft: &Draft,
    policy: &Policy,
    mut get: impl FnMut(&str, &[String]) -> Result<Value>,
) -> Result<(String, Vec<ObservedCheck>)> {
    policy.validate()?;
    let head = draft.head.as_str();
    if !valid_oid(head) {
        return reject("bind the draft to a full candidate commit before verifying checks");
    }
    let id = pr_number(draft, policy)?;
    let base = &order.base;
    if let Some(path) = repository::candidate_history_paths(order, base, head)?
        .into_iter()
        .find(|path| policy.protects(path))
    {
        return reject(format!(
            "candidate changes a protected check or ownership path: {path:?}"
        ));
    }
    let endpoint = format!("repos/{}/pulls/{id}", policy.repository);
    validate_pr(&get(&endpoint, &[])?, policy, id, head, base)?;
    let check_endpoint = format!("repos/{}/commits/{head}/check-runs", policy.repository);
    let mut observed = Vec::new();
    for required in &policy.required_checks {
        let value = get(
            &check_endpoint,
            &[
                format!("check_name={}", required.name),
                format!("app_id={}", required.app_id),
                "filter=latest".into(),
                "per_page=100".into(),
            ],
        )?;
        observed.push(validate_check(&value, required, head)?);
    }
    if let Some(failure) = &order.ci_failure {
        let CiFailure::Github(original) = failure else {
            return reject("GitHub cannot verify a Bitbucket CI repair");
        };
        if failed_job_with(
            policy,
            original.run_id,
            original.run_attempt,
            base,
            &mut get,
        )?
        .as_ref()
            != Some(original)
        {
            return reject("the original GitHub failure changed; review the repair intent again");
        }
    }
    validate_pr(&get(&endpoint, &[])?, policy, id, head, base)?;
    Ok((head.to_owned(), observed))
}

pub(crate) fn observe_shared_ci_checks(
    order: &WorkOrder,
    draft: &Draft,
    policy: &Policy,
    gh: &Path,
) -> Result<Vec<ObservedCheck>> {
    validate_gh(gh)?;
    let (_, checks) = observe_candidate_checks(order, draft, policy, |endpoint, fields| {
        read(gh, endpoint, fields)
    })?;
    Ok(checks)
}

fn pr_number(draft: &Draft, policy: &Policy) -> Result<u64> {
    let prefix = format!("https://github.com/{}/pull/", policy.repository);
    let id: u64 = draft
        .reference
        .strip_prefix(&prefix)
        .filter(|suffix| suffix.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or("draft is not a canonical PR URL for the configured GitHub repository")?
        .parse()?;
    if id == 0 || draft.reference != format!("{prefix}{id}") {
        return reject("invalid GitHub pull request number");
    }
    Ok(id)
}

pub fn observe_change_request(
    receipt: &Receipt,
    policy: &Policy,
    gh: &Path,
    review_id: u64,
    note: String,
) -> Result<ReviewRecord> {
    let draft = receipt
        .draft
        .as_ref()
        .ok_or("bind the published pull request first")?;
    if receipt.publication_hold.is_some()
        || receipt.candidate.as_deref() != Some(draft.head.as_str())
    {
        return reject(
            "bind the current candidate to a confirmed pull request before importing feedback",
        );
    }
    observe_change_request_at(
        draft,
        &receipt.work_order.base,
        policy,
        gh,
        review_id,
        Some(note),
    )
}

pub fn observe_change_request_at(
    draft: &Draft,
    base: &str,
    policy: &Policy,
    gh: &Path,
    review_id: u64,
    note: Option<String>,
) -> Result<ReviewRecord> {
    validate_gh(gh)?;
    policy.validate()?;
    if review_id == 0
        || policy.reviewer_ids.is_empty()
        || !valid_oid(base)
        || !valid_oid(&draft.head)
    {
        return reject(
            "a GitHub review ID, pinned revision and approved reviewer list are required",
        );
    }
    let signed_in = if note.is_none() {
        Some(
            read(gh, "user", &[])?["id"]
                .as_u64()
                .filter(|id| *id > 0)
                .ok_or("GitHub CLI must authenticate as the reviewer")?,
        )
    } else {
        None
    };
    let number = pr_number(draft, policy)?;
    let endpoint = format!("repos/{}/pulls/{number}", policy.repository);
    let author = |value: &Value| -> Result<u64> {
        validate_pr(value, policy, number, &draft.head, base)?;
        value["user"]["id"]
            .as_u64()
            .filter(|id| *id > 0)
            .ok_or_else(|| "GitHub PR author identity is missing".into())
    };
    let pr_author = author(&read(gh, &endpoint, &[])?)?;
    let reviews = format!("{endpoint}/reviews");
    let selected = format!("{reviews}/{review_id}");
    let pr_api = format!("https://api.github.com/{endpoint}");
    let validate = |review: &Value| -> Result<_> {
        if review["id"].as_u64() != Some(review_id)
            || review["commit_id"] != draft.head
            || review["state"] != "CHANGES_REQUESTED"
            || !review["pull_request_url"]
                .as_str()
                .is_some_and(|url| url.eq_ignore_ascii_case(&pr_api))
        {
            return reject(
                "GitHub review is not an active change request for the exact pull request head",
            );
        }
        let reviewer = review["user"]["id"].as_u64()
            .filter(|id| *id != pr_author && policy.reviewer_ids.contains(id))
            .filter(|_| review["user"]["type"] == "User")
            .ok_or("GitHub change request must come from an approved reviewer other than the PR author")?;
        if signed_in.is_some_and(|id| id != reviewer) {
            return reject("GitHub change request belongs to a different GitHub account");
        }
        let submitted = review["submitted_at"]
            .as_str()
            .ok_or("GitHub review has no submission time")?;
        let submitted =
            time::OffsetDateTime::parse(submitted, &time::format_description::well_known::Rfc3339)?;
        let body = signed_in.map(|_| review["body"].as_str().unwrap_or_default().to_owned());
        Ok((reviewer, submitted, body))
    };
    let observation = validate(&read(gh, &selected, &[])?)?;
    let history = read_review_history(gh, &reviews)?;
    let mut found = false;
    for (id, row) in &history {
        if *id == review_id {
            if validate(row)? != observation {
                return reject("GitHub change request changed while reading the review history");
            }
            found = true;
        } else if found
            && row["user"]["id"].as_u64() == Some(observation.0)
            && !matches!(row["state"].as_str(), Some("COMMENTED" | "PENDING"))
        {
            return reject(
                "a later decision from the GitHub reviewer superseded this change request",
            );
        }
    }
    if !found
        || validate(&read(gh, &selected, &[])?)? != observation
        || author(&read(gh, &endpoint, &[])?)? != pr_author
    {
        return reject("the GitHub review or pull request changed during inspection");
    }
    let note = match note {
        Some(note) => note,
        None => crate::review::normalize_repair_note(
            observation.2.as_deref().unwrap_or_default(),
            "GitHub",
        )?,
    };
    Ok(ReviewRecord {
        schema_version: 1,
        draft: draft.clone(),
        source: EvidenceSource::OperatorObserved,
        actor: format!("github:user:{}", observation.0),
        evidence: format!("{}#pullrequestreview-{review_id}", draft.reference),
        note,
    })
}

fn read_review_history(gh: &Path, endpoint: &str) -> Result<Vec<(u64, Value)>> {
    let mut history = Vec::new();
    let mut ids = BTreeSet::new();
    for page in 1..=5 {
        let response = read(
            gh,
            endpoint,
            &["per_page=100".into(), format!("page={page}")],
        )?;
        let Value::Array(rows) = response else {
            return reject("missing GitHub pull request review history");
        };
        if rows.len() > 100 {
            return reject("GitHub returned an oversized review page");
        }
        let final_page = rows.len() < 100;
        for row in rows {
            let id = row["id"]
                .as_u64()
                .filter(|id| *id > 0 && ids.insert(*id))
                .ok_or("GitHub review history contains missing or duplicate review identity")?;
            history.push((id, row));
        }
        if final_page {
            return Ok(history);
        }
    }
    reject("GitHub review history exceeds the inspection limit")
}

fn validate_pr(pr: &Value, policy: &Policy, number: u64, head: &str, base: &str) -> Result<()> {
    if pr["number"].as_u64() != Some(number)
        || pr["state"] != "open"
        || pr.pointer("/head/sha").and_then(Value::as_str) != Some(head)
        || pr.pointer("/base/sha").and_then(Value::as_str) != Some(base)
        || pr.pointer("/base/ref").and_then(Value::as_str) != Some(policy.base_branch.as_str())
        || ["/base/repo/full_name", "/head/repo/full_name"]
            .iter()
            .any(|pointer| {
                pr.pointer(pointer)
                    .and_then(Value::as_str)
                    .is_none_or(|repository| !repository.eq_ignore_ascii_case(&policy.repository))
            })
    {
        return reject(
            "the live pull request repository, base, state, or head differs from the pilot candidate",
        );
    }
    Ok(())
}

pub fn observe_review_draft(
    policy: &Policy,
    gh: &Path,
    target: &crate::twg::DraftTarget,
    number: u64,
) -> Result<String> {
    validate_gh(gh)?;
    policy.validate()?;
    if number == 0
        || number >= 1_000_000_000_000_000_000
        || policy.repository != target.repository
        || policy.base_branch != target.base_branch
        || !valid_oid(&target.head)
        || !valid_oid(&target.base)
    {
        return reject("the GitHub policy and shared candidate must identify the same target");
    }
    let repository = &target.repository;
    let pr = read(gh, &format!("repos/{repository}/pulls/{number}"), &[])?;
    validate_pr(&pr, policy, number, &target.head, &target.base)?;
    let url = format!("https://github.com/{repository}/pull/{number}");
    if pr["head"]["ref"] != target.source_branch || pr["html_url"] != url || pr["draft"] != true {
        return reject(
            "the GitHub pull request must be an open draft from the pinned source branch",
        );
    }
    Ok(url)
}

pub fn observe_merge(receipt: &Receipt, policy: &Policy, gh: &Path) -> Result<MergeObservation> {
    validate_gh(gh)?;
    policy.validate()?;
    let draft = receipt
        .draft
        .as_ref()
        .ok_or("bind the published GitHub pull request first")?;
    if receipt.publication_hold.is_some()
        || receipt.candidate.as_deref() != Some(draft.head.as_str())
    {
        return reject(
            "bind the confirmed draft to the verified candidate before recording a merge",
        );
    }
    let number = pr_number(draft, policy)?;
    let endpoint = format!("repos/{}/pulls/{number}", policy.repository);
    let read_outcome = || -> Result<(MergeObservation, Option<u64>)> {
        let pr = read(gh, &endpoint, &[])?;
        let outcome = parse_merge(&pr, policy, number, draft, &receipt.work_order.base)?;
        Ok((outcome, pr["user"]["id"].as_u64().filter(|id| *id > 0)))
    };
    let (mut observation, author) = read_outcome()?;
    let configured_policy = policy.merge_approval_policy()?;
    crate::review::merge_approval_matches(
        receipt,
        "github_checks_observed",
        configured_policy.as_deref(),
    )?;
    let approvals = if configured_policy.is_some() {
        let author = author.ok_or("GitHub PR author identity is missing")?;
        let history = read_review_history(gh, &format!("{endpoint}/reviews"))?;
        merge_approvals(
            &history,
            policy,
            &observation.draft,
            observation.merged_at,
            author,
            &format!("https://api.github.com/{endpoint}"),
        )?
    } else {
        Vec::new()
    };
    if read_outcome()? != (observation.clone(), author) {
        return reject("GitHub merge outcome changed during inspection");
    }
    observation.approvals = approvals;
    Ok(observation)
}

pub fn observe_shared_merge_until(
    draft: &Draft,
    base: &str,
    policy: &Policy,
    gh: &Path,
    evaluated_at: i64,
    deadline: Instant,
) -> Result<Option<craft_factory_evidence::GithubMergeOutcome>> {
    validate_gh(gh)?;
    policy.validate()?;
    if !valid_oid(base) || !valid_oid(&draft.head) {
        return reject("merge observation requires full pinned commits");
    }
    let number = pr_number(draft, policy)?;
    let repository = &policy.repository;
    let endpoint = format!("repos/{repository}/pulls/{number}");
    let read = |endpoint: &str, fields: &[String]| -> Result<Value> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return reject("GitHub merge check timed out");
        }
        Ok(serde_json::from_slice(&read_bytes_for(
            gh,
            endpoint,
            fields,
            remaining.min(Duration::from_secs(12)),
        )?)?)
    };
    let pr = read(&endpoint, &[])?;
    if merge_scope_matches(&pr, policy, number, draft)
        && pr["merged"] == false
        && matches!(pr["state"].as_str(), Some("open" | "closed"))
    {
        return Ok(None);
    }
    let merged = parse_merge(&pr, policy, number, draft, base)?;
    if merged.merged_at <= evaluated_at {
        return reject("the GitHub merge must occur after the shadow evaluation");
    }
    let branch_endpoint = format!(
        "repos/{repository}/branches/{}",
        encode_branch(&policy.base_branch)
    );
    let tip = || -> Result<String> {
        let branch = read(&branch_endpoint, &[])?;
        branch["commit"]["sha"]
            .as_str()
            .filter(|sha| branch["name"] == policy.base_branch && valid_oid(sha))
            .map(str::to_owned)
            .ok_or_else(|| "GitHub did not return the destination branch tip".into())
    };
    let destination_tip = tip()?;
    let descendant = |ancestor: &str, child: &str| -> Result<()> {
        let comparison = read(
            &format!("repos/{repository}/compare/{ancestor}...{child}"),
            &["per_page=1".into()],
        )?;
        if comparison["base_commit"]["sha"] != ancestor
            || comparison["merge_base_commit"]["sha"] != ancestor
            || comparison["status"] != "ahead"
            || comparison["behind_by"] != 0
            || comparison["ahead_by"]
                .as_u64()
                .is_none_or(|count| count == 0)
        {
            return reject("GitHub could not prove merge ancestry on the approved destination");
        }
        Ok(())
    };
    descendant(base, &merged.result_commit)?;
    if destination_tip != merged.result_commit {
        descendant(&merged.result_commit, &destination_tip)?;
    }
    if parse_merge(&read(&endpoint, &[])?, policy, number, draft, base)? != merged
        || tip()? != destination_tip
    {
        return reject("GitHub merge or destination changed during inspection");
    }
    Ok(Some(craft_factory_evidence::GithubMergeOutcome {
        pull_request: draft.reference.clone(),
        head: draft.head.clone(),
        pinned_base: base.into(),
        destination_branch: policy.base_branch.clone(),
        result_commit: merged.result_commit,
        destination_tip,
        merged_at: merged.merged_at,
        actor: merged.actor,
        reference: merged.reference,
    }))
}

fn merge_approvals(
    history: &[(u64, Value)],
    policy: &Policy,
    draft: &Draft,
    cutoff: i64,
    author: u64,
    pr_api: &str,
) -> Result<Vec<MergeApproval>> {
    let mut latest = BTreeMap::new();
    for (review_id, review) in history {
        let Some(reviewer) = review["user"]["id"].as_u64().filter(|id| {
            *id != author && policy.reviewer_ids.contains(id) && review["user"]["type"] == "User"
        }) else {
            continue;
        };
        let state = review["state"]
            .as_str()
            .ok_or("GitHub review has no decision state")?;
        if matches!(state, "COMMENTED" | "PENDING") {
            continue;
        }
        if !matches!(state, "APPROVED" | "CHANGES_REQUESTED" | "DISMISSED")
            || review["pull_request_url"]
                .as_str()
                .is_none_or(|url| !url.eq_ignore_ascii_case(pr_api))
        {
            return reject("GitHub approval evidence contains an unexpected decision or PR");
        }
        let submitted = review["submitted_at"]
            .as_str()
            .ok_or("GitHub review has no submission time")?;
        let submitted_at =
            time::OffsetDateTime::parse(submitted, &time::format_description::well_known::Rfc3339)?
                .unix_timestamp();
        if submitted_at <= 0 {
            return reject("invalid GitHub review submission time");
        }
        if submitted_at == cutoff {
            return reject(
                "GitHub could not establish whether a review decision preceded the merge",
            );
        }
        if submitted_at > cutoff {
            continue;
        }
        if let Some((_, previous, _, _)) = latest.insert(
            reviewer,
            (*review_id, submitted_at, state, &review["commit_id"]),
        ) && previous > submitted_at
        {
            return reject("GitHub review decisions are out of order");
        }
    }
    let mut approvals = Vec::new();
    for (reviewer, (review_id, approved_at, state, head)) in latest {
        if state == "CHANGES_REQUESTED" {
            return reject("an approved GitHub reviewer still had a request for changes at merge");
        }
        if state == "APPROVED" && *head == draft.head {
            approvals.push(MergeApproval {
                actor: format!("github:user:{reviewer}"),
                approved_at,
                reference: format!("{}#pullrequestreview-{review_id}", draft.reference),
            });
        }
    }
    if approvals.len() < policy.minimum_merge_approvals {
        return reject(
            "the merged GitHub revision lacks the required independent approvals before merge",
        );
    }
    Ok(approvals)
}

fn parse_merge(
    pr: &Value,
    policy: &Policy,
    number: u64,
    draft: &Draft,
    pinned_base: &str,
) -> Result<MergeObservation> {
    if !merge_scope_matches(pr, policy, number, draft)
        || pr["state"] != "closed"
        || pr["merged"] != true
    {
        return reject(
            "GitHub has not reported a merge of the exact reviewed head into the approved destination",
        );
    }
    let commit = pr["merge_commit_sha"]
        .as_str()
        .filter(|commit| valid_oid(commit))
        .ok_or("GitHub did not report a full merged result commit")?;
    let actor = pr["merged_by"]["id"]
        .as_u64()
        .filter(|id| *id > 0)
        .ok_or("GitHub did not report the merge actor")?;
    let timestamp = pr["merged_at"]
        .as_str()
        .ok_or("GitHub did not report the merge time")?;
    let merged_at =
        time::OffsetDateTime::parse(timestamp, &time::format_description::well_known::Rfc3339)?
            .unix_timestamp();
    if merged_at <= 0 {
        return reject("invalid GitHub merge time");
    }
    Ok(MergeObservation {
        schema_version: 1,
        provider: "github".into(),
        draft: draft.clone(),
        pinned_base: pinned_base.into(),
        destination_branch: policy.base_branch.clone(),
        result_commit: commit.into(),
        merged_at,
        actor: format!("github:user:{actor}"),
        reference: format!("https://github.com/{}/commit/{commit}", policy.repository),
        approvals: Vec::new(),
    })
}

fn merge_scope_matches(pr: &Value, policy: &Policy, number: u64, draft: &Draft) -> bool {
    pr["number"].as_u64() == Some(number)
        && pr["html_url"] == draft.reference
        && pr["head"]["sha"] == draft.head
        && pr["base"]["ref"] == policy.base_branch
        && [&pr["base"], &pr["head"]].iter().all(|side| {
            side["repo"]["full_name"]
                .as_str()
                .is_some_and(|repo| repo.eq_ignore_ascii_case(&policy.repository))
        })
}

fn validate_check(response: &Value, required: &RequiredCheck, head: &str) -> Result<ObservedCheck> {
    let checks = response["check_runs"]
        .as_array()
        .ok_or("missing GitHub check runs")?;
    if response["total_count"].as_u64() != Some(checks.len() as u64) {
        return reject("GitHub returned an incomplete check result");
    }
    let mut matching = checks.iter().filter(|check| {
        check["name"].as_str() == Some(required.name.as_str())
            && check.pointer("/app/id").and_then(Value::as_u64) == Some(required.app_id)
    });
    let check = matching
        .next()
        .ok_or_else(|| format!("missing required check {:?}", required.name))?;
    if matching.next().is_some()
        || check["head_sha"].as_str() != Some(head)
        || check["status"] != "completed"
        || check["conclusion"] != "success"
    {
        return reject(format!(
            "required check {:?} is ambiguous, stale, or unsuccessful",
            required.name
        ));
    }
    Ok(ObservedCheck {
        id: check["id"]
            .as_u64()
            .filter(|id| *id > 0)
            .ok_or("missing check run identity")?,
        name: required.name.clone(),
        app_id: required.app_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn check_must_match_name_issuer_head_and_conclusion() {
        let required = RequiredCheck {
            name: "Trusted tests".into(),
            app_id: 72,
        };
        let mut run = json!({
            "id": 1, "name": "Trusted tests", "app": {"id": 72}, "head_sha": "candidate",
            "status": "completed", "conclusion": "success"
        });
        let wrap = |check| json!({"total_count": 1, "check_runs": [check]});
        assert!(validate_check(&wrap(run.clone()), &required, "candidate").is_ok());
        assert!(validate_check(&wrap(run.clone()), &required, "new head").is_err());
        run["app"]["id"] = json!(90);
        assert!(validate_check(&wrap(run.clone()), &required, "candidate").is_err());
        run["app"]["id"] = json!(72);
        run["conclusion"] = json!("failure");
        assert!(validate_check(&wrap(run), &required, "candidate").is_err());
    }
}
