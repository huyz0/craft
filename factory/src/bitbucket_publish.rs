use std::{
    path::Path,
    process::Stdio,
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use ureq::Agent;

use crate::{
    Result, order::valid_oid, process_tree::ProcessTree, reject, repository, twg::DraftTarget,
};

pub struct Publisher {
    agent: Agent,
    token: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DraftUpdate {
    pub previous_head: String,
    pub pull_request: u64,
}

impl DraftUpdate {
    pub fn new(target: &DraftTarget, head: &str, native_url: &str) -> Result<Self> {
        let prefix = format!("https://bitbucket.org/{}/pull-requests/", target.repository);
        let id = native_url.strip_prefix(&prefix).unwrap_or_default();
        if !valid_oid(head)
            || head.len() != target.head.len()
            || head == target.head
            || head == target.base
            || id.is_empty()
            || id.len() > 18
            || id.starts_with('0')
            || !id.bytes().all(|byte| byte.is_ascii_digit())
        {
            return reject("invalid existing Bitbucket draft binding");
        }
        Ok(Self {
            previous_head: head.to_owned(),
            pull_request: id.parse()?,
        })
    }
}

impl Publisher {
    pub fn open() -> Result<Self> {
        let token = std::env::var("CRAFT_FACTORY_BITBUCKET_TOKEN")
            .map_err(|_| "CRAFT_FACTORY_BITBUCKET_TOKEN is required on the publisher host")?;
        if !(24..=512).contains(&token.len()) || !token.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return reject("invalid Bitbucket publisher credential");
        }
        Ok(Self {
            token,
            agent: Agent::config_builder()
                .https_only(true)
                .max_redirects(0)
                .proxy(None)
                .timeout_global(Some(Duration::from_secs(15)))
                .build()
                .into(),
        })
    }

    pub fn assert_base(&self, target: &DraftTarget) -> Result<()> {
        validate_target(target)?;
        self.assert_branch(&target.repository, &target.base_branch, &target.base)
    }

    pub fn assert_source_absent(&self, target: &DraftTarget) -> Result<()> {
        let url = branch_url(&target.repository, &target.source_branch);
        match self
            .agent
            .get(&url)
            .header("authorization", format!("Bearer {}", self.token))
            .call()
        {
            Err(ureq::Error::StatusCode(404)) => Ok(()),
            Ok(_) => reject("the reserved Bitbucket source branch already exists"),
            Err(_) => reject("could not confirm the reserved source branch is absent"),
        }
    }

    fn assert_branch(&self, repository: &str, branch: &str, expected: &str) -> Result<()> {
        let url = branch_url(repository, branch);
        let mut response = self
            .agent
            .get(&url)
            .header("authorization", format!("Bearer {}", self.token))
            .call()
            .map_err(|_| "Bitbucket branch read failed")?;
        if response.status() != 200 {
            return reject("Bitbucket branch read did not succeed");
        }
        let observed: Value = response
            .body_mut()
            .with_config()
            .limit(256 * 1024)
            .read_json()?;
        if observed["name"] != branch || observed["target"]["hash"] != expected {
            return reject("Bitbucket branch differs from its pinned commit");
        }
        Ok(())
    }

    pub fn create(&self, target: &DraftTarget, checkout: &Path, work_id: &str) -> Result<Value> {
        self.assert_base(target)?;
        self.assert_source_absent(target)?;
        push_branch(checkout, target, "", &self.token)?;
        self.assert_branch(&target.repository, &target.source_branch, &target.head)?;
        self.assert_base(target)?;
        let endpoint = format!(
            "https://api.bitbucket.org/2.0/repositories/{}/pullrequests",
            target.repository
        );
        let mut created = self
            .agent
            .post(&endpoint)
            .header("authorization", format!("Bearer {}", self.token))
            .send_json(draft_body(target, work_id))
            .map_err(|_| "Bitbucket draft write did not return a usable response")?;
        if created.status() != 201 {
            return reject("Bitbucket draft write did not return HTTP 201");
        }
        let result: Value = created
            .body_mut()
            .with_config()
            .limit(256 * 1024)
            .read_json()?;
        let id = result["id"]
            .as_u64()
            .filter(|id| *id > 0)
            .ok_or("Bitbucket did not supply a draft ID")?;
        self.confirm(target, id)
    }

    pub fn assert_update_target(&self, target: &DraftTarget, update: &DraftUpdate) -> Result<()> {
        self.assert_base(target)?;
        self.assert_branch(
            &target.repository,
            &target.source_branch,
            &update.previous_head,
        )?;
        let reviewed = DraftTarget {
            head: update.previous_head.clone(),
            ..target.clone()
        };
        self.read_draft(&reviewed, update.pull_request)?;
        Ok(())
    }

    pub fn update(
        &self,
        target: &DraftTarget,
        checkout: &Path,
        update: &DraftUpdate,
    ) -> Result<Value> {
        self.assert_update_target(target, update)?;
        // The isolated pack validation already proved that head descends from previous_head.
        push_branch(checkout, target, &update.previous_head, &self.token)?;
        self.confirm(target, update.pull_request)
    }

    fn read_draft(&self, target: &DraftTarget, id: u64) -> Result<String> {
        let url = format!(
            "https://api.bitbucket.org/2.0/repositories/{}/pullrequests/{id}",
            target.repository
        );
        let mut read = self
            .agent
            .get(&url)
            .header("authorization", format!("Bearer {}", self.token))
            .call()
            .map_err(|_| "Bitbucket draft readback failed")?;
        if read.status() != 200 {
            return reject("Bitbucket draft readback did not succeed");
        }
        let observed: Value = read
            .body_mut()
            .with_config()
            .limit(256 * 1024)
            .read_json()?;
        verify_draft(target, id, &observed)
    }

    fn confirm(&self, target: &DraftTarget, id: u64) -> Result<Value> {
        let url = self.read_draft(target, id)?;
        self.assert_branch(&target.repository, &target.source_branch, &target.head)?;
        self.assert_base(target)?;
        Ok(json!({
            "outcome": "confirmed", "head": target.head, "native_url": url,
            "source_branch": target.source_branch, "base_branch": target.base_branch,
            "evidence": format!("Bitbucket REST readback confirmed draft {url} and its pinned revisions")
        }))
    }
}

pub(crate) fn branch_url(repository: &str, branch: &str) -> String {
    let encoded = branch.replace('/', "%2F");
    format!("https://api.bitbucket.org/2.0/repositories/{repository}/refs/branches/{encoded}")
}

fn draft_body(target: &DraftTarget, work_id: &str) -> Value {
    json!({
        "title": format!("Craft Factory draft for {work_id}"),
        "description": format!("Work order {work_id}. Candidate: {}. Base: {}.", target.head, target.base),
        "source": {"branch": {"name": target.source_branch}},
        "destination": {"branch": {"name": target.base_branch}},
        "draft": true,
    })
}

fn verify_draft(target: &DraftTarget, id: u64, observed: &Value) -> Result<String> {
    let url = format!(
        "https://bitbucket.org/{}/pull-requests/{id}",
        target.repository
    );
    if observed["id"].as_u64() != Some(id)
        || observed["state"] != "OPEN"
        || observed["draft"] != true
        || observed["links"]["html"]["href"] != url
        || observed["source"]["repository"]["full_name"] != target.repository
        || observed["destination"]["repository"]["full_name"] != target.repository
        || observed["source"]["branch"]["name"] != target.source_branch
        || observed["destination"]["branch"]["name"] != target.base_branch
        || !crate::twg::hash_matches(observed.pointer("/source/commit/hash"), &target.head)
        || !crate::twg::hash_matches(observed.pointer("/destination/commit/hash"), &target.base)
    {
        return reject("Bitbucket draft readback does not match the reserved candidate and base");
    }
    Ok(url)
}

fn validate_target(target: &DraftTarget) -> Result<()> {
    if target.repository.split('/').count() != 2
        || !target.repository.split('/').all(|part| {
            !part.is_empty()
                && part.bytes().all(|b| {
                    b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-')
                })
        })
        || !valid_oid(&target.head)
        || !valid_oid(&target.base)
        || target.head == target.base
        || target.source_branch == target.base_branch
        || !target.source_branch.starts_with("craft-factory/")
    {
        return reject("publication target is outside the Bitbucket draft policy");
    }
    for branch in [&target.base_branch, &target.source_branch] {
        if !branch
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-'))
            || !repository::git_command()
                .args(["check-ref-format", &format!("refs/heads/{branch}")])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?
                .success()
        {
            return reject("invalid publication branch");
        }
    }
    Ok(())
}

fn push_arguments(target: &DraftTarget, expected: &str, url: &str) -> Vec<String> {
    let reference = format!("refs/heads/{}", target.source_branch);
    vec![
        "push".to_owned(),
        "--porcelain".to_owned(),
        format!("--force-with-lease={reference}:{expected}"),
        url.to_owned(),
        format!("{}:{reference}", target.head),
    ]
}

fn push_branch(checkout: &Path, target: &DraftTarget, expected: &str, token: &str) -> Result<()> {
    let url = format!("https://bitbucket.org/{}.git", target.repository);
    let credential = STANDARD.encode(format!("x-token-auth:{token}"));
    let mut command = repository::isolated_git_command();
    command
        .arg("-C")
        .arg(checkout)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_COUNT", "1")
        .env(
            "GIT_CONFIG_KEY_0",
            "http.https://bitbucket.org/.extraHeader",
        )
        .env(
            "GIT_CONFIG_VALUE_0",
            format!("Authorization: Basic {credential}"),
        )
        .env_remove("CRAFT_FACTORY_PUBLISHER_TOKEN")
        .env_remove("CRAFT_FACTORY_BITBUCKET_TOKEN")
        .args([
            "-c",
            "core.hooksPath=.git/factory-disabled-hooks",
            "-c",
            "http.followRedirects=false",
            "-c",
            "http.sslVerify=true",
            "-c",
            "http.lowSpeedLimit=1",
            "-c",
            "http.lowSpeedTime=15",
            "-c",
            "credential.helper=",
        ])
        .args(push_arguments(target, expected, &url))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    ProcessTree::configure(&mut command);
    let mut child = command.spawn()?;
    let tree = match ProcessTree::attach(&child) {
        Ok(tree) => tree,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error.into());
        }
    };
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        if let Some(status) = child.try_wait()? {
            return if status.success() {
                Ok(())
            } else {
                reject("Bitbucket source-branch push did not succeed")
            };
        }
        if Instant::now() >= deadline {
            drop(tree);
            let _ = child.wait();
            return reject("Bitbucket source-branch push timed out");
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> DraftTarget {
        DraftTarget {
            repository: "team/service".into(),
            head: "a".repeat(40),
            base: "b".repeat(40),
            source_branch: "craft-factory/test".into(),
            base_branch: "main".into(),
        }
    }

    #[test]
    fn draft_response_must_bind_both_reserved_refs() {
        let target = target();
        let body = draft_body(&target, "test");
        assert_eq!(body["draft"], true);
        assert!(body.get("reviewers").is_none());
        assert_eq!(body["source"]["branch"]["name"], target.source_branch);
        assert_eq!(body["destination"]["branch"]["name"], target.base_branch);
        assert_eq!(
            branch_url(&target.repository, &target.source_branch),
            "https://api.bitbucket.org/2.0/repositories/team/service/refs/branches/craft-factory%2Ftest"
        );
        let mut reply = json!({
            "id": 12, "draft": true, "state": "OPEN",
            "links": {"html": {"href": "https://bitbucket.org/team/service/pull-requests/12"}},
            "source": {"branch": {"name": target.source_branch}, "commit": {"hash": target.head},
                       "repository": {"full_name": target.repository}},
            "destination": {"branch": {"name": target.base_branch}, "commit": {"hash": &target.base[..12]},
                            "repository": {"full_name": target.repository}},
        });
        assert!(verify_draft(&target, 12, &reply).is_ok());
        reply["draft"] = json!(false);
        assert!(verify_draft(&target, 12, &reply).is_err());
        reply["draft"] = json!(true);
        reply["state"] = json!("DECLINED");
        assert!(verify_draft(&target, 12, &reply).is_err());
        reply["state"] = json!("OPEN");
        reply["destination"]["commit"]["hash"] = json!("c".repeat(40));
        assert!(verify_draft(&target, 12, &reply).is_err());
    }

    #[test]
    fn draft_update_is_bound_to_one_existing_pr() {
        let target = target();
        let reviewed = "c".repeat(40);
        let url = "https://bitbucket.org/team/service/pull-requests/42";
        let update = DraftUpdate::new(&target, &reviewed, url).unwrap();
        assert_eq!(update.pull_request, 42);
        for url in [
            "https://bitbucket.org/team/other/pull-requests/42",
            "https://bitbucket.org/team/service/pull-requests/042",
            "https://bitbucket.org/team/service/pull-requests/42?example=1",
        ] {
            assert!(DraftUpdate::new(&target, &reviewed, url).is_err());
        }
        assert!(DraftUpdate::new(&target, &target.head, url).is_err());
    }

    // The local-path transport starts a shell for receive-pack.
    #[cfg(unix)]
    #[test]
    fn source_branch_lease_rejects_a_concurrent_change() {
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("craft-push-{}-{id}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let git = |args: &[&str]| {
            repository::git_command()
                .current_dir(&root)
                .args(args)
                .output()
                .unwrap()
        };
        let commit = |name: &str| {
            let result = git(&[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                name,
            ]);
            assert!(result.status.success(), "{result:?}");
            String::from_utf8(git(&["rev-parse", "HEAD"]).stdout)
                .unwrap()
                .trim()
                .to_owned()
        };
        assert!(git(&["init", "--quiet"]).status.success());
        assert!(
            git(&["init", "--quiet", "--bare", "forge.git"])
                .status
                .success()
        );
        let push = |target: &DraftTarget, previous: &str| {
            let args = push_arguments(target, previous, "./forge.git");
            git(&args.iter().map(String::as_str).collect::<Vec<_>>())
        };
        let mut target = target();
        let reviewed = commit("Reviewed");
        target.head.clone_from(&reviewed);
        let created = push(&target, "");
        assert!(
            created.status.success(),
            "{}",
            String::from_utf8_lossy(&created.stderr)
        );
        let repaired = commit("Repair");
        let maintainer = commit("Maintainer");
        target.head.clone_from(&maintainer);
        assert!(push(&target, &reviewed).status.success());
        target.head = repaired;
        assert!(!push(&target, &reviewed).status.success());
        assert_eq!(
            String::from_utf8(
                git(&[
                    "--git-dir=forge.git",
                    "rev-parse",
                    "refs/heads/craft-factory/test"
                ])
                .stdout
            )
            .unwrap()
            .trim(),
            maintainer,
        );
        target.head = commit("Repair of maintainer version");
        assert!(push(&target, &maintainer).status.success());
        std::fs::remove_dir_all(root).unwrap();
    }
}
