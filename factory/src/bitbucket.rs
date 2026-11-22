use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use craft_factory_evidence::bitbucket_change::valid_slug;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::store::Receipt;
use crate::{Result, reject, repository};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub schema_version: u32,
    pub workspace: String,
    pub repository: String,
    pub base_branch: String,
    pub craft: PathBuf,
    pub craft_state: PathBuf,
    pub craft_profile: String,
    pub required_steps: Vec<String>,
    pub check_definitions: Vec<String>,
}

impl Policy {
    fn validate(&self) -> Result<()> {
        let steps: BTreeSet<_> = self.required_steps.iter().map(String::as_str).collect();
        if self.schema_version != 1
            || !valid_slug(&self.workspace)
            || !valid_slug(&self.repository)
            || self.base_branch.trim().is_empty()
            || self.craft_profile.trim().is_empty()
            || !self.craft.is_absolute()
            || !self.craft_state.is_absolute()
            || steps.is_empty()
            || steps.len() != self.required_steps.len()
            || steps
                .iter()
                .any(|step| step.trim().is_empty() || step.len() > 200)
            || self.check_definitions.iter().any(|path| {
                path.contains(['\\', ':'])
                    || path.split('/').any(|part| matches!(part, "" | "." | ".."))
            })
        {
            return reject("invalid Bitbucket pilot policy");
        }
        Ok(())
    }

    fn read(&self, command: [&str; 2], args: &[&str]) -> Result<Value> {
        #[cfg(windows)]
        if self
            .craft
            .extension()
            .and_then(|ext| ext.to_str())
            .is_none_or(|ext| !ext.eq_ignore_ascii_case("exe"))
        {
            return reject("the Bitbucket Craft binary must be an executable file");
        }
        let response = Command::new(&self.craft)
            .arg("bitbucket")
            .args(command)
            .args([
                "--repo",
                &self.repository,
                "--profile",
                &self.craft_profile,
                "--state-root",
            ])
            .arg(&self.craft_state)
            .args(["--timeout-ms", "10000", "--max-bytes", "65536"])
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()?;
        let mut envelope: Value = serde_json::from_slice(&response.stdout)?;
        if !response.status.success() || envelope["ok"] != true {
            let code = envelope
                .pointer("/error/code")
                .and_then(Value::as_str)
                .filter(|code| {
                    code.len() <= 80
                        && code
                            .bytes()
                            .all(|byte| byte.is_ascii_uppercase() || byte == b'_')
                })
                .unwrap_or("UNKNOWN");
            return reject(format!("Craft could not read Bitbucket ({code})"));
        }
        if envelope["command"] != format!("bitbucket.{}.{}", command[0], command[1])
            || envelope["schema_version"] != "1.0"
            || envelope
                .pointer("/data/target/provider")
                .and_then(Value::as_str)
                != Some("bitbucket")
            || envelope
                .pointer("/data/target/tenant")
                .and_then(Value::as_str)
                != Some(&self.workspace)
        {
            return reject(
                "Craft read did not identify the configured Bitbucket workspace and command",
            );
        }
        let page = envelope["page"].take();
        let mut data = envelope["data"].take();
        data.as_object_mut()
            .ok_or("Craft returned no provider data")?
            .insert("page".into(), page);
        Ok(data)
    }
}

pub fn observe(receipt: &Receipt, policy: &Policy, pipeline: &str) -> Result<Value> {
    policy.validate()?;
    if receipt.publication_hold.is_some() {
        return reject("reconcile publication before verifying Bitbucket checks");
    }
    let draft = receipt
        .draft
        .as_ref()
        .ok_or("bind the human-published pull request first")?;
    let head = receipt.candidate.as_deref().ok_or("no current candidate")?;
    if head != draft.head {
        return reject("bind the draft to the current candidate before checking Bitbucket");
    }
    let full_name = format!("{}/{}", policy.workspace, policy.repository);
    let prefix = format!("https://bitbucket.org/{full_name}/pull-requests/");
    let pr: u64 = draft
        .reference
        .strip_prefix(&prefix)
        .filter(|id| id.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or("draft is not a canonical PR URL for the configured Bitbucket repository")?
        .parse()?;
    if pr == 0 {
        return reject("invalid Bitbucket pull request number");
    }
    let protected = |path: &str| {
        std::iter::once("bitbucket-pipelines.yml")
            .chain(policy.check_definitions.iter().map(String::as_str))
            .any(|check| {
                let path = path.to_ascii_lowercase();
                let check = check.to_ascii_lowercase();
                path == check
                    || path
                        .strip_prefix(&check)
                        .is_some_and(|part| part.starts_with('/'))
            })
    };
    if let Some(path) =
        repository::candidate_history_paths(&receipt.work_order, &receipt.work_order.base, head)?
            .into_iter()
            .find(|path| protected(path))
    {
        return reject(format!(
            "candidate changes a protected check definition: {path:?}"
        ));
    }
    let pull_request = || {
        policy.read(
            ["pullrequest", "get"],
            &[
                "--pull-request-id",
                &pr.to_string(),
                "--fields",
                "id,state,source_repository,source_branch,destination_branch",
            ],
        )
    };
    validate_pr(
        &pull_request()?["item"],
        &full_name,
        &policy.base_branch,
        pr,
    )?;
    let uuid = pipeline.trim_matches(['{', '}']).to_ascii_lowercase();
    if uuid.split('-').map(str::len).ne([8, 4, 4, 4, 12])
        || !uuid
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
    {
        return reject("supply a Bitbucket pipeline UUID");
    }
    let pipeline = policy.read(
        ["pipeline", "get"],
        &[
            "--pipeline-uuid",
            &uuid,
            "--fields",
            "id,build_number,commit,state,result",
        ],
    )?;
    validate_pipeline(&pipeline["item"], &uuid, head)?;
    let steps = policy.read(
        ["step", "list"],
        &[
            "--pipeline-uuid",
            &uuid,
            "--fields",
            "id,name,state,result",
            "--limit",
            "50",
            "--page-size",
            "50",
        ],
    )?;
    if steps.pointer("/page/has_more").and_then(Value::as_bool) != Some(false)
        || steps.pointer("/page/truncated").and_then(Value::as_bool) != Some(false)
        || steps.pointer("/page/consistency").and_then(Value::as_str) != Some("live")
        || steps["cached"] != false
    {
        return reject("Bitbucket steps could not be observed in one fresh complete page");
    }
    let steps = steps["items"]
        .as_array()
        .ok_or("Bitbucket step list is missing")?;
    let mut observed = Vec::new();
    for name in &policy.required_steps {
        let mut matching = steps
            .iter()
            .filter(|step| step["name"].as_str() == Some(name));
        let step = matching
            .next()
            .ok_or_else(|| format!("missing required Bitbucket step {name:?}"))?;
        if matching.next().is_some()
            || step["state"] != "COMPLETED"
            || step["result"] != "SUCCESSFUL"
        {
            return reject(format!(
                "required Bitbucket step {name:?} is ambiguous or unsuccessful"
            ));
        }
        let id = step["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or("Bitbucket step identity is missing")?;
        observed.push(json!({"id": id, "name": name}));
    }
    validate_pr(
        &pull_request()?["item"],
        &full_name,
        &policy.base_branch,
        pr,
    )?;
    Ok(json!({
        "provider": "bitbucket", "pr": draft.reference, "head": head,
        "pull_request_head_source": "operator_attestation", "pipeline": uuid,
        "build_number": pipeline["item"]["build_number"], "checks": observed
    }))
}

fn validate_pr(item: &Value, repo: &str, base: &str, id: u64) -> Result<()> {
    if item["id"].as_u64() != Some(id)
        || item["state"] != "OPEN"
        || item["source_repository"].as_str() != Some(repo)
        || item["source_branch"].as_str().is_none_or(str::is_empty)
        || item["destination_branch"].as_str() != Some(base)
    {
        return reject("Bitbucket PR is not the open same-repository PR expected by this pilot");
    }
    Ok(())
}

fn validate_pipeline(item: &Value, uuid: &str, head: &str) -> Result<()> {
    if item["id"].as_str() != Some(uuid)
        || item["commit"].as_str() != Some(head)
        || item["state"] != "COMPLETED"
        || item["result"] != "SUCCESSFUL"
    {
        return reject(
            "Bitbucket pipeline is stale, incomplete, or unsuccessful for the candidate",
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_verification_requires_native_identity_and_exact_candidate() {
        let mut result = json!({"id":"pipeline", "commit":"new head", "state":"COMPLETED", "result":"SUCCESSFUL"});
        assert!(validate_pipeline(&result, "pipeline", "new head").is_ok());
        assert!(validate_pipeline(&result, "other pipeline", "new head").is_err());
        assert!(validate_pipeline(&result, "pipeline", "old head").is_err());
        result["result"] = json!("FAILED");
        assert!(validate_pipeline(&result, "pipeline", "new head").is_err());
    }
}
