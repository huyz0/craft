use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use clap::{Args, Subcommand};
use craft_factory_evidence::{
    GithubMergeQueueReadiness, GithubShadowPolicy, queue_identity::identifier,
};
use serde_json::{Value, json};

use crate::{
    Result, github, order::WorkOrder, read_json, reject, repository, review::Draft, snapshot,
};

use super::{
    CandidatePack, Checkout, PACK_LIMIT, Queue, QueueRole, bind_checkout, load_shared_ci_policy,
    read_queue_token_file, sha256, validate_queue_token, work_text,
};

#[derive(Args)]
pub struct Options {
    #[arg(long)]
    queue: String,
    #[arg(long)]
    ca: Option<PathBuf>,
    #[arg(long)]
    queue_token_file: Option<PathBuf>,
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Approve standing bounds for low-risk GitHub enqueue.
    ApprovePolicy {
        policy: PathBuf,
        #[arg(long)]
        digest: String,
    },
    /// Revoke standing authorization before any further send starts.
    RevokePolicy {
        policy_id: String,
        #[arg(long)]
        evidence: String,
    },
    /// Authorize an exact candidate under a standing policy as its named publisher.
    Authorize {
        intent_id: String,
        #[arg(long)]
        policy_id: String,
        #[arg(long)]
        receipt_id: String,
        #[arg(long)]
        head: String,
        #[arg(long)]
        report_digest: String,
    },
    /// Authorize one exact readiness receipt for a named publisher.
    Approve {
        intent_id: String,
        #[arg(long)]
        receipt_id: String,
        #[arg(long)]
        head: String,
        #[arg(long)]
        report_digest: String,
        #[arg(long)]
        publisher: String,
        #[arg(long)]
        evidence: String,
        #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(i32).range(1..=600))]
        valid_seconds: i32,
    },
    /// Recheck readiness, then send the native queue-only request once.
    Send {
        intent_id: String,
        #[arg(long)]
        checkout: PathBuf,
        #[arg(long)]
        policy: PathBuf,
        #[arg(long)]
        gh: PathBuf,
        #[arg(long)]
        github_token_file: PathBuf,
    },
    /// Pick and enqueue at most one ready revision under a standing policy.
    Poll(PollOptions),
    /// Check for an exact native queue entry after an ambiguous send.
    Reconcile {
        intent_id: String,
        #[arg(long)]
        gh: PathBuf,
        #[arg(long)]
        evidence: String,
    },
    /// End an unused approval or close a reconciled transfer without a queue receipt.
    Abandon {
        intent_id: String,
        #[arg(long)]
        evidence: String,
    },
    /// Verify that an acknowledged PR left the native queue without merging.
    Clear {
        intent_id: String,
        #[arg(long)]
        gh: PathBuf,
        #[arg(long)]
        evidence: String,
    },
    Status {
        intent_id: String,
    },
    Holds {
        #[arg(long)]
        after_intent: Option<String>,
        #[arg(long)]
        include_queued: bool,
    },
}

#[derive(Args)]
struct PollOptions {
    policy_id: String,
    #[arg(long)]
    repository_map: PathBuf,
    #[arg(long)]
    checkout_root: PathBuf,
    #[arg(long)]
    policy: PathBuf,
    #[arg(long)]
    gh: PathBuf,
    #[arg(long)]
    github_token_file: PathBuf,
    #[arg(long)]
    report_file: Option<PathBuf>,
}

pub fn run(options: Options) -> Result<Value> {
    let role = match &options.action {
        Action::Send { .. } | Action::Authorize { .. } | Action::Poll(_) => QueueRole::Publisher,
        _ => QueueRole::Publication,
    };
    let queue = Queue::open_with_token_file(
        &options.queue,
        options.ca.as_deref(),
        role,
        options.queue_token_file.as_deref(),
    )?;
    let request = match options.action {
        Action::ApprovePolicy { policy, digest } => {
            let approved: craft_factory_evidence::github_risk::EnqueuePolicy =
                load_shared_ci_policy(&policy, &digest)?;
            if !approved.valid() {
                return reject("invalid GitHub standing enqueue policy");
            }
            json!({"action":"approve_github_enqueue_policy", "policy":approved})
        }
        Action::RevokePolicy {
            policy_id,
            evidence,
        } => {
            json!({"action":"revoke_github_enqueue_policy", "policy_id":policy_id, "evidence":evidence})
        }
        Action::Authorize {
            intent_id,
            policy_id,
            receipt_id,
            head,
            report_digest,
        } => json!({"action":"authorize_github_enqueue_policy", "target":{
            "intent_id":intent_id, "policy_id":policy_id, "receipt_id":receipt_id,
            "head":head, "report_digest":report_digest
        }}),
        Action::Approve {
            intent_id,
            receipt_id,
            head,
            report_digest,
            publisher,
            evidence,
            valid_seconds,
        } => json!({"action":"approve_github_enqueue", "approval":{
            "intent_id":intent_id, "receipt_id":receipt_id, "head":head, "report_digest":report_digest,
            "publisher_id":publisher, "evidence":evidence, "valid_seconds":valid_seconds,
        }}),
        Action::Send {
            intent_id,
            checkout,
            policy,
            gh,
            github_token_file,
        } => {
            return send(
                &queue,
                &intent_id,
                checkout,
                &policy,
                &gh,
                &github_token_file,
            );
        }
        Action::Poll(scheduled) => {
            let credential =
                crate::evidence_capture::credential_path("CRAFT_FACTORY_PUBLISHER_TOKEN");
            crate::evidence_capture::check_output_paths(
                &[scheduled.report_file.as_deref()],
                [
                    Some(scheduled.repository_map.as_path()),
                    Some(scheduled.policy.as_path()),
                    Some(scheduled.gh.as_path()),
                    Some(scheduled.github_token_file.as_path()),
                    options.queue_token_file.as_deref(),
                    options.ca.as_deref(),
                    credential.as_deref(),
                ]
                .into_iter()
                .flatten(),
                "scheduled GitHub enqueue report path must be distinct from the inputs",
            )?;
            let path = scheduled.report_file.clone();
            let report = poll(&queue, scheduled)?;
            if let Some(path) = path {
                crate::evidence_capture::replace_json(&path, &report)?;
            }
            return Ok(report);
        }
        Action::Reconcile {
            intent_id,
            gh,
            evidence,
        } => {
            if !work_text(&evidence, 500) {
                return reject("provide a reconciliation evidence reference");
            }
            let reply =
                queue.request(&json!({"action":"github_enqueue_status", "intent_id":intent_id}))?;
            let intent = &reply["github_enqueue"];
            match intent["state"].as_str() {
                Some("queued" | "merged") => return Ok(reply),
                Some("sending" | "uncertain") => {}
                _ => return reject("this GitHub enqueue has no unresolved send"),
            }
            let entry = github::observe_enqueued_head(
                &gh,
                text_field(intent, "repository")?,
                &bound_draft(intent)?,
                text_field(intent, "base_branch")?,
                text_field(intent, "pull_request_node_id")?,
            )?;
            resolution(
                "resolve_github_enqueue",
                &intent_id,
                "queued",
                Some(&entry),
                &evidence,
            )
        }
        Action::Abandon {
            intent_id,
            evidence,
        } => resolution(
            "resolve_github_enqueue",
            &intent_id,
            "abandoned",
            None,
            &evidence,
        ),
        Action::Clear {
            intent_id,
            gh,
            evidence,
        } => {
            if !work_text(&evidence, 500) {
                return reject("provide a queue clearance evidence reference");
            }
            let reply =
                queue.request(&json!({"action":"github_enqueue_status", "intent_id":intent_id}))?;
            let intent = &reply["github_enqueue"];
            if intent["state"] == "cleared" && intent["clear_evidence"] == evidence {
                return Ok(reply);
            }
            if intent["state"] != "queued" {
                return reject("queue clearance requires an acknowledged GitHub entry");
            }
            let node = text_field(intent, "pull_request_node_id")?;
            let head = github::observe_queue_clearance(
                &gh,
                text_field(intent, "repository")?,
                &bound_draft(intent)?,
                text_field(intent, "base_branch")?,
                node,
            )?;
            json!({"action":"clear_github_enqueue", "intent_id":intent_id, "observation":{
                "pull_request_node_id":node,"observed_head":head,"evidence":evidence
            }})
        }
        Action::Status { intent_id } => {
            json!({"action":"github_enqueue_status", "intent_id":intent_id})
        }
        Action::Holds {
            after_intent,
            include_queued,
        } => {
            json!({"action":"github_enqueue_holds", "after_intent":after_intent, "include_queued":include_queued})
        }
    };
    queue.request_bounded(&request, 256 * 1024)
}

fn poll(queue: &Queue, options: PollOptions) -> Result<Value> {
    if !identifier(&options.policy_id) || !options.checkout_root.is_absolute() {
        return reject("provide the standing policy ID and an absolute private checkout root");
    }
    let root = options.checkout_root.canonicalize()?;
    if !root.is_dir() || !options.gh.is_absolute() || !options.gh.is_file() {
        return reject(
            "scheduled enqueue requires an existing checkout root and an absolute GitHub CLI",
        );
    }
    let (policy, policy_digest): (github::Policy, _) = crate::shadow_merge::load(&options.policy)?;
    if !policy.require_merge_queue {
        return reject("scheduled enqueue requires a GitHub policy that mandates the native queue");
    }
    let maps: BTreeMap<String, Checkout> = read_json(&options.repository_map)?;
    let mapped = maps
        .get(&policy.repository)
        .ok_or("no trusted checkout is mapped for the scheduled GitHub repository")?;
    if root.starts_with(mapped.path.canonicalize()?) {
        return reject("the private checkout root must be outside the trusted repository");
    }
    validate_queue_token(read_queue_token_file(
        &options.github_token_file,
        "GitHub publisher token",
    )?)?;
    let reply = queue.request_bounded(
        &json!({"action":"claim_scheduled_github_enqueue", "policy_id":options.policy_id,
            "policy_digest":policy_digest}),
        800 * 1024,
    )?;
    let claim = &reply["github_enqueue_dispatch"];
    if claim.as_object().is_some_and(serde_json::Map::is_empty) {
        return Ok(json!({"state":"idle", "policy_id":options.policy_id}));
    }
    let intent = &claim["intent"];
    let intent_id = text_field(intent, "id")?;
    let candidate: CandidatePack = serde_json::from_value(claim["candidate"].clone())?;
    if !identifier(intent_id)
        || intent["scheduled"] != true
        || intent["state"] != "approved"
        || intent["send_possible"] != false
        || intent["standing_policy_id"] != options.policy_id
        || intent["repository"] != policy.repository
        || intent["base_branch"] != policy.base_branch
        || intent["work_id"] != candidate.work_id
        || candidate.repository != policy.repository
        || intent["head"] != candidate.commit
    {
        return reject("scheduled enqueue returned a different candidate binding");
    }
    let order = bind_checkout(
        candidate.work_order,
        mapped,
        &candidate.work_id,
        &candidate.repository,
        candidate.generation,
    )?;
    repository::validate_ci_anchor(&order)?;
    if snapshot::reference(&order)?.is_some() {
        snapshot::verify(
            &order,
            mapped
                .jira_snapshot
                .as_deref()
                .ok_or("map the approved Jira snapshot before enqueue")?,
        )?;
    }
    let pack = STANDARD.decode(candidate.pack_base64)?;
    if pack.len() as u64 > PACK_LIMIT
        || !pack.starts_with(b"PACK")
        || sha256(&pack) != candidate.digest
    {
        return reject("scheduled candidate pack failed size, format, or digest validation");
    }
    with_temporary_checkout(&root, |destination| {
        let checkout = repository::prepare_pack_for_verification(
            &order,
            &destination,
            &candidate.commit,
            &pack,
            candidate.repair_head.as_deref(),
        )?;
        send(
            queue,
            intent_id,
            checkout,
            &options.policy,
            &options.gh,
            &options.github_token_file,
        )
    })
}

pub(super) fn with_temporary_checkout(
    root: &Path,
    run: impl FnOnce(PathBuf) -> Result<Value>,
) -> Result<Value> {
    let mut nonce = [0; 16];
    getrandom::getrandom(&mut nonce).map_err(|_| "could not generate a GitHub checkout name")?;
    let directory = root.join(format!("github-{:032x}", u128::from_be_bytes(nonce)));
    fs::create_dir(&directory)?;
    let result = run(directory.join("candidate"));
    if fs::remove_dir_all(&directory).is_err() {
        eprintln!(
            "could not remove the temporary checkout at {}",
            directory.display()
        );
    }
    result
}

fn text_field<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value[field]
        .as_str()
        .ok_or_else(|| format!("missing GitHub enqueue field {field}").into())
}

pub(super) fn bound_draft(binding: &Value) -> Result<Draft> {
    Ok(Draft {
        head: text_field(binding, "head")?.to_owned(),
        reference: text_field(binding, "pull_request")?.to_owned(),
    })
}

fn resolution(
    action: &str,
    intent: &str,
    outcome: &str,
    entry: Option<&str>,
    evidence: &str,
) -> Value {
    json!({"action":action, "intent_id":intent, "resolution":{
        "outcome":outcome,"queue_entry_id":entry,"evidence":evidence,
    }})
}

fn send(
    queue: &Queue,
    intent_id: &str,
    checkout: PathBuf,
    policy_path: &Path,
    gh: &Path,
    token_file: &Path,
) -> Result<Value> {
    if !identifier(intent_id) || !checkout.is_absolute() {
        return reject("provide the approved intent ID and an absolute prepared checkout");
    }
    let reply = queue.request_bounded(
        &json!({"action":"prepare_github_enqueue", "intent_id":intent_id}),
        256 * 1024,
    )?;
    let prepared = &reply["github_enqueue"];
    let intent = &prepared["intent"];
    let target = &prepared["target"];
    let pin: GithubShadowPolicy = serde_json::from_value(target["github_policy"].clone())?;
    let readiness: GithubMergeQueueReadiness = serde_json::from_value(target["readiness"].clone())?;
    let policy: github::Policy = load_shared_ci_policy(policy_path, &pin.policy_file_digest)?;
    let draft = bound_draft(target)?;
    let mut payload = target["work_order"].clone();
    payload
        .as_object_mut()
        .ok_or("the approved enqueue work order is missing")?
        .insert("repository".into(), serde_json::to_value(checkout)?);
    let order: WorkOrder = serde_json::from_value(payload)?;
    if !policy.require_merge_queue
        || !readiness.valid()
        || target["work_id"] != order.id
        || target["repository"] != policy.repository
        || target["base"] != order.base
        || target["base_branch"] != policy.base_branch
        || intent["id"] != intent_id
        || intent["state"] != "approved"
        || intent["head"] != draft.head
        || intent["pull_request"] != draft.reference
        || intent["pull_request_node_id"] != readiness.pull_request_node_id
    {
        return reject("the checkout and policy must match the approved enqueue target");
    }
    order.validate()?;
    repository::validate_base(&order)?;
    repository::validate_ci_anchor(&order)?;
    let read = github::shadow_merge_candidate(
        &order,
        &draft,
        true,
        &policy,
        gh,
        &pin.protection_digest,
        time::OffsetDateTime::now_utc().unix_timestamp(),
    )?;
    if read["shadow_eligible"] != true || read["evidence"]["merge_queue"] != target["readiness"] {
        return reject(format!(
            "GitHub enqueue preflight blocked for {intent_id}: {}",
            read["blocking_codes"]
        ));
    }
    if !intent["standing_policy_id"].is_null() {
        let measured = repository::github_enqueue_change_evidence(&order, &draft.head)?;
        if serde_json::to_value(measured)? != target["risk"] {
            return reject(
                "GitHub enqueue change history differs from the recorded policy evidence",
            );
        }
    }
    let token = validate_queue_token(read_queue_token_file(token_file, "GitHub publisher token")?)?;
    let began = queue.request(&json!({"action":"begin_github_enqueue", "intent_id":intent_id}))?;
    let authorization = &began["github_enqueue"]["intent"];
    if authorization["id"] != intent_id
        || authorization["state"] != "sending"
        || authorization["head"] != draft.head
        || authorization["pull_request"] != draft.reference
        || authorization["pull_request_node_id"] != readiness.pull_request_node_id
    {
        return reject("queue did not confirm authorization for the exact GitHub enqueue");
    }
    match github::enqueue_reviewed_head(gh, &token, intent_id, &readiness.pull_request_node_id, &draft.head) {
        Ok(entry) => queue.request(&resolution(
            "record_github_enqueue", intent_id, "queued", Some(&entry), "GitHub returned the exact merge queue entry",
        )).map_err(|_| format!("GitHub returned an entry for {intent_id} but the queue acknowledgement is uncertain. Reconcile before continuing.").into()),
        Err(_) => {
            let _ = queue.request(&resolution("record_github_enqueue", intent_id, "uncertain", None,
                "GitHub did not return a validated queue entry"));
            reject(format!("GitHub enqueue outcome is uncertain for {intent_id}. Reconcile before continuing."))
        }
    }
}
