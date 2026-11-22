use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use clap::Args;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    Result, reject, release_sources,
    release_trace::{self, MAX_BUNDLE_BYTES, Observation, Target, Trace, Version, read_evidence},
    sha256,
};

pub mod otlp;

#[derive(Args)]
pub struct Options {
    /// Approved policy for this investigation.
    policy: PathBuf,
    #[arg(long)]
    release: PathBuf,
    #[arg(long)]
    telemetry: PathBuf,
    /// Start of the requested window as UTC Unix seconds.
    #[arg(long)]
    from: u64,
    /// End of the requested window as UTC Unix seconds.
    #[arg(long)]
    to: u64,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    schema_version: u32,
    target: Target,
    approval_reference: String,
    #[serde(default)]
    release_basis: ReleaseBasis,
    release_digest: String,
    #[serde(default)]
    telemetry_basis: TelemetryBasis,
    telemetry_digest: String,
    authorized_window: Window,
    max_window_seconds: u64,
    release_max_age_seconds: u64,
    max_records: usize,
    log_codes: Vec<String>,
    metrics: Vec<MetricKey>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum ReleaseBasis {
    #[default]
    Bundle,
    SourceManifest,
}

impl ReleaseBasis {
    fn is_bundle(&self) -> bool {
        *self == Self::Bundle
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum TelemetryBasis {
    #[default]
    Normalized,
    OtlpArchive,
}

impl TelemetryBasis {
    fn is_normalized(&self) -> bool {
        *self == Self::Normalized
    }
}

pub enum TelemetrySource<'a> {
    Normalized(&'a Path),
    Otlp(&'a otlp::Archive),
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Telemetry {
    schema_version: u32,
    target: Target,
    coverage: Window,
    observation: Observation,
    logs: Vec<Log>,
    metrics: Vec<Metric>,
}

#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Window {
    from: u64,
    to: u64,
}

impl Window {
    fn valid(&self) -> bool {
        self.from > 0 && self.to > self.from
    }

    fn contains(&self, other: &Self) -> bool {
        self.from <= other.from && other.to <= self.to
    }

    fn covers(&self, timestamp: u64) -> bool {
        (self.from..=self.to).contains(&timestamp)
    }
}

#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Log {
    timestamp: u64,
    version: Version,
    code: String,
    severity: Severity,
}

#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum Severity {
    Debug,
    Info,
    Warn,
    Error,
    Critical,
}

#[derive(Clone, Deserialize, Serialize, Eq, PartialEq, Hash, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MetricKey {
    name: String,
    unit: String,
}

#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Metric {
    timestamp: u64,
    version: Version,
    metric: MetricKey,
    value: f64,
}

#[derive(Serialize)]
pub struct Report {
    schema_version: u32,
    approval_reference: String,
    release_digest: String,
    #[serde(skip_serializing_if = "ReleaseBasis::is_bundle")]
    release_basis: ReleaseBasis,
    #[serde(skip_serializing_if = "TelemetryBasis::is_normalized")]
    telemetry_basis: TelemetryBasis,
    telemetry_digest: String,
    window: Window,
    telemetry_coverage: Window,
    telemetry_observation: Observation,
    release: Trace,
    telemetry_correlated: bool,
    logs: Vec<Log>,
    metrics: Vec<Metric>,
}

fn approved_name(value: &str) -> bool {
    (1..=100).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'%'))
}

impl Policy {
    fn validate(&self) -> Result<()> {
        if self.schema_version != 1
            || !self.target.valid()
            || self.approval_reference.trim().is_empty()
            || self.approval_reference.len() > 500
            || self.approval_reference.chars().any(char::is_control)
            || !release_trace::valid_digest(&self.release_digest)
            || !release_trace::valid_digest(&self.telemetry_digest)
            || !self.authorized_window.valid()
            || !(1..=3600).contains(&self.max_window_seconds)
            || !(1..=3600).contains(&self.release_max_age_seconds)
            || !(1..=500).contains(&self.max_records)
            || self.log_codes.len() > 100
            || self.metrics.len() > 100
            || self.log_codes.iter().any(|code| !approved_name(code))
            || self
                .metrics
                .iter()
                .any(|metric| !approved_name(&metric.name) || !approved_name(&metric.unit))
        {
            return reject("invalid diagnostic policy");
        }
        Ok(())
    }
}

impl Telemetry {
    fn validate(&self, policy: &Policy) -> Result<()> {
        if self.schema_version != 1
            || self.target != policy.target
            || !self.observation.valid()
            || !self.coverage.valid()
            || self.observation.observed_at < self.coverage.to
            || !policy.authorized_window.contains(&self.coverage)
        {
            return reject(
                "telemetry target, coverage or observation differs from the approved scope",
            );
        }
        let codes: HashSet<_> = policy.log_codes.iter().collect();
        let metrics: HashSet<_> = policy.metrics.iter().collect();
        if self.logs.iter().any(|entry| {
            !entry.version.valid()
                || !self.coverage.covers(entry.timestamp)
                || !codes.contains(&entry.code)
        }) || self.metrics.iter().any(|entry| {
            !entry.version.valid()
                || !self.coverage.covers(entry.timestamp)
                || !entry.value.is_finite()
                || !metrics.contains(&entry.metric)
        }) {
            return reject("telemetry contains an unapproved event, metric, version or timestamp");
        }
        Ok(())
    }
}

pub fn run(options: Options) -> Result<Report> {
    Case::load(&options.policy, &options.release, &options.telemetry)?
        .report(options.from, options.to)
}

pub struct Case {
    inputs: Inputs,
    release: Vec<u8>,
}

pub struct ArchiveCase {
    inputs: Inputs,
    release: Trace,
}

struct Inputs {
    policy: Policy,
    policy_digest: String,
    telemetry: Telemetry,
    source_binding: Option<otlp::Binding>,
}

impl Case {
    pub fn load(policy_path: &Path, release_path: &Path, telemetry_path: &Path) -> Result<Self> {
        Self::load_with_policy_pin(policy_path, release_path, telemetry_path, None)
    }

    pub fn load_with_policy_pin(
        policy_path: &Path,
        release_path: &Path,
        telemetry_path: &Path,
        policy_digest: Option<&str>,
    ) -> Result<Self> {
        let inputs = Inputs::load(
            policy_path,
            TelemetrySource::Normalized(telemetry_path),
            ReleaseBasis::Bundle,
            policy_digest,
        )?;
        let release = read_evidence(release_path, MAX_BUNDLE_BYTES)?;
        if sha256(&release) != inputs.policy.release_digest {
            return reject("diagnostic evidence differs from the approved digest");
        }
        let _ = release_trace::parse(&release)?;
        Ok(Self { inputs, release })
    }

    pub fn policy_digest(&self) -> &str {
        &self.inputs.policy_digest
    }

    pub fn allows_window(&self, from: u64, to: u64) -> bool {
        self.inputs.within_scope(&Window { from, to })
    }

    pub fn report(&self, from: u64, to: u64) -> Result<Report> {
        let policy = &self.inputs.policy;
        let window = self.inputs.window(from, to)?;
        let release = release_trace::trace(
            &self.release,
            policy.target.clone(),
            to,
            policy.release_max_age_seconds,
        )?;
        self.inputs.report(window, release)
    }
}

impl ArchiveCase {
    pub fn load(
        policy_path: &Path,
        manifest_path: &Path,
        telemetry_source: TelemetrySource<'_>,
        policy_digest: &str,
    ) -> Result<Self> {
        let inputs = Inputs::load(
            policy_path,
            telemetry_source,
            ReleaseBasis::SourceManifest,
            Some(policy_digest),
        )?;
        let policy = &inputs.policy;
        let release = release_sources::load_trace(
            manifest_path,
            &policy.release_digest,
            policy.release_max_age_seconds,
        )?
        .trace;
        let to = release.as_of();
        if !release.fits_diagnostic_scope(&policy.target, to, policy.release_max_age_seconds)
            || !policy.authorized_window.covers(to)
            || to == policy.authorized_window.from
            || !inputs.telemetry.coverage.covers(to)
            || to == inputs.telemetry.coverage.from
        {
            return reject(
                "release source trace differs from the approved diagnostic target, time, or digest",
            );
        }
        if let (Some(binding), Some((observed_version, _))) =
            (&inputs.source_binding, release.verified_deployment())
        {
            if &binding.version != observed_version {
                return reject("OTLP collection version does not match the observed deployment");
            }
            let observed_container = release.observed_kubernetes_container().ok_or(
                "Kubernetes snapshot lacks the running container ID required for OTLP diagnosis",
            )?;
            let approved_container = binding.kubernetes_container.as_ref().ok_or(
                "OTLP diagnosis of a Kubernetes deployment requires an approved container identity",
            )?;
            if observed_container != approved_container {
                return reject(
                    "OTLP collection container does not match the observed Kubernetes container",
                );
            }
        }
        Ok(Self { inputs, release })
    }

    pub fn policy_digest(&self) -> &str {
        &self.inputs.policy_digest
    }

    pub fn allows_window(&self, from: u64, to: u64) -> bool {
        self.release.as_of() == to && self.inputs.within_scope(&Window { from, to })
    }

    pub fn report(&self, from: u64, to: u64) -> Result<Report> {
        let window = self.inputs.window(from, to)?;
        if to != self.release.as_of() {
            return reject(
                "release source trace differs from the approved diagnostic target, time, or digest",
            );
        }
        self.inputs.report(window, self.release.clone())
    }
}

impl Inputs {
    fn load(
        policy_path: &Path,
        telemetry_source: TelemetrySource<'_>,
        basis: ReleaseBasis,
        expected_digest: Option<&str>,
    ) -> Result<Self> {
        let policy_bytes = read_evidence(policy_path, 64 * 1024)?;
        let policy_digest = sha256(&policy_bytes);
        if expected_digest.is_some_and(|expected| expected != policy_digest) {
            return reject("diagnostic policy does not match the approved digest");
        }
        let policy: Policy = serde_json::from_slice(&policy_bytes)
            .map_err(|_| "invalid diagnostic policy format")?;
        policy.validate()?;
        if policy.release_basis != basis {
            return reject("diagnostic policy does not approve this release evidence format");
        }
        let (telemetry, source_binding) = match (policy.telemetry_basis, telemetry_source) {
            (TelemetryBasis::Normalized, TelemetrySource::Normalized(path)) => {
                let raw = read_evidence(path, MAX_BUNDLE_BYTES)?;
                if sha256(&raw) != policy.telemetry_digest {
                    return reject("diagnostic evidence differs from the approved digest");
                }
                let telemetry = serde_json::from_slice(&raw)
                    .map_err(|_| "diagnosis requires the structured redacted telemetry format")?;
                (telemetry, None)
            }
            (TelemetryBasis::OtlpArchive, TelemetrySource::Otlp(archive)) => {
                let (telemetry, binding) = otlp::load(archive, &policy.telemetry_digest)?;
                (telemetry, Some(binding))
            }
            _ => {
                return reject("diagnostic policy does not approve this telemetry evidence format");
            }
        };
        telemetry.validate(&policy)?;
        Ok(Self {
            policy,
            policy_digest,
            telemetry,
            source_binding,
        })
    }

    fn within_scope(&self, window: &Window) -> bool {
        window.valid()
            && self.policy.authorized_window.contains(window)
            && self.telemetry.coverage.contains(window)
    }

    fn window(&self, from: u64, to: u64) -> Result<Window> {
        let window = Window { from, to };
        if !self.within_scope(&window) || to - from > self.policy.max_window_seconds {
            return reject("requested diagnostic window exceeds the approved scope");
        }
        Ok(window)
    }

    fn report(&self, window: Window, release: Trace) -> Result<Report> {
        let policy = &self.policy;
        let telemetry = &self.telemetry;
        let binding = release.verified_deployment();
        let mut logs = Vec::new();
        let mut metrics = Vec::new();
        if let Some((version, deployed_at)) = binding {
            let running_since = release
                .observed_runtime_start()
                .map_or(deployed_at, |started| started.max(deployed_at));
            let includes = |timestamp, observed: &Version| {
                window.covers(timestamp) && timestamp >= running_since && observed == version
            };
            logs.extend(
                telemetry
                    .logs
                    .iter()
                    .filter(|entry| includes(entry.timestamp, &entry.version))
                    .take(policy.max_records + 1)
                    .cloned(),
            );
            if logs.len() > policy.max_records {
                return reject("diagnostic output exceeds the approved record limit");
            }
            metrics.extend(
                telemetry
                    .metrics
                    .iter()
                    .filter(|entry| includes(entry.timestamp, &entry.version))
                    .take(policy.max_records - logs.len() + 1)
                    .cloned(),
            );
            if logs.len() + metrics.len() > policy.max_records {
                return reject("diagnostic output exceeds the approved record limit");
            }
            logs.sort_by_key(|entry| entry.timestamp);
            metrics.sort_by_key(|entry| entry.timestamp);
        }
        Ok(Report {
            schema_version: 1,
            approval_reference: policy.approval_reference.clone(),
            release_digest: policy.release_digest.clone(),
            release_basis: policy.release_basis,
            telemetry_basis: policy.telemetry_basis,
            telemetry_digest: policy.telemetry_digest.clone(),
            window,
            telemetry_coverage: telemetry.coverage.clone(),
            telemetry_observation: telemetry.observation.clone(),
            telemetry_correlated: binding.is_some(),
            release,
            logs,
            metrics,
        })
    }
}
