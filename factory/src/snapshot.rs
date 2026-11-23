use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::order::{PinnedReference, WorkOrder};
use crate::{Result, reject, shared_runner::sha256};

const LIMIT: u64 = 256 * 1024;
pub(crate) const GITHUB_ISSUE_PREFIX: &str = "github-issue:";

#[derive(Deserialize)]
struct IssueSnapshot {
    key: String,
    url: String,
    intent: String,
    acceptance: Vec<String>,
}

pub fn reference(order: &WorkOrder) -> Result<Option<&PinnedReference>> {
    let mut references = order.context.iter().filter(|entry| {
        is_github_issue(entry)
            || (entry.revision.starts_with("sha256:")
                && entry.location.starts_with("https://")
                && entry.location.contains("/browse/"))
    });
    let found = references.next();
    if references.next().is_some() {
        return reject("use one digest-pinned tracker issue for a work order");
    }
    Ok(found)
}

pub(crate) fn github_issue_url(key: &str) -> Option<String> {
    let (repository, raw_number) = key.split_once('#')?;
    let (owner, name) = repository.split_once('/')?;
    let valid = |part: &str| {
        !matches!(part, "" | "." | "..")
            && part.len() <= 100
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    };
    let number = raw_number.parse::<u64>().ok()?;
    (valid(owner) && valid(name) && number > 0 && number.to_string() == raw_number)
        .then(|| format!("https://github.com/{repository}/issues/{number}"))
}

pub(crate) fn is_github_issue(reference: &PinnedReference) -> bool {
    reference.location.starts_with(GITHUB_ISSUE_PREFIX)
}

pub(crate) fn save(
    order: &WorkOrder,
    destination: &Path,
    filename: &str,
    bytes: &[u8],
) -> Result<(PathBuf, PathBuf)> {
    if !destination.is_absolute() {
        return reject("provide an absolute new directory for the issue snapshot");
    }
    crate::evidence_capture::create_directory(destination)?;
    let snapshot_path = destination.join(filename);
    fs::File::create_new(&snapshot_path)?.write_all(bytes)?;
    verify(order, &snapshot_path)?;
    let order_path = destination.join("work-order.json");
    serde_json::to_writer_pretty(fs::File::create_new(&order_path)?, order)?;
    Ok((snapshot_path, order_path))
}

pub fn verify(order: &WorkOrder, path: &Path) -> Result<()> {
    verified_bytes(order, path).map(|_| ())
}

pub fn verified_bytes(order: &WorkOrder, path: &Path) -> Result<Vec<u8>> {
    let reference = reference(order)?.ok_or("the work order has no digest-pinned issue")?;
    let github = is_github_issue(reference);
    let provider = if github { "GitHub issue" } else { "Jira" };
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > LIMIT {
        return reject(format!("the {provider} snapshot exceeds the size limit"));
    }
    if reference.revision != sha256(&bytes) {
        return reject(format!(
            "the {provider} snapshot differs from the approved digest; review the intent before proceeding"
        ));
    }
    let snapshot: IssueSnapshot = serde_json::from_slice(&bytes)?;
    let matches_url = if github {
        let approved_url = reference.location.strip_prefix(GITHUB_ISSUE_PREFIX);
        approved_url == Some(snapshot.url.as_str())
            && github_issue_url(&order.issue) == Some(snapshot.url)
    } else {
        snapshot.url == reference.location
            && snapshot.url.ends_with(&format!("/browse/{}", order.issue))
    };
    if snapshot.key != order.issue
        || !matches_url
        || snapshot.intent != order.intent
        || snapshot.acceptance != order.acceptance
    {
        return reject(format!(
            "the {provider} snapshot does not match the work order's issue, intent, or acceptance criteria"
        ));
    }
    Ok(bytes)
}
