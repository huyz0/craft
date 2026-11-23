use std::path::{Component, Path, PathBuf};

use crate::{
    Result, bitbucket_change, bitbucket_review, build_provenance, diagnosis, github_change,
    kubernetes_inventory, reject,
    release_trace::{Trace, read_evidence, trace_observations, valid_digest},
    sha256,
};
use clap::Args;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

const MAX_MANIFEST_BYTES: u64 = 32 * 1024;

#[derive(Args)]
pub struct Options {
    manifest: PathBuf,
    /// SHA-256 digest from the approving evidence channel.
    #[arg(long)]
    manifest_digest: String,
    /// Exit unsuccessfully when the trace contains a gap.
    #[arg(long)]
    pub require_complete: bool,
}

#[derive(Args)]
pub struct AssembleOptions {
    /// Kubernetes collector's sources.json.
    #[arg(long)]
    kubernetes: PathBuf,
    #[arg(long)]
    kubernetes_digest: String,
    /// Build collector's build-source.json.
    #[arg(long)]
    build: PathBuf,
    #[arg(long)]
    build_digest: String,
    /// Forge collector's change-source.json.
    #[arg(long)]
    change: PathBuf,
    #[arg(long)]
    change_digest: String,
    /// New manifest file in the directory containing all three archives.
    #[arg(long)]
    pub destination: PathBuf,
    /// Evaluation time in Unix seconds. Defaults to the latest evidence observation.
    #[arg(long)]
    as_of: Option<u64>,
}

#[derive(Args)]
pub struct DiagnosisOptions {
    /// Approved diagnostic policy with source_manifest as its release basis.
    policy: PathBuf,
    /// SHA-256 digest from the approving evidence channel.
    #[arg(long)]
    policy_digest: String,
    #[arg(long)]
    manifest: PathBuf,
    #[arg(long)]
    telemetry: PathBuf,
    #[arg(long)]
    from: u64,
    #[arg(long)]
    to: u64,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    schema_version: u32,
    as_of: u64,
    max_age_seconds: u64,
    kubernetes: Kubernetes,
    build: Option<BuildSource>,
    change: Option<ChangeSource>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Kubernetes {
    pod: SourceFile,
    replica_set: SourceFile,
    inventory: kubernetes_inventory::PodScope,
    deployment: kubernetes_inventory::ReplicaSetScope,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BuildSource {
    attestation: SourceFile,
    policy: SourceFile,
    observed_at: u64,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ChangeSource {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider: Option<ChangeProvider>,
    pull_request: SourceFile,
    approval: SourceFile,
    observed_at: u64,
    review: Option<ReviewSource>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum ChangeProvider {
    Bitbucket,
    Github,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReviewSource {
    policy: SourceFile,
    comment: SourceFile,
    #[serde(default)]
    activity: Vec<SourceFile>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourceFile {
    pub(crate) path: PathBuf,
    pub(crate) sha256: String,
}

#[derive(Serialize)]
pub struct Report {
    schema_version: u32,
    manifest_digest: String,
    pub trace: Trace,
}

pub fn run(options: Options) -> Result<Report> {
    load_trace(&options.manifest, &options.manifest_digest, u64::MAX)
}

pub fn assemble(options: &AssembleOptions) -> Result<(Vec<u8>, Report)> {
    let root = options
        .destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .canonicalize()?;
    let (mut manifest, kubernetes): (Manifest, _) = read_fragment(
        &root,
        &options.kubernetes,
        &options.kubernetes_digest,
        "Kubernetes source manifest",
    )?;
    if manifest.build.is_some() || manifest.change.is_some() {
        return reject("assembly requires the original Kubernetes-only source manifest");
    }
    let (mut build, build_dir): (BuildSource, _) =
        read_fragment(&root, &options.build, &options.build_digest, "build source")?;
    let (mut change, change_dir): (ChangeSource, _) = read_fragment(
        &root,
        &options.change,
        &options.change_digest,
        "change source",
    )?;
    for file in [
        &mut manifest.kubernetes.pod,
        &mut manifest.kubernetes.replica_set,
    ] {
        file.rebase(&kubernetes)?;
    }
    for file in [&mut build.attestation, &mut build.policy] {
        file.rebase(&build_dir)?;
    }
    for file in [&mut change.pull_request, &mut change.approval] {
        file.rebase(&change_dir)?;
    }
    if let Some(review) = &mut change.review {
        for file in [&mut review.policy, &mut review.comment]
            .into_iter()
            .chain(&mut review.activity)
        {
            file.rebase(&change_dir)?;
        }
    }
    manifest.as_of = options.as_of.unwrap_or(
        manifest
            .as_of
            .max(build.observed_at)
            .max(change.observed_at),
    );
    manifest.build = Some(build);
    manifest.change = Some(change);
    let bytes = serde_json::to_vec_pretty(&manifest)?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return reject("assembled release manifest exceeds the permitted size");
    }
    let digest = sha256(&bytes);
    let report = verify_manifest(manifest, &root, digest, u64::MAX)?;
    Ok((bytes, report))
}

fn read_fragment<T: DeserializeOwned>(
    root: &Path,
    path: &Path,
    digest: &str,
    label: &str,
) -> Result<(T, PathBuf)> {
    let path = path
        .canonicalize()
        .map_err(|error| format!("cannot resolve {label}: {error}"))?;
    let directory = path
        .parent()
        .and_then(|path| path.strip_prefix(root).ok())
        .ok_or_else(|| format!("{label} must be inside the destination archive"))?;
    let bytes = read_evidence(&path, MAX_MANIFEST_BYTES)
        .map_err(|error| format!("cannot read {label}: {error}"))?;
    if sha256(&bytes) != digest {
        return reject(format!("{label} does not match the collector digest"));
    }
    Ok((serde_json::from_slice(&bytes)?, directory.to_path_buf()))
}

pub fn diagnose(options: DiagnosisOptions) -> Result<diagnosis::Report> {
    diagnosis::ArchiveCase::load(
        &options.policy,
        &options.manifest,
        diagnosis::TelemetrySource::Normalized(&options.telemetry),
        &options.policy_digest,
    )?
    .report(options.from, options.to)
}

pub(crate) fn load_trace(
    path: &Path,
    expected_digest: &str,
    freshness_limit: u64,
) -> Result<Report> {
    load_trace_at(path, expected_digest, freshness_limit, None)
}

pub(crate) fn load_trace_at(
    path: &Path,
    expected_digest: &str,
    freshness_limit: u64,
    as_of: Option<u64>,
) -> Result<Report> {
    let manifest_path = path.canonicalize()?;
    let root = manifest_path
        .parent()
        .ok_or("manifest has no archive directory")?;
    let bytes =
        read_evidence(&manifest_path, MAX_MANIFEST_BYTES).map_err(|error| error.to_string())?;
    let manifest_digest = sha256(&bytes);
    if manifest_digest != expected_digest {
        return reject("release source manifest does not match the approved digest");
    }
    let mut manifest: Manifest = serde_json::from_slice(&bytes)?;
    if let Some(cutoff) = as_of {
        if manifest.as_of == 0 || manifest.as_of > cutoff {
            return reject("release source manifest is newer than the rollout observation cutoff");
        }
        manifest.as_of = cutoff;
    }
    verify_manifest(manifest, root, manifest_digest, freshness_limit)
}

fn verify_manifest(
    manifest: Manifest,
    root: &Path,
    manifest_digest: String,
    freshness_limit: u64,
) -> Result<Report> {
    if manifest.schema_version != 1 || manifest.as_of == 0 || manifest.max_age_seconds == 0 {
        return reject("invalid source manifest version or trace time window");
    }
    let kube = manifest.kubernetes;
    let pod = kube.pod.read(
        root,
        "Kubernetes Pod",
        kubernetes_inventory::MAX_SNAPSHOT_BYTES,
    )?;
    let replica_set = kube.replica_set.read(
        root,
        "Kubernetes ReplicaSet",
        kubernetes_inventory::MAX_SNAPSHOT_BYTES,
    )?;
    let (inventory, deployment) = kubernetes_inventory::collect_deployment(
        &kube.inventory,
        &kube.deployment,
        &pod,
        &replica_set,
    )?;

    let build = if let Some(source) = manifest.build {
        let attestation = source.attestation.read(
            root,
            "build attestation",
            build_provenance::MAX_ATTESTATION_BYTES,
        )?;
        let policy =
            source
                .policy
                .read(root, "build policy", build_provenance::MAX_POLICY_BYTES)?;
        Some(build_provenance::collect(
            &attestation,
            &policy,
            &source.policy.sha256,
            &inventory.running.artifact_digest,
            source.observed_at,
        )?)
    } else {
        None
    };
    let change = if let Some(source) = manifest.change {
        let github = matches!(source.provider, Some(ChangeProvider::Github));
        let (snapshot_limit, approval_limit) = if github {
            (
                github_change::MAX_PULL_REQUEST_BYTES,
                github_change::MAX_APPROVAL_BYTES,
            )
        } else {
            (
                bitbucket_change::MAX_PULL_REQUEST_BYTES,
                bitbucket_change::MAX_APPROVAL_BYTES,
            )
        };
        let snapshot = source
            .pull_request
            .read(root, "pull request", snapshot_limit)?;
        let approval = source
            .approval
            .read(root, "change approval", approval_limit)?;
        let change = if github {
            if source.review.is_some() {
                return reject("a GitHub change archive cannot include a Bitbucket review section");
            }
            github_change::collect(
                &snapshot,
                &approval,
                &source.approval.sha256,
                source.observed_at,
            )?
        } else if let Some(review) = source.review {
            let policy = review.policy.read(
                root,
                "Bitbucket review policy",
                bitbucket_review::MAX_POLICY_BYTES,
            )?;
            let comment = review.comment.read(
                root,
                "Bitbucket review comment",
                bitbucket_review::MAX_COMMENT_BYTES,
            )?;
            let policy = bitbucket_review::Policy::parse(&policy, &review.policy.sha256)?;
            if review.activity.len() > bitbucket_review::MAX_ACTIVITY_PAGES {
                return reject("archived Bitbucket activity exceeds the permitted page limit");
            }
            let activity = review
                .activity
                .iter()
                .map(|page| {
                    page.read(
                        root,
                        "Bitbucket merge activity",
                        bitbucket_review::MAX_ACTIVITY_BYTES,
                    )
                })
                .collect::<Result<Vec<_>>>()?;
            let (derived, change) =
                policy.verify(&snapshot, &comment, &activity, source.observed_at)?;
            if derived != approval {
                return reject(
                    "archived Bitbucket approval differs from the original review evidence",
                );
            }
            change
        } else {
            let change = bitbucket_change::collect(
                &snapshot,
                &approval,
                &source.approval.sha256,
                source.observed_at,
            )?;
            if change
                .approval_reference
                .starts_with(bitbucket_review::REVIEW_REFERENCE)
            {
                return reject("Bitbucket native review requires its original comment and policy");
            }
            change
        };
        Some(change)
    } else {
        None
    };
    let trace = trace_observations(
        inventory,
        deployment,
        build,
        change,
        manifest.as_of,
        manifest.max_age_seconds.min(freshness_limit),
    )
    .map_err(|error| error.to_string())?;
    Ok(Report {
        schema_version: 1,
        manifest_digest,
        trace,
    })
}

impl SourceFile {
    pub(crate) fn pin(root: &Path, path: &Path, sha256: String) -> Result<Self> {
        let path = path.canonicalize()?;
        let path = path
            .strip_prefix(root)
            .map_err(|_| "source must be inside the archive")?;
        let source = Self {
            path: portable_archive_path(path)?,
            sha256,
        };
        source.validate("rollout source manifest")?;
        Ok(source)
    }

    fn validate(&self, label: &str) -> Result<()> {
        if !valid_digest(&self.sha256)
            || self.path.as_os_str().is_empty()
            || self
                .path
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return reject(format!(
                "{label} needs a SHA-256 pin and a path inside the archive"
            ));
        }
        Ok(())
    }

    fn rebase(&mut self, directory: &Path) -> Result<()> {
        self.validate("source file")?;
        self.path = portable_archive_path(&directory.join(&self.path))?;
        Ok(())
    }

    pub(crate) fn resolved_path(&self, root: &Path, label: &str) -> Result<PathBuf> {
        self.validate(label)?;
        let path = root
            .join(&self.path)
            .canonicalize()
            .map_err(|error| format!("cannot resolve {label} in the archive: {error}"))?;
        if !path.starts_with(root) {
            return reject(format!("{label} resolves outside the release archive"));
        }
        Ok(path)
    }

    fn read(&self, root: &Path, label: &str, limit: u64) -> Result<Vec<u8>> {
        let path = self.resolved_path(root, label)?;
        let bytes =
            read_evidence(&path, limit).map_err(|error| format!("cannot read {label}: {error}"))?;
        if sha256(&bytes) != self.sha256 {
            return reject(format!("{label} does not match its pinned digest"));
        }
        Ok(bytes)
    }
}

pub(crate) fn portable_archive_path(path: &Path) -> Result<PathBuf> {
    let names = path
        .components()
        .map(|component| match component {
            Component::Normal(name) => name.to_str().filter(|name| !name.contains(['\\', ':'])),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()
        .filter(|names| !names.is_empty())
        .ok_or("archive paths must be portable between Windows and Linux")?;
    Ok(PathBuf::from(names.join("/")))
}
