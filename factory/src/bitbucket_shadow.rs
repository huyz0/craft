use std::collections::{BTreeMap, BTreeSet};

use craft_factory_evidence::sha256;
use serde::Serialize;
use serde_json::{Value, json};

use crate::{Result, bitbucket_publish::branch_url, bitbucket_read::ReadClient, reject};

pub struct Target<'a> {
    pub repository: &'a str,
    pub pull_request: u64,
    pub head: &'a str,
    pub base: &'a str,
    pub base_branch: &'a str,
    pub author: &'a str,
    pub counted_reviewers: &'a BTreeSet<&'a String>,
    pub minimum_approvals: usize,
    pub standard_checks_digest: Option<&'a str>,
}

#[derive(Debug, Serialize)]
pub struct Preflight {
    endpoint: String,
    standard_checks_digest: String,
    checks: Vec<Check>,
    pub blockers: BTreeSet<&'static str>,
}

#[derive(Debug, Serialize)]
struct Check {
    kind: String,
    name: Option<String>,
    status: String,
    required: bool,
    blocking: bool,
}

pub fn observe(target: &Target<'_>) -> Result<Preflight> {
    let client = ReadClient::from_environment(
        "CRAFT_FACTORY_BITBUCKET_SHADOW_TOKEN",
        "CRAFT_FACTORY_BITBUCKET_SHADOW_EMAIL",
    )?;
    observe_with(target, |url| {
        let bytes = client.get(url, 256 * 1024)?;
        Ok(serde_json::from_slice(&bytes).map_err(|_| "invalid Bitbucket preflight response")?)
    })
}

fn observe_with(
    target: &Target<'_>,
    mut get: impl FnMut(&str) -> Result<Value>,
) -> Result<Preflight> {
    if !valid_branch(target.base_branch) {
        return reject("unsupported Bitbucket base branch");
    }
    let pull_request = format!(
        "https://api.bitbucket.org/2.0/repositories/{}/pullrequests/{}",
        target.repository, target.pull_request
    );
    let endpoint = format!("{pull_request}/mergeability/checks");
    let first = get(&pull_request)?;
    let source = validate_pull_request(target, &first)?;
    let report = get(&endpoint)?;
    for (branch, expected) in [(source, target.head), (target.base_branch, target.base)] {
        let reference = get(&branch_url(target.repository, branch))?;
        if reference["name"] != branch || reference["target"]["hash"] != expected {
            return reject("Bitbucket branch moved during preflight");
        }
    }
    let last = get(&pull_request)?;
    if validate_pull_request(target, &last)? != source {
        return reject("Bitbucket source branch changed during preflight");
    }
    let participants = last["participants"]
        .as_array()
        .ok_or("missing Bitbucket participants")?;
    if participants.len() > 200 {
        return reject("too many Bitbucket participants");
    }
    let mut blockers = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut approved = BTreeSet::new();
    for participant in participants {
        let actor = participant["user"]["uuid"]
            .as_str()
            .ok_or("missing Bitbucket participant UUID")?;
        if !seen.insert(actor) {
            return reject("duplicate Bitbucket participant");
        }
        if actor != target.author && participant["state"] == "changes_requested" {
            blockers.insert("native_changes_requested");
        }
        if participant["state"] == "approved" && participant["approved"] == true {
            approved.insert(actor);
        }
    }
    if !target
        .counted_reviewers
        .iter()
        .all(|actor| approved.contains(actor.as_str()))
    {
        blockers.insert("native_approval_not_current");
    }

    let values = report["values"]
        .as_array()
        .ok_or("missing Bitbucket mergeability checks")?;
    if values.len() > 100
        || !report["next"].is_null()
        || (!report["size"].is_null() && report["size"].as_u64() != Some(values.len() as u64))
    {
        return reject("incomplete Bitbucket mergeability checks");
    }
    let mut checks = Vec::new();
    let mut core = BTreeSet::new();
    let mut standard = BTreeMap::new();
    let mut native_minimum = None;
    for row in values {
        let kind = row["type"].as_str().ok_or("missing merge check type")?;
        let status = row["status"].as_str().ok_or("missing merge check status")?;
        if !matches!(
            status,
            "PASSED" | "FAILED" | "PENDING" | "SKIPPED" | "UNKNOWN"
        ) {
            return reject("unknown Bitbucket merge check status");
        }
        let required = row["required"].as_bool().ok_or("missing required flag")?;
        let blocking = row["blocking"].as_bool().ok_or("missing blocking flag")?;
        let name = match kind {
            "pullrequest_state_check"
            | "current_user_permission_check"
            | "git_mergeability_check" => {
                if !core.insert(kind) {
                    return reject("duplicate core Bitbucket merge check");
                }
                if status != "PASSED"
                    || (kind == "git_mergeability_check" && row["reason"] != "clean")
                {
                    blockers.insert("native_core_check_failed");
                }
                None
            }
            "standard_merge_check" => {
                let name = row["check"]["kind"]
                    .as_str()
                    .filter(|value| !value.is_empty() && value.len() <= 200)
                    .ok_or("missing standard merge check identity")?;
                let requirement = row["requirement"]
                    .as_object()
                    .ok_or("missing merge check requirement")?;
                if standard
                    .insert(
                        name,
                        json!({"required": required, "requirement": requirement}),
                    )
                    .is_some()
                {
                    return reject("duplicate standard Bitbucket merge check");
                }
                if name == "minimum_approvals" && required {
                    native_minimum = row["requirement"]["minimum_approvals"].as_u64();
                }
                Some(name.to_owned())
            }
            "custom_pre_merge_check" | "custom_on_merge_check" => Some(
                row["check"]["id"]
                    .as_str()
                    .filter(|value| !value.is_empty() && value.len() <= 500)
                    .ok_or("missing custom merge check identity")?
                    .to_owned(),
            ),
            "merge_queue_check" => {
                blockers.insert("native_merge_queue_requires_review");
                None
            }
            _ => return reject("unknown Bitbucket merge check type"),
        };
        if blocking || (required && status != "PASSED") {
            blockers.insert("native_merge_check_blocked");
        }
        checks.push(Check {
            kind: kind.into(),
            name,
            status: status.into(),
            required,
            blocking,
        });
    }
    if core.len() != 3 {
        blockers.insert("native_core_check_missing");
    }
    if native_minimum.is_none_or(|minimum| minimum < target.minimum_approvals as u64) {
        blockers.insert("native_approval_rule_not_enforced");
    }
    let standard_checks_digest = sha256(&serde_json::to_vec(&standard)?);
    match target.standard_checks_digest {
        None => {
            blockers.insert("native_policy_digest_unpinned");
        }
        Some(expected) if expected != standard_checks_digest => {
            blockers.insert("native_policy_digest_mismatch");
        }
        Some(_) => {}
    }
    Ok(Preflight {
        endpoint,
        standard_checks_digest,
        checks,
        blockers,
    })
}

pub(crate) fn valid_branch(branch: &str) -> bool {
    !branch.is_empty()
        && branch.len() <= 150
        && branch
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.'))
        && branch
            .split('/')
            .all(|part| !matches!(part, "" | "." | ".."))
}

fn validate_pull_request<'a>(target: &Target<'_>, pr: &'a Value) -> Result<&'a str> {
    let url = format!(
        "https://bitbucket.org/{}/pull-requests/{}",
        target.repository, target.pull_request
    );
    let source = pr["source"]["branch"]["name"]
        .as_str()
        .filter(|branch| valid_branch(branch))
        .ok_or("unsupported Bitbucket source branch")?;
    let same_commit = |value: &Value, expected: &str| {
        value
            .as_str()
            .is_some_and(|hash| hash.len() >= 12 && expected.starts_with(hash))
    };
    if pr["id"].as_u64() != Some(target.pull_request)
        || pr["links"]["html"]["href"] != url
        || pr["state"] != "OPEN"
        || pr["draft"] != false
        || pr["queued"] == true
        || pr["author"]["uuid"] != target.author
        || pr["source"]["repository"]["full_name"] != target.repository
        || pr["destination"]["repository"]["full_name"] != target.repository
        || pr["destination"]["branch"]["name"] != target.base_branch
        || !same_commit(&pr["source"]["commit"]["hash"], target.head)
        || !same_commit(&pr["destination"]["commit"]["hash"], target.base)
    {
        return reject("Bitbucket did not return the pinned open pull request");
    }
    Ok(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_preflight_pins_rules_and_correlates_exact_branch_heads() {
        let approval = "{00000000-0000-0000-0000-000000000002}".to_owned();
        let reviewers = BTreeSet::from([&approval]);
        let author = "{00000000-0000-0000-0000-000000000001}";
        let head = "a".repeat(40);
        let base = "b".repeat(40);
        let mut target = Target {
            repository: "team/service",
            pull_request: 42,
            head: &head,
            base: &base,
            base_branch: "main",
            author,
            counted_reviewers: &reviewers,
            minimum_approvals: 1,
            standard_checks_digest: None,
        };
        let pr = json!({
            "id": 42, "state": "OPEN", "draft": false, "author": {"uuid": author},
            "links": {"html": {"href": "https://bitbucket.org/team/service/pull-requests/42"}},
            "source": {"repository": {"full_name": "team/service"}, "branch": {"name": "work/task"},
                       "commit": {"hash": &head[..12]}},
            "destination": {"repository": {"full_name": "team/service"}, "branch": {"name": "main"},
                            "commit": {"hash": &base[..12]}},
            "participants": [{"user": {"uuid": approval}, "state": "approved", "approved": true}]
        });
        let checks = json!({"size": 4, "values": [
            {"type": "pullrequest_state_check", "status": "PASSED", "required": true, "blocking": false},
            {"type": "current_user_permission_check", "status": "PASSED", "required": true, "blocking": false},
            {"type": "git_mergeability_check", "status": "PASSED", "required": true, "blocking": false, "reason": "clean"},
            {"type": "standard_merge_check", "status": "PASSED", "required": true, "blocking": false,
             "check": {"kind": "minimum_approvals"}, "requirement": {"minimum_approvals": 1},
             "observed": {"approval_count": 1}}
        ]});
        let evaluate = |target: &Target<'_>, checks: &Value, last: &Value, destination: &str| {
            let mut pr_reads = 0;
            observe_with(target, |url| {
                match url.strip_prefix("https://api.bitbucket.org/2.0/repositories/team/service/") {
                    Some("pullrequests/42") => {
                        pr_reads += 1;
                        Ok(if pr_reads == 1 {
                            pr.clone()
                        } else {
                            last.clone()
                        })
                    }
                    Some("pullrequests/42/mergeability/checks") => Ok(checks.clone()),
                    Some("refs/branches/work%2Ftask") => {
                        Ok(json!({"name": "work/task", "target": {"hash": head}}))
                    }
                    Some("refs/branches/main") => {
                        Ok(json!({"name": "main", "target": {"hash": destination}}))
                    }
                    _ => panic!("unexpected URL {url}"),
                }
            })
        };
        let discovered = evaluate(&target, &checks, &pr, &base).unwrap();
        assert_eq!(
            discovered.blockers,
            BTreeSet::from(["native_policy_digest_unpinned"])
        );
        target.standard_checks_digest = Some(&discovered.standard_checks_digest);
        assert!(
            evaluate(&target, &checks, &pr, &base)
                .unwrap()
                .blockers
                .is_empty()
        );
        assert!(evaluate(&target, &checks, &pr, &head).is_err());
        assert!(evaluate(&target, &checks, &json!({"draft": true}), &base).is_err());

        let mut withdrawn = pr.clone();
        withdrawn["participants"][0]["state"] = json!("changes_requested");
        withdrawn["participants"][0]["approved"] = json!(false);
        let report = evaluate(&target, &checks, &withdrawn, &base).unwrap();
        assert!(report.blockers.contains("native_approval_not_current"));
        assert!(report.blockers.contains("native_changes_requested"));

        let mut changed = checks.clone();
        changed["values"][3]["required"] = json!(false);
        changed["values"][2]["status"] = json!("UNKNOWN");
        let report = evaluate(&target, &changed, &pr, &base).unwrap();
        assert!(report.blockers.contains("native_policy_digest_mismatch"));
        assert!(report.blockers.contains("native_core_check_failed"));
        assert!(
            report
                .blockers
                .contains("native_approval_rule_not_enforced")
        );
    }
}
