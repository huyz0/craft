use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use clap::Args;
use serde::Deserialize;
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::order::{PinnedReference, WorkOrder};
use crate::{Result, github, read_json, reject, shared_runner::sha256, snapshot, store::Store};

#[derive(Args)]
pub struct Options {
    policy: PathBuf,
    order: PathBuf,
    /// Digest from the preview reviewed by the operator.
    #[arg(long, requires = "destination")]
    approval_digest: Option<String>,
    /// New private directory for the frozen work order and issue snapshot.
    #[arg(long, requires = "approval_digest")]
    destination: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    schema_version: u32,
    gh: PathBuf,
    repository: String,
    number: u64,
    issue_id: u64,
}

#[derive(Args)]
pub struct StandingOptions {
    policy: PathBuf,
    order: PathBuf,
    /// Submit to the shared queue. Without a queue, preview the observation.
    #[arg(long)]
    queue: Option<String>,
    #[arg(long, requires = "queue")]
    ca: Option<PathBuf>,
    #[arg(long, requires = "queue")]
    intake_token_file: Option<PathBuf>,
    /// Atomically replace the last completed observation or queue decision.
    #[arg(long)]
    report_file: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StandingPolicy {
    issue: Policy,
    rule: String,
    approved_intent: Option<String>,
}

struct Observation {
    source: Value,
    updated_at: String,
    source_revision: i64,
    labels: Value,
}

pub fn poll_standing(options: StandingOptions) -> Result<Value> {
    let policy: StandingPolicy = read_json(&options.policy)?;
    let mounted = crate::evidence_capture::credential_path("CRAFT_FACTORY_INTAKE_TOKEN");
    crate::evidence_capture::check_output_paths(
        &[options.report_file.as_deref()],
        [
            Some(options.policy.as_path()),
            Some(options.order.as_path()),
            Some(policy.issue.gh.as_path()),
            options.ca.as_deref(),
            options.intake_token_file.as_deref(),
            mounted.as_deref(),
        ]
        .into_iter()
        .flatten(),
        "the GitHub standing report path must be distinct from the inputs",
    )?;
    let mut order: WorkOrder = read_json(options.order)?;
    let url = policy.issue.url(&order.issue)?;
    order.validate()?;
    if order
        .context
        .iter()
        .any(|pin| pin.location.starts_with("github-intent:"))
    {
        return reject("provide an unpinned GitHub standing work order template");
    }
    let observed = policy.issue.observe(&url)?;
    let labels = observed
        .labels
        .as_array()
        .ok_or("the GitHub issue is missing labels")?;
    let labels = labels
        .iter()
        .map(|label| label["name"].as_str().ok_or("invalid GitHub issue label"))
        .collect::<std::result::Result<BTreeSet<_>, _>>()?;
    let assignees = observed.source["assignees"]
        .as_array()
        .ok_or("the GitHub issue is missing assignees")?;
    let assignee = match assignees.as_slice() {
        [assignee] => assignee.as_u64().map(|id| id.to_string()),
        _ => None,
    };
    let digest = observed.source["intent_digest"]
        .as_str()
        .ok_or("the GitHub issue is missing an intent fingerprint")?;
    let snapshot = json!({
        "source": format!("github:{}", policy.issue.repository),
        "issue": format!("GITHUB-{}", policy.issue.number),
        "source_issue_id": policy.issue.issue_id,
        "source_revision": observed.source_revision,
        "observed_at": OffsetDateTime::now_utc().unix_timestamp(),
        "assignee": assignee, "status": observed.source["state"], "labels": labels,
        "intent_digest": digest, "intent_approved": policy.approved_intent.as_deref() == Some(digest),
    });
    let mut result =
        json!({"schema_version": 1, "rule": policy.rule, "url": url, "snapshot": snapshot});
    if let Some(queue) = options.queue {
        order.context.push(PinnedReference {
            location: format!(
                "github-intent:{}/{}",
                policy.issue.repository, policy.issue.issue_id
            ),
            revision: digest.to_owned(),
        });
        let receipt = crate::shared_runner::submit_standing(
            &queue,
            options.ca.as_deref(),
            options.intake_token_file.as_deref(),
            &policy.rule,
            &snapshot,
            Some(&serde_json::to_value(order)?),
        )?;
        if !receipt["decision"]["state"].is_string() {
            return reject("the shared queue did not return a standing decision");
        }
        result["decision"] = receipt["decision"].clone();
    }
    if let Some(path) = &options.report_file {
        crate::evidence_capture::replace_json(path, &result)?;
    }
    Ok(result)
}

pub fn capture(options: Options) -> Result<Value> {
    let policy: Policy = read_json(options.policy)?;
    let mut order: WorkOrder = read_json(options.order)?;
    order.validate()?;
    let url = policy.url(&order.issue)?;
    if snapshot::reference(&order)?.is_some() {
        return reject("provide the unpinned work order template to capture a GitHub issue");
    }
    let Observation {
        source, updated_at, ..
    } = policy.observe(&url)?.require_open()?;
    let approval = sha256(&serde_json::to_vec(&json!([
        "github-supervised-issue-v1",
        url,
        source,
        order,
    ]))?);
    if options
        .approval_digest
        .as_ref()
        .is_some_and(|expected| expected != &approval)
    {
        return reject(
            "the GitHub issue or work order changed after review; capture a new preview",
        );
    }
    let prepared = if let Some(destination) = options.destination {
        let document = serde_json::to_vec_pretty(&json!({
            "key": order.issue, "url": url, "intent": order.intent, "acceptance": order.acceptance,
            "source": source, "approval_digest": approval,
        }))?;
        let reference = PinnedReference {
            location: format!("{}{url}", snapshot::GITHUB_ISSUE_PREFIX),
            revision: sha256(&document),
        };
        order.context.push(reference.clone());
        order.validate()?;
        let (path, order_path) = snapshot::save(
            &order,
            &destination,
            "github-issue-snapshot.json",
            &document,
        )?;
        Some(json!({"work_order": order_path, "issue_snapshot": path, "pin": reference}))
    } else {
        None
    };
    Ok(
        json!({"schema_version": 1, "issue": order.issue, "url": url, "source": source,
        "updated_at": updated_at, "approval_digest": approval, "prepared": prepared}),
    )
}

pub fn verify_live(state: &Path, id: &str, policy: &Path) -> Result<Value> {
    let policy: Policy = read_json(policy)?;
    let mut store = Store::open(state)?;
    let order = store.status(id)?.work_order;
    let url = policy.url(&order.issue)?;
    let path = store.issue_snapshot_path(id)?;
    let frozen: Value = serde_json::from_slice(&snapshot::verified_bytes(&order, &path)?)?;
    let source = &frozen["source"];
    if frozen["url"] != url
        || source["repository"] != policy.repository
        || source["issue_id"] != policy.issue_id
        || source["intent_digest"].as_str().is_none()
    {
        return reject("the GitHub policy does not match the approved snapshot identity");
    }
    let Observation {
        source: current,
        updated_at,
        ..
    } = policy.observe(&url)?.require_open()?;
    if &current != source {
        return reject(
            "the GitHub issue changed since approval; review the current issue before proceeding",
        );
    }
    Ok(
        json!({"schema_version": 1, "work_order_id": id, "issue": order.issue,
        "decision": "current", "updated_at": updated_at}),
    )
}

impl Policy {
    fn url(&self, issue: &str) -> Result<String> {
        if self.schema_version != 1
            || self.issue_id == 0
            || issue != format!("{}#{}", self.repository, self.number)
        {
            return reject(
                "the GitHub policy must identify the work order's exact repository and issue",
            );
        }
        snapshot::github_issue_url(issue).ok_or_else(|| "invalid GitHub issue key".into())
    }

    fn observe(&self, url: &str) -> Result<Observation> {
        github::validate_gh(&self.gh)?;
        let repo_endpoint = format!("repos/{}", self.repository);
        let endpoint = format!("{repo_endpoint}/issues/{}", self.number);
        let data: Value = serde_json::from_slice(&github::read_bytes(&self.gh, &endpoint, &[])?)?;
        if data.get("pull_request").is_some()
            || data["number"] != self.number
            || data["id"] != self.issue_id
            || data["html_url"] != url
            || data["repository_url"] != format!("https://api.github.com/{repo_endpoint}")
        {
            return reject("GitHub did not return the pinned issue identity");
        }
        let state = data["state"]
            .as_str()
            .filter(|state| matches!(*state, "open" | "closed"))
            .ok_or("the GitHub issue has an unknown state")?;
        let title = data["title"]
            .as_str()
            .filter(|value| !value.trim().is_empty())
            .ok_or("the GitHub issue is missing its title")?;
        let body = match data.get("body") {
            Some(Value::String(body)) => Some(body.as_str()),
            Some(Value::Null) => None,
            _ => return reject("the GitHub issue is missing its body field"),
        };
        let updated_at = data["updated_at"]
            .as_str()
            .ok_or("the GitHub issue is missing a valid update time")?;
        let source_revision = OffsetDateTime::parse(updated_at, &Rfc3339)?.unix_timestamp();
        let accounts = data["assignees"]
            .as_array()
            .ok_or("the GitHub issue is missing assignees")?;
        let mut assignees = BTreeSet::new();
        for account in accounts {
            let id = account["id"]
                .as_u64()
                .filter(|id| *id > 0)
                .ok_or("invalid GitHub issue assignee ID")?;
            if !assignees.insert(id) {
                return reject("duplicate GitHub issue assignee ID");
            }
        }
        let digest = sha256(&serde_json::to_vec(&(title, body))?);
        Ok(Observation {
            source: json!({"repository": self.repository, "issue_id": self.issue_id,
                "intent_digest": digest, "assignees": assignees, "state": state}),
            updated_at: updated_at.to_owned(),
            source_revision,
            labels: data["labels"].clone(),
        })
    }
}

impl Observation {
    fn require_open(self) -> Result<Self> {
        if self.source["state"] != "open" {
            return reject("the pinned GitHub issue is no longer open");
        }
        Ok(self)
    }
}
