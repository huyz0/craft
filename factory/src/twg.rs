use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::order::valid_oid;
use crate::store::Receipt;
use crate::{Result, reject, repository};
use craft_factory_evidence::bitbucket_change::normalize_uuid;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub schema_version: u32,
    pub twg: PathBuf,
    pub profile: String,
    pub site: String,
    pub workspace: String,
    pub repository: String,
    pub base_branch: String,
    pub required_steps: Vec<String>,
    pub check_definitions: Vec<String>,
}

use craft_factory_evidence::ci::FailedStep;

#[derive(Debug)]
pub(crate) enum ObsoleteFailure {
    BranchMoved,
    BuildSuperseded(u64),
}

impl ObsoleteFailure {
    pub(crate) fn reason(&self) -> &'static str {
        match self {
            Self::BranchMoved => "branch_moved",
            Self::BuildSuperseded(_) => "workflow_superseded",
        }
    }
}

impl std::fmt::Display for ObsoleteFailure {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BranchMoved => output.write_str(
                "Bitbucket branch does not resolve to the expected commit for the approved failure",
            ),
            Self::BuildSuperseded(build) => write!(
                output,
                "Bitbucket build {build} superseded the selected failure; inspect the newer run"
            ),
        }
    }
}

impl std::error::Error for ObsoleteFailure {}

impl Policy {
    pub fn repository_name(&self) -> String {
        format!("{}/{}", self.workspace, self.repository)
    }

    fn recent_builds(&self, branch: &str) -> Result<std::collections::BTreeSet<u64>> {
        let recent = self.read(&[
            "bb", "pipeline", "query", "--branch", branch, "--limit", "30",
        ])?;
        let builds = recent
            .as_array()
            .ok_or("Bitbucket did not return a recent build history")?;
        if builds.len() > 30 {
            return reject("Bitbucket returned more builds than the history limit");
        }
        let mut numbers = std::collections::BTreeSet::new();
        for build in builds {
            let number = build["build_number"]
                .as_u64()
                .filter(|number| *number > 0)
                .ok_or("Bitbucket recent history is missing a build identity")?;
            if !numbers.insert(number) {
                return reject("Bitbucket recent history contains a duplicate build identity");
            }
        }
        Ok(numbers)
    }

    fn read(&self, args: &[&str]) -> Result<Value> {
        let mut command = Command::new(&self.twg);
        command
            .args(args)
            .args([
                "--profile",
                &self.profile,
                "--site",
                &self.site,
                "--workspace",
                &self.workspace,
                "--repo",
                &self.repository,
                "--mode",
                "agent",
                "--output-summary",
                "none",
                "-o",
                "json",
            ])
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
        let (status, output) = crate::process_tree::read_bounded(
            &mut command,
            "TWG",
            Duration::from_secs(8),
            4 * 1024 * 1024,
        )?;
        let mut response: Value = serde_json::from_slice(&output)?;
        if !status.success() || response["ok"] == false {
            return reject(format!(
                "TWG could not read Bitbucket ({})",
                response["error"]["code"]
                    .as_str()
                    .unwrap_or("unknown error")
            ));
        }
        if response.get("data").is_some() {
            Ok(response["data"].take())
        } else {
            Ok(response)
        }
    }

    fn branch_tip(&self, branch: &str) -> Result<String> {
        let query = format!("--query={branch}");
        let result = self.read(&["bb", "branch", "query", &query, "--limit", "50"])?;
        let branches = result
            .as_array()
            .ok_or("TWG branch query did not return an array")?;
        let mut exact = branches
            .iter()
            .filter(|row| row["name"].as_str() == Some(branch));
        let head = exact
            .next()
            .and_then(|row| row.pointer("/target/hash"))
            .and_then(Value::as_str)
            .filter(|head| valid_oid(head));
        match (head, exact.next()) {
            (Some(head), None) => Ok(head.into()),
            _ => reject(format!(
                "Bitbucket branch {branch:?} does not identify a unique commit"
            )),
        }
    }

    fn assert_branch(&self, branch: &str, expected: &str) -> Result<()> {
        if self.branch_tip(branch)? != expected {
            return reject(format!(
                "Bitbucket branch {branch:?} does not resolve to the expected commit"
            ));
        }
        Ok(())
    }
}

pub fn ci_branch_tip(policy: &Policy) -> Result<String> {
    validate_policy(policy, 1)?;
    policy.branch_tip(&policy.base_branch)
}

#[derive(Clone, Deserialize, PartialEq, Eq)]
pub struct DraftTarget {
    pub repository: String,
    pub head: String,
    pub base: String,
    pub source_branch: String,
    pub base_branch: String,
}

pub fn observe_draft(policy: &Policy, target: &DraftTarget, id: u64) -> Result<String> {
    validate_policy(policy, id)?;
    if target.repository != policy.repository_name()
        || target.base_branch != policy.base_branch
        || !valid_oid(&target.head)
        || !valid_oid(&target.base)
    {
        return reject(
            "draft reservation and TWG policy do not identify the same repository and base",
        );
    }
    let url = format!(
        "https://bitbucket.org/{}/pull-requests/{id}",
        target.repository
    );
    let verify = || -> Result<()> {
        let value = policy.read(&["bb", "pull-requests", "get", &id.to_string(), "--strict"])?;
        let pr = match value.as_array() {
            Some(rows) if rows.len() == 1 => &rows[0],
            Some(_) => return reject("expected exactly one PR from TWG"),
            None => &value,
        };
        if pr["id"].as_u64() != Some(id)
            || pr["state"] != "OPEN"
            || pr["draft"] != true
            || pr["links"]["html"]["href"] != url
            || pr["source"]["repository"]["full_name"] != target.repository
            || pr["destination"]["repository"]["full_name"] != target.repository
            || pr["source"]["branch"]["name"] != target.source_branch
            || pr["destination"]["branch"]["name"] != target.base_branch
            || !hash_matches(pr.pointer("/source/commit/hash"), &target.head)
            || !hash_matches(pr.pointer("/destination/commit/hash"), &target.base)
        {
            return reject("Bitbucket did not return the exact open draft and pinned revision");
        }
        Ok(())
    };
    verify()?;
    policy.assert_branch(&target.source_branch, &target.head)?;
    policy.assert_branch(&target.base_branch, &target.base)?;
    verify()?;
    Ok(url)
}

pub(crate) fn validate_policy(policy: &Policy, build: u64) -> Result<()> {
    let slug = |value: &str| {
        !value.is_empty()
            && value
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_'))
    };
    if policy.schema_version != 1
        || !policy.twg.is_absolute()
        || !policy.twg.is_file()
        || !slug(&policy.workspace)
        || !slug(&policy.repository)
        || policy.base_branch.is_empty()
        || policy.profile.is_empty()
        || !policy.site.ends_with(".atlassian.net")
        || build == 0
        || policy.required_steps.is_empty()
        || policy
            .required_steps
            .iter()
            .any(|name| name.trim().is_empty())
    {
        return reject("invalid TWG Bitbucket pilot policy");
    }
    #[cfg(windows)]
    if policy
        .twg
        .extension()
        .and_then(|ext| ext.to_str())
        .is_none_or(|ext| !ext.eq_ignore_ascii_case("exe"))
    {
        return reject("the TWG binary must be an executable file");
    }
    Ok(())
}

pub fn failed_step(policy: &Policy, build: u64, step: &str, base: &str) -> Result<FailedStep> {
    validate_policy(policy, build)?;
    let step_uuid = normalize_uuid(step).ok_or("supply a full Bitbucket step UUID")?;
    if !valid_oid(base) {
        return reject("CI repair requires a full pinned source commit");
    }
    let repo = format!("{}/{}", policy.workspace, policy.repository);
    let run = policy.read(&["bb", "pipeline", "get", "--pipeline", &build.to_string()])?;
    let evidence = parse_failed_step(&run, policy, &repo, build, &step_uuid, base)?;
    policy.reject_superseded_build(&run, &evidence)?;
    if policy.branch_tip(&policy.base_branch)? != base {
        return Err(ObsoleteFailure::BranchMoved.into());
    }
    let refreshed = policy.read(&["bb", "pipeline", "get", "--pipeline", &build.to_string()])?;
    if parse_failed_step(&refreshed, policy, &repo, build, &step_uuid, base)? != evidence {
        return reject("the selected step attempt changed during inspection; select it again");
    }
    Ok(evidence)
}

impl Policy {
    fn reject_superseded_build(&self, selected: &Value, evidence: &FailedStep) -> Result<()> {
        let numbers = self.recent_builds(&evidence.branch)?;
        if !numbers.contains(&evidence.build) {
            return reject(
                "selected failure is outside the recent Bitbucket history; inspect it manually",
            );
        }
        let later = numbers.range((
            std::ops::Bound::Excluded(evidence.build),
            std::ops::Bound::Unbounded,
        ));
        if later.clone().count() > 8 {
            return reject(
                "too many newer builds to confirm the selected failure; inspect them manually",
            );
        }
        let workflow = &selected["target"]["selector"];
        for number in later.rev() {
            let run = self.read(&["bb", "pipeline", "get", "--pipeline", &number.to_string()])?;
            let Some(target) = matching_branch_target(
                &run,
                &evidence.repository,
                *number,
                &evidence.branch,
                &evidence.commit,
            )?
            else {
                continue;
            };
            let selector = &target["selector"];
            let selected_type = workflow["type"].as_str().filter(|kind| !kind.is_empty());
            let newer_type = selector["type"].as_str().filter(|kind| !kind.is_empty());
            if let (Some(selected), Some(newer)) = (selected_type, newer_type) {
                let pattern = workflow["pattern"]
                    .as_str()
                    .filter(|pattern| !pattern.is_empty());
                if selected == newer
                    && (selected == "default"
                        || pattern.is_some_and(|value| selector["pattern"] == value))
                {
                    return Err(ObsoleteFailure::BuildSuperseded(*number).into());
                }
            }
            let same_or_uncertain = match (workflow["type"].as_str(), selector["type"].as_str()) {
                (Some(selected), Some(newer)) if !selected.is_empty() && !newer.is_empty() => {
                    selected == newer
                        && (selected == "default"
                            || match (workflow["pattern"].as_str(), selector["pattern"].as_str()) {
                                (Some(selected), Some(newer)) => selected == newer,
                                _ => true,
                            })
                }
                _ => true,
            };
            if same_or_uncertain {
                return reject(format!(
                    "Bitbucket build {number} might have superseded the selected failure; inspect the newer run"
                ));
            }
        }
        Ok(())
    }
}

fn matching_branch_target<'a>(
    run: &'a Value,
    repo: &str,
    build: u64,
    branch: &str,
    commit: &str,
) -> Result<Option<&'a Value>> {
    if run["build_number"].as_u64() != Some(build) || run["repository"]["full_name"] != repo {
        return reject("Bitbucket returned an unexpected build while inspecting history");
    }
    let target = &run["target"];
    match target["type"].as_str() {
        Some("pipeline_ref_target") => (),
        Some("pipeline_commit_target" | "pipeline_pullrequest_target") => return Ok(None),
        _ => return reject("Bitbucket returned an unknown or missing pipeline target type"),
    }
    match target["ref_type"].as_str() {
        Some("branch") => (),
        Some("named_branch" | "bookmark" | "tag") => return Ok(None),
        _ => return reject("Bitbucket returned an unknown or missing pipeline ref type"),
    }
    let name = target["ref_name"]
        .as_str()
        .filter(|name| !name.is_empty())
        .ok_or("Bitbucket did not identify the pipeline branch")?;
    let hash = target["commit"]["hash"]
        .as_str()
        .filter(|hash| valid_oid(hash))
        .ok_or("Bitbucket did not identify the pipeline commit")?;
    Ok((name == branch && hash == commit).then_some(target))
}

pub fn poll_failed_step(
    policy: &Policy,
    step_name: &str,
    base: &str,
) -> Result<Option<FailedStep>> {
    validate_policy(policy, 1)?;
    if !valid_oid(base) || !policy.required_steps.iter().any(|step| step == step_name) {
        return reject("polling requires a pinned commit and a configured step name");
    }
    policy.assert_branch(&policy.base_branch, base)?;
    let repo = policy.repository_name();
    for number in policy
        .recent_builds(&policy.base_branch)?
        .into_iter()
        .rev()
        .take(8)
    {
        let run = policy.read(&["bb", "pipeline", "get", "--pipeline", &number.to_string()])?;
        let Some(target) = matching_branch_target(&run, &repo, number, &policy.base_branch, base)?
        else {
            continue;
        };
        if target["selector"]["type"]
            .as_str()
            .filter(|kind| !kind.is_empty())
            .ok_or("Bitbucket did not identify the pipeline selector")?
            != "default"
        {
            continue;
        }
        if run["state"]["name"] != "COMPLETED" || run["state"]["result"]["name"] != "FAILED" {
            return Ok(None);
        }
        let mut selected = run["steps"]
            .as_array()
            .ok_or("Bitbucket did not return steps for the latest failed build")?
            .iter()
            .filter(|step| step["name"] == step_name);
        let Some(step) = selected.next() else {
            return Ok(None);
        };
        if selected.next().is_some() {
            return reject("the configured step name is ambiguous in the latest failed build");
        }
        if step["state"]["result"]["name"] != "FAILED" {
            return Ok(None);
        }
        let uuid = step["uuid"].as_str().ok_or("the failed step has no UUID")?;
        return failed_step(policy, number, uuid, base).map(Some);
    }
    Ok(None)
}

fn parse_failed_step(
    run: &Value,
    policy: &Policy,
    repo: &str,
    build: u64,
    step_uuid: &str,
    base: &str,
) -> Result<FailedStep> {
    if run["build_number"].as_u64() != Some(build)
        || run["repository"]["full_name"] != repo
        || run["target"]["type"] != "pipeline_ref_target"
        || run["target"]["ref_type"] != "branch"
        || run["target"]["ref_name"] != policy.base_branch
        || run["target"]["commit"]["hash"] != base
        || run["state"]["name"] != "COMPLETED"
        || run["state"]["result"]["name"] != "FAILED"
    {
        return reject(
            "selected pipeline is not a completed failure for the configured branch and pinned commit",
        );
    }
    let pipeline_uuid = run["uuid"]
        .as_str()
        .and_then(normalize_uuid)
        .ok_or("Bitbucket pipeline identity is missing")?;
    let steps = run["steps"]
        .as_array()
        .ok_or("Bitbucket steps are missing")?;
    let mut matches = steps.iter().filter(|step| {
        step["uuid"].as_str().and_then(normalize_uuid).as_deref() == Some(step_uuid)
    });
    let step = matches
        .next()
        .ok_or("selected step is missing from the pipeline")?;
    if matches.next().is_some()
        || step["state"]["name"] != "COMPLETED"
        || step["state"]["result"]["name"] != "FAILED"
    {
        return reject("selected step is ambiguous or no longer failed");
    }
    let step_name = step["name"]
        .as_str()
        .filter(|name| policy.required_steps.iter().any(|allowed| allowed == name))
        .ok_or("selected failing step is outside the configured CI checks")?;
    let completed_on = step["completed_on"]
        .as_str()
        .filter(|value| !value.trim().is_empty() && value.len() <= 80)
        .ok_or("Bitbucket did not identify the completed step attempt")?;
    Ok(FailedStep {
        repository: repo.into(),
        branch: policy.base_branch.clone(),
        commit: base.into(),
        build,
        pipeline_uuid,
        step_uuid: step_uuid.into(),
        step_name: step_name.into(),
        completed_on: completed_on.into(),
        evidence_url: format!(
            "https://bitbucket.org/{repo}/pipelines/results/{build}/steps/%7B{step_uuid}%7D"
        ),
    })
}

pub fn observe(receipt: &Receipt, policy: &Policy, build: u64) -> Result<Value> {
    if receipt.publication_hold.is_some() {
        return reject("reconcile publication before verifying Bitbucket checks");
    }
    let draft = receipt
        .draft
        .as_ref()
        .ok_or("bind the human-published PR first")?;
    let head = receipt
        .candidate
        .as_deref()
        .ok_or("missing current candidate")?;
    if draft.head != head {
        return reject("bind the draft to the current candidate before checking Bitbucket");
    }
    observe_candidate_checks(&receipt.work_order, draft, policy, build)
}

pub(crate) fn observe_candidate_checks(
    order: &crate::order::WorkOrder,
    draft: &crate::review::Draft,
    policy: &Policy,
    build: u64,
) -> Result<Value> {
    validate_policy(policy, build)?;
    let head = &draft.head;
    let base = &order.base;
    let repo = format!("{}/{}", policy.workspace, policy.repository);
    if order.ci_failure.as_ref().is_some_and(|failure| {
        let crate::order::CiFailure::Bitbucket(failure) = failure else {
            return true;
        };
        failure.repository != repo
            || failure.branch != policy.base_branch
            || !policy.required_steps.contains(&failure.step_name)
    }) {
        return reject(
            "verify the CI repair on the original repository and branch with the failed step required",
        );
    }
    let prefix = format!("https://bitbucket.org/{repo}/pull-requests/");
    let id: u64 = draft
        .reference
        .strip_prefix(&prefix)
        .ok_or("PR has the wrong repository")?
        .parse()?;
    if id == 0 {
        return reject("invalid Bitbucket PR number");
    }
    for path in repository::candidate_history_paths(order, base, head)? {
        let path = path.to_ascii_lowercase();
        if std::iter::once("bitbucket-pipelines.yml")
            .chain(policy.check_definitions.iter().map(String::as_str))
            .any(|entry| {
                let entry = entry.to_ascii_lowercase();
                path == entry
                    || path
                        .strip_prefix(&entry)
                        .is_some_and(|suffix| suffix.starts_with('/'))
            })
        {
            return reject(format!(
                "candidate changes a protected check definition: {path:?}"
            ));
        }
    }

    let pull_request = || -> Result<String> {
        let result = policy.read(&["bb", "pull-requests", "get", &id.to_string(), "--strict"])?;
        let pr = if let Some(rows) = result.as_array() {
            match rows.as_slice() {
                [pr] => pr,
                _ => return reject("expected exactly one PR from TWG"),
            }
        } else {
            &result
        };
        if pr["id"].as_u64() != Some(id)
            || pr["state"] != "OPEN"
            || pr.pointer("/links/html/href").and_then(Value::as_str) != Some(&draft.reference)
            || pr
                .pointer("/source/repository/full_name")
                .and_then(Value::as_str)
                != Some(&repo)
            || pr
                .pointer("/destination/branch/name")
                .and_then(Value::as_str)
                != Some(&policy.base_branch)
            || !hash_matches(pr.pointer("/source/commit/hash"), head)
            || !hash_matches(pr.pointer("/destination/commit/hash"), base)
            || pr
                .pointer("/destination/repository/full_name")
                .and_then(Value::as_str)
                != Some(&repo)
        {
            return reject("TWG PR does not match the bound candidate and configured destination");
        }
        Ok(pr
            .pointer("/source/branch/name")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or("PR source branch is missing")?
            .to_owned())
    };
    let source = pull_request()?;
    policy.assert_branch(&source, head)?;
    policy.assert_branch(&policy.base_branch, base)?;
    let run = policy.read(&["bb", "pipeline", "get", "--pipeline", &build.to_string()])?;
    if run["build_number"].as_u64() != Some(build)
        || run["repository"]["full_name"] != repo
        || run["target"]["commit"]["hash"].as_str() != Some(head)
        || run["target"]["pullrequest"]["id"].as_u64() != Some(id)
        || run["target"]["selector"]["type"] != "pull-requests"
        || !hash_matches(run.pointer("/target/destination_commit/hash"), base)
        || run["state"]["name"] != "COMPLETED"
        || run["state"]["result"]["name"] != "SUCCESSFUL"
    {
        return reject(
            "TWG pipeline is not a successful run for the current PR head and pinned base",
        );
    }
    let steps = run["steps"]
        .as_array()
        .ok_or("TWG pipeline steps are missing")?;
    let mut checks = Vec::new();
    for name in &policy.required_steps {
        let mut matching = steps
            .iter()
            .filter(|step| step["name"].as_str() == Some(name));
        let step = matching
            .next()
            .ok_or("required TWG pipeline step is missing")?;
        if matching.next().is_some()
            || step["state"]["name"] != "COMPLETED"
            || step["state"]["result"]["name"] != "SUCCESSFUL"
        {
            return reject(format!(
                "required TWG step {name:?} is ambiguous or unsuccessful"
            ));
        }
        let uuid = step["uuid"]
            .as_str()
            .and_then(normalize_uuid)
            .ok_or("missing TWG step UUID")?;
        checks.push(json!({"id": uuid, "name": name}));
    }
    if pull_request()? != source {
        return reject("PR source branch changed during verification");
    }
    if let Some(crate::order::CiFailure::Bitbucket(original)) = &order.ci_failure {
        let current = failed_step(policy, original.build, &original.step_uuid, base)?;
        if current != *original {
            return reject("the original failure attempt changed; review the repair intent again");
        }
    }
    policy.assert_branch(&source, head)?;
    policy.assert_branch(&policy.base_branch, base)?;
    let pipeline_uuid = run["uuid"]
        .as_str()
        .and_then(normalize_uuid)
        .ok_or("missing TWG pipeline UUID")?;
    Ok(json!({
        "provider":"bitbucket", "transport":"twg", "pr":draft.reference, "head":head, "base":base,
        "source_branch":source,
        "pipeline":format!("https://bitbucket.org/{repo}/pipelines/results/{build}"),
        "build_number":build, "pipeline_uuid":pipeline_uuid, "checks":checks
    }))
}

pub(crate) fn hash_matches(value: Option<&Value>, full: &str) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|prefix| prefix.len() >= 12 && full.starts_with(prefix))
}
