use std::collections::BTreeSet;
use std::path::Path;
use std::time::Instant;

use craft_factory_evidence::bitbucket_change::{
    MAX_PULL_REQUEST_BYTES, normalize_uuid, valid_slug,
};
use craft_factory_evidence::bitbucket_review::{
    MAX_ACTIVITY_BYTES, MAX_COMMENT_BYTES, collect_activity_pages, merge_activity,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::bitbucket_publish::branch_url;
use crate::bitbucket_read::{CURRENT_USER_URL, ReadClient};
use crate::bitbucket_shadow::valid_branch;
use crate::order::valid_oid;
use crate::review::{
    Draft, EvidenceSource, FeedbackTarget, MergeApproval, MergeObservation, ReviewRecord,
    merge_approval_matches, normalize_repair_note,
};
use crate::store::Receipt;
use crate::{Result, reject, twg::hash_matches};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    schema_version: u32,
    workspace: String,
    repository: String,
    repository_uuid: String,
    base_branch: String,
    reviewer_uuids: Vec<String>,
    #[serde(default)]
    minimum_merge_approvals: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Decision {
    source_commit: String,
    work_item: String,
    note: Option<String>,
}

struct Target<'a> {
    draft: &'a Draft,
    repo: String,
    uuid: String,
    pr_id: u64,
    endpoint: String,
}

impl Policy {
    pub(crate) fn base_branch(&self) -> &str {
        &self.base_branch
    }

    pub(crate) fn shared_merge_repository(&self) -> Result<String> {
        if self.schema_version != 1
            || !valid_slug(&self.workspace)
            || !valid_slug(&self.repository)
            || !valid_branch(&self.base_branch)
            || normalize_uuid(&self.repository_uuid).is_none()
        {
            return reject("invalid Bitbucket review policy scope");
        }
        Ok(format!("{}/{}", self.workspace, self.repository))
    }

    pub fn merge_approval_policy(
        &self,
        receipt: &Receipt,
        repo: &str,
        branch: &str,
    ) -> Result<Option<String>> {
        let target = self.target(receipt)?;
        if target.repo != repo || self.base_branch != branch {
            return reject(
                "the Bitbucket feedback policy does not match the CI repository and branch",
            );
        }
        let reviewers = self.reviewers()?;
        if self.minimum_merge_approvals > reviewers.len() {
            return reject("the Bitbucket approval minimum exceeds the eligible reviewer count");
        }
        if self.minimum_merge_approvals == 0 {
            return Ok(None);
        }
        Ok(Some(craft_factory_evidence::sha256(&serde_json::to_vec(
            &(
                "bitbucket",
                target.repo,
                target.uuid,
                &self.base_branch,
                self.minimum_merge_approvals,
                reviewers,
            ),
        )?)))
    }

    fn reviewers(&self) -> Result<BTreeSet<String>> {
        let reviewers: BTreeSet<_> = self
            .reviewer_uuids
            .iter()
            .filter_map(|value| normalize_uuid(value))
            .collect();
        if reviewers.len() > 32 || reviewers.len() != self.reviewer_uuids.len() {
            return reject("Bitbucket reviewer UUIDs must be valid and distinct");
        }
        Ok(reviewers)
    }

    fn target<'a>(&self, receipt: &'a Receipt) -> Result<Target<'a>> {
        let draft = receipt
            .draft
            .as_ref()
            .ok_or("bind the published Bitbucket pull request first")?;
        if receipt.publication_hold.is_some()
            || receipt.candidate.as_deref() != Some(draft.head.as_str())
        {
            return reject(
                "bind the current candidate to a confirmed Bitbucket pull request first",
            );
        }
        self.target_for_draft(draft)
    }

    fn target_for_draft<'a>(&self, draft: &'a Draft) -> Result<Target<'a>> {
        let repo = self.shared_merge_repository()?;
        let uuid =
            normalize_uuid(&self.repository_uuid).ok_or("invalid Bitbucket repository UUID")?;
        let prefix = format!("https://bitbucket.org/{repo}/pull-requests/");
        let pr_id = draft
            .reference
            .strip_prefix(&prefix)
            .ok_or("the draft is outside the approved Bitbucket repository")?
            .parse()?;
        if pr_id == 0 || draft.reference != format!("{prefix}{pr_id}") {
            return reject("the draft is not a canonical Bitbucket pull request URL");
        }
        let endpoint =
            format!("https://api.bitbucket.org/2.0/repositories/{repo}/pullrequests/{pr_id}");
        Ok(Target {
            draft,
            repo,
            uuid,
            pr_id,
            endpoint,
        })
    }
}

impl Target<'_> {
    fn matches_repository(&self, pr: &Value) -> bool {
        [
            &pr["source"]["repository"],
            &pr["destination"]["repository"],
        ]
        .iter()
        .all(|repo| {
            repo["full_name"] == self.repo && uuid(repo).as_deref() == Some(self.uuid.as_str())
        })
    }
}

pub(crate) fn read_client() -> Result<ReadClient> {
    read_client_with_credentials(None, None)
}

pub(crate) const EVIDENCE_TOKEN: &str = "CRAFT_FACTORY_BITBUCKET_EVIDENCE_TOKEN";

pub(crate) fn read_client_with_credentials(
    token_file: Option<&Path>,
    email: Option<String>,
) -> Result<ReadClient> {
    ReadClient::from_credentials(
        EVIDENCE_TOKEN,
        "CRAFT_FACTORY_BITBUCKET_EVIDENCE_EMAIL",
        token_file,
        email,
    )
}

pub fn observe(
    receipt: &Receipt,
    policy: &Policy,
    comment: u64,
    note: String,
) -> Result<ReviewRecord> {
    let client = read_client()?;
    observe_with(receipt, policy, comment, note, |url, limit| {
        Ok(serde_json::from_slice(&client.get(url, limit)?)?)
    })
}

fn observe_with(
    receipt: &Receipt,
    policy: &Policy,
    comment_id: u64,
    note: String,
    get: impl FnMut(&str, u64) -> Result<Value>,
) -> Result<ReviewRecord> {
    observe_target_with(
        policy.target(receipt)?,
        &receipt.work_order.base,
        &receipt.work_order.issue,
        policy,
        comment_id,
        Some(note),
        get,
    )
}

pub(crate) fn observe_assigned(
    binding: &FeedbackTarget,
    policy: &Policy,
    comment_id: u64,
) -> Result<ReviewRecord> {
    let client = read_client()?;
    observe_target_with(
        policy.target_for_draft(&binding.draft)?,
        &binding.base,
        binding
            .issue
            .as_deref()
            .ok_or("the Bitbucket assignment has no issue reference")?,
        policy,
        comment_id,
        None,
        |url, limit| Ok(serde_json::from_slice(&client.get(url, limit)?)?),
    )
}

fn observe_target_with(
    target: Target<'_>,
    base: &str,
    issue: &str,
    policy: &Policy,
    comment_id: u64,
    local_note: Option<String>,
    mut get: impl FnMut(&str, u64) -> Result<Value>,
) -> Result<ReviewRecord> {
    let reviewers = policy.reviewers()?;
    if reviewers.is_empty() || comment_id == 0 || !valid_oid(base) || !valid_oid(&target.draft.head)
    {
        return reject("invalid Bitbucket feedback policy or comment identity");
    }
    let signed_in = if local_note.is_none() {
        let account = get(CURRENT_USER_URL, MAX_COMMENT_BYTES)?;
        Some(uuid(&account).ok_or("Bitbucket evidence credentials must identify the reviewer")?)
    } else {
        None
    };
    let Target {
        draft,
        repo,
        pr_id,
        endpoint,
        ..
    } = &target;
    let head = &draft.head;
    let comment_url = format!("{endpoint}/comments/{comment_id}");
    let validate_pr = |pr: &Value| -> Result<(String, String)> {
        let source = pr["source"]["branch"]["name"]
            .as_str()
            .filter(|branch| valid_branch(branch))
            .ok_or("invalid Bitbucket source branch")?;
        let author = uuid(&pr["author"]).ok_or("missing Bitbucket PR author UUID")?;
        if pr["id"].as_u64() != Some(*pr_id)
            || pr["state"] != "OPEN"
            || pr["links"]["html"]["href"] != draft.reference
            || pr["destination"]["branch"]["name"] != policy.base_branch
            || !target.matches_repository(pr)
            || !hash_matches(pr.pointer("/source/commit/hash"), head)
            || !hash_matches(pr.pointer("/destination/commit/hash"), base)
        {
            return reject(
                "Bitbucket pull request does not match the approved repository, head and base",
            );
        }
        Ok((source.to_owned(), author))
    };
    let validate_comment = |comment: &Value| -> Result<(String, Option<String>)> {
        let reviewer = uuid(&comment["user"])
            .filter(|actor| reviewers.contains(actor))
            .ok_or("Bitbucket repair comment requires an approved reviewer")?;
        if signed_in
            .as_ref()
            .is_some_and(|account| account != &reviewer)
        {
            return reject("Bitbucket change request belongs to a different Bitbucket account");
        }
        if !active_comment(comment, *pr_id, comment_id) {
            return reject(
                "Bitbucket repair evidence requires the selected active top-level PR comment",
            );
        }
        let (marker, decision) = comment["content"]["raw"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .split_once('\n')
            .ok_or("missing explicit Bitbucket change request marker")?;
        if marker.trim_end() != "Craft request changes v1" {
            return reject("missing explicit Bitbucket change request marker");
        }
        let decision: Decision = serde_json::from_str(decision)
            .map_err(|_| "invalid Bitbucket change request marker")?;
        if decision.source_commit != *head || decision.work_item != issue {
            return reject(
                "Bitbucket reviewer did not request changes on the exact commit and work item",
            );
        }
        Ok((reviewer, decision.note))
    };
    let first = get(endpoint, MAX_PULL_REQUEST_BYTES)?;
    let (source, author) = validate_pr(&first)?;
    let comment = get(&comment_url, MAX_COMMENT_BYTES)?;
    let (reviewer, note) = validate_comment(&comment)?;
    if reviewer == author {
        return reject("Bitbucket change request must come from someone other than the PR author");
    }
    require_active_request(&first, &reviewer)?;
    for (branch, expected) in [
        (source.as_str(), head.as_str()),
        (&policy.base_branch, base),
    ] {
        let live = get(&branch_url(repo, branch), MAX_PULL_REQUEST_BYTES)?;
        if live["name"] != branch || live["target"]["hash"] != expected {
            return reject("Bitbucket branch moved away from the approved review revision");
        }
    }
    if get(&comment_url, MAX_COMMENT_BYTES)? != comment {
        return reject("the Bitbucket repair comment changed during inspection");
    }
    let last = get(endpoint, MAX_PULL_REQUEST_BYTES)?;
    if validate_pr(&last)? != (source, author) {
        return reject("the Bitbucket pull request changed during inspection");
    }
    require_active_request(&last, &reviewer)?;
    Ok(ReviewRecord {
        schema_version: 1,
        draft: (*draft).clone(),
        source: EvidenceSource::OperatorObserved,
        actor: format!("bitbucket:user:{reviewer}"),
        evidence: format!("{}#comment-{comment_id}", draft.reference),
        note: match local_note {
            Some(note) => note,
            None => normalize_repair_note(note.as_deref().unwrap_or_default(), "Bitbucket")?,
        },
    })
}

fn uuid(value: &Value) -> Option<String> {
    value["uuid"].as_str().and_then(normalize_uuid)
}

pub fn observe_merge(
    receipt: &Receipt,
    policy: &Policy,
    comments: &[u64],
) -> Result<MergeObservation> {
    let client = read_client()?;
    observe_merge_with(receipt, policy, comments, |url, limit| {
        client.get(url, limit)
    })
}

pub(crate) fn observe_shared_merge_until(
    draft: &Draft,
    repository: &str,
    base: &str,
    branch: &str,
    policy: &Policy,
    client: &ReadClient,
    deadline: Instant,
) -> Result<Option<craft_factory_evidence::BitbucketMergeOutcome>> {
    observe_shared_merge_with(draft, repository, base, branch, policy, |url, limit| {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return reject("Bitbucket merge check timed out");
        }
        client.get_for(url, limit, remaining)
    })
}

fn observe_shared_merge_with(
    draft: &Draft,
    repository: &str,
    base: &str,
    branch: &str,
    policy: &Policy,
    get: impl FnMut(&str, u64) -> Result<Vec<u8>>,
) -> Result<Option<craft_factory_evidence::BitbucketMergeOutcome>> {
    let target = policy.target_for_draft(draft)?;
    if repository != target.repo
        || branch != policy.base_branch
        || !valid_oid(base)
        || !valid_oid(&draft.head)
    {
        return reject(
            "the Bitbucket review policy or candidate differs from the confirmed publication",
        );
    }
    let Some(merge) = observe_merge_for_target(&target, branch, base, None, get)? else {
        return Ok(None);
    };
    Ok(Some(craft_factory_evidence::BitbucketMergeOutcome {
        pull_request: merge.draft.reference,
        head: merge.draft.head,
        pinned_base: merge.pinned_base,
        destination_branch: merge.destination_branch,
        repository_uuid: target.uuid,
        result_commit: merge.result_commit,
        merged_at: merge.merged_at,
        actor: merge.actor,
        reference: merge.reference,
    }))
}

fn observe_merge_with(
    receipt: &Receipt,
    policy: &Policy,
    comments: &[u64],
    get: impl FnMut(&str, u64) -> Result<Vec<u8>>,
) -> Result<MergeObservation> {
    let target = policy.target(receipt)?;
    let approval_policy =
        policy.merge_approval_policy(receipt, &target.repo, &policy.base_branch)?;
    merge_approval_matches(
        receipt,
        "bitbucket_checks_observed",
        approval_policy.as_deref(),
    )?;
    let ids: BTreeSet<_> = comments.iter().copied().collect();
    if ids.contains(&0)
        || ids.len() != comments.len()
        || ids.len() > policy.reviewers()?.len()
        || ids.len() < policy.minimum_merge_approvals
        || (approval_policy.is_none() && !ids.is_empty())
    {
        return reject(
            "provide distinct Bitbucket approval comment IDs for the configured minimum and reviewer roster",
        );
    }
    observe_merge_for_target(
        &target,
        &policy.base_branch,
        &receipt.work_order.base,
        Some((receipt, policy, ids)),
        get,
    )?
    .ok_or_else(|| UNCONFIRMED_MERGE.into())
}

pub(crate) const UNCONFIRMED_MERGE: &str =
    "Bitbucket has not reported a merge of the exact reviewed head into the approved destination";

fn observe_merge_for_target(
    target: &Target<'_>,
    branch: &str,
    base: &str,
    approval_policy: Option<(&Receipt, &Policy, BTreeSet<u64>)>,
    mut get: impl FnMut(&str, u64) -> Result<Vec<u8>>,
) -> Result<Option<MergeObservation>> {
    let head = &target.draft.head;
    let matches_scope = |pr: &Value| {
        pr["id"].as_u64() == Some(target.pr_id)
            && pr["links"]["html"]["href"] == target.draft.reference
            && pr["destination"]["branch"]["name"] == branch
            && target.matches_repository(pr)
            && hash_matches(pr.pointer("/source/commit/hash"), head)
    };
    let parse_pr = |pr: &Value| -> Result<(String, String)> {
        if pr["state"] != "MERGED" || !matches_scope(pr) {
            return reject(UNCONFIRMED_MERGE);
        }
        let commit = pr["merge_commit"]["hash"]
            .as_str()
            .filter(|hash| commit_prefix(hash, head.len()))
            .ok_or("Bitbucket did not report a result commit")?;
        let actor = uuid(&pr["closed_by"]).ok_or("Bitbucket did not report the merge actor")?;
        Ok((commit.to_owned(), actor))
    };
    let first: Value = serde_json::from_slice(&get(&target.endpoint, MAX_PULL_REQUEST_BYTES)?)?;
    if approval_policy.is_none()
        && matches_scope(&first)
        && matches!(first["state"].as_str(), Some("OPEN" | "DECLINED"))
    {
        let prefix = first
            .pointer("/source/commit/hash")
            .and_then(Value::as_str)
            .ok_or(UNCONFIRMED_MERGE)?;
        if resolve_commit(&target.repo, prefix, head.len(), &mut get)? == *head {
            return Ok(None);
        }
        return reject(UNCONFIRMED_MERGE);
    }
    let (result_prefix, actor) = parse_pr(&first)?;
    let activity_url = format!("{}/activity", target.endpoint);
    let pages = collect_activity_pages(&activity_url, |url| {
        get(url, MAX_ACTIVITY_BYTES).map_err(|error| error.to_string().into())
    })
    .map_err(|error| error.to_string())?;
    let event = merge_activity(
        &activity_url,
        target.pr_id,
        &target.repo,
        &target.uuid,
        branch,
        &pages,
    )
    .map_err(|error| error.to_string())?;
    if !head.starts_with(&event.source_hash)
        || resolve_commit(&target.repo, &event.source_hash, head.len(), &mut get)? != *head
    {
        return reject("Bitbucket merge activity does not match the exact reviewed head");
    }
    let result_commit = resolve_commit(&target.repo, &result_prefix, head.len(), &mut get)?;
    let mut approvals = Vec::new();
    let mut seen = BTreeSet::new();
    if let Some((receipt, policy, ids)) = &approval_policy {
        for &id in ids {
            let url = format!("{}/comments/{id}", target.endpoint);
            let raw = get(&url, MAX_COMMENT_BYTES)?;
            let comment: Value = serde_json::from_slice(&raw)?;
            let (reviewer, approval) =
                parse_approval(&comment, id, target, policy, receipt, &first, event.date)?;
            if !seen.insert(reviewer) {
                return reject("Bitbucket approval comments must come from distinct reviewers");
            }
            if get(&url, MAX_COMMENT_BYTES)? != raw {
                return reject("a Bitbucket approval comment changed during inspection");
            }
            approvals.push(approval);
        }
    }
    let last: Value = serde_json::from_slice(&get(&target.endpoint, MAX_PULL_REQUEST_BYTES)?)?;
    if parse_pr(&last)? != (result_prefix, actor.clone()) {
        return reject("Bitbucket merge outcome changed during inspection");
    }
    if let Some((_, policy, _)) = approval_policy.filter(|_| !approvals.is_empty()) {
        if uuid(&last["author"]) != uuid(&first["author"]) {
            return reject("Bitbucket pull request author changed during inspection");
        }
        verify_approval_state(&last, &policy.reviewers()?, &seen)?;
    }
    let merged_at = event.date.unix_timestamp();
    if merged_at <= 0 {
        return reject("invalid Bitbucket merge time");
    }
    Ok(Some(MergeObservation {
        schema_version: 1,
        provider: "bitbucket".into(),
        draft: target.draft.clone(),
        pinned_base: base.to_owned(),
        destination_branch: branch.to_owned(),
        reference: format!(
            "https://bitbucket.org/{}/commits/{result_commit}",
            target.repo
        ),
        result_commit,
        merged_at,
        actor: format!("bitbucket:user:{actor}"),
        approvals,
    }))
}

fn commit_prefix(prefix: &str, oid_len: usize) -> bool {
    (12..=oid_len).contains(&prefix.len())
        && prefix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn active_comment(comment: &Value, pr_id: u64, comment_id: u64) -> bool {
    comment["type"] == "pullrequest_comment"
        && comment["deleted"] == false
        && (comment["pending"].is_null() || comment["pending"] == false)
        && comment["resolution"].is_null()
        && comment["inline"].is_null()
        && comment["parent"].is_null()
        && comment["id"].as_u64() == Some(comment_id)
        && comment["pullrequest"]["id"].as_u64() == Some(pr_id)
}

fn parse_approval(
    comment: &Value,
    id: u64,
    target: &Target<'_>,
    policy: &Policy,
    receipt: &Receipt,
    pr: &Value,
    merged_at: time::OffsetDateTime,
) -> Result<(String, MergeApproval)> {
    if !active_comment(comment, target.pr_id, id) {
        return reject("Bitbucket approval evidence requires an active top-level PR comment");
    }
    let author = uuid(&pr["author"]).ok_or("missing Bitbucket PR author UUID")?;
    let reviewers = policy.reviewers()?;
    let reviewer = uuid(&comment["user"])
        .filter(|actor| actor != &author && reviewers.contains(actor))
        .filter(|_| comment["user"]["type"] == "user")
        .ok_or("Bitbucket approval requires an eligible reviewer other than the PR author")?;
    let (marker, payload) = comment["content"]["raw"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .split_once('\n')
        .ok_or("missing explicit Bitbucket merge approval marker")?;
    if marker.trim_end() != "Craft approve merge v1" {
        return reject("missing explicit Bitbucket merge approval marker");
    }
    let decision: Decision =
        serde_json::from_str(payload).map_err(|_| "invalid Bitbucket merge approval marker")?;
    if decision.source_commit != target.draft.head || decision.work_item != receipt.work_order.issue
    {
        return reject("Bitbucket approval does not name the exact merged revision and work item");
    }
    let parse_time = |field: &str| -> Result<time::OffsetDateTime> {
        let value = comment[field]
            .as_str()
            .ok_or("Bitbucket approval comment has no timestamp")?;
        Ok(time::OffsetDateTime::parse(
            value,
            &time::format_description::well_known::Rfc3339,
        )?)
    };
    let created = parse_time("created_on")?;
    let updated = parse_time("updated_on")?;
    if created.unix_timestamp() <= 0 || updated < created || updated >= merged_at {
        return reject("Bitbucket approval comment and its current text must precede the merge");
    }
    verify_approval_state(pr, &reviewers, &BTreeSet::from([reviewer.clone()]))?;
    let approval = MergeApproval {
        actor: format!("bitbucket:user:{reviewer}"),
        approved_at: updated.unix_timestamp(),
        reference: format!("{}#comment-{id}", target.draft.reference),
    };
    Ok((reviewer, approval))
}

fn verify_approval_state(
    pr: &Value,
    eligible: &BTreeSet<String>,
    approvals: &BTreeSet<String>,
) -> Result<()> {
    let participants = pr["participants"]
        .as_array()
        .ok_or("Bitbucket participants are missing")?;
    let mut seen = BTreeSet::new();
    let mut approved = BTreeSet::new();
    for participant in participants {
        let Some(actor) = uuid(&participant["user"]).filter(|id| eligible.contains(id)) else {
            continue;
        };
        if !seen.insert(actor.clone()) {
            return reject("Bitbucket returned duplicate reviewer state");
        }
        if participant["state"] == "changes_requested" {
            return reject("an eligible Bitbucket reviewer still has a request for changes");
        }
        if participant["approved"] == true && participant["state"] == "approved" {
            approved.insert(actor);
        }
    }
    if !approvals.is_subset(&approved) {
        return reject("a Bitbucket reviewer's native approval is missing or withdrawn");
    }
    Ok(())
}

fn resolve_commit(
    repo: &str,
    prefix: &str,
    oid_len: usize,
    get: &mut impl FnMut(&str, u64) -> Result<Vec<u8>>,
) -> Result<String> {
    if !commit_prefix(prefix, oid_len) {
        return reject("invalid Bitbucket merge commit reference");
    }
    if prefix.len() == oid_len && valid_oid(prefix) {
        return Ok(prefix.to_owned());
    }
    let url = format!("https://api.bitbucket.org/2.0/repositories/{repo}/commit/{prefix}");
    let commit: Value = serde_json::from_slice(&get(&url, MAX_PULL_REQUEST_BYTES)?)?;
    commit["hash"]
        .as_str()
        .filter(|hash| {
            commit["type"] == "commit"
                && valid_oid(hash)
                && hash.len() == oid_len
                && hash.starts_with(prefix)
        })
        .map(str::to_owned)
        .ok_or_else(|| "Bitbucket did not resolve the expected full commit".into())
}

fn require_active_request(pr: &Value, reviewer: &str) -> Result<()> {
    let participants = pr["participants"]
        .as_array()
        .ok_or("Bitbucket participants are missing")?;
    let mut matching = participants
        .iter()
        .filter(|row| uuid(&row["user"]).as_deref() == Some(reviewer));
    let participant = matching
        .next()
        .ok_or("Bitbucket feedback author is no longer a participant")?;
    if matching.next().is_some()
        || participant["state"] != "changes_requested"
        || participant["approved"] != false
    {
        return reject("Bitbucket reviewer does not have one active request for changes");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{NativeCi, State};
    use serde_json::json;

    const AUTHOR: &str = "00000000-0000-0000-0000-000000000001";
    const REVIEWER: &str = "00000000-0000-0000-0000-000000000002";
    const REPO: &str = "00000000-0000-0000-0000-000000000003";
    const PR: &str = "https://bitbucket.org/team/service/pull-requests/42";
    const ENDPOINT: &str =
        "https://api.bitbucket.org/2.0/repositories/team/service/pullrequests/42";

    fn fixture() -> (Policy, Receipt) {
        let head = "b".repeat(40);
        let base = "a".repeat(40);
        let policy = Policy {
            schema_version: 1,
            workspace: "team".into(),
            repository: "service".into(),
            repository_uuid: REPO.into(),
            base_branch: "main".into(),
            reviewer_uuids: vec![AUTHOR.into(), REVIEWER.into()],
            minimum_merge_approvals: 0,
        };
        let receipt = Receipt {
            schema_version: 1, work_order: serde_json::from_value(json!({
                "schema_version": 1, "id": "fixture", "issue": "TEST-1", "owner": "owner",
                "repository": "fixture", "base": base, "intent": "repair", "scope": ["src"],
                "acceptance": ["test passes"], "context": [], "skill": {"location":"skill","revision":base}
            })).unwrap(),
            state: State::VerificationAttested, candidate: Some(head.clone()),
            draft: Some(Draft { reference: PR.into(), head: head.clone() }),
            publication_hold: None, events: vec![], attestations: vec![],
            native_ci: NativeCi::Passed, native_merge: None, snapshot_problem: None, review_ready: true,
        };
        (policy, receipt)
    }

    #[test]
    fn merge_record_requires_matching_activity_and_resolves_abbreviated_commits() {
        let (policy, receipt) = fixture();
        let head = receipt.candidate.as_ref().unwrap();
        let result_commit = "c".repeat(40);
        let repo = json!({"full_name":"team/service","uuid":REPO});
        let original = json!({
            "id":42,"state":"MERGED","links":{"html":{"href":PR}},"closed_by":{"uuid":REVIEWER},
            "source":{"commit":{"hash":&head[..12]},"repository":repo},
            "destination":{"branch":{"name":"main"},"commit":{"hash":&result_commit[..12]},"repository":repo},
            "merge_commit":{"hash":&result_commit[..12]},"updated_on":"2026-11-22T00:01:00Z"
        });
        let merge = json!({
            "pull_request":{"id":42},"update":{"state":"MERGED","date":"2026-11-22T00:00:00Z",
                "source":{"commit":{"hash":&head[..12]}},"destination":{"branch":{"name":"main"},"repository":repo}}
        });
        let activity = format!("{ENDPOINT}/activity");
        let next = format!("{activity}?page=2");
        let source_url = format!(
            "https://api.bitbucket.org/2.0/repositories/team/service/commit/{}",
            &head[..12]
        );
        let result_url = format!(
            "https://api.bitbucket.org/2.0/repositories/team/service/commit/{}",
            &result_commit[..12]
        );

        for (fault, expected) in [
            ("", ""),
            ("open", ""),
            ("open_wrong_head", "has not reported a merge"),
            (
                "other_endpoint",
                "pagination left the approved pull request",
            ),
            ("other_destination", "approved PR, head, and destination"),
            ("duplicate", "multiple merge events"),
            ("stale", "exact reviewed head"),
            ("wrong_resolution", "resolve the expected full commit"),
            ("drift", "merge outcome changed"),
        ] {
            let mut first = original.clone();
            let mut last = original.clone();
            let mut page_one = json!({"values":[],"next":next});
            let mut page_two = json!({"values":[merge]});
            let mut resolved = json!({"type":"commit","hash":head});
            match fault {
                "open" => {
                    first["state"] = json!("OPEN");
                    first["source"]["commit"]["hash"] = json!(head);
                }
                "open_wrong_head" => {
                    first["state"] = json!("OPEN");
                    first["source"]["commit"]["hash"] = json!(result_commit);
                }
                "other_endpoint" => {
                    page_one["next"] = json!("https://example.invalid/another-activity?page=2")
                }
                "other_destination" => {
                    page_two["values"][0]["update"]["destination"]["branch"]["name"] =
                        json!("release")
                }
                "duplicate" => page_two["values"] = json!([merge, merge]),
                "stale" => {
                    page_two["values"][0]["update"]["source"]["commit"]["hash"] =
                        json!(&result_commit[..12])
                }
                "wrong_resolution" => resolved["hash"] = json!(result_commit),
                "drift" => last["closed_by"]["uuid"] = json!(AUTHOR),
                "" => (),
                _ => unreachable!(),
            }
            let mut reads = [
                (ENDPOINT, MAX_PULL_REQUEST_BYTES, first),
                (&activity, MAX_ACTIVITY_BYTES, page_one),
                (&next, MAX_ACTIVITY_BYTES, page_two),
                (&source_url, MAX_PULL_REQUEST_BYTES, resolved),
                (
                    &result_url,
                    MAX_PULL_REQUEST_BYTES,
                    json!({"type":"commit","hash":result_commit}),
                ),
                (ENDPOINT, MAX_PULL_REQUEST_BYTES, last),
            ]
            .into_iter();
            let outcome = observe_shared_merge_with(
                receipt.draft.as_ref().unwrap(),
                "team/service",
                &receipt.work_order.base,
                "main",
                &policy,
                |url, limit| {
                    let (expected_url, expected_limit, response) = reads.next().unwrap();
                    assert_eq!((url, limit), (expected_url, expected_limit));
                    Ok(serde_json::to_vec(&response).unwrap())
                },
            );
            if fault == "open" {
                assert!(outcome.unwrap().is_none());
            } else if fault.is_empty() {
                let observation = outcome.unwrap().unwrap();
                assert!(reads.next().is_none());
                assert_eq!(observation.result_commit, result_commit);
                assert_eq!(observation.head, *head);
                assert_eq!(observation.pull_request, PR);
                assert_eq!(observation.repository_uuid, normalize_uuid(REPO).unwrap());
                assert_eq!(observation.actor, format!("bitbucket:user:{REVIEWER}"));
                assert_eq!(observation.pinned_base, receipt.work_order.base);
                assert_eq!(
                    observation.merged_at,
                    time::OffsetDateTime::parse(
                        "2026-11-22T00:00:00Z",
                        &time::format_description::well_known::Rfc3339,
                    )
                    .unwrap()
                    .unix_timestamp()
                );
            } else {
                let error = outcome.unwrap_err().to_string();
                assert!(error.contains(expected), "{fault}: {error}");
            }
        }
    }

    #[test]
    fn merge_approval_requires_an_active_reviewer_comment_from_before_merge() {
        let (mut policy, mut receipt) = fixture();
        policy.minimum_merge_approvals = 1;
        let head = receipt.candidate.clone().unwrap();
        let digest = policy
            .merge_approval_policy(&receipt, "team/service", "main")
            .unwrap();
        assert!(
            policy
                .merge_approval_policy(&receipt, "team/other", "main")
                .is_err()
        );
        receipt.events.push(crate::store::Event {
            kind: "bitbucket_checks_observed".into(),
            candidate: Some(head.clone()),
            note: json!({"merge_approval_policy":digest}).to_string(),
        });
        assert!(observe_merge_with(&receipt, &policy, &[], |_, _| unreachable!()).is_err());
        assert!(observe_merge_with(&receipt, &policy, &[61, 61], |_, _| unreachable!()).is_err());
        let repo = json!({"full_name":"team/service","uuid":REPO});
        let merged = json!({
            "id":42,"state":"MERGED","links":{"html":{"href":PR}},"closed_by":{"uuid":AUTHOR},"author":{"uuid":AUTHOR},
            "source":{"commit":{"hash":head},"repository":repo},"destination":{"branch":{"name":"main"},"repository":repo},
            "merge_commit":{"hash":"c".repeat(40)},
            "participants":[{"user":{"uuid":REVIEWER},"approved":true,"state":"approved"}]
        });
        let activity = json!({"values":[{"pull_request":{"id":42},"update":{
            "state":"MERGED","date":"2026-11-22T00:00:00Z","source":{"commit":{"hash":head}},
            "destination":{"branch":{"name":"main"},"repository":repo}
        }}]});
        let approved = json!({
            "id":61,"type":"pullrequest_comment","deleted":false,"pullrequest":{"id":42},
            "created_on":"2026-11-21T23:55:00Z","updated_on":"2026-11-21T23:59:00Z",
            "user":{"type":"user","uuid":REVIEWER},"content":{"raw":format!(
                "Craft approve merge v1\n{}",json!({"source_commit":head,"work_item":"TEST-1"})
            )}
        });
        for (fault, expected) in [
            ("", ""),
            ("self", "other than the PR author"),
            ("stale", "exact merged revision and work item"),
            ("late_edit", "must precede the merge"),
            ("withdrawn", "missing or withdrawn"),
            ("objection", "request for changes"),
            ("disabled", "approval policy differs"),
        ] {
            let first = merged.clone();
            let mut last = merged.clone();
            let mut comment = approved.clone();
            policy.minimum_merge_approvals = usize::from(fault != "disabled");
            match fault {
                "self" => comment["user"]["uuid"] = json!(AUTHOR),
                "stale" => {
                    comment["content"]["raw"] = json!(format!(
                        "Craft approve merge v1\n{}",
                        json!({
                            "source_commit":"c".repeat(40),"work_item":"TEST-1"
                        })
                    ))
                }
                "late_edit" => comment["updated_on"] = json!("2026-11-22T00:01:00Z"),
                "withdrawn" => last["participants"][0]["approved"] = json!(false),
                "objection" => last["participants"][0]["state"] = json!("changes_requested"),
                "" | "disabled" => (),
                _ => unreachable!(),
            }
            let activity_url = format!("{ENDPOINT}/activity");
            let comment_url = format!("{ENDPOINT}/comments/61");
            let mut reads = [
                (ENDPOINT, first),
                (&activity_url, activity.clone()),
                (&comment_url, comment.clone()),
                (&comment_url, comment),
                (ENDPOINT, last),
            ]
            .into_iter();
            let observed = observe_merge_with(&receipt, &policy, &[61], |url, _| {
                assert_ne!(
                    fault, "disabled",
                    "policy drift must be detected before a provider read"
                );
                let (expected_url, response) = reads.next().unwrap();
                assert_eq!(url, expected_url);
                Ok(serde_json::to_vec(&response).unwrap())
            });
            if fault.is_empty() {
                let merge = observed.unwrap();
                assert!(reads.next().is_none());
                assert_eq!(merge.approvals.len(), 1);
                assert_eq!(
                    merge.approvals[0].actor,
                    format!("bitbucket:user:{REVIEWER}")
                );
                assert_eq!(merge.approvals[0].reference, format!("{PR}#comment-61"));
                assert!(merge.approvals[0].approved_at < merge.merged_at);
            } else {
                let error = observed.unwrap_err().to_string();
                assert!(error.contains(expected), "{fault}: {error}");
            }
        }
    }

    #[test]
    fn repair_feedback_requires_active_independent_review_and_exact_commit() {
        let (policy, receipt) = fixture();
        let head = receipt.candidate.as_ref().unwrap();
        let base = &receipt.work_order.base;
        let repo = json!({"full_name":"team/service","uuid":REPO});
        let pr = json!({
            "id":42,"state":"OPEN","links":{"html":{"href":PR}},"author":{"uuid":AUTHOR},
            "source":{"branch":{"name":"repair/one"},"commit":{"hash":&head[..12]},"repository":repo},
            "destination":{"branch":{"name":"main"},"commit":{"hash":&base[..12]},"repository":repo},
            "participants":[{"user":{"uuid":REVIEWER},"state":"changes_requested","approved":false}]
        });
        let comment = json!({
            "type":"pullrequest_comment","id":61,"deleted":false,"pullrequest":{"id":42},
            "user":{"uuid":format!("{{{REVIEWER}}}")},"content":{"raw":format!("Craft request changes v1\n{}",json!({
                "source_commit":head,"work_item":"TEST-1"
            }))}
        });
        let comment_url = format!("{ENDPOINT}/comments/61");
        let source = branch_url("team/service", "repair/one");
        let destination = branch_url("team/service", "main");
        assert!(source.ends_with("/repair%2Fone"));
        for (fault, expected) in [
            ("", ""),
            ("self", "other than the PR author"),
            ("stale_marker", "exact commit and work item"),
            ("moved_ref", "branch moved"),
            ("withdrawn", "one active request"),
            ("edited", "comment changed"),
            ("deleted", "active top-level PR comment"),
            ("pending", "active top-level PR comment"),
            ("resolved", "active top-level PR comment"),
            ("changed_repo", "approved repository"),
        ] {
            let mut first = pr.clone();
            let mut last = pr.clone();
            let mut first_comment = comment.clone();
            let mut last_comment = comment.clone();
            let mut live_src = json!({"name":"repair/one","target":{"hash":head}});
            match fault {
                "self" => first["author"]["uuid"] = json!(REVIEWER),
                "stale_marker" => {
                    first_comment["content"]["raw"] = json!(format!(
                        "Craft request changes v1\n{}",
                        json!({"source_commit":base,"work_item":"TEST-1"})
                    ))
                }
                "moved_ref" => live_src["target"]["hash"] = json!(base),
                "withdrawn" => last["participants"][0]["state"] = json!("approved"),
                "edited" => last_comment["updated_on"] = json!("2026-11-22T00:00:00Z"),
                "deleted" => first_comment["deleted"] = json!(true),
                "pending" => first_comment["pending"] = json!(true),
                "resolved" => first_comment["resolution"] = json!({"type":"comment_resolution"}),
                "changed_repo" => first["source"]["repository"]["uuid"] = json!(AUTHOR),
                "" => (),
                _ => unreachable!(),
            }
            let mut reads = [
                (ENDPOINT, MAX_PULL_REQUEST_BYTES, first),
                (&comment_url, MAX_COMMENT_BYTES, first_comment),
                (&source, MAX_PULL_REQUEST_BYTES, live_src),
                (
                    &destination,
                    MAX_PULL_REQUEST_BYTES,
                    json!({"name":"main","target":{"hash":base}}),
                ),
                (&comment_url, MAX_COMMENT_BYTES, last_comment),
                (ENDPOINT, MAX_PULL_REQUEST_BYTES, last),
            ]
            .into_iter();
            let result = observe_with(
                &receipt,
                &policy,
                61,
                "Approved local summary".into(),
                |url, limit| {
                    let (expected_url, expected_limit, response) = reads.next().unwrap();
                    assert_eq!((url, limit), (expected_url, expected_limit));
                    Ok(response)
                },
            );
            if fault.is_empty() {
                let record = result.unwrap();
                assert!(reads.next().is_none());
                assert_eq!(record.actor, format!("bitbucket:user:{REVIEWER}"));
                assert_eq!(record.evidence, format!("{PR}#comment-61"));
                assert_eq!(record.note, "Approved local summary");
            } else {
                let error = result.unwrap_err().to_string();
                assert!(error.contains(expected), "{fault}: {error}");
            }
        }
    }

    #[test]
    fn assigned_review_requires_the_credential_owner_and_a_bound_comment_note() {
        let (policy, receipt) = fixture();
        let draft = receipt.draft.as_ref().unwrap();
        let head = &draft.head;
        let base = &receipt.work_order.base;
        let repo = json!({"full_name":"team/service","uuid":REPO});
        let pr = json!({
            "id":42,"state":"OPEN","links":{"html":{"href":PR}},"author":{"uuid":AUTHOR},
            "source":{"branch":{"name":"feature"},"commit":{"hash":head},"repository":repo},
            "destination":{"branch":{"name":"main"},"commit":{"hash":base},"repository":repo},
            "participants":[{"user":{"uuid":REVIEWER},"state":"changes_requested","approved":false}]
        });
        for (account, issue, note, error) in [
            (REVIEWER, "TEST-1", Some("Handle\n the empty value"), ""),
            (
                AUTHOR,
                "TEST-1",
                Some("Handle the empty value"),
                "different Bitbucket account",
            ),
            (
                REVIEWER,
                "TEST-2",
                Some("Handle the empty value"),
                "exact commit and work item",
            ),
            (REVIEWER, "TEST-1", None, "1 to 1000 characters"),
        ] {
            let comment = json!({
                "type":"pullrequest_comment","id":61,"deleted":false,"pullrequest":{"id":42},
                "user":{"uuid":REVIEWER},"content":{"raw":format!("Craft request changes v1\n{}",json!({
                    "source_commit":head,"work_item":issue,"note":note
                }))}
            });
            let result = observe_target_with(
                policy.target_for_draft(draft).unwrap(),
                base,
                "TEST-1",
                &policy,
                61,
                None,
                |url, _| match url {
                    CURRENT_USER_URL => Ok(json!({"uuid":format!("{{{account}}}")})),
                    ENDPOINT => Ok(pr.clone()),
                    "https://api.bitbucket.org/2.0/repositories/team/service/pullrequests/42/comments/61" => {
                        Ok(comment.clone())
                    }
                    "https://api.bitbucket.org/2.0/repositories/team/service/refs/branches/feature" => {
                        Ok(json!({"name":"feature","target":{"hash":head}}))
                    }
                    "https://api.bitbucket.org/2.0/repositories/team/service/refs/branches/main" => {
                        Ok(json!({"name":"main","target":{"hash":base}}))
                    }
                    _ => panic!("unexpected Bitbucket read {url}"),
                },
            );
            if error.is_empty() {
                let review = result.unwrap();
                assert_eq!(review.note, "Handle the empty value");
                assert_eq!(review.evidence, format!("{PR}#comment-61"));
            } else {
                let observed = result.unwrap_err().to_string();
                assert!(observed.contains(error), "{observed}");
            }
        }
    }
}
