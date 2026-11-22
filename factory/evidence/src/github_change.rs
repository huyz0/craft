use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{
    Result, reject,
    release_trace::{Change, Observation},
    sha256, valid_oid,
};

pub const MAX_APPROVAL_BYTES: u64 = 32 * 1024;
pub const MAX_PULL_REQUEST_BYTES: u64 = 1024 * 1024;

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Approval {
    schema_version: u32,
    repository: String,
    repository_id: u64,
    destination_branch: String,
    pull_request_number: u64,
    approved_source_commit: String,
    result_commit: String,
    work_item: String,
    intent_reference: String,
    intent_revision: String,
    approval_reference: String,
    approved_at: u64,
    merged_at: u64,
}

fn approved(bytes: &[u8], digest: &str) -> Result<Approval> {
    if bytes.len() as u64 > MAX_APPROVAL_BYTES || sha256(bytes) != digest {
        return reject(
            "GitHub change approval does not match the approved digest or exceeds the byte limit",
        );
    }
    let approval: Approval = serde_json::from_slice(bytes)?;
    let repo = approval.repository.split_once('/');
    if approval.schema_version != 1
        || repo.is_none_or(|(owner, name)| {
            !crate::bitbucket_change::valid_slug(owner)
                || !crate::bitbucket_change::valid_slug(name)
        })
        || approval.repository_id == 0
        || approval.pull_request_number == 0
        || !valid_oid(&approval.approved_source_commit)
        || !valid_oid(&approval.result_commit)
        || approval.approved_at == 0
        || approval.approved_at > approval.merged_at
        || [
            &approval.destination_branch,
            &approval.work_item,
            &approval.intent_reference,
            &approval.intent_revision,
            &approval.approval_reference,
        ]
        .iter()
        .any(|value| {
            value.trim().is_empty() || value.len() > 500 || value.chars().any(char::is_control)
        })
    {
        return reject("invalid approved GitHub change record");
    }
    Ok(approval)
}

pub fn approved_endpoint(bytes: &[u8], digest: &str) -> Result<String> {
    let approval = approved(bytes, digest)?;
    Ok(format!(
        "repos/{}/pulls/{}",
        approval.repository, approval.pull_request_number
    ))
}

pub fn collect(
    snapshot: &[u8],
    approval_bytes: &[u8],
    approval_digest: &str,
    observed_at: u64,
) -> Result<Change> {
    if snapshot.len() as u64 > MAX_PULL_REQUEST_BYTES {
        return reject("GitHub pull request snapshot exceeds the permitted byte limit");
    }
    let approval = approved(approval_bytes, approval_digest)?;
    let pr: Value = serde_json::from_slice(snapshot)?;
    let repository = &approval.repository;
    let url = format!(
        "https://github.com/{repository}/pull/{}",
        approval.pull_request_number
    );
    if pr["number"].as_u64() != Some(approval.pull_request_number)
        || pr["state"] != "closed"
        || pr["merged"] != true
        || pr["html_url"]
            .as_str()
            .is_none_or(|native| !native.eq_ignore_ascii_case(&url))
        || pr["base"]["ref"] != approval.destination_branch
        || [&pr["base"]["repo"], &pr["head"]["repo"]]
            .iter()
            .any(|repo| {
                repo["id"].as_u64() != Some(approval.repository_id)
                    || repo["full_name"]
                        .as_str()
                        .is_none_or(|name| !name.eq_ignore_ascii_case(repository))
            })
    {
        return reject(
            "GitHub snapshot does not identify the approved merged pull request and destination",
        );
    }
    if pr["head"]["sha"] != approval.approved_source_commit
        || pr["merge_commit_sha"] != approval.result_commit
    {
        return reject(
            "GitHub snapshot does not match the approved source head and exact merge result",
        );
    }
    let timestamp = |key: &str| {
        let time = OffsetDateTime::parse(pr[key].as_str()?, &Rfc3339).ok()?;
        u64::try_from(time.unix_timestamp())
            .ok()
            .filter(|epoch| *epoch > 0)
    };
    if timestamp("created_at").is_none_or(|created| created > approval.approved_at)
        || timestamp("merged_at") != Some(approval.merged_at)
        || approval.merged_at > observed_at
    {
        return reject("GitHub snapshot and the approved merge record have inconsistent times");
    }
    Ok(Change {
        repository: format!("github:{repository}"),
        merge_reference: Some(format!(
            "https://github.com/{repository}/commit/{}",
            approval.result_commit
        )),
        result_commit: approval.result_commit,
        pull_request: url,
        work_item: approval.work_item,
        intent_reference: approval.intent_reference,
        intent_revision: approval.intent_revision,
        approval_reference: approval.approval_reference,
        approved_source_commit: Some(approval.approved_source_commit),
        merged_at: approval.merged_at,
        observation: Observation {
            source: "github-merged-pull-request".into(),
            reference: format!(
                "github:{repository}/pull/{}#{}#approval:{approval_digest}",
                approval.pull_request_number,
                sha256(snapshot)
            ),
            observed_at,
        },
    })
}
