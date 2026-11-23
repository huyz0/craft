use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::Deserialize;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{
    Result,
    bitbucket_change::{
        Approval, MAX_PULL_REQUEST_BYTES, collect, normalize_uuid, pull_request_endpoint,
        validate_approval,
    },
    reject,
    release_trace::Change,
    sha256,
};

pub const MAX_POLICY_BYTES: u64 = 32 * 1024;
pub const MAX_COMMENT_BYTES: u64 = 128 * 1024;
pub const MAX_ACTIVITY_BYTES: u64 = 1024 * 1024;
pub const MAX_ACTIVITY_PAGES: usize = 8;
pub const REVIEW_REFERENCE: &str = "bitbucket-native-review-v1:";

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    schema_version: u32,
    workspace: String,
    repository: String,
    repository_uuid: String,
    destination_branch: String,
    pull_request_id: u64,
    approval_comment_id: u64,
    /// UUID of the human change approver authorized by the policy owner.
    approver_uuid: String,
    approved_source_commit: String,
    result_commit: String,
    work_item: String,
    intent_reference: String,
    intent_revision: String,
    /// Legacy external merge event reference. Omit both merge fields to use Bitbucket activity.
    merge_reference: Option<String>,
    /// UTC Unix seconds for a legacy external merge event.
    merged_at: Option<u64>,
}

impl Policy {
    pub fn parse(bytes: &[u8], digest: &str) -> Result<Self> {
        if bytes.len() as u64 > MAX_POLICY_BYTES || sha256(bytes) != digest {
            return reject(
                "Bitbucket review policy does not match the approved digest and size limit",
            );
        }
        let policy: Self = serde_json::from_slice(bytes)?;
        if policy.merge_reference.is_some() != policy.merged_at.is_some() {
            return reject("provide both external merge fields or use native activity");
        }
        let merged_at = policy.merged_at.unwrap_or(1);
        validate_approval(&policy.approval(
            merged_at,
            "pending",
            merged_at,
            policy.merge_reference.as_deref().unwrap_or("pending"),
        ))?;
        if policy.approval_comment_id == 0 || normalize_uuid(&policy.approver_uuid).is_none() {
            return reject("invalid Bitbucket review approver or comment scope");
        }
        Ok(policy)
    }

    pub fn endpoint(&self) -> String {
        pull_request_endpoint(&self.workspace, &self.repository, self.pull_request_id)
    }

    pub fn comment_endpoint(&self) -> String {
        format!("{}/comments/{}", self.endpoint(), self.approval_comment_id)
    }

    pub fn activity_endpoint(&self) -> Option<String> {
        self.merged_at
            .is_none()
            .then(|| format!("{}/activity", self.endpoint()))
    }

    pub fn collect_activity(
        &self,
        get: impl FnMut(&str) -> Result<Vec<u8>>,
    ) -> Result<Vec<Vec<u8>>> {
        let Some(endpoint) = self.activity_endpoint() else {
            return Ok(Vec::new());
        };
        collect_activity_pages(&endpoint, get)
    }

    fn activity_merge(&self, pages: &[Vec<u8>]) -> Result<(OffsetDateTime, String)> {
        let endpoint = self
            .activity_endpoint()
            .ok_or("unexpected Bitbucket native activity")?;
        let event = merge_activity(
            &endpoint,
            self.pull_request_id,
            &format!("{}/{}", self.workspace, self.repository),
            &self.repository_uuid,
            &self.destination_branch,
            pages,
        )?;
        if event.source_hash != self.approved_source_commit {
            return reject(
                "Bitbucket merge activity does not match the approved PR, head, and destination",
            );
        }
        Ok((event.date, event.reference))
    }

    fn approval(
        &self,
        approved_at: u64,
        approval_reference: &str,
        merged_at: u64,
        merge_reference: &str,
    ) -> Approval {
        Approval {
            schema_version: self.schema_version,
            workspace: self.workspace.clone(),
            repository: self.repository.clone(),
            repository_uuid: self.repository_uuid.clone(),
            destination_branch: self.destination_branch.clone(),
            pull_request_id: self.pull_request_id,
            approved_source_commit: self.approved_source_commit.clone(),
            result_commit: self.result_commit.clone(),
            work_item: self.work_item.clone(),
            intent_reference: self.intent_reference.clone(),
            intent_revision: self.intent_revision.clone(),
            approval_reference: approval_reference.to_owned(),
            approved_at,
            merge_reference: merge_reference.to_owned(),
            merged_at,
        }
    }

    pub fn verify(
        &self,
        snapshot: &[u8],
        raw_comment: &[u8],
        activity: &[Vec<u8>],
        observed_at: u64,
    ) -> Result<(Vec<u8>, Change)> {
        if raw_comment.len() as u64 > MAX_COMMENT_BYTES
            || snapshot.len() as u64 > MAX_PULL_REQUEST_BYTES
        {
            return reject("Bitbucket review evidence exceeds the permitted byte limit");
        }
        let pr: Author = serde_json::from_slice(snapshot)?;
        let author = normalize_uuid(&pr.author.uuid).ok_or("invalid Bitbucket PR author UUID")?;
        let comment: Comment = serde_json::from_slice(raw_comment)?;
        let approver =
            normalize_uuid(&comment.user.uuid).ok_or("invalid Bitbucket comment author UUID")?;
        if comment.kind != "pullrequest_comment"
            || comment.deleted
            || comment.inline.is_some()
            || comment.parent.is_some()
            || comment.id != self.approval_comment_id
            || comment.pullrequest.id != self.pull_request_id
            || normalize_uuid(&self.approver_uuid).as_deref() != Some(approver.as_str())
            || author == approver
        {
            return reject(
                "Bitbucket approval comment is deleted, mis-scoped, or from an ineligible author",
            );
        }
        let created = OffsetDateTime::parse(&comment.created_on, &Rfc3339)?;
        let updated = OffsetDateTime::parse(&comment.updated_on, &Rfc3339)?;
        let approved_at = u64::try_from(updated.unix_timestamp())?;
        let (merge_time, merge_reference) =
            if let (Some(seconds), Some(reference)) = (self.merged_at, &self.merge_reference) {
                if !activity.is_empty() {
                    return reject("external merge policy cannot include native activity");
                }
                (
                    OffsetDateTime::from_unix_timestamp(i64::try_from(seconds)?)?,
                    reference.clone(),
                )
            } else {
                self.activity_merge(activity)?
            };
        if updated < created || updated >= merge_time {
            return reject(
                "Bitbucket approval comment must predate the merge and its current text must be pre-merge",
            );
        }
        let raw = comment.content.raw.trim();
        let (marker, body) = raw
            .split_once('\n')
            .ok_or("missing explicit Bitbucket release approval")?;
        if marker.trim_end() != "Craft release approval v1" {
            return reject("missing explicit Bitbucket release approval");
        }
        let decision: Decision = serde_json::from_str(body)
            .map_err(|_| "invalid Bitbucket release approval decision")?;
        if decision.decision != "approve"
            || decision.source_commit != self.approved_source_commit
            || decision.work_item != self.work_item
            || decision.intent_reference != self.intent_reference
            || decision.intent_revision != self.intent_revision
        {
            return reject(
                "Bitbucket approval comment does not approve the exact source commit and intent",
            );
        }
        let reference = format!(
            "{REVIEW_REFERENCE}https://bitbucket.org/{}/{}/pull-requests/{}#comment-{}#{}",
            self.workspace,
            self.repository,
            self.pull_request_id,
            self.approval_comment_id,
            sha256(raw_comment)
        );
        let merged_at = u64::try_from(merge_time.unix_timestamp())?;
        let approval = serde_json::to_vec_pretty(&self.approval(
            approved_at,
            &reference,
            merged_at,
            &merge_reference,
        ))?;
        let change = collect(snapshot, &approval, &sha256(&approval), observed_at)?;
        Ok((approval, change))
    }
}

pub fn collect_activity_pages(
    endpoint: &str,
    mut get: impl FnMut(&str) -> Result<Vec<u8>>,
) -> Result<Vec<Vec<u8>>> {
    let mut pages = Vec::new();
    scan_activity(endpoint, |url| {
        let raw = get(url)?;
        let page = ActivityPage::parse(&raw)?;
        pages.push(raw);
        Ok(page)
    })?;
    Ok(pages)
}

pub struct MergeActivity {
    pub date: OffsetDateTime,
    pub source_hash: String,
    pub reference: String,
}

pub fn merge_activity(
    endpoint: &str,
    pull_request_id: u64,
    repository: &str,
    repository_uuid: &str,
    destination_branch: &str,
    pages: &[Vec<u8>],
) -> Result<MergeActivity> {
    let repository_uuid =
        normalize_uuid(repository_uuid).ok_or("invalid Bitbucket repository UUID")?;
    let mut pages = pages.iter();
    let mut merged = None;
    scan_activity(endpoint, |_| {
        let raw = pages
            .next()
            .ok_or("incomplete archived Bitbucket activity")?;
        let page = ActivityPage::parse(raw)?;
        for entry in &page.values {
            let Some(update) = entry
                .update
                .as_ref()
                .filter(|update| update.state == "MERGED")
            else {
                continue;
            };
            if entry
                .pull_request
                .as_ref()
                .is_none_or(|pr| pr.id != pull_request_id)
                || update.destination.as_ref().is_none_or(|destination| {
                    destination.branch.name != destination_branch
                        || destination.repository.full_name != repository
                        || normalize_uuid(&destination.repository.uuid).as_deref()
                            != Some(repository_uuid.as_str())
                })
            {
                return reject(
                    "Bitbucket merge activity does not match the approved PR, head, and destination",
                );
            }
            let source = update
                .source
                .as_ref()
                .ok_or("Bitbucket merge activity has no source commit")?;
            let date = OffsetDateTime::parse(
                update
                    .date
                    .as_deref()
                    .ok_or("Bitbucket merge activity has no event date")?,
                &Rfc3339,
            )?;
            if merged.is_some() {
                return reject("Bitbucket activity lists multiple merge events");
            }
            merged = Some(MergeActivity {
                date,
                source_hash: source.commit.hash.clone(),
                reference: format!("{endpoint}#{}", sha256(raw)),
            });
        }
        Ok(page)
    })?;
    if pages.next().is_some() {
        return reject("archived Bitbucket activity contains unexpected pages");
    }
    merged.ok_or_else(|| "Bitbucket activity does not contain a merge event".into())
}

fn scan_activity(
    endpoint: &str,
    mut page_at: impl FnMut(&str) -> Result<ActivityPage>,
) -> Result<()> {
    let prefix = format!("{endpoint}?");
    let mut url = endpoint.to_owned();
    let mut visited = BTreeSet::new();
    for _ in 0..MAX_ACTIVITY_PAGES {
        if !visited.insert(url.clone()) {
            return reject("Bitbucket activity pagination repeated a page");
        }
        let Some(next) = page_at(&url)?.next else {
            return Ok(());
        };
        if next.len() > 2048
            || !next.strip_prefix(&prefix).is_some_and(|query| {
                !query.is_empty()
                    && query
                        .bytes()
                        .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b'#' | b'\\'))
            })
        {
            return reject("Bitbucket activity pagination left the approved pull request");
        }
        url = next;
    }
    reject("Bitbucket activity exceeds the permitted page limit")
}

#[derive(Deserialize)]
struct ActivityPage {
    values: Vec<ActivityEntry>,
    next: Option<String>,
}

impl ActivityPage {
    fn parse(raw: &[u8]) -> Result<Self> {
        if raw.len() as u64 > MAX_ACTIVITY_BYTES {
            return reject("Bitbucket activity page exceeds the permitted byte limit");
        }
        Ok(serde_json::from_slice(raw)?)
    }
}

#[derive(Deserialize)]
struct ActivityEntry {
    update: Option<Update>,
    pull_request: Option<PullRequest>,
}

#[derive(Deserialize)]
struct Update {
    state: String,
    date: Option<String>,
    source: Option<ActivitySource>,
    destination: Option<ActivityDestination>,
}

#[derive(Deserialize)]
struct ActivitySource {
    commit: ActivityCommit,
}

#[derive(Deserialize)]
struct ActivityCommit {
    hash: String,
}

#[derive(Deserialize)]
struct ActivityDestination {
    branch: ActivityBranch,
    repository: ActivityRepository,
}

#[derive(Deserialize)]
struct ActivityBranch {
    name: String,
}

#[derive(Deserialize)]
struct ActivityRepository {
    full_name: String,
    uuid: String,
}

#[derive(Deserialize)]
struct Author {
    author: User,
}

#[derive(Deserialize)]
struct User {
    uuid: String,
}

#[derive(Deserialize)]
struct Comment {
    #[serde(rename = "type")]
    kind: String,
    id: u64,
    deleted: bool,
    inline: Option<serde::de::IgnoredAny>,
    parent: Option<serde::de::IgnoredAny>,
    pullrequest: PullRequest,
    user: User,
    content: Content,
    created_on: String,
    updated_on: String,
}

#[derive(Deserialize)]
struct PullRequest {
    id: u64,
}

#[derive(Deserialize)]
struct Content {
    raw: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Decision {
    decision: String,
    source_commit: String,
    work_item: String,
    intent_reference: String,
    intent_revision: String,
}
