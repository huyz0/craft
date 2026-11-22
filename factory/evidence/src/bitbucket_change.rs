use std::path::PathBuf;

use crate::{
    Result, reject,
    release_trace::{Bundle, Change, Observation, read_evidence},
    sha256, valid_oid,
};
use clap::Args;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub const MAX_APPROVAL_BYTES: u64 = 32 * 1024;
pub const MAX_PULL_REQUEST_BYTES: u64 = 1024 * 1024;

#[derive(Args)]
pub struct Options {
    /// Existing release evidence bundle.
    bundle: PathBuf,
    /// Raw response from the Bitbucket Cloud pull request GET endpoint.
    #[arg(long)]
    pull_request_snapshot: PathBuf,
    /// Owner-approved change record bound to the reviewed head and result commit.
    #[arg(long)]
    approval: PathBuf,
    /// Approved SHA-256 digest of the exact change record file.
    #[arg(long)]
    approval_digest: String,
    /// UTC Unix seconds when the pull request was captured.
    #[arg(long)]
    observed_at: u64,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Approval {
    pub(crate) schema_version: u32,
    pub(crate) workspace: String,
    pub(crate) repository: String,
    pub(crate) repository_uuid: String,
    pub(crate) destination_branch: String,
    pub(crate) pull_request_id: u64,
    pub(crate) approved_source_commit: String,
    pub(crate) result_commit: String,
    pub(crate) work_item: String,
    pub(crate) intent_reference: String,
    pub(crate) intent_revision: String,
    pub(crate) approval_reference: String,
    pub(crate) approved_at: u64,
    pub(crate) merge_reference: String,
    pub(crate) merged_at: u64,
}

#[derive(Deserialize)]
struct PullRequest {
    #[serde(rename = "type")]
    kind: String,
    id: u64,
    state: String,
    source: Source,
    destination: Destination,
    merge_commit: Option<Commit>,
    created_on: String,
    updated_on: String,
}

#[derive(Deserialize)]
struct Source {
    commit: Commit,
}

#[derive(Deserialize)]
struct Commit {
    hash: String,
}

#[derive(Deserialize)]
struct Destination {
    branch: Branch,
    repository: Repository,
}

#[derive(Deserialize)]
struct Branch {
    name: String,
}

#[derive(Deserialize)]
struct Repository {
    full_name: String,
    uuid: String,
}

pub fn run(options: Options) -> Result<Bundle> {
    let approval =
        read_evidence(&options.approval, MAX_APPROVAL_BYTES).map_err(|error| error.to_string())?;
    let snapshot = read_evidence(&options.pull_request_snapshot, MAX_PULL_REQUEST_BYTES)
        .map_err(|error| error.to_string())?;
    let change = collect(
        &snapshot,
        &approval,
        &options.approval_digest,
        options.observed_at,
    )?;
    Bundle::with_change(&options.bundle, change).map_err(|error| error.to_string().into())
}

fn approved(approval_bytes: &[u8], approval_digest: &str) -> Result<(Approval, String)> {
    if approval_bytes.len() as u64 > MAX_APPROVAL_BYTES {
        return reject("Bitbucket change record exceeds the permitted byte limit");
    }
    if sha256(approval_bytes) != approval_digest {
        return reject("change record does not match the approved digest");
    }
    let approval: Approval = serde_json::from_slice(approval_bytes)?;
    let approved_uuid = validate_approval(&approval)?;
    Ok((approval, approved_uuid))
}

pub(crate) fn validate_approval(approval: &Approval) -> Result<String> {
    let approved_uuid = normalize_uuid(&approval.repository_uuid)
        .ok_or("invalid approved Bitbucket repository UUID")?;
    if approval.schema_version != 1
        || !valid_slug(&approval.workspace)
        || !valid_slug(&approval.repository)
        || approval.pull_request_id == 0
        || !valid_oid(&approval.approved_source_commit)
        || !valid_oid(&approval.result_commit)
        || [
            &approval.destination_branch,
            &approval.work_item,
            &approval.intent_reference,
            &approval.intent_revision,
            &approval.approval_reference,
            &approval.merge_reference,
        ]
        .iter()
        .any(|field| {
            field.trim().is_empty() || field.len() > 500 || field.chars().any(char::is_control)
        })
        || approval.approved_at == 0
        || approval.approved_at > approval.merged_at
    {
        return reject("invalid approved Bitbucket change record");
    }
    Ok(approved_uuid)
}

pub fn approved_pull_request_endpoint(
    approval_bytes: &[u8],
    approval_digest: &str,
) -> Result<String> {
    let (approval, _) = approved(approval_bytes, approval_digest)?;
    Ok(pull_request_endpoint(
        &approval.workspace,
        &approval.repository,
        approval.pull_request_id,
    ))
}

pub(crate) fn pull_request_endpoint(workspace: &str, repository: &str, id: u64) -> String {
    format!("https://api.bitbucket.org/2.0/repositories/{workspace}/{repository}/pullrequests/{id}")
}

pub fn collect(
    snapshot: &[u8],
    approval_bytes: &[u8],
    approval_digest: &str,
    observed_at: u64,
) -> Result<Change> {
    if snapshot.len() as u64 > MAX_PULL_REQUEST_BYTES {
        return reject("Bitbucket pull request snapshot exceeds the permitted byte limit");
    }
    let (approval, approved_uuid) = approved(approval_bytes, approval_digest)?;
    let pr: PullRequest = serde_json::from_slice(snapshot)?;
    let full_name = format!("{}/{}", approval.workspace, approval.repository);
    if pr.kind != "pullrequest"
        || pr.state != "MERGED"
        || pr.id != approval.pull_request_id
        || pr.destination.repository.full_name != full_name
        || normalize_uuid(&pr.destination.repository.uuid).as_deref()
            != Some(approved_uuid.as_str())
        || pr.destination.branch.name != approval.destination_branch
    {
        return reject(
            "Bitbucket snapshot does not identify the approved merged pull request and destination",
        );
    }
    if pr.source.commit.hash != approval.approved_source_commit
        || pr
            .merge_commit
            .as_ref()
            .is_none_or(|commit| commit.hash != approval.result_commit)
    {
        return reject(
            "Bitbucket snapshot does not match the approved source head and exact merge result",
        );
    }
    if timestamp(&pr.created_on).is_none_or(|created| created > approval.approved_at)
        || timestamp(&pr.updated_on)
            .is_none_or(|updated| updated < approval.merged_at || updated > observed_at)
    {
        return reject("Bitbucket snapshot and the approved merge record have inconsistent times");
    }
    let pull_request = format!("https://bitbucket.org/{full_name}/pull-requests/{}", pr.id);
    Ok(Change {
        repository: format!("bitbucket:{full_name}"),
        result_commit: approval.result_commit,
        pull_request,
        work_item: approval.work_item,
        intent_reference: approval.intent_reference,
        intent_revision: approval.intent_revision,
        approval_reference: approval.approval_reference,
        approved_source_commit: Some(approval.approved_source_commit),
        merge_reference: Some(approval.merge_reference),
        merged_at: approval.merged_at,
        observation: Observation {
            source: "bitbucket-merged-pull-request".to_owned(),
            reference: format!(
                "bitbucket:{full_name}/pull-requests/{}#{}#approval:{}",
                pr.id,
                sha256(snapshot),
                approval_digest
            ),
            observed_at,
        },
    })
}

fn timestamp(value: &str) -> Option<u64> {
    let time = OffsetDateTime::parse(value, &Rfc3339).ok()?;
    u64::try_from(time.unix_timestamp())
        .ok()
        .filter(|time| *time > 0)
}

pub fn valid_slug(value: &str) -> bool {
    value.len() <= 100
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

pub fn normalize_uuid(value: &str) -> Option<String> {
    let value = value
        .strip_prefix('{')
        .and_then(|rest| rest.strip_suffix('}'))
        .unwrap_or(value);
    (value.split('-').map(str::len).eq([8, 4, 4, 4, 12])
        && value
            .bytes()
            .all(|byte| byte == b'-' || byte.is_ascii_hexdigit()))
    .then(|| value.to_ascii_lowercase())
}
