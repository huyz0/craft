use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

use clap::Args;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Result, reject, valid_oid};

pub(crate) const MAX_BUNDLE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Args)]
pub struct Options {
    /// Evidence exported from the inventory, deployment, build, and change systems.
    bundle: PathBuf,
    #[arg(long)]
    service: String,
    #[arg(long)]
    environment: String,
    #[arg(long)]
    region: String,
    #[arg(long)]
    resource: String,
    /// Incident or observation time as UTC Unix seconds.
    #[arg(long)]
    as_of: u64,
    /// Maximum age of runtime and deployment observations in seconds.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    max_age_seconds: u64,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Bundle {
    schema_version: u32,
    inventory: Vec<Inventory>,
    deployments: Vec<Deployment>,
    builds: Vec<Build>,
    changes: Vec<Change>,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq, Hash, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub service: String,
    pub environment: String,
    pub region: String,
    pub resource: String,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Version {
    pub deployment_id: String,
    pub config_revision: String,
    pub artifact_digest: String,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub source: String,
    pub reference: String,
    pub observed_at: u64,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Inventory {
    pub target: Target,
    pub owner: String,
    pub running: Version,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running_since: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) kubernetes_container: Option<KubernetesContainer>,
    pub observation: Observation,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct KubernetesContainer {
    pub pod_uid: String,
    pub container_name: String,
    pub container_id: String,
}

impl KubernetesContainer {
    pub(crate) fn valid(&self) -> bool {
        [&self.pod_uid, &self.container_name, &self.container_id]
            .iter()
            .all(|value| kubernetes_identifier(value))
    }
}

pub(crate) fn kubernetes_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_'))
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Deployment {
    pub target: Target,
    pub version: Version,
    pub repository: String,
    pub deployed_at: u64,
    pub observation: Observation,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Build {
    pub artifact_digest: String,
    pub repository: String,
    pub source_commit: String,
    pub built_at: u64,
    pub observation: Observation,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub repository: String,
    pub result_commit: String,
    pub pull_request: String,
    pub work_item: String,
    pub intent_reference: String,
    pub intent_revision: String,
    pub approval_reference: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_source_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_reference: Option<String>,
    pub merged_at: u64,
    pub observation: Observation,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "snake_case")]
enum Reason {
    Missing,
    Ambiguous,
    Stale,
    Mismatch,
    InconsistentTime,
}

#[derive(Clone, Serialize)]
struct Gap {
    stage: &'static str,
    reason: Reason,
}

#[derive(Clone, Serialize)]
pub struct Trace {
    schema_version: u32,
    target: Target,
    as_of: u64,
    max_age_seconds: u64,
    inventory: Option<Inventory>,
    deployment: Option<Deployment>,
    build: Option<Build>,
    change: Option<Change>,
    gap: Option<Gap>,
}

impl Trace {
    pub(crate) fn without_inventory(target: Target, as_of: u64, max_age_seconds: u64) -> Self {
        Self::empty(target, as_of, max_age_seconds).stop("inventory", Reason::Missing)
    }

    fn empty(target: Target, as_of: u64, max_age_seconds: u64) -> Self {
        Self {
            schema_version: 1,
            target,
            as_of,
            max_age_seconds,
            inventory: None,
            deployment: None,
            build: None,
            change: None,
            gap: None,
        }
    }

    pub fn is_complete(&self) -> bool {
        self.gap.is_none()
    }

    pub fn deployment_verified(&self) -> bool {
        self.verified_deployment().is_some()
    }

    pub(crate) fn fits_diagnostic_scope(&self, target: &Target, as_of: u64, max_age: u64) -> bool {
        &self.target == target && self.as_of == as_of && self.max_age_seconds <= max_age
    }

    pub(crate) fn as_of(&self) -> u64 {
        self.as_of
    }

    pub(crate) fn observed_runtime_start(&self) -> Option<u64> {
        self.inventory.as_ref()?.running_since
    }

    pub(crate) fn observed_kubernetes_container(&self) -> Option<&KubernetesContainer> {
        self.inventory.as_ref()?.kubernetes_container.as_ref()
    }
    fn stop(mut self, stage: &'static str, reason: Reason) -> Self {
        self.gap = Some(Gap { stage, reason });
        self
    }

    pub(crate) fn verified_deployment(&self) -> Option<(&Version, u64)> {
        if self
            .gap
            .as_ref()
            .is_some_and(|gap| matches!(gap.stage, "inventory" | "deployment"))
        {
            return None;
        }
        self.deployment
            .as_ref()
            .map(|deployment| (&deployment.version, deployment.deployed_at))
    }

    pub(crate) fn verified_change(&self) -> Option<&Change> {
        if self.gap.is_some() {
            return None;
        }
        self.change.as_ref()
    }

    pub(crate) fn target(&self) -> &Target {
        &self.target
    }

    pub(crate) fn fresh_through(&self) -> Option<u64> {
        Some(
            self.inventory
                .as_ref()?
                .observation
                .observed_at
                .min(self.deployment.as_ref()?.observation.observed_at)
                .saturating_add(self.max_age_seconds),
        )
    }
}

pub fn trace_observations(
    inventory: Inventory,
    deployment: Deployment,
    build: Option<Build>,
    change: Option<Change>,
    as_of: u64,
    max_age_seconds: u64,
) -> Result<Trace> {
    let target = inventory.target.clone();
    let bundle = Bundle {
        schema_version: 1,
        inventory: vec![inventory],
        deployments: vec![deployment],
        builds: build.into_iter().collect(),
        changes: change.into_iter().collect(),
    };
    bundle.validate()?;
    trace_bundle(&bundle, target, as_of, max_age_seconds)
}

pub(crate) fn present(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 1000 && !value.chars().any(char::is_control)
}

impl Target {
    pub(crate) fn valid(&self) -> bool {
        [
            &self.service,
            &self.environment,
            &self.region,
            &self.resource,
        ]
        .iter()
        .all(|value| present(value))
    }
}

impl Version {
    pub(crate) fn valid(&self) -> bool {
        present(&self.deployment_id)
            && present(&self.config_revision)
            && valid_digest(&self.artifact_digest)
    }
}

impl Observation {
    pub(crate) fn valid(&self) -> bool {
        present(&self.source) && present(&self.reference) && self.observed_at > 0
    }
}

pub fn valid_digest(digest: &str) -> bool {
    digest
        .strip_prefix("sha256:")
        .is_some_and(|hash| hash.len() == 64 && valid_oid(hash))
}

impl Bundle {
    pub fn with_inventory(path: &Path, observation: Inventory) -> Result<Self> {
        let mut bundle = parse(&read_evidence(path, MAX_BUNDLE_BYTES)?)?;
        bundle.add_inventory(observation)?;
        bundle.finish_import()
    }

    pub fn with_inventory_and_deployment(
        path: &Path,
        inventory: Inventory,
        deployment: Deployment,
    ) -> Result<Self> {
        let mut bundle = parse(&read_evidence(path, MAX_BUNDLE_BYTES)?)?;
        bundle.add_inventory(inventory)?;
        let mut matching = bundle.deployments.iter().filter(|previous| {
            previous.target == deployment.target
                && previous.observation.observed_at == deployment.observation.observed_at
        });
        if let Some(previous) = matching.next() {
            if previous != &deployment || matching.next().is_some() {
                return reject(
                    "release bundle already has conflicting deployment evidence for that target and time",
                );
            }
        } else {
            bundle.deployments.push(deployment);
        }
        bundle.finish_import()
    }

    fn add_inventory(&mut self, observation: Inventory) -> Result<()> {
        let mut matching = self.inventory.iter_mut().filter(|previous| {
            previous.target == observation.target
                && previous.observation.observed_at == observation.observation.observed_at
        });
        if let Some(previous) = matching.next() {
            let mut enriched = previous.clone();
            if enriched.kubernetes_container.is_none() {
                enriched.kubernetes_container = observation.kubernetes_container.clone();
            }
            if enriched != observation || matching.next().is_some() {
                return reject(
                    "release bundle already has conflicting inventory for that target and time",
                );
            }
            *previous = enriched;
        } else {
            self.inventory.push(observation);
        }
        Ok(())
    }

    pub fn with_build(path: &Path, build: Build) -> Result<Self> {
        let mut bundle = parse(&read_evidence(path, MAX_BUNDLE_BYTES)?)?;
        let mut matching = bundle.builds.iter().filter(|previous| {
            previous.repository == build.repository
                && previous.artifact_digest == build.artifact_digest
        });
        if let Some(previous) = matching.next() {
            if previous != &build || matching.next().is_some() {
                return reject(
                    "release bundle already has conflicting build evidence for that repository and artifact",
                );
            }
        } else {
            bundle.builds.push(build);
        }
        bundle.finish_import()
    }

    pub fn with_change(path: &Path, change: Change) -> Result<Self> {
        let mut bundle = parse(&read_evidence(path, MAX_BUNDLE_BYTES)?)?;
        let mut matching = bundle.changes.iter().filter(|previous| {
            previous.repository == change.repository
                && (previous.result_commit == change.result_commit
                    || previous.pull_request == change.pull_request)
        });
        if let Some(previous) = matching.next() {
            if previous != &change || matching.next().is_some() {
                return reject(
                    "release bundle already has conflicting evidence for that pull request or result commit",
                );
            }
        } else {
            bundle.changes.push(change);
        }
        bundle.finish_import()
    }

    fn finish_import(self) -> Result<Self> {
        self.validate()?;
        if serde_json::to_vec_pretty(&self)?.len() as u64 >= MAX_BUNDLE_BYTES {
            return reject("enriched release bundle exceeds the permitted byte limit");
        }
        Ok(self)
    }

    fn validate(&self) -> Result<()> {
        if self.schema_version != 1 {
            return reject("unsupported release evidence version");
        }
        if !self.inventory.iter().all(|item| {
            item.target.valid()
                && present(&item.owner)
                && item.running.valid()
                && item
                    .running_since
                    .is_none_or(|time| time > 0 && time <= item.observation.observed_at)
                && item
                    .kubernetes_container
                    .as_ref()
                    .is_none_or(KubernetesContainer::valid)
                && item.observation.valid()
        }) {
            return reject(
                "inventory evidence requires an exact target, owner, running version, and observation",
            );
        }
        if !self.deployments.iter().all(|item| {
            item.target.valid()
                && item.version.valid()
                && present(&item.repository)
                && item.deployed_at > 0
                && item.observation.valid()
        }) {
            return reject(
                "deployment evidence requires an exact target, deployed version, repository, and observation",
            );
        }
        if !self.builds.iter().all(|item| {
            valid_digest(&item.artifact_digest)
                && present(&item.repository)
                && valid_oid(&item.source_commit)
                && item.built_at > 0
                && item.observation.valid()
        }) {
            return reject(
                "build evidence requires a SHA-256 artifact digest, repository, full commit, and observation",
            );
        }
        if !self.changes.iter().all(|item| {
            [
                &item.repository,
                &item.pull_request,
                &item.work_item,
                &item.intent_reference,
                &item.intent_revision,
                &item.approval_reference,
            ]
            .iter()
            .all(|value| present(value))
                && valid_oid(&item.result_commit)
                && item.merged_at > 0
                && item.approved_source_commit.as_deref().is_none_or(valid_oid)
                && item.merge_reference.as_deref().is_none_or(present)
                && item.observation.valid()
        }) {
            return reject(
                "change evidence requires a full result commit, PR, work item, pinned intent, and approval",
            );
        }
        Ok(())
    }
}

fn one<T>(mut candidates: impl Iterator<Item = T>) -> std::result::Result<T, Reason> {
    let value = candidates.next().ok_or(Reason::Missing)?;
    if candidates.next().is_some() {
        return Err(Reason::Ambiguous);
    }
    Ok(value)
}

fn latest<T>(
    records: impl Iterator<Item = T>,
    observed_at: impl Fn(&T) -> u64,
) -> std::result::Result<T, Reason> {
    let mut records: Vec<_> = records.collect();
    records.sort_unstable_by_key(|record| std::cmp::Reverse(observed_at(record)));
    let mut records = records.into_iter();
    let latest = records.next().ok_or(Reason::Missing)?;
    if records
        .next()
        .is_some_and(|record| observed_at(&record) == observed_at(&latest))
    {
        return Err(Reason::Ambiguous);
    }
    Ok(latest)
}

pub fn read_evidence(path: &Path, limit: u64) -> Result<Vec<u8>> {
    if !path.metadata()?.is_file() {
        return reject("evidence must be a regular file");
    }
    let input = File::open(path)?;
    let mut data = Vec::new();
    input.take(limit + 1).read_to_end(&mut data)?;
    if data.len() as u64 > limit {
        return reject("evidence exceeds the permitted byte limit");
    }
    Ok(data)
}

pub fn run(options: Options) -> Result<Trace> {
    let target = Target {
        service: options.service,
        environment: options.environment,
        region: options.region,
        resource: options.resource,
    };
    trace(
        &read_evidence(&options.bundle, MAX_BUNDLE_BYTES)?,
        target,
        options.as_of,
        options.max_age_seconds,
    )
}

pub(crate) fn trace(
    data: &[u8],
    target: Target,
    as_of: u64,
    max_age_seconds: u64,
) -> Result<Trace> {
    let bundle = parse(data)?;
    trace_bundle(&bundle, target, as_of, max_age_seconds)
}

pub(crate) fn trace_bundle(
    bundle: &Bundle,
    target: Target,
    as_of: u64,
    max_age_seconds: u64,
) -> Result<Trace> {
    if !target.valid() || as_of == 0 || max_age_seconds == 0 {
        return reject(
            "release trace requires an exact target and a positive UTC observation time",
        );
    }

    let stale = |time| as_of - time > max_age_seconds;
    let mut trace = Trace::empty(target, as_of, max_age_seconds);
    let inventory = match latest(
        bundle
            .inventory
            .iter()
            .filter(|item| item.target == trace.target && item.observation.observed_at <= as_of),
        |item| item.observation.observed_at,
    ) {
        Ok(item) => item.clone(),
        Err(reason) => return Ok(trace.stop("inventory", reason)),
    };
    let runtime_time = inventory.observation.observed_at;
    let running_since = inventory.running_since;
    trace.inventory = Some(inventory);
    if stale(runtime_time) {
        return Ok(trace.stop("inventory", Reason::Stale));
    }

    let deployment = match latest(
        bundle
            .deployments
            .iter()
            .filter(|item| item.target == trace.target && item.observation.observed_at <= as_of),
        |item| item.observation.observed_at,
    ) {
        Ok(item) => item.clone(),
        Err(reason) => return Ok(trace.stop("deployment", reason)),
    };
    let matched = trace
        .inventory
        .as_ref()
        .is_some_and(|item| item.running == deployment.version);
    let deployed_at = deployment.deployed_at;
    let deployed_observed = deployment.observation.observed_at;
    trace.deployment = Some(deployment);
    if stale(deployed_observed) {
        return Ok(trace.stop("deployment", Reason::Stale));
    }
    if !matched {
        return Ok(trace.stop("deployment", Reason::Mismatch));
    }
    if deployed_at > runtime_time
        || deployed_at > deployed_observed
        || running_since.is_some_and(|started_at| deployed_at > started_at)
    {
        return Ok(trace.stop("deployment", Reason::InconsistentTime));
    }

    let deployment = trace.deployment.as_ref().expect("deployment was recorded");
    let build = match one(bundle.builds.iter().filter(|item| {
        item.repository == deployment.repository
            && item.artifact_digest == deployment.version.artifact_digest
            && item.observation.observed_at <= as_of
    })) {
        Ok(item) => item.clone(),
        Err(reason) => return Ok(trace.stop("build", reason)),
    };
    let build_time_valid =
        build.built_at <= deployed_at && build.built_at <= build.observation.observed_at;
    trace.build = Some(build);
    if !build_time_valid {
        return Ok(trace.stop("build", Reason::InconsistentTime));
    }

    let build = trace.build.as_ref().expect("build was recorded");
    let change = match one(bundle.changes.iter().filter(|item| {
        item.repository == build.repository
            && item.result_commit == build.source_commit
            && item.observation.observed_at <= as_of
    })) {
        Ok(item) => item.clone(),
        Err(reason) => return Ok(trace.stop("change", reason)),
    };
    let change_time_valid =
        change.merged_at <= deployed_at && change.merged_at <= change.observation.observed_at;
    trace.change = Some(change);
    if !change_time_valid {
        return Ok(trace.stop("change", Reason::InconsistentTime));
    }
    Ok(trace)
}

pub(crate) fn parse(data: &[u8]) -> Result<Bundle> {
    let bundle: Bundle = serde_json::from_slice(data)?;
    bundle.validate()?;
    Ok(bundle)
}
