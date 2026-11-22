use std::path::{Path, PathBuf};

use clap::Args;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::jira_standing::JiraReader;
use crate::order::{PinnedReference, WorkOrder};
use crate::{Result, read_json, reject, shared_runner::sha256, snapshot, store::Store};

#[derive(Args)]
pub struct Options {
    policy: PathBuf,
    order: PathBuf,
    /// Digest from the preview reviewed by the operator.
    #[arg(long, requires = "destination")]
    approval_digest: Option<String>,
    /// New private directory for the frozen work order and snapshot.
    #[arg(long, requires = "approval_digest")]
    destination: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    schema_version: u32,
    craft: PathBuf,
    craft_state: PathBuf,
    craft_profile: String,
    cloud_id: String,
    issue_id: String,
    issue_url: String,
}

pub fn capture(options: Options) -> Result<Value> {
    let policy: Policy = read_json(options.policy)?;
    let mut order: WorkOrder = read_json(options.order)?;
    let url: ureq::http::Uri = policy.issue_url.parse()?;
    if policy.schema_version != 1
        || url.scheme_str() != Some("https")
        || url
            .authority()
            .is_none_or(|host| host.as_str().contains('@'))
        || url.path() != format!("/browse/{}", order.issue)
        || url.query().is_some()
        || policy.issue_url.contains('#')
    {
        return reject(
            "the Jira policy requires an HTTPS browse URL for the work order's exact issue",
        );
    }
    if snapshot::reference(&order)?.is_some() {
        return reject("provide the unpinned work order template to capture a Jira snapshot");
    }
    let issue = policy.observe(&order.issue)?;
    let approval = sha256(&serde_json::to_vec(&json!([
        "jira-supervised-snapshot-v1",
        policy.cloud_id,
        policy.issue_id,
        policy.issue_url,
        issue["intent_digest"],
        issue["assignee"],
        issue["status"],
        order,
    ]))?);
    if options
        .approval_digest
        .as_ref()
        .is_some_and(|expected| expected != &approval)
    {
        return reject("the Jira issue or work order changed after review; capture a new preview");
    }
    let document = serde_json::to_vec_pretty(&json!({
        "key": order.issue, "url": policy.issue_url, "intent": order.intent, "acceptance": order.acceptance,
        "source": {"cloud_id": policy.cloud_id, "issue_id": policy.issue_id,
            "intent_digest": issue["intent_digest"], "assignee": issue["assignee"], "status": issue["status"]},
        "approval_digest": approval,
    }))?;
    let reference = PinnedReference {
        location: policy.issue_url.clone(),
        revision: sha256(&document),
    };
    order.context.push(reference.clone());
    order.validate()?;
    let prepared = if let Some(destination) = options.destination {
        let (snapshot_path, order_path) =
            snapshot::save(&order, &destination, "jira-snapshot.json", &document)?;
        Some(json!({"work_order": order_path, "jira_snapshot": snapshot_path, "pin": reference}))
    } else {
        None
    };
    Ok(
        json!({"schema_version": 1, "issue": order.issue, "url": policy.issue_url,
            "assignee": issue["assignee"], "status": issue["status"], "source_revision": issue["source_revision"],
            "observed_at": issue["observed_at"],
        "approval_digest": approval, "prepared": prepared}),
    )
}

pub fn verify_live(state: &Path, id: &str, policy: &Path) -> Result<Value> {
    let policy: Policy = read_json(policy)?;
    let mut store = Store::open(state)?;
    let order = store.status(id)?.work_order;
    let path = store.issue_snapshot_path(id)?;
    let frozen: Value = serde_json::from_slice(&snapshot::verified_bytes(&order, &path)?)?;
    let source = &frozen["source"];
    if source["intent_digest"].as_str().is_none() || source.get("assignee").is_none() {
        return reject(
            "this snapshot has no native Jira fingerprint; capture and review the issue to enable a live check",
        );
    }
    if policy.schema_version != 1
        || frozen["url"] != policy.issue_url
        || source["cloud_id"] != policy.cloud_id
        || source["issue_id"] != policy.issue_id
    {
        return reject("the Jira policy does not match the approved snapshot identity");
    }
    let current = policy.observe(&order.issue)?;
    if ["intent_digest", "assignee", "status"]
        .into_iter()
        .any(|key| current[key] != source[key])
    {
        return reject(
            "the Jira issue changed since approval; review the current issue before proceeding",
        );
    }
    Ok(
        json!({"schema_version": 1, "work_order_id": id, "issue": order.issue,
        "decision": "current", "source_revision": current["source_revision"], "observed_at": current["observed_at"]}),
    )
}

impl Policy {
    fn observe(&self, issue: &str) -> Result<Value> {
        let reader = JiraReader {
            craft: &self.craft,
            craft_state: &self.craft_state,
            craft_profile: &self.craft_profile,
            cloud_id: &self.cloud_id,
        };
        reader.validate()?;
        reader.observe(issue, &self.issue_id)
    }
}
