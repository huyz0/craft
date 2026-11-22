use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    path::{Path, PathBuf},
};

use clap::Args;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    Result, reject,
    release_sources::{SourceFile, load_trace_at, portable_archive_path},
    release_trace::{
        self, Change, MAX_BUNDLE_BYTES, Target, Trace, present, read_evidence, valid_digest,
    },
    sha256, valid_oid,
};

#[derive(Args)]
pub struct Options {
    /// Approved change and required rollout targets.
    policy: PathBuf,
    /// Release observations to evaluate.
    #[arg(long)]
    release: PathBuf,
    /// Observation cutoff as UTC Unix seconds.
    #[arg(long)]
    as_of: u64,
    /// Exit with failure after writing the report if any required rollout remains open.
    #[arg(long)]
    pub require_complete: bool,
}

#[derive(Args)]
pub struct SourceOptions {
    /// Approved change and required rollout targets.
    policy: PathBuf,
    /// SHA-256 digest of the approved remediation policy.
    #[arg(long)]
    policy_digest: String,
    /// Index of pinned release source manifests.
    #[arg(long)]
    sources: PathBuf,
    /// SHA-256 digest of the approved release source index.
    #[arg(long)]
    sources_digest: String,
    /// Common observation cutoff as UTC Unix seconds.
    #[arg(long)]
    as_of: u64,
    /// Exit with failure after writing the report if any required rollout remains open.
    #[arg(long)]
    pub require_complete: bool,
}

#[derive(Args)]
pub struct PrepareArchiveOptions {
    /// Approved change and required rollout targets within the archive directory.
    policy: PathBuf,
    #[arg(long)]
    policy_digest: String,
    #[arg(long)]
    work_id: String,
    /// Reviewed PR head from the shared queue, before merge.
    #[arg(long)]
    reviewed_head: String,
    /// Assembled manifest and its pinned digest. Repeat for each observed target.
    #[arg(long = "release", num_args = 2, value_names = ["MANIFEST", "SHA256"], required = true)]
    releases: Vec<OsString>,
    /// Observation cutoff as UTC Unix seconds.
    #[arg(long)]
    as_of: u64,
    /// New index file. Place it alongside the operator's rollout catalog.
    #[arg(long)]
    pub destination: PathBuf,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveEntry {
    pub work_id: String,
    pub policy: PathBuf,
    pub policy_digest: String,
    pub sources: PathBuf,
    pub sources_digest: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceIndex {
    schema_version: u32,
    releases: Vec<SourceFile>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    schema_version: u32,
    change: ExpectedChange,
    artifact_digest: String,
    release_max_age_seconds: u64,
    required_targets: Vec<Requirement>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExpectedChange {
    repository: String,
    source_commit: String,
    pull_request: String,
    work_item: String,
    intent_reference: String,
    intent_revision: String,
    approval_reference: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Requirement {
    target: Target,
    config_revision: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Verified,
    EvidenceGap,
    ArtifactPending,
    ConfigurationPending,
    ChangeMismatch,
}

#[derive(Serialize)]
struct TargetResult {
    expected_config_revision: String,
    status: Status,
    release: Trace,
}

#[derive(Serialize)]
pub struct Report {
    schema_version: u32,
    policy_digest: String,
    evidence_digest: String,
    as_of: u64,
    expected_change: ExpectedChange,
    expected_artifact_digest: String,
    pub rollout_verified: bool,
    verified_targets: usize,
    remaining_targets: usize,
    targets: Vec<TargetResult>,
}

pub struct VerifiedRollout<'a> {
    pub repository: &'a str,
    pub source_commit: &'a str,
    pub pull_request: &'a str,
    pub work_item: &'a str,
    pub artifact_digest: &'a str,
    pub policy_digest: &'a str,
    pub evidence_digest: &'a str,
    pub fresh_through: u64,
}

impl Report {
    pub fn pending_targets(&self) -> Vec<serde_json::Value> {
        self.targets
            .iter()
            .filter(|result| result.status != Status::Verified)
            .map(|result| {
                serde_json::json!({
                    "target": result.release.target(),
                    "status": result.status,
                })
            })
            .collect()
    }

    pub fn binds_reviewed_head(&self, head: &str) -> bool {
        self.rollout_verified
            && self.targets.iter().all(|result| {
                result
                    .release
                    .verified_change()
                    .and_then(|change| change.approved_source_commit.as_deref())
                    == Some(head)
            })
    }

    pub fn proof_for(&self, target: &Target) -> Option<VerifiedRollout<'_>> {
        if !self.rollout_verified
            || !self
                .targets
                .iter()
                .any(|result| result.release.target() == target)
        {
            return None;
        }
        // Every target in a verified report has a fresh deployment binding.
        let fresh_through = self
            .targets
            .iter()
            .filter_map(|result| result.release.fresh_through())
            .min()?;
        Some(VerifiedRollout {
            repository: &self.expected_change.repository,
            source_commit: &self.expected_change.source_commit,
            pull_request: &self.expected_change.pull_request,
            work_item: &self.expected_change.work_item,
            artifact_digest: &self.expected_artifact_digest,
            policy_digest: &self.policy_digest,
            evidence_digest: &self.evidence_digest,
            fresh_through,
        })
    }
}

impl ExpectedChange {
    fn valid(&self) -> bool {
        valid_oid(&self.source_commit)
            && [
                &self.repository,
                &self.pull_request,
                &self.work_item,
                &self.intent_reference,
                &self.intent_revision,
                &self.approval_reference,
            ]
            .iter()
            .all(|value| present(value))
    }

    fn matches(&self, change: &Change) -> bool {
        self.repository == change.repository
            && self.source_commit == change.result_commit
            && self.pull_request == change.pull_request
            && self.work_item == change.work_item
            && self.intent_reference == change.intent_reference
            && self.intent_revision == change.intent_revision
            && self.approval_reference == change.approval_reference
    }
}

impl Policy {
    fn validate(&self, as_of: u64) -> Result<()> {
        if as_of == 0
            || self.schema_version != 1
            || !self.change.valid()
            || !valid_digest(&self.artifact_digest)
            || !(1..=3600).contains(&self.release_max_age_seconds)
            || !(1..=32).contains(&self.required_targets.len())
        {
            return reject("invalid remediation policy");
        }
        let mut targets = HashSet::new();
        if self.required_targets.iter().any(|item| {
            !item.target.valid() || !present(&item.config_revision) || !targets.insert(&item.target)
        }) {
            return reject(
                "remediation requires unique, exact targets with configuration revisions",
            );
        }
        Ok(())
    }
}

fn evaluate(release: &Trace, artifact: &str, config: &str, expected: &ExpectedChange) -> Status {
    let Some((version, _)) = release.verified_deployment() else {
        return Status::EvidenceGap;
    };
    if version.artifact_digest != artifact {
        return Status::ArtifactPending;
    }
    if version.config_revision != config {
        return Status::ConfigurationPending;
    }
    match release.verified_change() {
        Some(change) if expected.matches(change) => Status::Verified,
        Some(_) => Status::ChangeMismatch,
        None => Status::EvidenceGap,
    }
}

pub fn run(options: Options) -> Result<Report> {
    let policy_bytes = read_evidence(&options.policy, 64 * 1024)?;
    let evidence = read_evidence(&options.release, MAX_BUNDLE_BYTES)?;
    evaluate_bytes(&policy_bytes, &evidence, options.as_of)
}

pub fn evaluate_bytes(policy_bytes: &[u8], evidence: &[u8], as_of: u64) -> Result<Report> {
    let policy: Policy = serde_json::from_slice(policy_bytes)?;
    policy.validate(as_of)?;
    let bundle = release_trace::parse(evidence)?;
    evaluate_targets(
        policy,
        sha256(policy_bytes),
        sha256(evidence),
        as_of,
        |target, max_age| release_trace::trace_bundle(&bundle, target, as_of, max_age),
    )
}

pub fn run_sources(options: SourceOptions) -> Result<Report> {
    let policy_bytes = read_evidence(&options.policy, 64 * 1024)?;
    evaluate_sources(
        &policy_bytes,
        &options.policy_digest,
        &options.sources,
        &options.sources_digest,
        options.as_of,
    )
}

pub fn prepare_archive(options: &PrepareArchiveOptions) -> Result<(Vec<u8>, ArchiveEntry, Report)> {
    if !(1..=100).contains(&options.work_id.len())
        || !options
            .work_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        || !valid_oid(&options.reviewed_head)
        || options.releases.len() > 64
    {
        return reject("invalid work ID, reviewed head, or release count");
    }
    let root = options
        .destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .canonicalize()?;
    let index_name = portable_archive_path(Path::new(
        options
            .destination
            .file_name()
            .ok_or("rollout index destination must have a file name")?,
    ))?;
    let policy = SourceFile::pin(&root, &options.policy, options.policy_digest.clone())?;
    let policy_file = policy.resolved_path(&root, "remediation policy")?;
    let policy_bytes = read_evidence(&policy_file, 64 * 1024)?;
    let releases = options
        .releases
        .chunks_exact(2)
        .map(|pair| {
            let digest = pair[1].to_str().ok_or("invalid release source digest")?;
            SourceFile::pin(&root, Path::new(&pair[0]), digest.to_owned())
        })
        .collect::<Result<_>>()?;
    let index_bytes = serde_json::to_vec_pretty(&SourceIndex {
        schema_version: 1,
        releases,
    })?;
    if index_bytes.len() > 32 * 1024 {
        return reject("rollout index exceeds the permitted byte limit");
    }
    let entry = ArchiveEntry {
        work_id: options.work_id.clone(),
        policy: policy.path,
        policy_digest: options.policy_digest.clone(),
        sources: index_name,
        sources_digest: sha256(&index_bytes),
    };
    let report = evaluate_index(
        &policy_bytes,
        &entry.policy_digest,
        &root,
        &index_bytes,
        &entry.sources_digest,
        options.as_of,
    )?;
    if report.rollout_verified && !report.binds_reviewed_head(&options.reviewed_head) {
        return reject("rollout does not identify the reviewed candidate head");
    }
    Ok((index_bytes, entry, report))
}

pub fn evaluate_sources(
    policy_bytes: &[u8],
    policy_digest: &str,
    sources: &Path,
    sources_digest: &str,
    as_of: u64,
) -> Result<Report> {
    let index = sources.canonicalize()?;
    let root = index
        .parent()
        .ok_or("remediation source index has no archive directory")?;
    let bytes = read_evidence(&index, 32 * 1024)?;
    evaluate_index(
        policy_bytes,
        policy_digest,
        root,
        &bytes,
        sources_digest,
        as_of,
    )
}

fn evaluate_index(
    policy_bytes: &[u8],
    policy_digest: &str,
    root: &Path,
    bytes: &[u8],
    sources_digest: &str,
    as_of: u64,
) -> Result<Report> {
    if sha256(policy_bytes) != policy_digest {
        return reject("remediation policy does not match the approved digest");
    }
    if sha256(bytes) != sources_digest {
        return reject("remediation sources do not match the approved digest");
    }
    let policy: Policy = serde_json::from_slice(policy_bytes)?;
    policy.validate(as_of)?;
    let index: SourceIndex = serde_json::from_slice(bytes)?;
    if index.schema_version != 1 || index.releases.len() > policy.required_targets.len() {
        return reject("invalid remediation source index or too many releases");
    }
    let mut traces = HashMap::new();
    for release in index.releases {
        let path = release.resolved_path(root, "rollout source manifest")?;
        let trace = load_trace_at(
            &path,
            &release.sha256,
            policy.release_max_age_seconds,
            Some(as_of),
        )?
        .trace;
        let target = trace.target().clone();
        if !policy
            .required_targets
            .iter()
            .any(|required| required.target == target)
        {
            return reject("rollout source manifest is outside the required targets");
        }
        if traces.insert(target, trace).is_some() {
            return reject("multiple rollout source manifests identify the same target");
        }
    }
    evaluate_targets(
        policy,
        policy_digest.to_owned(),
        sources_digest.to_owned(),
        as_of,
        |target, max_age| {
            Ok(traces
                .remove(&target)
                .unwrap_or_else(|| Trace::without_inventory(target, as_of, max_age)))
        },
    )
}

fn evaluate_targets(
    policy: Policy,
    policy_digest: String,
    evidence_digest: String,
    as_of: u64,
    mut trace_for: impl FnMut(Target, u64) -> Result<Trace>,
) -> Result<Report> {
    let mut targets = Vec::with_capacity(policy.required_targets.len());
    let mut verified_targets = 0;
    for requirement in policy.required_targets {
        let release = trace_for(requirement.target, policy.release_max_age_seconds)?;
        let status = evaluate(
            &release,
            &policy.artifact_digest,
            &requirement.config_revision,
            &policy.change,
        );
        if status == Status::Verified {
            verified_targets += 1;
        }
        targets.push(TargetResult {
            status,
            expected_config_revision: requirement.config_revision,
            release,
        });
    }
    let remaining_targets = targets.len() - verified_targets;
    Ok(Report {
        schema_version: 1,
        policy_digest,
        evidence_digest,
        as_of,
        expected_change: policy.change,
        expected_artifact_digest: policy.artifact_digest,
        rollout_verified: remaining_targets == 0,
        verified_targets,
        remaining_targets,
        targets,
    })
}
