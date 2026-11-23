use std::path::PathBuf;

use clap::Args;
use craft_factory_evidence::{
    github_change::{MAX_APPROVAL_BYTES, approved_endpoint, collect},
    release_trace::read_evidence,
    sha256,
};
use serde_json::{Value, json};

use crate::{
    Result,
    evidence_capture::{create_directory, now, require_new_directory, write_new},
    github,
};

#[derive(Args)]
pub struct Options {
    /// Owner-approved change record bound to the reviewed head and merge result.
    approval: PathBuf,
    #[arg(long)]
    approval_digest: String,
    #[arg(long)]
    gh: PathBuf,
    /// New directory for the original evidence and manifest change section.
    #[arg(long)]
    destination: PathBuf,
}

pub fn run(options: Options) -> Result<Value> {
    require_new_directory(&options.destination)?;
    github::validate_gh(&options.gh)?;
    let approval =
        read_evidence(&options.approval, MAX_APPROVAL_BYTES).map_err(|error| error.to_string())?;
    let endpoint = approved_endpoint(&approval, &options.approval_digest)
        .map_err(|error| error.to_string())?;
    let snapshot = github::read_bytes(&options.gh, &endpoint, &[])?;
    let observed_at = now()?;
    let change = collect(&snapshot, &approval, &options.approval_digest, observed_at)
        .map_err(|error| error.to_string())?;
    let source = serde_json::to_vec_pretty(&json!({
        "provider": "github",
        "pull_request": {"path": "pull-request.json", "sha256": sha256(&snapshot)},
        "approval": {"path": "approval.json", "sha256": options.approval_digest},
        "observed_at": observed_at,
    }))?;
    create_directory(&options.destination)?;
    write_new(&options.destination.join("pull-request.json"), &snapshot)?;
    write_new(&options.destination.join("approval.json"), &approval)?;
    let source_path = options.destination.join("change-source.json");
    write_new(&source_path, &source)?;
    Ok(json!({
        "schema_version": 1, "source": source_path, "source_digest": sha256(&source),
        "pull_request": change.pull_request, "result_commit": change.result_commit,
        "observed_at": observed_at,
    }))
}
