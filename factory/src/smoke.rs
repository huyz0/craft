use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde_json::{Value, json};

use crate::order::{PinnedReference, WorkOrder};
use crate::review::{Draft, ReviewRecord};
use crate::store::{Attestation, State, Store};
use crate::{Result, reject, repository};

const ORDER: &str = "DEMO-1";
const PR: &str = "https://forge.example.invalid/synthetic/repo/pull/1";

pub fn run(destination: PathBuf) -> Result<Value> {
    if !destination.is_absolute() {
        return reject("smoke destination must be an absolute path to a new directory");
    }
    std::fs::create_dir(&destination)?;
    let root = destination;
    let repo = root.join("repo");
    std::fs::create_dir(&repo)?;
    let git = |args: &[&str]| -> Result<String> {
        let output = repository::isolated_git_command()
            .current_dir(&repo)
            .args(["-c", "core.hooksPath=.git/no-hooks"])
            .args(args)
            .stdin(Stdio::null())
            .output()?;
        if !output.status.success() {
            return reject(format!(
                "local Git command failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(String::from_utf8(output.stdout)?.trim().into())
    };
    git(&["init", "--quiet", "--template=", "-b", "main"])?;
    std::fs::write(repo.join("greeting.txt"), "hello\n")?;
    git(&["add", "greeting.txt"])?;
    git(&[
        "-c",
        "user.name=Factory smoke",
        "-c",
        "user.email=factory@example.invalid",
        "commit",
        "--quiet",
        "-m",
        "Synthetic baseline",
    ])?;
    let base = git(&["rev-parse", "HEAD"])?;
    let order = WorkOrder {
        schema_version: 1,
        id: ORDER.into(),
        issue: ORDER.into(),
        owner: "Synthetic pilot operator".into(),
        repository: repo,
        base: base.clone(),
        intent: "Update a disposable synthetic greeting".into(),
        acceptance: vec!["The greeting includes the word factory".into()],
        scope: vec!["greeting.txt".into()],
        context: vec![PinnedReference {
            location: "fixture:DEMO-1".into(),
            revision: "1".into(),
        }],
        skill: PinnedReference {
            location: "fixture:offline".into(),
            revision: "1".into(),
        },
        require_native_checks: false,
        dependency_intake_digest: None,
        ci_failure: None,
        ci_policy_digest: None,
        ci_anchor: None,
        execution_profile: None,
        agent_adapter: Default::default(),
    };
    order.validate()?;
    repository::validate_base(&order)?;
    let database = root.join("factory.db");
    let mut store = Store::open(&database)?;
    store.delegate(order.clone(), None)?;
    let first = edit(&order, &root.join("initial"), &base, "hello factory\n")?;
    store.submit(ORDER, &first, None, "Synthetic initial candidate")?;
    let first_draft = draft(&first);
    store.hold_publication(ORDER, &first, "Simulated missing publication response")?;
    drop(store);
    let mut store = Store::open(&database)?;
    if store.status(ORDER)?.publication_hold.is_none() {
        return reject("the offline smoke did not recover the publication hold");
    }
    store.bind_draft(ORDER, first_draft.clone())?;
    store.attest(
        ORDER,
        Attestation::Verification,
        record(first_draft.clone(), "fixture:initial-check"),
    )?;
    store.attest(
        ORDER,
        Attestation::ChangesRequested,
        record(first_draft.clone(), "fixture:human-feedback"),
    )?;
    let repair = edit(&order, &root.join("repair"), &first, "Hello, factory!\n")?;
    store.submit(ORDER, &repair, Some(&first), "Synthetic repair candidate")?;
    let repair_draft = draft(&repair);
    store.bind_draft(ORDER, repair_draft.clone())?;
    if store
        .attest(
            ORDER,
            Attestation::Close,
            record(first_draft, "fixture:stale-close"),
        )
        .is_ok()
    {
        return reject("the offline smoke accepted a stale candidate");
    }
    store.attest(
        ORDER,
        Attestation::Verification,
        record(repair_draft.clone(), "fixture:repair-check"),
    )?;
    let receipt = store.attest(
        ORDER,
        Attestation::Close,
        record(repair_draft, "fixture:closed"),
    )?;
    if receipt.state != State::Closed || receipt.candidate.as_deref() != Some(&repair) {
        return reject("the offline smoke did not close on the repaired head");
    }
    Ok(json!({
        "schema_version": 1, "simulated": true, "passed": true,
        "workspace": root, "state": receipt.state,
        "initial_candidate": first, "repaired_candidate": repair,
        "checks": ["publication hold recovered after restart", "stale result rejected", "repair closed on the same draft"]
    }))
}

fn edit(order: &WorkOrder, destination: &Path, from: &str, text: &str) -> Result<String> {
    let checkout = repository::prepare_checkout(order, destination, from)?;
    std::fs::write(checkout.join("greeting.txt"), text)?;
    repository::commit_worker_changes(order, &checkout, from)?;
    repository::import_candidate(order, &checkout, from)
}

fn draft(head: &str) -> Draft {
    Draft {
        reference: PR.into(),
        head: head.into(),
    }
}

fn record(draft: Draft, evidence: &str) -> ReviewRecord {
    ReviewRecord {
        schema_version: 1,
        draft,
        source: crate::review::EvidenceSource::Simulated,
        actor: "Synthetic reviewer".into(),
        evidence: evidence.into(),
        note: "Simulated smoke evidence".into(),
    }
}
